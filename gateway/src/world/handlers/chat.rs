//! Realm Chat dispatcher: the `CMSG_MESSAGECHAT` kinds that become Realm Chat Lines, `/afk` and
//! `/dnd`, and `CMSG_CHAT_IGNORED`. The Gateway reads the Speaker Facts on the Home Shard, the
//! Module decides the audience on Realm-core, and the Relay
//! (`stdb::world_view::realm_chat_appeared`) delivers the line. Party, Raid, Raid Leader, Raid
//! Warning, Channel, Guild, Officer and Whisper lines are all Realm Chat Lines. Say, yell and
//! every kind this file does not own pass through.

use super::super::*;
use lyracore_shared::chat::{chat_kind, language, ChatRefusal};
use wow_world_messages::vanilla::{CMSG_CHAT_IGNORED, SMSG_NOTIFICATION};

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

/// What the Gateway read about a whisper's target on whichever World Shard holds it
/// (`world::whisper::target_facts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WhisperTargetFacts {
    pub(crate) guid: u64,
    pub(crate) race: u8,
    /// The canonical spelling.
    pub(crate) name: String,
    pub(crate) ignores_speaker: bool,
    /// 0, `chat_kind::AFK` or `chat_kind::DND`.
    pub(crate) away_kind: u8,
    /// The stored Auto-Reply. Empty means the Module's default text.
    pub(crate) away_message: String,
}

/// One whisper on its way to the `realm_whisper` reducer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WhisperRequest {
    pub(crate) language: u32,
    pub(crate) message: String,
    pub(crate) speaker: SpeakerFacts,
    pub(crate) target: WhisperTargetFacts,
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
    /// Durable Request on the speaker's Home Shard: one `/afk` or `/dnd`.
    fn set_away(&self, speaker_guid: u64, kind: u8, message: String) -> Result<()>;
    /// Durable Read across every World Shard: the online Character the typed name reaches, with
    /// the facts the Module's whisper Gates need. `None` when no online Character has that name.
    fn whisper_target(
        &self,
        speaker_guid: u64,
        typed_name: &str,
    ) -> Result<Option<WhisperTargetFacts>>;
    /// Durable Request on Realm-core.
    fn realm_whisper(&self, speaker_guid: u64, request: WhisperRequest) -> Result<ChatOutcome>;
}

impl ChatActionStore for crate::stdb::Coordinator {
    fn speaker_facts(&self, speaker_guid: u64) -> Result<Option<SpeakerFacts>> {
        crate::stdb::Coordinator::speaker_facts(self, speaker_guid)
    }

    fn realm_chat(&self, speaker_guid: u64, request: RealmChatRequest) -> Result<ChatOutcome> {
        crate::stdb::Coordinator::realm_chat(self, speaker_guid, request)
    }

    fn set_away(&self, speaker_guid: u64, kind: u8, message: String) -> Result<()> {
        crate::stdb::Coordinator::set_away(self, speaker_guid, kind, message)
    }

    fn whisper_target(
        &self,
        speaker_guid: u64,
        typed_name: &str,
    ) -> Result<Option<WhisperTargetFacts>> {
        whisper::target_facts(self, speaker_guid, typed_name)
    }

