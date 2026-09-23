//! Realm Chat dispatcher: the `CMSG_MESSAGECHAT` kinds that become Realm Chat Lines. The Gateway
//! reads the Speaker Facts on the Home Shard, the Module decides the audience on Realm-core, and
//! the Relay (`stdb::world_view::realm_chat_appeared`) delivers the line. Say, yell and every kind
//! this file does not own pass through.

use super::super::*;
use lyracore_shared::chat::{chat_kind, chat_tag_for, ChatRefusal};
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
        use crate::stdb::bindings::{GameCharacterTableAccess, GameWorldEntityTableAccess};
        let guard = self.0.coord();
        let db = &guard.conn.db;
        let Some(entity) = db.game_world_entity().guid().find(&speaker_guid) else {
            return Ok(None);
        };
        let name = db
            .game_character()
            .guid()
            .find(&speaker_guid)
            .map(|character| character.name)
            .unwrap_or_default();
        Ok(Some(SpeakerFacts {
            // UNIT_FIELD_BYTES_0 byte 0 is the race.
            race: (entity.unit_bytes_0 & 0xFF) as u8,
            chat_tag: chat_tag_for(entity.player_flags),
            name,
        }))
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
        chat_type => {
            return Ok(ChatActionOutcome::PassThrough(
                ClientOpcodeMessage::CMSG_MESSAGECHAT(Box::new(CMSG_MESSAGECHAT {
                    chat_type,
                    language,
                    message,
                })),
            ))
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
fn is_transport_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn the_full_language_word_reaches_the_module() {
        let store = store(None);
        handled(dispatch_chat_action(&store, player(), party(Language::Addon)).unwrap());
        assert_eq!(store.requests.lock().unwrap()[0].1.language, 0xFFFF_FFFF);
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
        for refusal in [
            ChatRefusal::NotInWorld,
            ChatRefusal::UnsupportedKind,
            ChatRefusal::EmptyMessage,
        ] {
            let store = store(Some(Ok(ChatOutcome::Refused(refusal))));
            let outbound =
                handled(dispatch_chat_action(&store, player(), party(Language::Common)).unwrap());
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
