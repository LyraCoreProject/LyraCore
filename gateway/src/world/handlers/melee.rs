//! Melee-attack dispatcher: durable engagement, session transition, and client stance messages.

use super::super::*;
use crate::stdb::{classify, DurableFailure};
use crate::world::family::{ProtocolFamily, ProtocolReply, ProtocolRequest, ProtocolSession};

pub(crate) trait MeleeActionStore: Send + Sync {
    fn start_attack(&self, actor: Actor, target_guid: u64) -> Result<()>;
    /// Disarm the actor's outgoing engagement row. Also the ranged auto-repeat teardown call;
    /// `attack_stop` states the rule the two share.
    fn stop_attack(&self, actor: Actor) -> Result<()>;
}

pub(crate) struct Melee;

impl<St: MeleeActionStore + ?Sized> ProtocolFamily<St> for Melee {
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let msg = request.message()?;
        match msg {
            ClientOpcodeMessage::CMSG_ATTACKSWING(s) => {
                let target_guid = s.guid.guid();
                log::info!(
                "world[autoshot]: CMSG_ATTACKSWING target={target_guid} ranged_repeat_active={} (account {})",
                matches!(&session.state, WorldState::InWorld(world) if world.ranged_repeat),
                session.account_id
            );
                attack_start(store, session, target_guid)
            }
            ClientOpcodeMessage::CMSG_ATTACKSTOP => {
                log::info!(
                    "world[autoshot]: CMSG_ATTACKSTOP ranged_repeat_active={} (account {})",
                    matches!(&session.state, WorldState::InWorld(world) if world.ranged_repeat),
                    session.account_id
                );
                attack_stop(store, session)
            }
            other => Err(anyhow!("request routed to Melee: {other}")),
        }
    }
}

/// The Character's entity is gone, so no further action can be served. End the World Session so the client can log in again.
fn desync_exit(error: anyhow::Error, opcode: &str) -> anyhow::Error {
    error.context(format!(
        "Character entity missing on {opcode}: desync ends the World Session"
    ))
}

/// Arm the durable engagement first; session state and client stance follow only on success.
fn attack_start<St: MeleeActionStore + ?Sized>(
    store: &St,
    session: &mut ProtocolSession,
    target_guid: u64,
) -> Result<ProtocolReply> {
    // Not in the world: no combat state to arm and no attacker guid to name.
    let Some(actor) = session.actor() else {
        return Ok(ProtocolReply::default());
    };
    if let Err(e) = store.start_attack(actor, target_guid) {
        let DurableFailure::Refusal { reason } = classify(&e) else {
            return Err(e);
        };
        // A corpse or a friendly target must be answered: with no refusal the client hangs in
        // combat stance and never swings. Both are transient per-swing failures, not fatal.
        let refusal = if reason.contains(lyracore_shared::ERR_ATTACK_TARGET_DEAD) {
            Some(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET)
        } else if reason.contains(lyracore_shared::ERR_ATTACK_FRIENDLY) {
            Some(ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK)
        } else if is_desync_error(&e) {
            return Err(desync_exit(e, "attackswing"));
        } else {
            // Out of range, retarget races: expected, so ignore.
            None
        };
        // Every non-desync refusal stays visible to operators, answered ones included.
        log::debug!(
            "world: start_attack ignored (account {}): {e}",
            session.account_id
        );
        return Ok(refusal
            .map(Outbound::One)
            .into_iter()
            .collect::<Vec<_>>()
            .into());
    }
    Ok({
        if let WorldState::InWorld(world) = &mut session.state {
            world.attacking_target = Some(target_guid);
            world.ranged_repeat = false;
        }
        ProtocolReply::from(vec![Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(
            Box::new(codec::build_attack_start(actor.guid(), target_guid)),
        ))])
    })
}

