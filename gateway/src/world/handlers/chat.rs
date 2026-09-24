//! Realm Chat dispatcher: the `CMSG_MESSAGECHAT` kinds that become Realm Chat Lines. The Gateway
//! reads the Speaker Facts on the Home Shard, the Module decides the audience on Realm-core, and
//! the Relay (`stdb::world_view::realm_chat_appeared`) delivers the line. Party and channel lines
//! are Realm Chat Lines. Say, yell and every kind this file does not own pass through.

use super::super::*;
use lyracore_shared::chat::{chat_kind, ChatRefusal};
use wow_world_messages::vanilla::SMSG_NOTIFICATION;

/// What the speaker's Home Shard knows about them. The Coordinator conveys race and chat tag to
/// the Module; `name` stays in the Gateway for lines that must name the speaker to someone else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpeakerFacts {
    pub(crate) race: u8,
    pub(crate) chat_tag: u8,
    pub(crate) name: String,
}

/// One client line on its way to the `realm_chat` reducer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RealmChatRequest {
    pub(crate) kind: u8,
    pub(crate) language: u32,
    /// CHANNEL only.
    pub(crate) channel_name: String,
    /// IGNORED only.
    pub(crate) target_guid: u64,
    pub(crate) message: String,
    pub(crate) speaker: SpeakerFacts,
}

/// How the Module answered one Realm Chat request. A timeout, transport or SDK failure stays an
/// `Err` with an unknown durable outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChatOutcome {
    Delivered,
    Refused(ChatRefusal),
}

pub(crate) trait ChatActionStore: Send + Sync {
    /// Durable Read on the speaker's Home Shard. `None` when the speaker has no live entity.
    fn speaker_facts(&self, speaker_guid: u64) -> Result<Option<SpeakerFacts>>;
    /// Durable Request on Realm-core. The Coordinator picks the database; handlers never do.
    fn realm_chat(&self, speaker_guid: u64, request: RealmChatRequest) -> Result<ChatOutcome>;
}

impl ChatActionStore for crate::stdb::Coordinator {
    fn speaker_facts(&self, speaker_guid: u64) -> Result<Option<SpeakerFacts>> {
        crate::stdb::Coordinator::speaker_facts(self, speaker_guid)
    }

