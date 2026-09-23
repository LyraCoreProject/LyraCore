//! Realm Chat: non-proximity chat committed on Realm-core with its complete recipient list.
//!
//! The Module decides who hears a line, in the transaction that validates it, and writes the whole
//! audience on one [`RealmChatEvent`] row. Every Gateway receives that row once and delivers it to
//! its own World Sessions on any Shard. The Gateway reads the Speaker Facts on the speaker's Home
//! Shard before the call, because Realm-core holds no Characters and no live entities.
//!
//! A new Chat Kind adds one arm to [`audience`] and an audience rule in its family's own file. The
//! table, the reducer and the Relay stay as they are. On an unsharded Realm, Realm-core is the one
//! database, so both deployments run this same path.

use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table, Timestamp};

use lyracore_shared::chat::{chat_kind, speakable_language, ChatRefusal};

/// One Realm Chat Line. Private: the owner-token Coordinator is its only reader. Reaped by the
/// shared event GC. [event]
#[table(accessor = game_realm_chat_event)]
pub struct RealmChatEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// `lyracore_shared::chat::chat_kind`, the `ChatMsg` wire value.
    pub kind: u8,
    /// The guid the packet names. For WHISPER_INFORM, AFK, DND and IGNORED that is the other party.
    pub speaker_guid: u64,
    /// The wire `Language`. The addon language is `0xFFFF_FFFF`.
    pub language: u32,
    /// `lyracore_shared::chat::chat_tag`: 0 none, 1 AFK, 2 DND.
    pub chat_tag: u8,
    /// The CHANNEL display name. Empty for every other kind.
    pub channel_name: String,
    pub message: String,
    /// Character guids: the Relay's whole audience.
    pub recipients: Vec<u64>,
    /// A recipient who ignores `speaker_guid` does not get the line.
    pub ignorable: bool,
    pub created_at: Timestamp,
}

/// What the Gateway read about the speaker on its Home Shard.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeakerFacts {
    /// `game_character.race`. Decides the languages the speaker knows.
    pub race: u8,
    /// `lyracore_shared::chat::chat_tag_for` of the live entity's `PLAYER_FLAGS`.
    pub chat_tag: u8,
}

/// One chat line a client sent, with the facts only the Gateway can read.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct RealmChatRequest {
    pub kind: u8,
    pub language: u32,
    /// CHANNEL only.
    pub channel_name: String,
    /// IGNORED only. The Gateway resolves the name.
    pub target_guid: u64,
    pub message: String,
    pub speaker: SpeakerFacts,
}

/// Who hears one line, as a family's audience rule answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChatAudience {
    pub(crate) recipients: Vec<u64>,
    pub(crate) ignorable: bool,
    pub(crate) channel_name: String,
}

/// A [`RealmChatEvent`] without `id` and `created_at`. [`emit`] stamps both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RealmChatLine {
    pub(crate) kind: u8,
    pub(crate) speaker_guid: u64,
    pub(crate) language: u32,
    pub(crate) chat_tag: u8,
    pub(crate) channel_name: String,
    pub(crate) message: String,
    pub(crate) recipients: Vec<u64>,
    pub(crate) ignorable: bool,
}

/// Commit one Realm Chat Line for `request_actor`.
///
/// Operator-gated because the speaker is an argument: Realm-core has no live entity to derive it
/// from, so any other caller could speak as anybody in the Realm. A Refusal rolls the transaction
/// back and returns its stable tag.
#[reducer]
pub fn realm_chat(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    request: RealmChatRequest,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let speaker_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let line = compose(speaker_guid, request, |request| {
        audience(ctx, speaker_guid, request)
    })
    .map_err(|refusal| refused_chat(refusal, speaker_guid))?;
    emit(ctx, line);
    Ok(())
}

/// Validate one request and build its line. The message is checked first, then the language, then
/// the audience, so a line that fails two Gates reports the same Refusal every time.
fn compose(
    speaker_guid: u64,
    request: RealmChatRequest,
    audience: impl FnOnce(&RealmChatRequest) -> Result<ChatAudience, ChatRefusal>,
) -> Result<RealmChatLine, ChatRefusal> {
    let message =
        crate::chat::normalized_message(&request.message).ok_or(ChatRefusal::EmptyMessage)?;
    let language = speakable_language(request.kind, request.speaker.race, request.language)?;
    let audience = audience(&request)?;
    Ok(RealmChatLine {
        kind: request.kind,
        speaker_guid,
        language,
        chat_tag: request.speaker.chat_tag,
        channel_name: audience.channel_name,
        message,
        recipients: audience.recipients,
        ignorable: audience.ignorable,
    })
}

