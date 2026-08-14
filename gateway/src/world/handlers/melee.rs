//! Melee-attack dispatcher: durable engagement, session transition, and client stance messages.

use super::super::*;

pub(crate) trait MeleeActionStore: Send + Sync {
    fn start_attack(&self, account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()>;
    /// Disarm the actor's outgoing engagement row. Also the ranged auto-repeat teardown call;
    /// `attack_stop` states the rule the two share.
    fn stop_attack(&self, account_id: u64, actor_guid: u64) -> Result<()>;
}

impl MeleeActionStore for crate::stdb::Coordinator {
    fn start_attack(&self, account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::start_attack(self, account_id, actor_guid, target_guid)
    }

    fn stop_attack(&self, account_id: u64, actor_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::stop_attack(self, account_id, actor_guid)
    }
}

/// The melee-relevant session facts. `self_guid` is `None` outside the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeleeActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
    pub(crate) attacking_target: Option<u64>,
    pub(crate) ranged_repeat: bool,
}

impl MeleeActionPlayer {
    pub(crate) fn from_conn(conn: &WorldConn) -> Self {
        let (attacking_target, ranged_repeat) = match &conn.state {
            WorldState::InWorld(iw) => (iw.attacking_target, iw.ranged_repeat),
            WorldState::CharSelect => (None, false),
        };
        Self {
            account_id: conn.account_id,
            self_guid: social::self_guid(conn),
            attacking_target,
            ranged_repeat,
        }
    }
}

/// The session-state change a melee outcome asks the world session to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeleeTransition {
    /// Leave the session's combat state as it is (every refusal).
    Unchanged,
    /// Melee engagement armed on `target`. Clears ranged auto-repeat: the shared durable row now
    /// means melee.
    Engaged(u64),
    /// Melee engagement dropped. Leaves ranged auto-repeat alone: a stop that reaches here never
    /// had one armed.
    Disengaged,
}

impl MeleeTransition {
    /// A player who left the world meanwhile has no combat state to carry, so this is a no-op.
    pub(crate) fn apply(self, state: &mut WorldState) {
        let WorldState::InWorld(iw) = state else {
            return;
        };
        match self {
            Self::Unchanged => {}
            Self::Engaged(target_guid) => {
                iw.attacking_target = Some(target_guid);
                iw.ranged_repeat = false;
            }
            Self::Disengaged => iw.attacking_target = None,
        }
    }
}

pub(crate) enum MeleeActionOutcome {
    Handled {
        transition: MeleeTransition,
        outbound: Vec<Outbound>,
    },
    PassThrough(ClientOpcodeMessage),
}

pub(crate) fn dispatch_melee_action<St: MeleeActionStore + ?Sized>(
    store: &St,
    player: MeleeActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<MeleeActionOutcome> {
    match msg {
        ClientOpcodeMessage::CMSG_ATTACKSWING(s) => {
            let target_guid = s.guid.guid();
            log::info!(
                "world[autoshot]: CMSG_ATTACKSWING target={target_guid} ranged_repeat_active={} (account {})",
                player.ranged_repeat,
                player.account_id
            );
            attack_start(store, player, target_guid)
        }
        ClientOpcodeMessage::CMSG_ATTACKSTOP => {
            log::info!(
                "world[autoshot]: CMSG_ATTACKSTOP ranged_repeat_active={} (account {})",
                player.ranged_repeat,
                player.account_id
            );
            attack_stop(store, player)
        }
        other => Ok(MeleeActionOutcome::PassThrough(other)),
    }
}

/// The player's own entity is gone, so no further action can be served. Close the session rather
/// than leave the player in a frozen world with no recovery.
fn desync_exit(error: anyhow::Error, opcode: &str) -> anyhow::Error {
    error.context(format!(
        "player desync (entity missing) on {opcode} — closing session for a clean relog"
    ))
}

/// Arm the durable engagement first; session state and client stance follow only on success.
fn attack_start<St: MeleeActionStore + ?Sized>(
    store: &St,
    player: MeleeActionPlayer,
    target_guid: u64,
) -> Result<MeleeActionOutcome> {
    if let Err(e) = store.start_attack(
        player.account_id,
        player.self_guid.unwrap_or(0),
        target_guid,
    ) {
        let text = e.to_string();
        // A corpse or a friendly target must be answered: with no refusal the client hangs in
        // combat stance and never swings. Both are transient per-swing failures, not fatal.
        let refusal = if text.contains(lyracore_shared::ERR_ATTACK_TARGET_DEAD) {
            Some(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET)
        } else if text.contains(lyracore_shared::ERR_ATTACK_FRIENDLY) {
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
            player.account_id
        );
        return Ok(MeleeActionOutcome::Handled {
            transition: MeleeTransition::Unchanged,
            outbound: refusal.map(Outbound::One).into_iter().collect(),
        });
    }
    // Not in the world: no combat state to arm and no attacker guid to name. The durable call
    // already ran, and the module rejects an unresolved actor.
    let Some(self_guid) = player.self_guid else {
        return Ok(MeleeActionOutcome::Handled {
            transition: MeleeTransition::Unchanged,
            outbound: Vec::new(),
        });
    };
    Ok(MeleeActionOutcome::Handled {
        transition: MeleeTransition::Engaged(target_guid),
        outbound: vec![Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(
            Box::new(codec::build_attack_start(self_guid, target_guid)),
        ))],
    })
}