    fn realm_chat(&self, speaker_guid: u64, request: RealmChatRequest) -> Result<ChatOutcome> {
        crate::stdb::Coordinator::realm_chat(self, speaker_guid, request)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChatActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum ChatActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// cm mangos.sql:4044, sent by cm:ChatHandler.cpp:107-110.
const UNKNOWN_LANGUAGE_NOTICE: &str = "You don't know that language";

/// The wire `chat_kind` for a Raid, Raid Leader or Raid Warning line; `None` for every other
/// `CMSG_MESSAGECHAT_ChatType`, Party included (Party keeps its own arm for its special
/// `SMSG_PARTY_COMMAND_RESULT` Refusal). The three share one audience family on the Module side
/// and answer with the same silent Refusal here, so one arm covers all three.
fn raid_chat_kind(chat_type: &CMSG_MESSAGECHAT_ChatType) -> Option<u8> {
    match chat_type {
        CMSG_MESSAGECHAT_ChatType::Raid => Some(chat_kind::RAID),
        CMSG_MESSAGECHAT_ChatType::RaidLeader => Some(chat_kind::RAID_LEADER),
        CMSG_MESSAGECHAT_ChatType::RaidWarning => Some(chat_kind::RAID_WARNING),
        _ => None,
    }
}

/// Consume the `CMSG_MESSAGECHAT` kinds that are Realm Chat Lines and pass everything else on. A
/// new Chat Kind adds one arm here and answers its own Refusals before the shared ones.
pub(crate) fn dispatch_chat_action<St: ChatActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<ChatActionOutcome> {
    let CMSG_MESSAGECHAT {
        chat_type,
        language,
        message,
    } = match msg {
        ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) => *chat,
        other => return Ok(ChatActionOutcome::PassThrough(other)),
    };
    let outbound = match chat_type {
        CMSG_MESSAGECHAT_ChatType::Party => {
            match send_line(store, player, |speaker| RealmChatRequest {
                kind: chat_kind::PARTY,
                language: language.as_int(),
                channel_name: String::new(),
                target_guid: 0,
                message,
                speaker,
            })? {
                // Speaking from no party is today's "You aren't in a party" answer.
                Some(ChatRefusal::NotInGroup) => vec![Outbound::One(
                    ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                        codec::build_party_command_result(
                            PartyOperation::Leave,
                            String::new(),
                            PartyResult::NotInGroup,
                        ),
                    )),
                )],
                refusal => refusal_outbound(player, refusal),
            }
        }
        CMSG_MESSAGECHAT_ChatType::Channel { channel } => {
            let typed = channel.clone();
            match send_line(store, player, |speaker| RealmChatRequest {
                kind: chat_kind::CHANNEL,
                language: language.as_int(),
                channel_name: channel,
                target_guid: 0,
                message,
                speaker,
            })? {
                Some(ChatRefusal::Channel(refusal)) => vec![super::channel::refusal_notice(
                    refusal,
                    typed,
                    player.self_guid.unwrap_or(0),
                    String::new(),
                )],
                refusal => refusal_outbound(player, refusal),
            }
        }
        chat_type => {
            let Some(kind) = raid_chat_kind(&chat_type) else {
                return Ok(ChatActionOutcome::PassThrough(
                    ClientOpcodeMessage::CMSG_MESSAGECHAT(Box::new(CMSG_MESSAGECHAT {
                        chat_type,
                        language,
                        message,
                    })),
                ));
            };
            let refusal = send_line(store, player, |speaker| RealmChatRequest {
                kind,
                language: language.as_int(),
                channel_name: String::new(),
                target_guid: 0,
                message,
                speaker,
            })?;
            refusal_outbound(player, refusal)
        }
    };
    Ok(ChatActionOutcome::Handled { outbound })
}

/// Send one line through the Realm Chat path. `request` builds the Durable Request from the
/// Speaker Facts. `Ok(None)` means the line went out or was dropped; a Refusal comes back for the
/// calling arm to answer. Only a lost reducer transport is fatal.
fn send_line<St: ChatActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    request: impl FnOnce(SpeakerFacts) -> RealmChatRequest,
) -> Result<Option<ChatRefusal>> {
    let Some(speaker_guid) = player.self_guid else {
        return Ok(None);
    };
    let Some(speaker) = store.speaker_facts(speaker_guid)? else {
        return Ok(None);
    };
    let request = request(speaker);
    let kind = request.kind;
    match store.realm_chat(speaker_guid, request) {
        Ok(ChatOutcome::Delivered) => Ok(None),
        Ok(ChatOutcome::Refused(refusal)) => Ok(Some(refusal)),
        Err(error) if is_transport_failure(&error) => Err(error),
        Err(error) => {
            log::debug!(
                "world: chat kind {kind} dropped (account {}): {error:#}",
                player.account_id
            );
            Ok(None)
        }
    }
}

/// The answer every Chat Kind shares. Vanilla answers most chat Refusals with silence.
fn refusal_outbound(player: ChatActionPlayer, refusal: Option<ChatRefusal>) -> Vec<Outbound> {
    match refusal {
        None => Vec::new(),
        Some(ChatRefusal::UnknownLanguage) => vec![Outbound::One(
            ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
                notification: UNKNOWN_LANGUAGE_NOTICE.to_string(),
            })),
        )],
        Some(other) => {
            log::debug!(
                "world: chat refused (account {}): {}",
                player.account_id,
                other.as_tag()
            );
            Vec::new()
        }
    }
}