/// One arm per audience family. Each family's own file owns its rule.
fn audience(
    ctx: &ReducerContext,
    speaker_guid: u64,
    request: &RealmChatRequest,
) -> Result<ChatAudience, ChatRefusal> {
    match request.kind {
        chat_kind::PARTY => crate::group::party_chat_audience(ctx, speaker_guid),
        _ => Err(ChatRefusal::UnsupportedKind),
    }
}

/// The only writer of `game_realm_chat_event`. A kind with several packets per request calls it
/// once per line.
pub(crate) fn emit(ctx: &ReducerContext, line: RealmChatLine) {
    ctx.db.game_realm_chat_event().insert(RealmChatEvent {
        id: 0,
        kind: line.kind,
        speaker_guid: line.speaker_guid,
        language: line.language,
        chat_tag: line.chat_tag,
        channel_name: line.channel_name,
        message: line.message,
        recipients: line.recipients,
        ignorable: line.ignorable,
        created_at: ctx.timestamp,
    });
}

/// Reducer edge: only the tag crosses to the Gateway; the detail stays in Module logs.
fn refused_chat(refusal: ChatRefusal, speaker_guid: u64) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("chat refused {tag}: speaker {speaker_guid}");
    tag.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::code_of;
    use lyracore_shared::chat::{chat_tag, language};

    fn request(kind: u8, race: u8, language: u32, message: &str) -> RealmChatRequest {
        RealmChatRequest {
            kind,
            language,
            channel_name: String::new(),
            target_guid: 0,
            message: message.to_string(),
            speaker: SpeakerFacts {
                race,
                chat_tag: chat_tag::DND,
            },
        }
    }

    fn party(recipients: &[u64]) -> Result<ChatAudience, ChatRefusal> {
        Ok(ChatAudience {
            recipients: recipients.to_vec(),
            ignorable: false,
            channel_name: String::new(),
        })
    }

    #[test]
    fn a_party_line_carries_the_audience_language_and_speaker_tag() {
        let line = compose(
            10,
            request(chat_kind::PARTY, 1, language::COMMON, "  form up  "),
            |_| party(&[10, 20]),
        )
        .unwrap();
        assert_eq!(
            line,
            RealmChatLine {
                kind: chat_kind::PARTY,
                speaker_guid: 10,
                language: language::COMMON,
                chat_tag: chat_tag::DND,
                channel_name: String::new(),
                message: "form up".to_string(),
                recipients: vec![10, 20],
                ignorable: false,
            }
        );
    }

    #[test]
    fn an_empty_line_is_refused_before_any_other_gate() {
        let asked = std::cell::Cell::new(false);
        let refusal = compose(
            10,
            request(chat_kind::PARTY, 1, language::ORCISH, " \t "),
            |_| {
                asked.set(true);
                party(&[10])
            },
        );
        assert_eq!(refusal, Err(ChatRefusal::EmptyMessage));
        assert!(!asked.get(), "no audience is read for an empty line");
    }

    #[test]
    fn an_unknown_language_is_refused_before_the_audience_is_read() {
        let refusal = compose(
            10,
            request(chat_kind::PARTY, 1, language::ORCISH, "zug"),
            |_| Err(ChatRefusal::NotInGroup),
        );
        assert_eq!(refusal, Err(ChatRefusal::UnknownLanguage));
    }

    #[test]
    fn the_audience_refusal_is_the_answer_once_language_passes() {
        let refusal = compose(
            10,
            request(chat_kind::PARTY, 1, language::COMMON, "hi"),
            |_| Err(ChatRefusal::NotInGroup),
        );
        assert_eq!(refusal, Err(ChatRefusal::NotInGroup));
    }

    /// The speaker's guid is an argument, so the operator gate is the entire authorization. A gate
    /// that is present but neutralized (`if false`, `let _ =`, an early return above it) is no gate,
    /// so the scan anchors to the opening brace.
    #[test]
    fn the_realm_chat_reducer_is_operator_gated() {
        let body = code_of(include_str!("realm_chat.rs"), "pub fn realm_chat(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`realm_chat` no longer OPENS with the operator gate. Body was:\n{body}"
        );
    }
}
