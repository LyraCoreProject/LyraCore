//! Realm Chat: non-proximity chat committed on Realm-core with its complete recipient list.
//!
//! The Module decides who hears a line, in the transaction that validates it, and writes the whole
//! audience on one [`RealmChatEvent`] row. Every Gateway receives that row once and delivers it to
//! its own World Sessions on any Shard. The Gateway reads the Speaker Facts on the speaker's Home
//! Shard before the call, because Realm-core holds no Characters and no live entities.
//!
//! A new Chat Kind adds one arm to [`audience`] and an audience rule in its family's own file. The
//! table, the reducer and the Relay stay as they are. On an unsharded Realm, Realm-core is the one
//! database, so both deployments run this same path. A whisper is several lines to two parties,
//! so it has its own reducer, [`realm_whisper`], and its own line plan, [`whisper_lines`].

use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table, Timestamp};

use lyracore_shared::chat::{chat_kind, chat_tag, speakable_language, ChatRefusal};

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
    /// IGNORED only: the Character the ignorer's client dropped a line from.
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
    // The IGNORED notice carries no tag (cm:ChatHandler.cpp:813).
    let chat_tag = if request.kind == chat_kind::IGNORED {
        chat_tag::NONE
    } else {
        request.speaker.chat_tag
    };
    Ok(RealmChatLine {
        kind: request.kind,
        speaker_guid,
        language,
        chat_tag,
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
    use crate::group::RaidChatKind;
    match request.kind {
        chat_kind::PARTY => crate::group::party_chat_audience(ctx, speaker_guid),
        chat_kind::CHANNEL => crate::channel::chat_audience(ctx, speaker_guid, request),
        chat_kind::GUILD | chat_kind::OFFICER => {
            crate::guild::chat::guild_chat_audience(ctx, speaker_guid, request)
        }
        chat_kind::RAID => crate::group::raid_chat_audience(ctx, speaker_guid, RaidChatKind::Raid),
        chat_kind::RAID_LEADER => {
            crate::group::raid_chat_audience(ctx, speaker_guid, RaidChatKind::RaidLeader)
        }
        chat_kind::RAID_WARNING => {
            crate::group::raid_chat_audience(ctx, speaker_guid, RaidChatKind::RaidWarning)
        }
        chat_kind::IGNORED => ignored_audience(request),
        _ => Err(ChatRefusal::UnsupportedKind),
    }
}

/// IGNORED: the ignorer's client dropped a line from `target_guid` and says so
/// (cm:ChatHandler.cpp:801-815). The notice goes to that Character alone. Nobody holds guid 0.
fn ignored_audience(request: &RealmChatRequest) -> Result<ChatAudience, ChatRefusal> {
    if request.target_guid == 0 {
        return Err(ChatRefusal::UnsupportedKind);
    }
    Ok(ChatAudience {
        recipients: vec![request.target_guid],
        ignorable: false,
        channel_name: String::new(),
    })
}

// ===========================================================================================
//  Whisper
// ===========================================================================================

/// What the Gateway read about a whisper's target on whichever World Shard holds it. Realm-core
/// holds no Characters, contact rows or live entities.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct WhisperTargetFacts {
    pub guid: u64,
    /// `game_character.race`. Decides the team.
    pub race: u8,
    /// The canonical spelling. The IGNORED notice carries it.
    pub name: String,
    /// The target has the speaker on its ignore list.
    pub ignores_speaker: bool,
    /// 0, `chat_kind::AFK` or `chat_kind::DND`, from the live entity's `PLAYER_FLAGS`.
    pub away_kind: u8,
    /// The stored Auto-Reply. Empty means the default text.
    pub away_message: String,
}

/// One whisper a client sent, with the facts only the Gateway can read.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct WhisperRequest {
    pub language: u32,
    pub message: String,
    pub speaker: SpeakerFacts,
    pub target: WhisperTargetFacts,
}

/// Commit the lines of one whisper from `request_actor`.
///
/// Operator-gated for the same reason as [`realm_chat`]: the speaker is an argument. A Refusal
/// rolls the transaction back and returns its stable tag, so a refused whisper writes no row.
#[reducer]
pub fn realm_whisper(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    request: WhisperRequest,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let speaker_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let lines = whisper_lines(speaker_guid, request)
        .map_err(|refusal| refused_chat(refusal, speaker_guid))?;
    for line in lines {
        emit(ctx, line);
    }
    Ok(())
}

