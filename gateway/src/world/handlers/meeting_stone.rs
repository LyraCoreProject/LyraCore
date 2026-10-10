//! Meeting Stone dispatcher: join, leave, the queue status query and `MSG_LOOKING_FOR_GROUP`.
//!
//! A JOIN is two Durable Requests: the Home Shard admits the actor at the stone, then the party
//! authority queues it with the Seekers' race and class, read realm-wide. Queue notices arrive on
//! the group event relay; only a refused party JOIN is answered here.

use super::super::*;
use crate::stdb::ignore_refusal;
use lyracore_shared::group::GROUP_MAX_MEMBERS;
use lyracore_shared::meeting_stone::{
    join_failure_for, queue_status, realm_op, MeetingStoneRefusal,
};
use wow_world_messages::vanilla::Area;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeekerFacts {
    pub(crate) character_guid: u64,
    pub(crate) race: u8,
    pub(crate) class: u8,
}

/// A timeout, transport or SDK failure stays an `Err` with an unknown outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeetingStoneOutcome {
    Ran,
    Refused(MeetingStoneRefusal),
}

pub(crate) trait MeetingStoneActionStore {
    /// Durable Request on the actor's Home Shard. Writes nothing.
    fn admit_meeting_stone(&self, actor: Actor, go_guid: u64) -> Result<MeetingStoneOutcome>;
    /// Durable Read of the Home Shard cache.
    fn meeting_stone_area(&self, go_guid: u64) -> Result<Option<u32>>;
    /// Durable Read of the party authority, in join order. `None` outside a party.
    fn party_members(&self, actor_guid: u64) -> Result<Option<Vec<u64>>>;
    /// One Seeker's race and class, from whichever World Shard holds it.
    fn seeker_facts(&self, character_guid: u64) -> Result<Option<SeekerFacts>>;
    /// Durable Request on the party authority.
    fn meeting_stone_op(
        &self,
        actor: Actor,
        op: u8,
        area_id: u32,
        seekers: Vec<SeekerFacts>,
    ) -> Result<MeetingStoneOutcome>;
    /// Durable Read of the party authority.
    fn queued_area(&self, character_guid: u64) -> Result<Option<u32>>;
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MeetingStonePlayer {
    pub(crate) account_id: u64,
    /// `None` outside the world.
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum MeetingStoneActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// Consume the meeting stone opcodes and `MSG_LOOKING_FOR_GROUP`, silently outside the world, and
/// pass everything else on.
pub(crate) fn dispatch_meeting_stone_action<St: MeetingStoneActionStore + ?Sized>(
    store: &St,
    player: MeetingStonePlayer,
    msg: ClientOpcodeMessage,
) -> Result<MeetingStoneActionOutcome> {
    let action = match msg {
        ClientOpcodeMessage::CMSG_MEETINGSTONE_JOIN(join) => Action::Join(join.guid.guid()),
        ClientOpcodeMessage::CMSG_MEETINGSTONE_LEAVE => Action::Leave,
        ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO => Action::Info,
        ClientOpcodeMessage::MSG_LOOKING_FOR_GROUP => Action::LookingForGroup,
        other => return Ok(MeetingStoneActionOutcome::PassThrough(other)),
    };
    let Some(actor) = player.self_guid.and_then(Actor::new) else {
        return Ok(MeetingStoneActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    let outbound = match action {
        Action::Join(go_guid) => join(store, player.account_id, actor, go_guid)?,
        Action::Leave => {
            run_op(
                store,
                player.account_id,
                actor,
                realm_op::LEAVE,
                0,
                Vec::new(),
            )?;
            Vec::new()
        }
        Action::Info => info(store, actor)?,
        Action::LookingForGroup => vec![Outbound::One(ServerOpcodeMessage::MSG_LOOKING_FOR_GROUP(
            codec::build_looking_for_group(),
        ))],
    };
    Ok(MeetingStoneActionOutcome::Handled { outbound })
}

enum Action {
    Join(u64),
    Leave,
    Info,
    LookingForGroup,
}

/// Every failure before the queue is silent, as in both cores. A refused party JOIN answers
/// `SMSG_MEETINGSTONE_JOINFAILED`.
fn join<St: MeetingStoneActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    actor: Actor,
    go_guid: u64,
) -> Result<Vec<Outbound>> {
    match ignore_refusal(
        "meeting stone admission",
        store.admit_meeting_stone(actor, go_guid),
    )? {
        Some(MeetingStoneOutcome::Ran) => {}
        Some(MeetingStoneOutcome::Refused(refusal)) => {
            log::debug!(
                "world: meeting stone admission refused {} (account {account_id})",
                refusal.as_tag()
            );
            return Ok(Vec::new());
        }
        None => return Ok(Vec::new()),
    }
    let Some(Some(area_id)) =
        ignore_refusal("meeting stone area read", store.meeting_stone_area(go_guid))?
    else {
        return Ok(Vec::new());
    };
    // Only the Gateway knows which areas the client protocol can name.
    if Area::try_from(area_id).is_err() {
        log::debug!("world: meeting stone {go_guid} names unknown area {area_id}");
        return Ok(Vec::new());
    }
    let Some(seekers) = ignore_refusal(
        "meeting stone seeker facts read",
        facts_for_join(store, actor),
    )?
    else {
        return Ok(Vec::new());
    };
    let outcome = run_op(store, account_id, actor, realm_op::JOIN, area_id, seekers)?;
    let failure = match outcome {
        Some(MeetingStoneOutcome::Refused(refusal)) => join_failure_for(refusal),
        _ => None,
    };
    Ok(failure
        .and_then(codec::build_meetingstone_joinfailed)
        .map(|packet| Outbound::One(ServerOpcodeMessage::SMSG_MEETINGSTONE_JOINFAILED(packet)))
        .into_iter()
        .collect())
}

/// The Seeker facts a JOIN conveys: the actor's, or every member's of its Party. The queue refuses
/// a Raid, so a Raid's actor conveys only its own.
fn facts_for_join<St: MeetingStoneActionStore + ?Sized>(
    store: &St,
    actor: Actor,
) -> Result<Vec<SeekerFacts>> {
    let guids = match store.party_members(actor.guid())? {
        Some(members) if members.len() <= GROUP_MAX_MEMBERS => members,
        _ => vec![actor.guid()],
    };
    let mut facts = Vec::with_capacity(guids.len());
    for guid in guids {
        facts.extend(store.seeker_facts(guid)?);
    }
    Ok(facts)
}

/// `None` when the op failed short of a transport loss.
fn run_op<St: MeetingStoneActionStore + ?Sized>(
    store: &St,
    account_id: u64,
    actor: Actor,
    op: u8,
    area_id: u32,
    seekers: Vec<SeekerFacts>,
) -> Result<Option<MeetingStoneOutcome>> {
    let outcome = ignore_refusal(
        "meeting stone op",
        store.meeting_stone_op(actor, op, area_id, seekers),
    )?;
    if let Some(MeetingStoneOutcome::Refused(refusal)) = outcome {
        log::debug!(
            "world: meeting stone op {op} refused {} (account {account_id})",
            refusal.as_tag()
        );
    }
    Ok(outcome)
}

/// `CMSG_MEETINGSTONE_INFO`, sent after a loading screen: JOINED for a Seeker's area, else NONE.
/// Both cores read a restore map nothing fills; this reports the real state.
fn info<St: MeetingStoneActionStore + ?Sized>(store: &St, actor: Actor) -> Result<Vec<Outbound>> {
    let Some(queued) =
        ignore_refusal("meeting stone status read", store.queued_area(actor.guid()))?
    else {
        return Ok(Vec::new());
    };
    let (area_id, status) = match queued {
        Some(area_id) => (area_id, queue_status::JOINED_QUEUE),
        None => (0, queue_status::NONE),
    };
    Ok(match codec::build_meetingstone_setqueue(area_id, status) {
        Some(packet) => vec![Outbound::One(
            ServerOpcodeMessage::SMSG_MEETINGSTONE_SETQUEUE(packet),
        )],
        None => {
            log::warn!(
                "world: meeting stone status for {} names unknown area {area_id}",
                actor.guid()
            );
            Vec::new()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::ReducerCallError;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{CMSG_MEETINGSTONE_JOIN, CMSG_PING};
    use wow_world_messages::Guid;

    const ACTOR: u64 = 7;
    const STONE: u64 = 900;
    /// The Deadmines.
    const AREA: u32 = 1581;
    const HUMAN: u8 = 1;
    const ORC: u8 = 2;
    const WARRIOR: u8 = 1;
    const PRIEST: u8 = 5;

    /// One `meeting_stone_op` call: actor, op, area and the conveyed facts.
    type QueueOp = (u64, u8, u32, Vec<SeekerFacts>);

    /// Records which database each call is meant for: `home` is the actor's Home Shard and
    /// `authority` the party authority. The Coordinator maps both to its one database on an
    /// unsharded realm.
    #[derive(Default)]
    struct FakeMeetingStones {
        admission: Option<MeetingStoneOutcome>,
        stone_area: Option<u32>,
        party: Option<Vec<u64>>,
        /// Facts by guid, as the World Shards hold them.
        facts: HashMap<u64, SeekerFacts>,
        op_outcome: Option<Result<MeetingStoneOutcome, fn() -> anyhow::Error>>,
        queued: Option<u32>,
        calls: Mutex<Vec<(&'static str, &'static str)>>,
        ops: Mutex<Vec<QueueOp>>,
    }

    impl FakeMeetingStones {
        fn at_stone() -> Self {
            Self {
                admission: Some(MeetingStoneOutcome::Ran),
                stone_area: Some(AREA),
                facts: HashMap::from([(ACTOR, facts(ACTOR, HUMAN, WARRIOR))]),
                ..Default::default()
            }
        }

        fn record(&self, database: &'static str, call: &'static str) {
            self.calls.lock().unwrap().push((database, call));
        }

        fn calls(&self) -> Vec<(&'static str, &'static str)> {
            self.calls.lock().unwrap().clone()
        }

        fn ops(&self) -> Vec<QueueOp> {
            self.ops.lock().unwrap().clone()
        }
    }

    fn facts(character_guid: u64, race: u8, class: u8) -> SeekerFacts {
        SeekerFacts {
            character_guid,
            race,
            class,
        }
    }

    impl MeetingStoneActionStore for FakeMeetingStones {
        fn admit_meeting_stone(&self, _actor: Actor, _go_guid: u64) -> Result<MeetingStoneOutcome> {
            self.record("home", "admit");
            Ok(self.admission.expect("admission configured"))
        }

        fn meeting_stone_area(&self, _go_guid: u64) -> Result<Option<u32>> {
            self.record("home", "stone_area");
            Ok(self.stone_area)
        }

        fn party_members(&self, _actor_guid: u64) -> Result<Option<Vec<u64>>> {
            self.record("authority", "party_members");
            Ok(self.party.clone())
        }

        fn seeker_facts(&self, character_guid: u64) -> Result<Option<SeekerFacts>> {
            Ok(self.facts.get(&character_guid).copied())
        }

        fn meeting_stone_op(
            &self,
            actor: Actor,
            op: u8,
            area_id: u32,
            seekers: Vec<SeekerFacts>,
        ) -> Result<MeetingStoneOutcome> {
            self.record("authority", "op");
            self.ops
                .lock()
                .unwrap()
                .push((actor.guid(), op, area_id, seekers));
            match &self.op_outcome {
                None => Ok(MeetingStoneOutcome::Ran),
                Some(Ok(outcome)) => Ok(*outcome),
                Some(Err(error)) => Err(error()),
            }
        }

        fn queued_area(&self, _character_guid: u64) -> Result<Option<u32>> {
            self.record("authority", "queued_area");
            Ok(self.queued)
        }
    }

    fn in_world() -> MeetingStonePlayer {
        MeetingStonePlayer {
            account_id: 1,
            self_guid: Some(ACTOR),
        }
    }

    fn join_stone() -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_MEETINGSTONE_JOIN(CMSG_MEETINGSTONE_JOIN {
            guid: Guid::new(STONE),
        })
    }

    fn handled(store: &FakeMeetingStones, msg: ClientOpcodeMessage) -> Vec<Outbound> {
        match dispatch_meeting_stone_action(store, in_world(), msg).unwrap() {
            MeetingStoneActionOutcome::Handled { outbound } => outbound,
            MeetingStoneActionOutcome::PassThrough(_) => panic!("the opcode was not consumed"),
        }
    }

    fn joinfailed_reason(outbound: &[Outbound]) -> u8 {
        match outbound {
            [Outbound::One(ServerOpcodeMessage::SMSG_MEETINGSTONE_JOINFAILED(packet))] => {
                packet.reason.as_int()
            }
            other => panic!("expected one JOINFAILED, got {} packets", other.len()),
        }
    }

    fn setqueue(outbound: &[Outbound]) -> (u32, u8) {
        match outbound {
            [Outbound::One(ServerOpcodeMessage::SMSG_MEETINGSTONE_SETQUEUE(packet))] => {
                (packet.area.as_int(), packet.status.as_int())
            }
            other => panic!("expected one SETQUEUE, got {} packets", other.len()),
        }
    }

    #[test]
    fn a_refused_admission_stops_before_the_party_authority() {
        let store = FakeMeetingStones {
            admission: Some(MeetingStoneOutcome::Refused(
                MeetingStoneRefusal::LevelOutOfRange,
            )),
            ..FakeMeetingStones::at_stone()
        };

        assert!(handled(&store, join_stone()).is_empty());
        assert_eq!(store.calls(), [("home", "admit")]);
    }

    #[test]
    fn a_lone_actor_joins_with_its_own_facts_on_the_party_authority() {
        let store = FakeMeetingStones::at_stone();

        assert!(handled(&store, join_stone()).is_empty());
        assert_eq!(
            store.calls(),
            [
                ("home", "admit"),
                ("home", "stone_area"),
                ("authority", "party_members"),
                ("authority", "op"),
            ]
        );
        assert_eq!(
            store.ops(),
            [(
                ACTOR,
                realm_op::JOIN,
                AREA,
                vec![facts(ACTOR, HUMAN, WARRIOR)]
            )]
        );
    }

    /// Each party Refusal answers with cmangos's failure byte.
    #[test]
    fn each_party_refusal_answers_its_join_failure_byte() {
        for (refusal, reason) in [
            (MeetingStoneRefusal::NotLeader, 1),
            (MeetingStoneRefusal::PartyFull, 2),
            (MeetingStoneRefusal::RaidGroup, 3),
        ] {
            let store = FakeMeetingStones {
                op_outcome: Some(Ok(MeetingStoneOutcome::Refused(refusal))),
                ..FakeMeetingStones::at_stone()
            };
            assert_eq!(
                joinfailed_reason(&handled(&store, join_stone())),
                reason,
                "{refusal:?}"
            );
        }
        let silent = FakeMeetingStones {
            op_outcome: Some(Ok(MeetingStoneOutcome::Refused(
                MeetingStoneRefusal::ActorUnavailable,
            ))),
            ..FakeMeetingStones::at_stone()
        };
        assert!(handled(&silent, join_stone()).is_empty());
    }

    #[test]
    fn a_stone_whose_area_the_client_cannot_name_stops_before_the_party_authority() {
        let store = FakeMeetingStones {
            stone_area: Some(0xFFFF_FFFF),
            ..FakeMeetingStones::at_stone()
        };

        assert!(handled(&store, join_stone()).is_empty());
        assert_eq!(store.calls(), [("home", "admit"), ("home", "stone_area")]);
    }

    /// The members stand on different World Shards; the Fake's facts table is that realm-wide
    /// read. A member no Shard can name is left out, and the Module gives it class 0.
    #[test]
    fn a_party_join_carries_every_members_facts() {
        const PRIEST_GUID: u64 = 8;
        const UNKNOWN: u64 = 9;
        let store = FakeMeetingStones {
            party: Some(vec![ACTOR, PRIEST_GUID, UNKNOWN]),
            facts: HashMap::from([
                (ACTOR, facts(ACTOR, ORC, WARRIOR)),
                (PRIEST_GUID, facts(PRIEST_GUID, ORC, PRIEST)),
            ]),
            ..FakeMeetingStones::at_stone()
        };

        handled(&store, join_stone());
        assert_eq!(
            store.ops(),
            [(
                ACTOR,
                realm_op::JOIN,
                AREA,
                vec![facts(ACTOR, ORC, WARRIOR), facts(PRIEST_GUID, ORC, PRIEST)]
            )]
        );
    }

    /// A Raid's 40 members are not read for a JOIN the queue refuses anyway.
    #[test]
    fn a_raid_join_carries_only_the_actors_facts() {
        let store = FakeMeetingStones {
            party: Some((ACTOR..ACTOR + 6).collect()),
            op_outcome: Some(Ok(MeetingStoneOutcome::Refused(
                MeetingStoneRefusal::RaidGroup,
            ))),
            ..FakeMeetingStones::at_stone()
        };

        assert_eq!(joinfailed_reason(&handled(&store, join_stone())), 3);
        assert_eq!(store.ops()[0].3, [facts(ACTOR, HUMAN, WARRIOR)]);
    }

    #[test]
    fn leave_runs_the_op_and_answers_nothing_itself() {
        let store = FakeMeetingStones::default();

        assert!(handled(&store, ClientOpcodeMessage::CMSG_MEETINGSTONE_LEAVE).is_empty());
        assert_eq!(store.calls(), [("authority", "op")]);
        assert_eq!(store.ops(), [(ACTOR, realm_op::LEAVE, 0, Vec::new())]);
    }

    #[test]
    fn info_answers_joined_for_a_seeker_and_none_otherwise() {
        let queued = FakeMeetingStones {
            queued: Some(AREA),
            ..Default::default()
        };
        assert_eq!(
            setqueue(&handled(
                &queued,
                ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO
            )),
            (AREA, queue_status::JOINED_QUEUE)
        );
        assert_eq!(queued.calls(), [("authority", "queued_area")]);

        let idle = FakeMeetingStones::default();
        assert_eq!(
            setqueue(&handled(&idle, ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO)),
            (0, queue_status::NONE)
        );
    }

    /// vmangos answers 0.
    #[test]
    fn looking_for_group_answers_zero() {
        let store = FakeMeetingStones::default();
        match handled(&store, ClientOpcodeMessage::MSG_LOOKING_FOR_GROUP).as_slice() {
            [Outbound::One(ServerOpcodeMessage::MSG_LOOKING_FOR_GROUP(answer))] => {
                assert_eq!(answer.unknown1, 0)
            }
            _ => panic!("expected one MSG_LOOKING_FOR_GROUP"),
        }
        assert!(store.calls().is_empty());
    }

    #[test]
    fn every_opcode_is_silent_outside_the_world() {
        let store = FakeMeetingStones::at_stone();
        let outside = MeetingStonePlayer {
            account_id: 1,
            self_guid: None,
        };
        for msg in [
            join_stone(),
            ClientOpcodeMessage::CMSG_MEETINGSTONE_LEAVE,
            ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO,
            ClientOpcodeMessage::MSG_LOOKING_FOR_GROUP,
        ] {
            match dispatch_meeting_stone_action(&store, outside, msg).unwrap() {
                MeetingStoneActionOutcome::Handled { outbound } => assert!(outbound.is_empty()),
                MeetingStoneActionOutcome::PassThrough(_) => panic!("not consumed"),
            }
        }
        assert!(store.calls().is_empty());
    }

    /// A Refusal the Module did not tag keeps the World Session; a Transport Loss ends it.
    #[test]
    fn only_a_transport_loss_ends_the_session() {
        let untagged = FakeMeetingStones {
            op_outcome: Some(Err(|| {
                ReducerCallError::refused("realm_meeting_stone_op", "boom").into()
            })),
            ..FakeMeetingStones::at_stone()
        };
        assert!(handled(&untagged, join_stone()).is_empty());

        let lost = FakeMeetingStones {
            op_outcome: Some(Err(|| {
                ReducerCallError::transport_lost("realm_meeting_stone_op").into()
            })),
            ..FakeMeetingStones::at_stone()
        };
        assert!(dispatch_meeting_stone_action(&lost, in_world(), join_stone()).is_err());
    }

    #[test]
    fn other_opcodes_pass_through() {
        let store = FakeMeetingStones::default();
        let msg = ClientOpcodeMessage::CMSG_PING(CMSG_PING::default());
        assert!(matches!(
            dispatch_meeting_stone_action(&store, in_world(), msg).unwrap(),
            MeetingStoneActionOutcome::PassThrough(_)
        ));
    }
}
