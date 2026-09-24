//! Chat Channel dispatcher: join, leave, password, the member list and the owner query. Ops are
//! Durable Requests on Realm-core; their success notices come back on the Relay
//! (`stdb::world_view::channel_notice_appeared`). A Refusal rolls back its transaction, so this
//! file answers it to the actor alone. Channel speech is a `CMSG_MESSAGECHAT` kind and lives in
//! `chat.rs`.

use super::super::*;
use super::chat::{is_transport_failure, ChatActionPlayer, ChatActionStore, SpeakerFacts};
use lyracore_shared::channel::{channel_op, notice, ChannelRefusal};
use lyracore_shared::faction::team_for_race;

/// One channel op on its way to the `realm_channel_op` reducer. The Coordinator conveys every field
/// but the speaker's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelRequest {
    /// As the client typed it.
    pub(crate) channel_name: String,
    pub(crate) password: String,
    pub(crate) target_guid: u64,
    pub(crate) target_name: String,
    pub(crate) target_race: u8,
    pub(crate) target_ignores_actor: bool,
    pub(crate) speaker: SpeakerFacts,
}

/// How the Module answered one channel op. A timeout, transport or SDK failure stays an `Err` with
/// an unknown durable outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelOutcome {
    Done,
    Refused(ChannelRefusal),
}

/// One Chat Channel as the Realm-core cache holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelRoster {
    /// The stored spelling.
    pub(crate) name: String,
    /// Wire channel flags.
    pub(crate) flags: u8,
    /// 0 for none.
    pub(crate) owner_guid: u64,
    /// The owner's name from whichever World Shard holds the Character. Empty when unresolved.
    pub(crate) owner_name: String,
    /// `(guid, raw member flags)` in join order.
    pub(crate) members: Vec<(u64, u8)>,
}

pub(crate) trait ChannelActionStore: ChatActionStore {
    /// Durable Request on Realm-core. The Coordinator picks the database; handlers never do.
    fn channel_op(
        &self,
        actor_guid: u64,
        op: u8,
        request: ChannelRequest,
    ) -> Result<ChannelOutcome>;
    /// Durable Read of the Realm-core cache. `None` when `team` has no channel by that name.
    fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>>;
}

impl ChannelActionStore for crate::stdb::Coordinator {
    fn channel_op(
        &self,
        actor_guid: u64,
        op: u8,
        request: ChannelRequest,
    ) -> Result<ChannelOutcome> {
        crate::stdb::Coordinator::channel_op(self, actor_guid, op, request)
    }

    fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>> {
        crate::stdb::Coordinator::channel_roster(self, team, channel_name)
    }
}

pub(crate) enum ChannelActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// What CHANNEL_OWNER names when a channel has no owner (vm:Channel.cpp:881-890).
const NO_OWNER: &str = "Nobody";
/// What CHANNEL_OWNER names when the owner's Character cannot be found (cm:Channel.cpp:817-824).
const OWNER_NOT_FOUND: &str = "PLAYER_NOT_FOUND";

/// Consume the channel opcodes and pass everything else on.
pub(crate) fn dispatch_channel_action<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<ChannelActionOutcome> {
    let outbound = match msg {
        // An empty name is dropped (cm:ChannelHandler.cpp:66-67, cm:ChannelHandler.cpp:88-89).
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(join) if join.channel_name.is_empty() => Vec::new(),
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(join) => run_op(
            store,
            player,
            channel_op::JOIN,
            join.channel_name,
            join.channel_password,
        )?,
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(leave) if leave.channel_name.is_empty() => {
            Vec::new()
        }
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(leave) => run_op(
            store,
            player,
            channel_op::LEAVE,
            leave.channel_name,
            String::new(),
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(password) => run_op(
            store,
            player,
            channel_op::PASSWORD,
            password.channel_name,
            password.channel_password,
        )?,
        ClientOpcodeMessage::CMSG_CHANNEL_LIST(list) => {
            read_roster(store, player, list.channel_name, |roster| {
                Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_LIST(Box::new(
                    codec::channel::build_channel_list(roster.name, roster.flags, &roster.members),
                )))
            })?
        }
        ClientOpcodeMessage::CMSG_CHANNEL_OWNER(owner) => {
            let typed = owner.channel_name.clone();
            read_roster(store, player, owner.channel_name, |roster| {
                let owner_name = match (roster.owner_guid, roster.owner_name) {
                    (0, _) => NO_OWNER.to_string(),
                    (_, name) if name.is_empty() => OWNER_NOT_FOUND.to_string(),
                    (_, name) => name,
                };
                notice_outbound(codec::channel::ChannelNoticeView {
                    notice: notice::CHANNEL_OWNER,
                    channel_name: typed,
                    text: owner_name,
                    ..Default::default()
                })
            })?
        }
        other => return Ok(ChannelActionOutcome::PassThrough(other)),
    };
    Ok(ChannelActionOutcome::Handled { outbound })
}