/// Request durable disengagement first; the session drops its target and the client leaves
/// combat stance after.
fn attack_stop<St: MeleeActionStore + ?Sized>(
    store: &St,
    session: &mut ProtocolSession,
) -> Result<ProtocolReply> {
    // Melee and ranged auto-repeat share one durable engagement row per attacker. The client sends
    // CMSG_ATTACKSTOP whenever it leaves melee stance, ranged loop armed or not, so honoring it
    // here would delete the auto-shot engagement: one shot, then silence. Only
    // CMSG_CANCEL_AUTO_REPEAT_SPELL tears that loop down.
    if matches!(&session.state, WorldState::InWorld(world) if world.ranged_repeat) {
        return Ok(ProtocolReply::default());
    }
    let Some(actor) = session.actor() else {
        return Ok({
            if let WorldState::InWorld(world) = &mut session.state {
                world.attacking_target = None;
            }
            ProtocolReply::from(Vec::new())
        });
    };
    if let Err(e) = store.stop_attack(actor) {
        if matches!(classify(&e), DurableFailure::TransportLoss) {
            return Err(e);
        }
        if is_desync_error(&e) {
            return Err(desync_exit(e, "attackstop"));
        }
        // A refused stop still clears the client's stance: the recorded target may already be dead,
        // and leaving the Character swinging at nothing is worse than a stale disarm.
        log::debug!(
            "world: stop_attack ignored (account {}): {e}",
            session.account_id
        );
    }
    let outbound: Vec<Outbound> = match &session.state {
        WorldState::InWorld(world) => world.attacking_target,
        WorldState::CharSelect => None,
    }
    .map(|target_guid| {
        Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(Box::new(
            codec::build_attack_stop(actor.guid(), target_guid),
        )))
    })
    .into_iter()
    .collect();
    Ok({
        if let WorldState::InWorld(world) = &mut session.state {
            world.attacking_target = None;
        }
        ProtocolReply::from(outbound)
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::stdb::ReducerCallError;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{Guid, CMSG_ATTACKSWING};

    #[derive(Default)]
    pub(crate) struct InMemoryMeleeActions {
        /// Recorded `start_attack` calls: actor and target.
        pub(crate) start_requests: Mutex<Vec<(u64, u64)>>,
        /// The Module's reason when `start_attack` is refused.
        pub(crate) start_error: Option<String>,
        pub(crate) stop_requests: Mutex<Vec<u64>>,
        /// The Module's reason when `stop_attack` is refused.
        pub(crate) stop_error: Option<String>,
        /// Every durable call fails with a Transport Loss.
        pub(crate) transport_lost: bool,
        /// The attackers with a live engagement: a started attack adds its actor, a stop removes it.
        pub(crate) engaged: Mutex<Vec<u64>>,
    }

    impl MeleeActionStore for InMemoryMeleeActions {
        fn start_attack(&self, actor: Actor, target_guid: u64) -> Result<()> {
            self.start_requests
                .lock()
                .unwrap()
                .push((actor.guid(), target_guid));
            if self.transport_lost {
                return Err(ReducerCallError::transport_lost("gw_attack").into());
            }
            if let Some(reason) = &self.start_error {
                return Err(ReducerCallError::refused("gw_attack", reason).into());
            }
            let mut engaged = self.engaged.lock().unwrap();
            if !engaged.contains(&actor.guid()) {
                engaged.push(actor.guid());
            }
            Ok(())
        }

        fn stop_attack(&self, actor: Actor) -> Result<()> {
            self.stop_requests.lock().unwrap().push(actor.guid());
            if self.transport_lost {
                return Err(ReducerCallError::transport_lost("gw_stop_attack").into());
            }
            if let Some(reason) = &self.stop_error {
                return Err(ReducerCallError::refused("gw_stop_attack", reason).into());
            }
            self.engaged
                .lock()
                .unwrap()
                .retain(|&guid| guid != actor.guid());
            Ok(())
        }
    }

    fn session(target: Option<u64>, ranged: bool) -> ProtocolSession {
        let mut session = ProtocolSession::in_world(7, 42);
        if let WorldState::InWorld(world) = &mut session.state {
            world.attacking_target = target;
            world.ranged_repeat = ranged;
        }
        session
    }

    fn combat_state(session: &ProtocolSession) -> (Option<u64>, bool) {
        match &session.state {
            WorldState::InWorld(world) => (world.attacking_target, world.ranged_repeat),
            WorldState::CharSelect => (None, false),
        }
    }

    fn swing(target: u64) -> ProtocolRequest {
        ClientOpcodeMessage::CMSG_ATTACKSWING(CMSG_ATTACKSWING {
            guid: Guid::new(target),
        })
        .into()
    }

    #[test]
    fn starting_melee_arms_the_engagement_and_clears_ranged_repeat() {
        let store = InMemoryMeleeActions::default();
        let mut session = session(None, true);
        let reply = Melee::handle(&store, &mut session, swing(90)).unwrap();
        assert_eq!(*store.engaged.lock().unwrap(), vec![42]);
        assert_eq!(combat_state(&session), (Some(90), false));
        assert!(
            matches!(reply.outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(message))] if message.attacker.guid() == 42 && message.victim.guid() == 90)
        );
    }

    #[test]
    fn refused_attack_preserves_the_previous_target_and_answers_the_refusal() {
        for (reason, expected) in [
            (
                lyracore_shared::ERR_ATTACK_TARGET_DEAD,
                Some(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET),
            ),
            (
                lyracore_shared::ERR_ATTACK_FRIENDLY,
                Some(ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK),
            ),
            ("target out of range", None),
        ] {
            let store = InMemoryMeleeActions {
                start_error: Some(reason.into()),
                ..Default::default()
            };
            let mut session = session(Some(80), true);
            let reply = Melee::handle(&store, &mut session, swing(90)).unwrap();
            assert_eq!(combat_state(&session), (Some(80), true));
            assert!(store.engaged.lock().unwrap().is_empty());
            match expected {
                Some(expected) => assert!(
                    matches!(reply.outbound.as_slice(), [Outbound::One(actual)] if *actual == expected)
                ),
                None => assert!(reply.outbound.is_empty()),
            }
        }
    }

    #[test]
    fn melee_requests_before_world_entry_do_nothing() {
        let store = InMemoryMeleeActions::default();
        let mut session = ProtocolSession::new(7, "TESTER".into());
        assert!(Melee::handle(&store, &mut session, swing(90))
            .unwrap()
            .outbound
            .is_empty());
        assert!(Melee::handle(
            &store,
            &mut session,
            ClientOpcodeMessage::CMSG_ATTACKSTOP.into()
        )
        .unwrap()
        .outbound
        .is_empty());
        assert!(store.start_requests.lock().unwrap().is_empty());
        assert!(store.stop_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn stopping_melee_disarms_the_engagement_and_clears_the_client_stance() {
        let store = InMemoryMeleeActions::default();
        store.engaged.lock().unwrap().push(42);
        let mut session = session(Some(90), false);
        let reply = Melee::handle(
            &store,
            &mut session,
            ClientOpcodeMessage::CMSG_ATTACKSTOP.into(),
        )
        .unwrap();
        assert!(store.engaged.lock().unwrap().is_empty());
        assert_eq!(combat_state(&session), (None, false));
        assert!(
            matches!(reply.outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(message))] if message.player.guid() == 42 && message.enemy.guid() == 90)
        );
    }

    #[test]
    fn stopping_without_a_target_does_not_invent_a_client_message() {
        let store = InMemoryMeleeActions::default();
        let mut session = session(None, false);
        let reply = Melee::handle(
            &store,
            &mut session,
            ClientOpcodeMessage::CMSG_ATTACKSTOP.into(),
        )
        .unwrap();
        assert!(reply.outbound.is_empty());
        assert_eq!(combat_state(&session), (None, false));
    }

    #[test]
    fn an_ordinary_stop_refusal_still_clears_the_target_and_client_stance() {
        let store = InMemoryMeleeActions {
            stop_error: Some("no engagement for that attacker".into()),
            ..Default::default()
        };
        let mut session = session(Some(90), false);
        let reply = Melee::handle(
            &store,
            &mut session,
            ClientOpcodeMessage::CMSG_ATTACKSTOP.into(),
        )
        .unwrap();
        assert_eq!(combat_state(&session), (None, false));
        assert!(matches!(
            reply.outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(_))]
        ));
    }

    #[test]
    fn stopping_melee_preserves_an_armed_ranged_repeat() {
        let store = InMemoryMeleeActions::default();
        store.engaged.lock().unwrap().push(42);
        let mut session = session(Some(90), true);
        let reply = Melee::handle(
            &store,
            &mut session,
            ClientOpcodeMessage::CMSG_ATTACKSTOP.into(),
        )
        .unwrap();
        assert_eq!(combat_state(&session), (Some(90), true));
        assert_eq!(*store.engaged.lock().unwrap(), vec![42]);
        assert!(store.stop_requests.lock().unwrap().is_empty());
        assert!(reply.outbound.is_empty());
    }

    #[test]
    fn a_missing_entity_ends_the_world_session_with_request_context() {
        let store = InMemoryMeleeActions {
            start_error: Some("no live entity for guid 42".into()),
            stop_error: Some("player 42 not in world".into()),
            ..Default::default()
        };
        for (request, context) in [
            (swing(90), "attackswing"),
            (ClientOpcodeMessage::CMSG_ATTACKSTOP.into(), "attackstop"),
        ] {
            let mut session = session(Some(90), false);
            let error = Melee::handle(&store, &mut session, request)
                .err()
                .expect("missing entity must end the World Session");
            assert!(format!("{error:#}").contains(context));
            assert_eq!(combat_state(&session), (Some(90), false));
        }
    }

    #[test]
    fn transport_loss_keeps_protocol_state_unchanged_and_ends_the_world_session() {
        let store = InMemoryMeleeActions {
            transport_lost: true,
            ..Default::default()
        };
        for request in [swing(90), ClientOpcodeMessage::CMSG_ATTACKSTOP.into()] {
            let mut session = session(Some(80), false);
            assert!(Melee::handle(&store, &mut session, request).is_err());
            assert_eq!(combat_state(&session), (Some(80), false));
        }
    }
}