    fn realm_whisper(&self, speaker_guid: u64, request: WhisperRequest) -> Result<ChatOutcome> {
        crate::stdb::Coordinator::realm_whisper(self, speaker_guid, request)
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

/// The wire `chat_kind` for a Chat Kind whose own audience Refusals are silent here: Raid, Raid
/// Leader, Raid Warning, Guild and Officer. `None` for every other `CMSG_MESSAGECHAT_ChatType`,
/// Party and Channel included (they keep their own arms for a Refusal that answers the client).
/// The shared Refusals still answer through `refusal_outbound` below — `UnknownLanguage` gets its
/// vanilla notice whichever of these five kinds sent the line. Each family owns its audience rule
/// on the Module side; this only picks the wire kind so they can share this arm's dispatch.
fn silent_refusal_chat_kind(chat_type: &CMSG_MESSAGECHAT_ChatType) -> Option<u8> {
    match chat_type {
        CMSG_MESSAGECHAT_ChatType::Raid => Some(chat_kind::RAID),
        CMSG_MESSAGECHAT_ChatType::RaidLeader => Some(chat_kind::RAID_LEADER),
        CMSG_MESSAGECHAT_ChatType::RaidWarning => Some(chat_kind::RAID_WARNING),
        CMSG_MESSAGECHAT_ChatType::Guild => Some(chat_kind::GUILD),
        CMSG_MESSAGECHAT_ChatType::Officer => Some(chat_kind::OFFICER),
        _ => None,
    }
}

/// Consume the `CMSG_MESSAGECHAT` kinds this file owns and `CMSG_CHAT_IGNORED`, and pass
/// everything else on. A new Chat Kind adds one arm here and answers its own Refusals before the
/// shared ones.
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
        ClientOpcodeMessage::CMSG_CHAT_IGNORED(CMSG_CHAT_IGNORED { guid }) => {
            let outbound = chat_ignored(store, player, guid.guid())?;
            return Ok(ChatActionOutcome::Handled { outbound });
        }
        other => return Ok(ChatActionOutcome::PassThrough(other)),
    };
    let outbound = match chat_type {
        CMSG_MESSAGECHAT_ChatType::Whisper { target_player } => {
            whisper(store, player, language.as_int(), message, target_player)?
        }
        CMSG_MESSAGECHAT_ChatType::Afk => set_away(store, player, chat_kind::AFK, message)?,
        CMSG_MESSAGECHAT_ChatType::Dnd => set_away(store, player, chat_kind::DND, message)?,
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
            let Some(kind) = silent_refusal_chat_kind(&chat_type) else {
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

/// One whisper: the target is resolved realm-wide, the Module applies the Gates on Realm-core, and
/// the lines return on the Relay. A name no online Character holds answers
/// `SMSG_CHAT_PLAYER_NOT_FOUND` with the typed name (cm:ChatHandler.cpp:243-266). A target read
/// that fails without a transport loss answers the same way: this Gateway cannot reach the target.
fn whisper<St: ChatActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    language: u32,
    message: String,
    typed_name: String,
) -> Result<Vec<Outbound>> {
    let Some(speaker_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let Some(speaker) = store.speaker_facts(speaker_guid)? else {
        return Ok(Vec::new());
    };
    let target = match store.whisper_target(speaker_guid, &typed_name) {
        Ok(Some(target)) => target,
        Ok(None) => return Ok(vec![player_not_found(typed_name)]),
        Err(error) if is_transport_failure(&error) => return Err(error),
        Err(error) => {
            log::debug!(
                "world: whisper target read failed (account {}): {error:#}",
                player.account_id
            );
            return Ok(vec![player_not_found(typed_name)]);
        }
    };
    let request = WhisperRequest {
        language,
        message,
        speaker,
        target,
    };
    match store.realm_whisper(speaker_guid, request) {
        Ok(ChatOutcome::Delivered) => Ok(Vec::new()),
        // cm:ChatHandler.cpp:268-275, cm:ChatHandler.cpp:824-828: an empty packet.
        Ok(ChatOutcome::Refused(ChatRefusal::WrongFaction)) => Ok(vec![Outbound::One(
            ServerOpcodeMessage::SMSG_CHAT_WRONG_FACTION,
        )]),
        Ok(ChatOutcome::Refused(refusal)) => Ok(refusal_outbound(player, Some(refusal))),
        Err(error) if is_transport_failure(&error) => Err(error),
        Err(error) => {
            log::debug!(
                "world: whisper dropped (account {}): {error:#}",
                player.account_id
            );
            Ok(Vec::new())
        }
    }
}

fn player_not_found(name: String) -> Outbound {
    Outbound::One(ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(Box::new(
        SMSG_CHAT_PLAYER_NOT_FOUND { name },
    )))
}

/// `/afk` or `/dnd`. The client prints its own notice, and observers see the `PLAYER_FLAGS`
/// change on the entity Relay, so nothing answers here.
fn set_away<St: ChatActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    kind: u8,
    message: String,
) -> Result<Vec<Outbound>> {
    let Some(speaker_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    match store.set_away(speaker_guid, kind, message) {
        Ok(()) => {}
        Err(error) if is_transport_failure(&error) => return Err(error),
        Err(error) => log::debug!(
            "world: away kind {kind} dropped (account {}): {error:#}",
            player.account_id
        ),
    }
    Ok(Vec::new())
}

/// `CMSG_CHAT_IGNORED`: this player's client dropped a line from `dropped_speaker`, who learns
/// "X is ignoring you." (cm:ChatHandler.cpp:801-815). The notice carries the ignorer's own name.
fn chat_ignored<St: ChatActionStore + ?Sized>(
    store: &St,
    player: ChatActionPlayer,
    dropped_speaker: u64,
) -> Result<Vec<Outbound>> {
    let refusal = send_line(store, player, |speaker| RealmChatRequest {
        kind: chat_kind::IGNORED,
        language: language::UNIVERSAL,
        channel_name: String::new(),
        target_guid: dropped_speaker,
        message: speaker.name.clone(),
        speaker,
    })?;
    Ok(refusal_outbound(player, refusal))
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
        away_failure: Option<String>,
        away_requests: Mutex<Vec<(u64, u8, String)>>,
        target: Option<Result<Option<WhisperTargetFacts>, String>>,
        target_reads: Mutex<Vec<(u64, String)>>,
        whisper_outcome: Option<Result<ChatOutcome, String>>,
        whispers: Mutex<Vec<(u64, WhisperRequest)>>,
    }

    fn answer<T: Clone>(configured: &Option<Result<T, String>>, default: T) -> Result<T> {
        match configured {
            None => Ok(default),
            Some(Ok(value)) => Ok(value.clone()),
            Some(Err(failure)) => Err(anyhow::anyhow!("{failure}")),
        }
    }

    impl ChatActionStore for InMemoryChatActions {
        fn speaker_facts(&self, speaker_guid: u64) -> Result<Option<SpeakerFacts>> {
            self.facts_reads.lock().unwrap().push(speaker_guid);
            Ok(self.facts.clone())
        }

        fn realm_chat(&self, speaker_guid: u64, request: RealmChatRequest) -> Result<ChatOutcome> {
            self.requests.lock().unwrap().push((speaker_guid, request));
            answer(&self.outcome, ChatOutcome::Delivered)
        }

        fn set_away(&self, speaker_guid: u64, kind: u8, message: String) -> Result<()> {
            self.away_requests
                .lock()
                .unwrap()
                .push((speaker_guid, kind, message));
            match &self.away_failure {
                None => Ok(()),
                Some(failure) => Err(anyhow::anyhow!("{failure}")),
            }
        }

        fn whisper_target(
            &self,
            speaker_guid: u64,
            typed_name: &str,
        ) -> Result<Option<WhisperTargetFacts>> {
            self.target_reads
                .lock()
                .unwrap()
                .push((speaker_guid, typed_name.to_string()));
            answer(&self.target, None)
        }

        fn realm_whisper(&self, speaker_guid: u64, request: WhisperRequest) -> Result<ChatOutcome> {
            self.whispers.lock().unwrap().push((speaker_guid, request));
            answer(&self.whisper_outcome, ChatOutcome::Delivered)
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
    /// so the Module's language Gate judges what the client sent. The addon bridge in
    /// `world/mod.rs` only intercepts its own STC-prefixed frames; an addon-language line from any
    /// other addon still reaches this dispatcher on Party, Raid, Guild or Officer
    /// (`gateway/src/codec/addon.rs`).
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

    /// Raid, Raid Leader, Raid Warning, Guild and Officer each route to the seam with their own
    /// Chat Kind. The Module decides the audience; the Gateway only conveys the request.
    #[test]
    fn raid_and_guild_lines_carry_their_own_chat_kind() {
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
            (CMSG_MESSAGECHAT_ChatType::Guild, chat_kind::GUILD),
            (CMSG_MESSAGECHAT_ChatType::Officer, chat_kind::OFFICER),
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

    /// `SendAddonMessage` shares the Guild and Officer channels with the addon bridge
    /// (`gateway/src/codec/addon.rs`). The addon language forwards to the Module unchanged, which
    /// keeps it rather than forcing Universal (`cm:ChatHandler.cpp:369,409`).
    #[test]
    fn guild_and_officer_lines_forward_the_addon_language() {
        for (chat_type, label) in [
            (CMSG_MESSAGECHAT_ChatType::Guild, "Guild"),
            (CMSG_MESSAGECHAT_ChatType::Officer, "Officer"),
        ] {
            let store = store(None);
            handled(
                dispatch_chat_action(&store, player(), line(chat_type, Language::Addon)).unwrap(),
            );
            assert_eq!(
                store.requests.lock().unwrap()[0].1.language,
                0xFFFF_FFFF,
                "{label}"
            );
        }
    }

    /// cmangos answers a raid or guild audience Refusal with silence, the same as every other
    /// Chat Kind: a member who cannot speak on it hears nothing back, and neither does anyone else
    /// (`cm:ChatHandler.cpp:367-369`, `cm:Guild.cpp:559-561`).
    #[test]
    fn every_raid_and_guild_chat_refusal_is_silent() {
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
        for refusal in [ChatRefusal::NotInGuild, ChatRefusal::NoGuildChatRight] {
            let store = store(Some(Ok(ChatOutcome::Refused(refusal))));
            let outbound = handled(
                dispatch_chat_action(
                    &store,
                    player(),
                    line(CMSG_MESSAGECHAT_ChatType::Guild, Language::Common),
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

    fn target() -> WhisperTargetFacts {
        WhisperTargetFacts {
            guid: 20,
            race: 3,
            name: "Vim".to_string(),
            ignores_speaker: true,
            away_kind: chat_kind::AFK,
            away_message: "brb".to_string(),
        }
    }

    fn whisper_store(
        target: Result<Option<WhisperTargetFacts>, String>,
        outcome: Option<Result<ChatOutcome, String>>,
    ) -> InMemoryChatActions {
        InMemoryChatActions {
            target: Some(target),
            whisper_outcome: outcome,
            ..store(None)
        }
    }

    fn whisper_to(name: &str) -> ClientOpcodeMessage {
        line(
            CMSG_MESSAGECHAT_ChatType::Whisper {
                target_player: name.to_string(),
            },
            Language::Common,
        )
    }

    fn not_found(outbound: Vec<Outbound>) -> String {
        match only(outbound) {
            ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(m) => m.name,
            other => panic!("expected SMSG_CHAT_PLAYER_NOT_FOUND, got {other}"),
        }
    }

    #[test]
    fn a_whisper_conveys_the_speaker_and_the_target_facts() {
        let store = whisper_store(Ok(Some(target())), None);
        let outbound = handled(dispatch_chat_action(&store, player(), whisper_to("vim")).unwrap());
        assert!(outbound.is_empty(), "the lines return on the Relay");
        assert_eq!(
            store.target_reads.lock().unwrap().as_slice(),
            &[(42, "vim".to_string())]
        );
        assert_eq!(
            store.whispers.lock().unwrap().as_slice(),
            &[(
                42,
                WhisperRequest {
                    language: 7,
                    message: "form up".to_string(),
                    speaker: speaker(),
                    target: target(),
                }
            )]
        );
    }

    /// cm:ChatHandler.cpp:243-266: the typed name comes back as the client typed it.
    #[test]
    fn a_whisper_to_nobody_online_answers_player_not_found() {
        let store = whisper_store(Ok(None), None);
        let outbound = handled(dispatch_chat_action(&store, player(), whisper_to("vIm")).unwrap());
        assert_eq!(not_found(outbound), "vIm");
        assert!(store.whispers.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_target_read_answers_player_not_found_and_keeps_the_session() {
        let store = whisper_store(Err("peer Shard cannot vouch".to_string()), None);
        let outbound = handled(dispatch_chat_action(&store, player(), whisper_to("Vim")).unwrap());
        assert_eq!(not_found(outbound), "Vim");
        assert!(store.whispers.lock().unwrap().is_empty());
    }

    /// cm:ChatHandler.cpp:824-828: the opcode alone, with an empty body.
    #[test]
    fn a_cross_faction_whisper_answers_wrong_faction() {
        let store = whisper_store(
            Ok(Some(target())),
            Some(Ok(ChatOutcome::Refused(ChatRefusal::WrongFaction))),
        );
        let outbound = handled(dispatch_chat_action(&store, player(), whisper_to("Vim")).unwrap());
        let message = only(outbound);
        assert!(matches!(
            message,
            ServerOpcodeMessage::SMSG_CHAT_WRONG_FACTION
        ));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        assert_eq!(wire, [0x00, 0x02, 0x19, 0x02], "size 2, opcode 0x0219");
    }

    #[test]
    fn every_other_whisper_refusal_is_silent() {
        for refusal in [ChatRefusal::EmptyMessage, ChatRefusal::UnsupportedKind] {
            let store = whisper_store(Ok(Some(target())), Some(Ok(ChatOutcome::Refused(refusal))));
            let outbound =
                handled(dispatch_chat_action(&store, player(), whisper_to("Vim")).unwrap());
            assert!(outbound.is_empty(), "{refusal:?}");
        }
    }

    #[test]
    fn a_whisper_from_a_speaker_without_a_live_entity_reads_nothing() {
        let store = InMemoryChatActions {
            facts: None,
            ..whisper_store(Ok(Some(target())), None)
        };
        let outbound = handled(dispatch_chat_action(&store, player(), whisper_to("Vim")).unwrap());
        assert!(outbound.is_empty());
        assert!(store.target_reads.lock().unwrap().is_empty());
        assert!(store.whispers.lock().unwrap().is_empty());
    }

    #[test]
    fn a_lost_transport_on_a_whisper_ends_the_session() {
        let lost = "realm_whisper reducer transport disconnected: channel closed".to_string();
        let store = whisper_store(Ok(Some(target())), Some(Err(lost.clone())));
        assert!(dispatch_chat_action(&store, player(), whisper_to("Vim")).is_err());
        let store = whisper_store(Err(lost), None);
        assert!(dispatch_chat_action(&store, player(), whisper_to("Vim")).is_err());
    }

    #[test]
    fn any_other_whisper_failure_is_silent() {
        let store = whisper_store(
            Ok(Some(target())),
            Some(Err("realm_whisper reducer timed out after 10s".to_string())),
        );
        let outbound = handled(dispatch_chat_action(&store, player(), whisper_to("Vim")).unwrap());
        assert!(outbound.is_empty());
    }

    /// `/afk` and `/dnd` answer nothing: the client prints its own notice.
    #[test]
    fn afk_and_dnd_set_the_away_status_of_the_speaker() {
        let store = store(None);
        for (chat_type, message) in [
            (CMSG_MESSAGECHAT_ChatType::Afk, "brb"),
            (CMSG_MESSAGECHAT_ChatType::Dnd, ""),
        ] {
            let away = ClientOpcodeMessage::CMSG_MESSAGECHAT(Box::new(CMSG_MESSAGECHAT {
                chat_type,
                language: Language::Universal,
                message: message.to_string(),
            }));
            let outbound = handled(dispatch_chat_action(&store, player(), away).unwrap());
            assert!(outbound.is_empty());
        }
        assert_eq!(
            store.away_requests.lock().unwrap().as_slice(),
            &[(42, 0x14, "brb".to_string()), (42, 0x15, String::new())]
        );
        assert!(store.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_away_request_is_fatal_only_for_a_lost_transport() {
        let afk = || line(CMSG_MESSAGECHAT_ChatType::Afk, Language::Universal);
        let refused = InMemoryChatActions {
            away_failure: Some("mover not in world".to_string()),
            ..store(None)
        };
        assert!(handled(dispatch_chat_action(&refused, player(), afk()).unwrap()).is_empty());
        let lost = InMemoryChatActions {
            away_failure: Some("gw_set_away reducer transport disconnected".to_string()),
            ..store(None)
        };
        assert!(dispatch_chat_action(&lost, player(), afk()).is_err());
    }

    /// cm:ChatHandler.cpp:801-815: the ignorer's name, to the Character whose line was dropped.
    #[test]
    fn chat_ignored_sends_the_ignorers_name_to_the_dropped_speaker() {
        let store = store(None);
        let ignored = ClientOpcodeMessage::CMSG_CHAT_IGNORED(CMSG_CHAT_IGNORED {
            guid: wow_world_base::vanilla::Guid::new(20),
        });
        let outbound = handled(dispatch_chat_action(&store, player(), ignored).unwrap());
        assert!(outbound.is_empty());
        assert_eq!(
            store.requests.lock().unwrap().as_slice(),
            &[(
                42,
                RealmChatRequest {
                    kind: 0x16,
                    language: 0,
                    channel_name: String::new(),
                    target_guid: 20,
                    message: "Speaker".to_string(),
                    speaker: speaker(),
                }
            )]
        );
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