/// Run one op as the player. Success notices return on the Relay; a Refusal is answered here. Only
/// a lost reducer transport is fatal.
fn run_op<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    op: u8,
    channel_name: String,
    password: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some(speaker) = store.speaker_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let request = ChannelRequest {
        channel_name: channel_name.clone(),
        password,
        target_guid: 0,
        target_name: String::new(),
        target_race: 0,
        target_ignores_actor: false,
        speaker,
    };
    match store.channel_op(actor_guid, op, request) {
        Ok(ChannelOutcome::Done) => Ok(Vec::new()),
        Ok(ChannelOutcome::Refused(refusal)) => Ok(vec![refusal_notice(
            refusal,
            channel_name,
            actor_guid,
            String::new(),
        )]),
        Err(error) if is_transport_failure(&error) => Err(error),
        Err(error) => {
            log::debug!(
                "world: channel op {op} dropped (account {}): {error:#}",
                player.account_id
            );
            Ok(Vec::new())
        }
    }
}

/// Answer a read with `answer` when the player is a member of the named channel, else NOT_MEMBER
/// with the name the client typed (cm:Channel.cpp:446-456, cm:Channel.cpp:479-488).
fn read_roster<St: ChannelActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    channel_name: String,
    answer: impl FnOnce(ChannelRoster) -> Outbound,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some(speaker) = store.speaker_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let roster = match store.channel_roster(team_for_race(speaker.race), &channel_name) {
        Ok(roster) => roster,
        Err(error) => {
            log::debug!(
                "world: channel read dropped (account {}): {error:#}",
                player.account_id
            );
            return Ok(Vec::new());
        }
    };
    let member = roster.filter(|roster| roster.members.iter().any(|&(guid, _)| guid == actor_guid));
    Ok(vec![match member {
        Some(roster) => answer(roster),
        None => refusal_notice(
            ChannelRefusal::NotMember,
            channel_name,
            actor_guid,
            String::new(),
        ),
    }])
}

/// The notice a Refusal answers to the actor alone, naming the channel as the client typed it.
/// PLAYER_ALREADY_MEMBER names `subject_guid`. The name notices carry `target_name`, the name the
/// client typed.
pub(super) fn refusal_notice(
    refusal: ChannelRefusal,
    channel_name: String,
    subject_guid: u64,
    target_name: String,
) -> Outbound {
    notice_outbound(codec::channel::ChannelNoticeView {
        notice: refusal.notice(),
        channel_name,
        subject_guid,
        text: target_name,
        ..Default::default()
    })
}

fn notice_outbound(view: codec::channel::ChannelNoticeView) -> Outbound {
    let (opcode, body) = codec::channel::notify(&view);
    Outbound::Raw { opcode, body }
}