/// A dead reducer transport cannot serve any further request, so it ends the World Session.
pub(super) fn is_transport_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::channel::ChannelRefusal;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{Language, CMSG_PING};

    #[derive(Default)]
    struct InMemoryChatActions {
        facts: Option<SpeakerFacts>,
        outcome: Option<Result<ChatOutcome, String>>,
        facts_reads: Mutex<Vec<u64>>,
        requests: Mutex<Vec<(u64, RealmChatRequest)>>,
    }

    impl ChatActionStore for InMemoryChatActions {
        fn speaker_facts(&self, speaker_guid: u64) -> Result<Option<SpeakerFacts>> {
            self.facts_reads.lock().unwrap().push(speaker_guid);
            Ok(self.facts.clone())
        }

        fn realm_chat(&self, speaker_guid: u64, request: RealmChatRequest) -> Result<ChatOutcome> {
            self.requests.lock().unwrap().push((speaker_guid, request));
            match &self.outcome {
                None => Ok(ChatOutcome::Delivered),
                Some(Ok(outcome)) => Ok(*outcome),
                Some(Err(failure)) => Err(anyhow::anyhow!("{failure}")),
            }
        }
    }

    fn speaker() -> SpeakerFacts {
        SpeakerFacts {
            race: 1,
            chat_tag: 2,
            name: "Speaker".to_string(),
        }
    }

    fn store(outcome: Option<Result<ChatOutcome, String>>) -> InMemoryChatActions {
        InMemoryChatActions {
            facts: Some(speaker()),
            outcome,
            ..Default::default()
        }
    }

    fn player() -> ChatActionPlayer {
        ChatActionPlayer {
            account_id: 7,
            self_guid: Some(42),
        }
    }

    fn line(chat_type: CMSG_MESSAGECHAT_ChatType, language: Language) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_MESSAGECHAT(Box::new(CMSG_MESSAGECHAT {
            chat_type,
            language,
            message: "form up".to_string(),
        }))
    }

    fn party(language: Language) -> ClientOpcodeMessage {
        line(CMSG_MESSAGECHAT_ChatType::Party, language)
    }

    fn handled(outcome: ChatActionOutcome) -> Vec<Outbound> {
        match outcome {
            ChatActionOutcome::Handled { outbound } => outbound,
            ChatActionOutcome::PassThrough(msg) => panic!("expected Handled, got {msg}"),
        }
    }

    fn only(outbound: Vec<Outbound>) -> ServerOpcodeMessage {
        let mut outbound = outbound.into_iter();
        match (outbound.next(), outbound.next()) {
            (Some(Outbound::One(msg)), None) => msg,
            _ => panic!("expected exactly one packet"),
        }
    }

    #[test]
    fn a_party_line_carries_the_speaker_facts_and_the_clients_language() {
        let store = store(None);
        let outbound =
            handled(dispatch_chat_action(&store, player(), party(Language::Common)).unwrap());
        assert!(outbound.is_empty(), "the line itself returns on the Relay");
        assert_eq!(store.facts_reads.lock().unwrap().as_slice(), &[42]);
        assert_eq!(
            store.requests.lock().unwrap().as_slice(),
            &[(
                42,
                RealmChatRequest {
                    kind: 1,
                    language: 7,
                    channel_name: String::new(),
                    target_guid: 0,
                    message: "form up".to_string(),
                    speaker: speaker(),
                }
            )]
        );
    }

    /// Every racial language reaches the Module as its wire value (gtker vanilla `language.rs`),
    /// so the Module's language Gate judges what the client sent. Addon-language lines never get
    /// here: the addon bridge in `world/mod.rs` takes them first.
    #[test]
    fn each_racial_language_reaches_the_module_as_its_wire_value() {
        for (language, wire) in [
            (Language::Common, 7),
            (Language::Orcish, 1),
            (Language::Dwarvish, 6),
            (Language::Darnassian, 2),
            (Language::Gutterspeak, 33),
            (Language::Taurahe, 3),
            (Language::Gnomish, 13),
            (Language::Troll, 14),
        ] {
            let store = store(None);
            handled(dispatch_chat_action(&store, player(), party(language)).unwrap());
            assert_eq!(
                store.requests.lock().unwrap()[0].1.language,
                wire,
                "{language:?}"
            );
        }
    }

    #[test]
    fn a_line_without_a_world_session_reads_and_requests_nothing() {
        let store = store(None);
        let no_session = ChatActionPlayer {
            account_id: 7,
            self_guid: None,
        };
        let outbound =
            handled(dispatch_chat_action(&store, no_session, party(Language::Common)).unwrap());
        assert!(outbound.is_empty());
        assert!(store.facts_reads.lock().unwrap().is_empty());
        assert!(store.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_speaker_without_a_live_entity_sends_nothing() {
        let store = InMemoryChatActions::default();
        let outbound =
            handled(dispatch_chat_action(&store, player(), party(Language::Common)).unwrap());
        assert!(outbound.is_empty());
        assert!(store.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn speaking_from_no_party_answers_not_in_group() {
        let store = store(Some(Ok(ChatOutcome::Refused(ChatRefusal::NotInGroup))));
        let outbound =
            handled(dispatch_chat_action(&store, player(), party(Language::Common)).unwrap());
        match only(outbound) {
            ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(result) => {
                assert_eq!(result.operation, PartyOperation::Leave);
                assert_eq!(result.member, "");
                assert_eq!(result.result, PartyResult::NotInGroup);
            }
            other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
        }
    }

    #[test]
    fn an_unknown_language_answers_the_vanilla_notification() {
        let store = store(Some(Ok(ChatOutcome::Refused(ChatRefusal::UnknownLanguage))));
        let outbound =
            handled(dispatch_chat_action(&store, player(), party(Language::Orcish)).unwrap());
        match only(outbound) {
            ServerOpcodeMessage::SMSG_NOTIFICATION(notice) => {
                assert_eq!(notice.notification, "You don't know that language");
            }
            other => panic!("expected SMSG_NOTIFICATION, got {other}"),
        }
    }

    #[test]
    fn every_other_refusal_is_silent() {
        for refusal in [ChatRefusal::UnsupportedKind, ChatRefusal::EmptyMessage] {
            let store = store(Some(Ok(ChatOutcome::Refused(refusal))));
            let outbound =
                handled(dispatch_chat_action(&store, player(), party(Language::Common)).unwrap());
            assert!(outbound.is_empty(), "{refusal:?}");
        }
    }

    /// Raid, Raid Leader and Raid Warning each route to the seam with their own Chat Kind. The
    /// Module decides the audience; the Gateway only conveys the request.
    #[test]
    fn raid_lines_carry_their_own_chat_kind() {
        for (chat_type, kind) in [
            (CMSG_MESSAGECHAT_ChatType::Raid, chat_kind::RAID),
            (
                CMSG_MESSAGECHAT_ChatType::RaidLeader,
                chat_kind::RAID_LEADER,
            ),
            (
                CMSG_MESSAGECHAT_ChatType::RaidWarning,
                chat_kind::RAID_WARNING,
            ),
        ] {
            let store = store(None);
            let outbound = handled(
                dispatch_chat_action(&store, player(), line(chat_type, Language::Common)).unwrap(),
            );
            assert!(outbound.is_empty(), "the line itself returns on the Relay");
            assert_eq!(
                store.requests.lock().unwrap()[0].1.kind,
                kind,
                "chat kind {kind:#x}"
            );
        }
    }

    /// cmangos answers a raid audience Refusal with silence, the same as every other Chat Kind
    /// (AC 3-5).
    #[test]
    fn every_raid_chat_refusal_is_silent() {
        for refusal in [
            ChatRefusal::NotRaid,
            ChatRefusal::NotRaidLeader,
            ChatRefusal::NotRaidLeaderOrAssistant,
        ] {
            let store = store(Some(Ok(ChatOutcome::Refused(refusal))));
            let outbound = handled(
                dispatch_chat_action(
                    &store,
                    player(),
                    line(CMSG_MESSAGECHAT_ChatType::Raid, Language::Common),
                )
                .unwrap(),
            );
            assert!(outbound.is_empty(), "{refusal:?}");
        }
    }

    #[test]
    fn a_lost_reducer_transport_ends_the_session() {
        let store = store(Some(Err(
            "realm_chat reducer transport disconnected: channel closed".to_string(),
        )));
        let error = dispatch_chat_action(&store, player(), party(Language::Common))
            .err()
            .expect("transport loss is fatal");
        assert!(error.to_string().contains("transport disconnected"));
    }

    #[test]
    fn any_other_failure_drops_the_line_and_keeps_the_session() {
        let store = store(Some(Err(
            "realm_chat reducer timed out after 10s".to_string()
        )));
        let outbound =
            handled(dispatch_chat_action(&store, player(), party(Language::Common)).unwrap());
        assert!(outbound.is_empty());
    }

    fn channel_line(channel: &str) -> ClientOpcodeMessage {
        line(
            CMSG_MESSAGECHAT_ChatType::Channel {
                channel: channel.to_string(),
            },
            Language::Dwarvish,
        )
    }

    #[test]
    fn a_channel_line_carries_the_typed_channel_and_the_clients_language() {
        let store = store(None);
        let outbound =
            handled(dispatch_chat_action(&store, player(), channel_line("trade - City")).unwrap());
        assert!(outbound.is_empty(), "the line itself returns on the Relay");
        assert_eq!(
            store.requests.lock().unwrap().as_slice(),
            &[(
                42,
                RealmChatRequest {
                    kind: 0x0E,
                    language: 6,
                    channel_name: "trade - City".to_string(),
                    target_guid: 0,
                    message: "form up".to_string(),
                    speaker: speaker(),
                }
            )]
        );
    }

    /// cm:Channel.cpp:593-664 answers each refused line with one notice to the speaker alone.
    #[test]
    fn a_refused_channel_line_answers_the_channels_notice() {
        for (refusal, code) in [
            (ChannelRefusal::NotMember, 0x05),
            (ChannelRefusal::Muted, 0x11),
            (ChannelRefusal::NotModerator, 0x06),
        ] {
            let store = store(Some(Ok(ChatOutcome::Refused(ChatRefusal::Channel(
                refusal,
            )))));
            let outbound =
                handled(dispatch_chat_action(&store, player(), channel_line("Rx")).unwrap());
            let mut outbound = outbound.into_iter();
            match (outbound.next(), outbound.next()) {
                (Some(Outbound::Raw { opcode, body }), None) => {
                    assert_eq!(opcode, 0x0099);
                    assert_eq!(body, [code, b'R', b'x', 0], "{refusal:?}");
                }
                _ => panic!("expected one SMSG_CHANNEL_NOTIFY for {refusal:?}"),
            }
        }
    }

    #[test]
    fn an_unknown_language_on_a_channel_answers_the_vanilla_notification() {
        let store = store(Some(Ok(ChatOutcome::Refused(ChatRefusal::UnknownLanguage))));
        let outbound = handled(dispatch_chat_action(&store, player(), channel_line("Rx")).unwrap());
        assert!(matches!(
            only(outbound),
            ServerOpcodeMessage::SMSG_NOTIFICATION(_)
        ));
    }

    #[test]
    fn say_and_yell_pass_through_untouched() {
        let store = store(None);
        for chat_type in [
            CMSG_MESSAGECHAT_ChatType::Say,
            CMSG_MESSAGECHAT_ChatType::Yell,
        ] {
            match dispatch_chat_action(&store, player(), line(chat_type, Language::Common)).unwrap()
            {
                ChatActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_MESSAGECHAT(chat)) => {
                    assert_eq!(chat.message, "form up");
                }
                _ => panic!("say and yell stay proximity chat"),
            }
        }
        assert!(store.facts_reads.lock().unwrap().is_empty());
        assert!(store.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn another_opcode_passes_through() {
        let store = store(None);
        let outcome = dispatch_chat_action(
            &store,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            ChatActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
        assert!(store.requests.lock().unwrap().is_empty());
    }
}