/// Request durable disengagement first; the session drops its target and the client leaves
/// combat stance after.
fn attack_stop<St: MeleeActionStore + ?Sized>(
    store: &St,
    player: MeleeActionPlayer,
) -> Result<MeleeActionOutcome> {
    // Melee and ranged auto-repeat share one durable engagement row per attacker. The client sends
    // CMSG_ATTACKSTOP whenever it leaves melee stance, ranged loop armed or not, so honoring it
    // here would delete the auto-shot engagement: one shot, then silence. Only
    // CMSG_CANCEL_AUTO_REPEAT_SPELL tears that loop down.
    if player.ranged_repeat {
        return Ok(MeleeActionOutcome::Handled {
            transition: MeleeTransition::Unchanged,
            outbound: Vec::new(),
        });
    }
    if let Err(e) = store.stop_attack(player.account_id, player.self_guid.unwrap_or(0)) {
        if is_desync_error(&e) {
            return Err(desync_exit(e, "attackstop"));
        }
        // A refused stop still clears the client's stance: the recorded target may already be dead,
        // and leaving the player swinging at nothing is worse than a stale disarm.
        log::debug!(
            "world: stop_attack ignored (account {}): {e}",
            player.account_id
        );
    }
    let outbound = match (player.self_guid, player.attacking_target) {
        (Some(self_guid), Some(target_guid)) => {
            vec![Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(
                Box::new(codec::build_attack_stop(self_guid, target_guid)),
            ))]
        }
        _ => Vec::new(),
    };
    Ok(MeleeActionOutcome::Handled {
        transition: MeleeTransition::Disengaged,
        outbound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{Guid, CMSG_ATTACKSWING, CMSG_PING};

    #[derive(Default)]
    struct InMemoryMeleeActions {
        start_requests: Mutex<Vec<(u64, u64, u64)>>,
        start_error: Option<String>,
        stop_requests: Mutex<Vec<(u64, u64)>>,
        stop_error: Option<String>,
    }

    impl MeleeActionStore for InMemoryMeleeActions {
        fn start_attack(&self, account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
            self.start_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid, target_guid));
            self.start_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }

        fn stop_attack(&self, account_id: u64, actor_guid: u64) -> Result<()> {
            self.stop_requests
                .lock()
                .unwrap()
                .push((account_id, actor_guid));
            self.stop_error
                .as_ref()
                .map_or_else(|| Ok(()), |error| Err(anyhow::anyhow!("{error}")))
        }
    }

    /// A player mid ranged auto-repeat: attack start must engage melee and clear that loop.
    fn player() -> MeleeActionPlayer {
        MeleeActionPlayer {
            account_id: 7,
            self_guid: Some(42),
            attacking_target: None,
            ranged_repeat: true,
        }
    }

    fn in_world(attacking_target: Option<u64>, ranged_repeat: bool) -> WorldState {
        WorldState::InWorld(InWorld {
            self_guid: 42,
            subs: PlayerSubscriptions::empty(),
            session_epoch: 1,
            attacking_target,
            looting_target: None,
            ranged_repeat,
        })
    }

    fn combat_state(state: &WorldState) -> (Option<u64>, bool) {
        match state {
            WorldState::InWorld(iw) => (iw.attacking_target, iw.ranged_repeat),
            WorldState::CharSelect => panic!("not in world"),
        }
    }

    /// A player in melee on `target`, with no ranged loop armed.
    fn engaged(target: u64) -> MeleeActionPlayer {
        MeleeActionPlayer {
            attacking_target: Some(target),
            ranged_repeat: false,
            ..player()
        }
    }

    fn attack_swing(target: u64) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_ATTACKSWING(CMSG_ATTACKSWING {
            guid: Guid::new(target),
        })
    }

    fn refused(error: &str) -> InMemoryMeleeActions {
        InMemoryMeleeActions {
            start_error: Some(error.into()),
            ..Default::default()
        }
    }

    #[test]
    fn attack_start_requests_the_durable_engagement_then_arms_the_session_and_client() {
        let actions = InMemoryMeleeActions::default();

        let outcome = dispatch_melee_action(&actions, player(), attack_swing(90)).unwrap();

        assert_eq!(
            actions.start_requests.lock().unwrap().as_slice(),
            &[(7, 42, 90)]
        );
        let MeleeActionOutcome::Handled {
            transition,
            outbound,
        } = outcome
        else {
            panic!("attack start must be handled by the melee seam");
        };
        assert_eq!(transition, MeleeTransition::Engaged(90));
        assert!(
            matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(a))]
                if a.attacker.guid() == 42 && a.victim.guid() == 90)
        );
        let mut state = in_world(None, true);
        transition.apply(&mut state);
        assert_eq!(combat_state(&state), (Some(90), false));
    }

    #[test]
    fn attack_start_from_an_inactive_ranged_state_gives_the_same_melee_transition() {
        let actions = InMemoryMeleeActions::default();
        let player = MeleeActionPlayer {
            ranged_repeat: false,
            ..player()
        };

        let outcome = dispatch_melee_action(&actions, player, attack_swing(90)).unwrap();

        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, .. }
                if transition == MeleeTransition::Engaged(90)
        ));
    }

    #[test]
    fn a_dead_target_replies_deadtarget_and_leaves_the_active_target_unchanged() {
        let actions = refused(lyracore_shared::ERR_ATTACK_TARGET_DEAD);

        let outcome = dispatch_melee_action(&actions, player(), attack_swing(90)).unwrap();

        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Unchanged
                    && matches!(outbound.as_slice(),
                        [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET)])
        ));
    }

    #[test]
    fn a_friendly_target_replies_cant_attack_and_leaves_the_active_target_unchanged() {
        let actions = refused(lyracore_shared::ERR_ATTACK_FRIENDLY);

        let outcome = dispatch_melee_action(&actions, player(), attack_swing(90)).unwrap();

        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Unchanged
                    && matches!(outbound.as_slice(),
                        [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK)])
        ));
    }

    #[test]
    fn an_ordinary_refusal_sends_no_client_message_and_keeps_the_session_alive() {
        let actions = refused("target out of range");

        let outcome = dispatch_melee_action(&actions, player(), attack_swing(90)).unwrap();

        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Unchanged && outbound.is_empty()
        ));
    }

    #[test]
    fn an_unresolved_player_requests_the_legacy_zero_actor_and_arms_nothing() {
        let actions = InMemoryMeleeActions::default();
        let player = MeleeActionPlayer {
            self_guid: None,
            ..player()
        };

        let outcome = dispatch_melee_action(&actions, player, attack_swing(90)).unwrap();

        assert_eq!(
            actions.start_requests.lock().unwrap().as_slice(),
            &[(7, 0, 90)]
        );
        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Unchanged && outbound.is_empty()
        ));
    }

    #[test]
    fn attack_stop_requests_the_durable_disengagement_then_clears_the_session_and_client() {
        let actions = InMemoryMeleeActions::default();

        let outcome =
            dispatch_melee_action(&actions, engaged(90), ClientOpcodeMessage::CMSG_ATTACKSTOP)
                .unwrap();

        assert_eq!(actions.stop_requests.lock().unwrap().as_slice(), &[(7, 42)]);
        let MeleeActionOutcome::Handled {
            transition,
            outbound,
        } = outcome
        else {
            panic!("attack stop must be handled by the melee seam");
        };
        assert_eq!(transition, MeleeTransition::Disengaged);
        assert!(
            matches!(outbound.as_slice(), [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(a))]
                if a.player.guid() == 42 && a.enemy.guid() == 90)
        );
        let mut state = in_world(Some(90), false);
        transition.apply(&mut state);
        assert_eq!(combat_state(&state), (None, false));
    }

    #[test]
    fn attack_stop_with_no_recorded_target_still_disengages_and_fabricates_no_message() {
        let actions = InMemoryMeleeActions::default();
        let player = MeleeActionPlayer {
            ranged_repeat: false,
            ..player()
        };

        let outcome =
            dispatch_melee_action(&actions, player, ClientOpcodeMessage::CMSG_ATTACKSTOP).unwrap();

        assert_eq!(actions.stop_requests.lock().unwrap().as_slice(), &[(7, 42)]);
        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Disengaged && outbound.is_empty()
        ));
    }

    /// A kill clears the durable row on another thread, so a late stop is refused. That is harmless.
    #[test]
    fn an_ordinary_stop_refusal_still_clears_the_target_and_keeps_the_session_alive() {
        let actions = InMemoryMeleeActions {
            stop_error: Some("no engagement for that attacker".into()),
            ..Default::default()
        };

        let outcome =
            dispatch_melee_action(&actions, engaged(90), ClientOpcodeMessage::CMSG_ATTACKSTOP)
                .unwrap();

        assert_eq!(
            actions.stop_requests.lock().unwrap().as_slice(),
            &[(7, 42)],
            "the durable stop is attempted before the refusal is swallowed"
        );
        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Disengaged
                    && matches!(outbound.as_slice(),
                        [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(_))])
        ));
    }

    #[test]
    fn attack_stop_from_an_unresolved_player_requests_the_legacy_zero_actor_and_echoes_nothing() {
        let actions = InMemoryMeleeActions::default();
        let player = MeleeActionPlayer {
            self_guid: None,
            ..engaged(90)
        };

        let outcome =
            dispatch_melee_action(&actions, player, ClientOpcodeMessage::CMSG_ATTACKSTOP).unwrap();

        assert_eq!(actions.stop_requests.lock().unwrap().as_slice(), &[(7, 0)]);
        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Disengaged && outbound.is_empty()
        ));
    }

    #[test]
    fn an_armed_ranged_repeat_consumes_the_stop_and_changes_nothing() {
        let actions = InMemoryMeleeActions::default();
        let player = MeleeActionPlayer {
            ranged_repeat: true,
            ..engaged(90)
        };

        let outcome =
            dispatch_melee_action(&actions, player, ClientOpcodeMessage::CMSG_ATTACKSTOP).unwrap();

        assert!(
            actions.stop_requests.lock().unwrap().is_empty(),
            "honoring the stop would delete the shared auto-shot engagement row"
        );
        assert!(matches!(
            outcome,
            MeleeActionOutcome::Handled { transition, outbound }
                if transition == MeleeTransition::Unchanged && outbound.is_empty()
        ));
    }

    #[test]
    fn unrelated_opcodes_pass_through_to_the_next_dispatcher() {
        let actions = InMemoryMeleeActions::default();

        let outcome = dispatch_melee_action(
            &actions,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();

        assert!(matches!(
            outcome,
            MeleeActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
        assert!(actions.start_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_player_desync_ends_the_session_with_attack_start_context() {
        let actions = refused("no live entity for guid 42");

        let error = match dispatch_melee_action(&actions, player(), attack_swing(90)) {
            Err(error) => error,
            Ok(_) => panic!("a missing player entity must end the session"),
        };

        let text = format!("{error:#}");
        assert!(
            text.contains("desync"),
            "expected desync context, got: {text}"
        );
        assert!(
            text.contains("attackswing"),
            "expected attack-start context, got: {text}"
        );
    }

    #[test]
    fn a_player_desync_on_stop_ends_the_session_with_attack_stop_context() {
        let actions = InMemoryMeleeActions {
            stop_error: Some("player 42 not in world".into()),
            ..Default::default()
        };

        let error = match dispatch_melee_action(
            &actions,
            engaged(90),
            ClientOpcodeMessage::CMSG_ATTACKSTOP,
        ) {
            Err(error) => error,
            Ok(_) => panic!("a missing player entity must end the session"),
        };

        let text = format!("{error:#}");
        assert!(
            text.contains("desync"),
            "expected desync context, got: {text}"
        );
        assert!(
            text.contains("attackstop"),
            "expected attack-stop context, got: {text}"
        );
    }
}