#[cfg(test)]
mod tests {
    use super::super::chat::{ChatOutcome, RealmChatRequest};
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        CMSG_CHANNEL_LIST, CMSG_CHANNEL_OWNER, CMSG_CHANNEL_PASSWORD, CMSG_JOIN_CHANNEL,
        CMSG_LEAVE_CHANNEL, CMSG_PING,
    };

    #[derive(Default)]
    struct InMemoryChannelActions {
        facts: Option<SpeakerFacts>,
        outcome: Option<Result<ChannelOutcome, String>>,
        roster: Option<ChannelRoster>,
        roster_failure: bool,
        ops: Mutex<Vec<(u64, u8, ChannelRequest)>>,
        roster_reads: Mutex<Vec<(u32, String)>>,
    }

    impl ChatActionStore for InMemoryChannelActions {
        fn speaker_facts(&self, _speaker_guid: u64) -> Result<Option<SpeakerFacts>> {
            Ok(self.facts.clone())
        }

        fn realm_chat(
            &self,
            _speaker_guid: u64,
            _request: RealmChatRequest,
        ) -> Result<ChatOutcome> {
            Ok(ChatOutcome::Delivered)
        }
    }

    impl ChannelActionStore for InMemoryChannelActions {
        fn channel_op(
            &self,
            actor_guid: u64,
            op: u8,
            request: ChannelRequest,
        ) -> Result<ChannelOutcome> {
            self.ops.lock().unwrap().push((actor_guid, op, request));
            match &self.outcome {
                None => Ok(ChannelOutcome::Done),
                Some(Ok(outcome)) => Ok(*outcome),
                Some(Err(failure)) => Err(anyhow::anyhow!("{failure}")),
            }
        }

        fn channel_roster(&self, team: u32, channel_name: &str) -> Result<Option<ChannelRoster>> {
            self.roster_reads
                .lock()
                .unwrap()
                .push((team, channel_name.to_string()));
            if self.roster_failure {
                anyhow::bail!("realm-core database lyracore-realm is not connected");
            }
            Ok(self.roster.clone())
        }
    }

    const ACTOR: u64 = 42;

    fn orc() -> SpeakerFacts {
        SpeakerFacts {
            race: 2,
            chat_tag: 0,
            name: "Grunt".to_string(),
        }
    }

    fn store(outcome: Option<Result<ChannelOutcome, String>>) -> InMemoryChannelActions {
        InMemoryChannelActions {
            facts: Some(orc()),
            outcome,
            ..Default::default()
        }
    }

    fn player() -> ChatActionPlayer {
        ChatActionPlayer {
            account_id: 7,
            self_guid: Some(ACTOR),
        }
    }

    fn join(name: &str, password: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(Box::new(CMSG_JOIN_CHANNEL {
            channel_name: name.to_string(),
            channel_password: password.to_string(),
        }))
    }

    fn leave(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(Box::new(CMSG_LEAVE_CHANNEL {
            channel_name: name.to_string(),
        }))
    }

    fn list(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_LIST(Box::new(CMSG_CHANNEL_LIST {
            channel_name: name.to_string(),
        }))
    }

    fn owner(name: &str) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CHANNEL_OWNER(Box::new(CMSG_CHANNEL_OWNER {
            channel_name: name.to_string(),
        }))
    }

    fn handled(outcome: ChannelActionOutcome) -> Vec<Outbound> {
        match outcome {
            ChannelActionOutcome::Handled { outbound } => outbound,
            ChannelActionOutcome::PassThrough(msg) => panic!("expected Handled, got {msg}"),
        }
    }

    /// The single raw packet `outbound` holds, as `(opcode, body)`.
    fn only_raw(outbound: Vec<Outbound>) -> (u16, Vec<u8>) {
        let mut outbound = outbound.into_iter();
        match (outbound.next(), outbound.next()) {
            (Some(Outbound::Raw { opcode, body }), None) => (opcode, body),
            _ => panic!("expected exactly one raw packet"),
        }
    }

    fn roster(members: &[(u64, u8)], owner_guid: u64, owner_name: &str) -> ChannelRoster {
        ChannelRoster {
            name: "Raiders".to_string(),
            flags: 0x01,
            owner_guid,
            owner_name: owner_name.to_string(),
            members: members.to_vec(),
        }
    }

    #[test]
    fn a_join_conveys_the_typed_name_the_password_and_the_speaker() {
        let store = store(None);
        let outbound =
            handled(dispatch_channel_action(&store, player(), join("Raiders", "hunter2")).unwrap());
        assert!(outbound.is_empty(), "YOU_JOINED returns on the Relay");
        assert_eq!(
            store.ops.lock().unwrap().as_slice(),
            &[(
                ACTOR,
                0,
                ChannelRequest {
                    channel_name: "Raiders".to_string(),
                    password: "hunter2".to_string(),
                    target_guid: 0,
                    target_name: String::new(),
                    target_race: 0,
                    target_ignores_actor: false,
                    speaker: orc(),
                }
            )]
        );
    }

    #[test]
    fn a_leave_and_a_password_change_run_their_ops() {
        let store = store(None);
        handled(dispatch_channel_action(&store, player(), leave("Raiders")).unwrap());
        let password =
            ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(Box::new(CMSG_CHANNEL_PASSWORD {
                channel_name: "Raiders".to_string(),
                channel_password: "sesame".to_string(),
            }));
        handled(dispatch_channel_action(&store, player(), password).unwrap());
        let ops = store.ops.lock().unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!((ops[0].1, ops[0].2.channel_name.as_str()), (1, "Raiders"));
        assert_eq!(
            (ops[1].1, ops[1].2.password.as_str()),
            (2, "sesame"),
            "PASSWORD carries the new password"
        );
    }

    #[test]
    fn an_empty_name_does_nothing() {
        let store = store(None);
        for msg in [join("", ""), leave("")] {
            let outbound = handled(dispatch_channel_action(&store, player(), msg).unwrap());
            assert!(outbound.is_empty());
        }
        assert!(store.ops.lock().unwrap().is_empty());
    }

    /// NOT_MEMBER 0x05 with the typed name and no payload.
    #[test]
    fn leaving_a_channel_you_are_not_on_answers_not_member_with_the_typed_name() {
        let store = store(Some(Ok(ChannelOutcome::Refused(ChannelRefusal::NotMember))));
        let outbound =
            handled(dispatch_channel_action(&store, player(), leave("rAiders")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (
                0x0099,
                vec![0x05, b'r', b'A', b'i', b'd', b'e', b'r', b's', 0]
            )
        );
    }

    #[test]
    fn each_join_refusal_answers_its_notice() {
        for (refusal, code) in [
            (ChannelRefusal::InvalidName, 0x1B),
            (ChannelRefusal::WrongPassword, 0x04),
            (ChannelRefusal::Banned, 0x13),
        ] {
            let store = store(Some(Ok(ChannelOutcome::Refused(refusal))));
            let outbound =
                handled(dispatch_channel_action(&store, player(), join("1x", "")).unwrap());
            assert_eq!(
                only_raw(outbound),
                (0x0099, vec![code, b'1', b'x', 0]),
                "{refusal:?}"
            );
        }
    }

    /// PLAYER_ALREADY_MEMBER 0x17 names the joiner (cm:Channel.cpp:64-72).
    #[test]
    fn a_repeat_join_of_a_custom_channel_names_the_joiner() {
        let store = store(Some(Ok(ChannelOutcome::Refused(
            ChannelRefusal::PlayerAlreadyMember,
        ))));
        let outbound = handled(dispatch_channel_action(&store, player(), join("Rx", "")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, vec![0x17, b'R', b'x', 0, 42, 0, 0, 0, 0, 0, 0, 0])
        );
    }

    #[test]
    fn a_non_moderator_setting_the_password_answers_not_moderator() {
        let store = store(Some(Ok(ChannelOutcome::Refused(
            ChannelRefusal::NotModerator,
        ))));
        let password =
            ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(Box::new(CMSG_CHANNEL_PASSWORD {
                channel_name: "Rx".to_string(),
                channel_password: "x".to_string(),
            }));
        let outbound = handled(dispatch_channel_action(&store, player(), password).unwrap());
        assert_eq!(only_raw(outbound), (0x0099, vec![0x06, b'R', b'x', 0]));
    }

    #[test]
    fn a_lost_reducer_transport_ends_the_session() {
        let store = store(Some(Err(
            "realm_channel_op reducer transport disconnected: channel closed".to_string(),
        )));
        let error = dispatch_channel_action(&store, player(), join("Rx", ""))
            .err()
            .expect("transport loss is fatal");
        assert!(error.to_string().contains("transport disconnected"));
    }

    #[test]
    fn any_other_failure_drops_the_op_and_keeps_the_session() {
        let store = store(Some(Err(
            "realm_channel_op reducer timed out after 10s".to_string()
        )));
        let outbound = handled(dispatch_channel_action(&store, player(), join("Rx", "")).unwrap());
        assert!(outbound.is_empty());
    }

    #[test]
    fn an_actor_without_a_live_entity_sends_nothing() {
        let store = InMemoryChannelActions::default();
        let outbound = handled(dispatch_channel_action(&store, player(), join("Rx", "")).unwrap());
        assert!(outbound.is_empty());
        assert!(store.ops.lock().unwrap().is_empty());
    }

    /// cm:Channel.cpp:490-515: the stored name and flags, then each member with raw flag bits.
    /// An Orc (race 2) reads the Horde's channels.
    #[test]
    fn the_member_list_carries_every_member_with_raw_flag_bits() {
        let store = InMemoryChannelActions {
            roster: Some(roster(&[(9, 0x03), (ACTOR, 0x08)], 9, "Thrall")),
            ..store(None)
        };
        let outbound = handled(dispatch_channel_action(&store, player(), list("raiders")).unwrap());
        assert_eq!(
            store.roster_reads.lock().unwrap().as_slice(),
            &[(67, "raiders".to_string())]
        );
        let mut outbound = outbound.into_iter();
        let Some(Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_LIST(list))) = outbound.next()
        else {
            panic!("expected SMSG_CHANNEL_LIST");
        };
        assert!(outbound.next().is_none());
        assert_eq!(list.channel_name, "Raiders");
        assert_eq!(list.channel_flags.as_int(), 0x01);
        let members: Vec<(u64, u8)> = list
            .members
            .iter()
            .map(|member| (member.guid.guid(), member.member_flags.as_int()))
            .collect();
        assert_eq!(members, [(9, 0x03), (ACTOR, 0x08)]);
    }

    #[test]
    fn a_non_member_asking_for_the_list_or_the_owner_gets_not_member() {
        for msg in [list("Raiders"), owner("Raiders")] {
            let store = InMemoryChannelActions {
                roster: Some(roster(&[(9, 0x03)], 9, "Thrall")),
                ..store(None)
            };
            let outbound = handled(dispatch_channel_action(&store, player(), msg).unwrap());
            assert_eq!(
                only_raw(outbound),
                (0x0099, [&[0x05][..], b"Raiders\0"].concat())
            );
        }
        let store = store(None);
        let outbound = handled(dispatch_channel_action(&store, player(), list("Nowhere")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x05][..], b"Nowhere\0"].concat()),
            "no channel at all"
        );
    }

    #[test]
    fn the_owner_query_names_the_owner() {
        let store = InMemoryChannelActions {
            roster: Some(roster(&[(9, 0x03), (ACTOR, 0)], 9, "Thrall")),
            ..store(None)
        };
        let outbound =
            handled(dispatch_channel_action(&store, player(), owner("Raiders")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x0B][..], b"Raiders\0Thrall\0"].concat())
        );
    }

    /// vm:Channel.cpp:881-890.
    #[test]
    fn a_channel_without_an_owner_names_nobody() {
        let store = InMemoryChannelActions {
            roster: Some(roster(&[(ACTOR, 0)], 0, "")),
            ..store(None)
        };
        let outbound =
            handled(dispatch_channel_action(&store, player(), owner("Trade - City")).unwrap());
        assert_eq!(
            only_raw(outbound),
            (0x0099, [&[0x0B][..], b"Trade - City\0Nobody\0"].concat())
        );
    }

    #[test]
    fn a_failed_read_answers_nothing() {
        let store = InMemoryChannelActions {
            roster_failure: true,
            ..store(None)
        };
        let outbound = handled(dispatch_channel_action(&store, player(), list("Raiders")).unwrap());
        assert!(outbound.is_empty());
    }

    #[test]
    fn another_opcode_passes_through() {
        let store = store(None);
        let outcome = dispatch_channel_action(
            &store,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            ChannelActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }
}