/// The lines one whisper produces, in the order the speaker sees them (cm:Player.cpp:16601-16645,
/// cm:ChatHandler.cpp:801-815):
///
/// 1. WHISPER to the target, naming the speaker, with the speaker's chat tag. Not sent when the
///    target ignores the speaker.
/// 2. WHISPER_INFORM to the speaker, naming the target, with no tag.
/// 3. AFK or DND to the speaker, naming the target, with the Auto-Reply, when the target has an
///    Away Status.
/// 4. IGNORED to the speaker, naming the target, with the target's name, when the target ignores
///    the speaker. The speaker's client prints "X is ignoring you."
///
/// The Gates run in the order [`compose`] uses: the message, the language (always Universal, and
/// the addon language is refused), then the team (cm:ChatHandler.cpp:268-275).
pub(crate) fn whisper_lines(
    speaker_guid: u64,
    request: WhisperRequest,
) -> Result<Vec<RealmChatLine>, ChatRefusal> {
    let message =
        crate::chat::normalized_message(&request.message).ok_or(ChatRefusal::EmptyMessage)?;
    let universal = speakable_language(chat_kind::WHISPER, request.speaker.race, request.language)?;
    let target = request.target;
    if !lyracore_shared::faction::same_team(request.speaker.race, target.race) {
        return Err(ChatRefusal::WrongFaction);
    }
    let to = |recipient: u64, kind: u8, named: u64, tag: u8, text: String| RealmChatLine {
        kind,
        speaker_guid: named,
        language: universal,
        chat_tag: tag,
        channel_name: String::new(),
        message: text,
        recipients: vec![recipient],
        ignorable: false,
    };
    let mut lines = Vec::with_capacity(3);
    if !target.ignores_speaker {
        lines.push(to(
            target.guid,
            chat_kind::WHISPER,
            speaker_guid,
            request.speaker.chat_tag,
            message.clone(),
        ));
    }
    lines.push(to(
        speaker_guid,
        chat_kind::WHISPER_INFORM,
        target.guid,
        chat_tag::NONE,
        message,
    ));
    if matches!(target.away_kind, chat_kind::AFK | chat_kind::DND) {
        let reply = if target.away_message.is_empty() {
            crate::away::default_reply(target.away_kind).to_string()
        } else {
            target.away_message
        };
        lines.push(to(
            speaker_guid,
            target.away_kind,
            target.guid,
            chat_tag::NONE,
            reply,
        ));
    }
    if target.ignores_speaker {
        lines.push(to(
            speaker_guid,
            chat_kind::IGNORED,
            target.guid,
            chat_tag::NONE,
            target.name,
        ));
    }
    Ok(lines)
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

    fn ignored(target_guid: u64) -> RealmChatRequest {
        RealmChatRequest {
            target_guid,
            ..request(chat_kind::IGNORED, 1, language::UNIVERSAL, "Ignorer")
        }
    }

    /// cm:ChatHandler.cpp:801-815: the notice names the ignorer, carries the ignorer's name and
    /// no tag, and goes to the Character whose line was dropped.
    #[test]
    fn an_ignored_notice_goes_to_the_dropped_speaker_alone() {
        let line = compose(30, ignored(10), ignored_audience).unwrap();
        assert_eq!(
            line,
            RealmChatLine {
                kind: 0x16,
                speaker_guid: 30,
                language: 0,
                chat_tag: 0,
                channel_name: String::new(),
                message: "Ignorer".to_string(),
                recipients: vec![10],
                ignorable: false,
            }
        );
    }

    #[test]
    fn an_ignored_notice_without_a_target_is_refused() {
        assert_eq!(
            compose(30, ignored(0), ignored_audience),
            Err(ChatRefusal::UnsupportedKind)
        );
    }

    const SPEAKER: u64 = 10;
    const TARGET: u64 = 20;

    fn whisper(
        target_race: u8,
        away_kind: u8,
        away_message: &str,
        ignores: bool,
    ) -> WhisperRequest {
        WhisperRequest {
            language: 7,
            message: " meet me at the gate ".to_string(),
            speaker: SpeakerFacts {
                race: 1,
                chat_tag: chat_tag::DND,
            },
            target: WhisperTargetFacts {
                guid: TARGET,
                race: target_race,
                name: "Target".to_string(),
                ignores_speaker: ignores,
                away_kind,
                away_message: away_message.to_string(),
            },
        }
    }

    fn line(kind: u8, named: u64, tag: u8, message: &str, recipient: u64) -> RealmChatLine {
        RealmChatLine {
            kind,
            speaker_guid: named,
            language: 0,
            chat_tag: tag,
            channel_name: String::new(),
            message: message.to_string(),
            recipients: vec![recipient],
            ignorable: false,
        }
    }

    /// cm:Player.cpp:16601-16622: the target hears the line in Universal with the speaker's tag,
    /// and the speaker gets the echo naming the target with no tag.
    #[test]
    fn a_whisper_is_the_incoming_line_then_the_echo() {
        assert_eq!(
            whisper_lines(SPEAKER, whisper(3, 0, "", false)),
            Ok(vec![
                line(0x06, SPEAKER, 2, "meet me at the gate", TARGET),
                line(0x07, TARGET, 0, "meet me at the gate", SPEAKER),
            ])
        );
    }

    /// cm:Player.cpp:16630-16644: an away target adds its Auto-Reply after the echo.
    #[test]
    fn an_afk_target_adds_its_auto_reply_after_the_echo() {
        assert_eq!(
            whisper_lines(SPEAKER, whisper(1, 0x14, "brb food", false)),
            Ok(vec![
                line(0x06, SPEAKER, 2, "meet me at the gate", TARGET),
                line(0x07, TARGET, 0, "meet me at the gate", SPEAKER),
                line(0x14, TARGET, 0, "brb food", SPEAKER),
            ])
        );
    }

    #[test]
    fn an_away_target_without_text_replies_with_the_default() {
        let lines = whisper_lines(SPEAKER, whisper(1, 0x14, "", false)).unwrap();
        assert_eq!(
            lines[2],
            line(0x14, TARGET, 0, "Away from Keyboard", SPEAKER)
        );
        let lines = whisper_lines(SPEAKER, whisper(1, 0x15, "", false)).unwrap();
        assert_eq!(lines[2], line(0x15, TARGET, 0, "Do not Disturb", SPEAKER));
    }

    /// cm:ChatHandler.cpp:801-815, fx:ChatFrame.lua:1403-1404: the ignorer gets nothing, and the
    /// speaker keeps the echo and learns "Target is ignoring you."
    #[test]
    fn an_ignoring_target_gets_nothing_and_the_speaker_learns_it() {
        assert_eq!(
            whisper_lines(SPEAKER, whisper(1, 0, "", true)),
            Ok(vec![
                line(0x07, TARGET, 0, "meet me at the gate", SPEAKER),
                line(0x16, TARGET, 0, "Target", SPEAKER),
            ])
        );
    }

    #[test]
    fn an_ignoring_away_target_still_auto_replies_before_the_notice() {
        assert_eq!(
            whisper_lines(SPEAKER, whisper(1, 0x15, "busy", true)),
            Ok(vec![
                line(0x07, TARGET, 0, "meet me at the gate", SPEAKER),
                line(0x15, TARGET, 0, "busy", SPEAKER),
                line(0x16, TARGET, 0, "Target", SPEAKER),
            ])
        );
    }

    /// cm:ChatHandler.cpp:268-275: a Human whispering an Orc is refused.
    #[test]
    fn a_whisper_to_the_other_team_is_refused() {
        assert_eq!(
            whisper_lines(SPEAKER, whisper(2, 0, "", false)),
            Err(ChatRefusal::WrongFaction)
        );
    }

    #[test]
    fn an_empty_whisper_is_refused_before_the_team() {
        let mut request = whisper(2, 0, "", false);
        request.message = "   ".to_string();
        assert_eq!(
            whisper_lines(SPEAKER, request),
            Err(ChatRefusal::EmptyMessage)
        );
    }

    /// `SendAddonMessage` in 1.12 has no whisper target.
    #[test]
    fn an_addon_whisper_is_refused() {
        let mut request = whisper(1, 0, "", false);
        request.language = 0xFFFF_FFFF;
        assert_eq!(
            whisper_lines(SPEAKER, request),
            Err(ChatRefusal::UnsupportedKind)
        );
    }

    #[test]
    fn a_self_whisper_is_the_incoming_line_and_the_echo() {
        let mut request = whisper(1, 0, "", false);
        request.target.guid = SPEAKER;
        assert_eq!(
            whisper_lines(SPEAKER, request),
            Ok(vec![
                line(0x06, SPEAKER, 2, "meet me at the gate", SPEAKER),
                line(0x07, SPEAKER, 0, "meet me at the gate", SPEAKER),
            ])
        );
    }

    /// The speaker's guid is an argument, so the operator gate is the entire authorization. A gate
    /// that is present but neutralized (`if false`, `let _ =`, an early return above it) is no gate,
    /// so the scan anchors to the opening brace.
    #[test]
    fn the_realm_chat_reducers_are_operator_gated() {
        for signature in ["pub fn realm_chat(", "pub fn realm_whisper("] {
            let body = code_of(include_str!("realm_chat.rs"), signature);
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{signature}` no longer OPENS with the operator gate. Body was:\n{body}"
            );
        }
    }
}
