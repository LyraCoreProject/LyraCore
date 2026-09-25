//! Chat Flood Limiter: the Gateway's in-memory count of fast chat lines per World Session.
//!
//! cmangos keeps the same count on the player and the mute on the session, and saves neither
//! (cm:Player.cpp:16344-16377, cm:ChatHandler.cpp:157-167). The limiter runs before any Durable
//! Request, so a muted line costs no reducer call. The defaults are cmangos's
//! (cm:mangosd.conf.dist.in:1158-1160).

use std::time::{Duration, Instant};

use lyracore_shared::chat::{chat_kind, speakable_language};
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{CMSG_MESSAGECHAT_ChatType, Language, SMSG_NOTIFICATION};

/// `ChatFlood.MessageDelay`: a counted line that arrives sooner than this after the previous one
/// is fast.
const FAST_LINE_WINDOW: Duration = Duration::from_secs(1);
/// `ChatFlood.MessageCount`: this many fast lines in a row mute the World Session.
const FAST_LINES_TO_MUTE: u32 = 10;
/// `ChatFlood.MuteTime`.
const MUTE: Duration = Duration::from_secs(10);

/// What the limiter does with one client message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FloodRule {
    /// A chat line the limiter counts and refuses while muted.
    Counted,
    /// Refused while muted, never counted: `CMSG_TEXT_EMOTE` (cm:ChatHandler.cpp:761-766).
    RefusedWhileMuted,
    /// Everything else, including `/afk`, `/dnd` and any addon-language line
    /// (cm:ChatHandler.cpp:113-119, cm:ChatHandler.cpp:157).
    Free,
}

fn rule(msg: &ClientOpcodeMessage) -> FloodRule {
    match msg {
        ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) => {
            let away = matches!(
                chat.chat_type,
                CMSG_MESSAGECHAT_ChatType::Afk | CMSG_MESSAGECHAT_ChatType::Dnd
            );
            if away || chat.language == Language::Addon {
                FloodRule::Free
            } else {
                FloodRule::Counted
            }
        }
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(_) => FloodRule::RefusedWhileMuted,
        _ => FloodRule::Free,
    }
}

/// The `chat_kind` a client `CMSG_MESSAGECHAT` line carries, for every chat type this limiter
/// counts. `None` for a type with no Chat Kind of its own (`rule` never marks those `Counted`).
fn wire_chat_kind(chat_type: &CMSG_MESSAGECHAT_ChatType) -> Option<u8> {
    match chat_type {
        CMSG_MESSAGECHAT_ChatType::Say => Some(chat_kind::SAY),
        CMSG_MESSAGECHAT_ChatType::Yell => Some(chat_kind::YELL),
        CMSG_MESSAGECHAT_ChatType::Emote => Some(chat_kind::EMOTE),
        CMSG_MESSAGECHAT_ChatType::Party => Some(chat_kind::PARTY),
        CMSG_MESSAGECHAT_ChatType::Raid => Some(chat_kind::RAID),
        CMSG_MESSAGECHAT_ChatType::RaidLeader => Some(chat_kind::RAID_LEADER),
        CMSG_MESSAGECHAT_ChatType::RaidWarning => Some(chat_kind::RAID_WARNING),
        CMSG_MESSAGECHAT_ChatType::Guild => Some(chat_kind::GUILD),
        CMSG_MESSAGECHAT_ChatType::Officer => Some(chat_kind::OFFICER),
        CMSG_MESSAGECHAT_ChatType::Channel { .. } => Some(chat_kind::CHANNEL),
        CMSG_MESSAGECHAT_ChatType::Whisper { .. } => Some(chat_kind::WHISPER),
        _ => None,
    }
}

/// cm:ChatHandler.cpp:100-111: the language check runs, and returns on failure, before
/// `UpdateSpeakTime`, so a line the language Gate refuses never reaches the mute check and never
/// counts. `race` is `None` when the Gateway cannot read the speaker (no live entity); such a line
/// counts as usual; the Module's own Gate is the fallback authority when this read is stale.
fn language_refused(msg: &ClientOpcodeMessage, race: Option<u8>) -> bool {
    let ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) = msg else {
        return false;
    };
    let (Some(kind), Some(race)) = (wire_chat_kind(&chat.chat_type), race) else {
        return false;
    };
    speakable_language(kind, race, chat.language.as_int()).is_err()
}

#[derive(Debug, Default)]
pub(crate) struct ChatFloodLimiter {
    /// The previous counted line's time plus [`FAST_LINE_WINDOW`].
    fast_until: Option<Instant>,
    fast_lines: u32,
    muted_until: Option<Instant>,
}

impl ChatFloodLimiter {
    /// Judge one client message at `now`. `Some` is the answer to a refused message, which must go
    /// no further. `exempt` is asked only when a line would mute: cmangos never mutes a game master
    /// for flooding (cm:Player.cpp:16346-16348), so the cost of that read lands on the tenth fast
    /// line only. `speaker_race` is asked for every counted line, to keep a line the language Gate
    /// would refuse off the count (cm:ChatHandler.cpp:100-111); that line still reaches the Module,
    /// which answers "You don't know that language" as its own authority.
    pub(crate) fn judge(
        &mut self,
        msg: &ClientOpcodeMessage,
        now: Instant,
        exempt: impl FnOnce() -> bool,
        speaker_race: impl FnOnce() -> Option<u8>,
    ) -> Option<ServerOpcodeMessage> {
        let rule = rule(msg);
        if rule == FloodRule::Free {
            return None;
        }
        if rule == FloodRule::Counted && language_refused(msg, speaker_race()) {
            return None;
        }
        if let Some(remaining) = self.muted_for(now) {
            return Some(wait_notice(remaining));
        }
        if rule == FloodRule::Counted {
            self.count(now, exempt);
        }
        None
    }

    fn muted_for(&self, now: Instant) -> Option<Duration> {
        self.muted_until
            .and_then(|until| until.checked_duration_since(now))
            .filter(|remaining| !remaining.is_zero())
    }

    /// `Player::UpdateSpeakTime`: the tenth fast line in a row mutes and starts the count again.
    /// A mute never shortens one already running.
    fn count(&mut self, now: Instant, exempt: impl FnOnce() -> bool) {
        if self.fast_until.is_some_and(|until| now < until) {
            self.fast_lines += 1;
            if self.fast_lines >= FAST_LINES_TO_MUTE {
                if !exempt() {
                    let mute_until = now + MUTE;
                    self.muted_until = Some(
                        self.muted_until
                            .map_or(mute_until, |until| until.max(mute_until)),
                    );
                }
                self.fast_lines = 0;
            }
        } else {
            self.fast_lines = 0;
        }
        self.fast_until = Some(now + FAST_LINE_WINDOW);
    }
}

/// cm mangos.sql:3981 (`LANG_WAIT_BEFORE_SPEAKING`) with the remaining whole seconds formatted by
/// cm Util.cpp:211-232 `secsToTimeString`. The mute never reaches a minute, so only its seconds
/// part is ever printed. cmangos counts in whole wall-clock seconds; rounding up keeps the first
/// answer at "10 Second(s)." and the last at "1 Second(s).".
fn wait_notice(remaining: Duration) -> ServerOpcodeMessage {
    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
        notification: format!("You must wait {seconds} Second(s). before speaking again."),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::{Guid, TextEmote, CMSG_MESSAGECHAT, CMSG_TEXT_EMOTE};

    fn line(chat_type: CMSG_MESSAGECHAT_ChatType, language: Language) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_MESSAGECHAT(Box::new(CMSG_MESSAGECHAT {
            chat_type,
            language,
            message: "spam".to_string(),
        }))
    }

    fn say() -> ClientOpcodeMessage {
        line(CMSG_MESSAGECHAT_ChatType::Say, Language::Common)
    }

    fn text_emote() -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(Box::new(CMSG_TEXT_EMOTE {
            text_emote: TextEmote::Wave,
            emote: 3,
            target: Guid::new(0),
        }))
    }

    fn notice(answer: Option<ServerOpcodeMessage>) -> Option<String> {
        answer.map(|message| match message {
            ServerOpcodeMessage::SMSG_NOTIFICATION(notice) => notice.notification,
            other => panic!("expected SMSG_NOTIFICATION, got {other}"),
        })
    }

    fn never_exempt() -> bool {
        false
    }

    /// A Human, who knows Common: every fixture line in this file speaks Common or forces
    /// Universal, so this race never trips the language check.
    fn human_race() -> Option<u8> {
        Some(1)
    }

    /// Send `count` lines 100 ms apart from `start` and return the answer to each.
    fn flood(limiter: &mut ChatFloodLimiter, start: Instant, count: u64) -> Vec<Option<String>> {
        (0..count)
            .map(|n| {
                let now = start + Duration::from_millis(100 * n);
                notice(limiter.judge(&say(), now, never_exempt, human_race))
            })
            .collect()
    }

    /// The first line starts the count at zero and each of the next ten adds one
    /// (cm:Player.cpp:16354-16374), so the eleventh fast line mutes and still goes out.
    #[test]
    fn the_eleventh_fast_line_mutes_and_the_twelfth_is_refused() {
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        let answers = flood(&mut limiter, start, 12);
        assert_eq!(answers[..11], vec![None; 11]);
        assert_eq!(
            answers[11].as_deref(),
            Some("You must wait 10 Second(s). before speaking again.")
        );
    }

    #[test]
    fn the_mute_lasts_ten_seconds_from_the_line_that_set_it() {
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        flood(&mut limiter, start, 11);
        let muted_at = start + Duration::from_millis(1000);
        assert_eq!(
            notice(limiter.judge(
                &say(),
                muted_at + Duration::from_millis(9_500),
                never_exempt,
                human_race
            ))
            .as_deref(),
            Some("You must wait 1 Second(s). before speaking again.")
        );
        assert_eq!(
            notice(limiter.judge(
                &say(),
                muted_at + Duration::from_secs(10),
                never_exempt,
                human_race
            )),
            None,
            "cm:Player.cpp:16374-16377 lets the speaker talk once the mute time is reached"
        );
    }

    #[test]
    fn a_pause_of_one_second_starts_the_count_again() {
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        flood(&mut limiter, start, 10);
        let after_pause = start + Duration::from_millis(900 + 1_000);
        let answers = flood(&mut limiter, after_pause, 12);
        assert_eq!(
            answers[..11],
            vec![None; 11],
            "the nine fast lines before the pause no longer count"
        );
        assert!(answers[11].is_some());
    }

    #[test]
    fn a_muted_line_does_not_count_or_extend_the_mute() {
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        flood(&mut limiter, start, 11);
        let muted_at = start + Duration::from_millis(1000);
        for n in 1..=30 {
            limiter.judge(
                &say(),
                muted_at + Duration::from_millis(100 * n),
                never_exempt,
                human_race,
            );
        }
        assert_eq!(
            notice(limiter.judge(
                &say(),
                muted_at + Duration::from_secs(10),
                never_exempt,
                human_race
            )),
            None
        );
    }

    #[test]
    fn a_text_emote_is_refused_while_muted_and_never_counted() {
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        for n in 0..30 {
            let now = start + Duration::from_millis(10 * n);
            assert_eq!(
                notice(limiter.judge(&text_emote(), now, never_exempt, human_race)),
                None
            );
        }
        flood(&mut limiter, start + Duration::from_secs(5), 11);
        assert_eq!(
            notice(limiter.judge(
                &text_emote(),
                start + Duration::from_secs(7),
                never_exempt,
                human_race
            ))
            .as_deref(),
            Some("You must wait 9 Second(s). before speaking again.")
        );
    }

    /// Every counted kind the 1.12 client can send, each spoken while muted.
    #[test]
    fn every_other_chat_kind_counts_and_is_refused_while_muted() {
        let kinds = [
            CMSG_MESSAGECHAT_ChatType::Say,
            CMSG_MESSAGECHAT_ChatType::Yell,
            CMSG_MESSAGECHAT_ChatType::Emote,
            CMSG_MESSAGECHAT_ChatType::Party,
            CMSG_MESSAGECHAT_ChatType::Raid,
            CMSG_MESSAGECHAT_ChatType::RaidLeader,
            CMSG_MESSAGECHAT_ChatType::RaidWarning,
            CMSG_MESSAGECHAT_ChatType::Guild,
            CMSG_MESSAGECHAT_ChatType::Officer,
            CMSG_MESSAGECHAT_ChatType::Whisper {
                target_player: "Rx".to_string(),
            },
            CMSG_MESSAGECHAT_ChatType::Channel {
                channel: "Rx".to_string(),
            },
        ];
        for chat_type in kinds {
            let start = Instant::now();
            let mut limiter = ChatFloodLimiter::default();
            let spoken = line(chat_type.clone(), Language::Common);
            for n in 0..11 {
                let now = start + Duration::from_millis(100 * n);
                assert_eq!(limiter.judge(&spoken, now, never_exempt, human_race), None);
            }
            assert!(
                limiter
                    .judge(
                        &spoken,
                        start + Duration::from_secs(2),
                        never_exempt,
                        human_race
                    )
                    .is_some(),
                "{chat_type:?}"
            );
        }
    }

    /// cm:ChatHandler.cpp:113-119 and :157 keep `/afk`, `/dnd` and the addon language out of the
    /// count and out of the mute.
    #[test]
    fn away_and_addon_lines_never_count_and_pass_while_muted() {
        let free = [
            line(CMSG_MESSAGECHAT_ChatType::Afk, Language::Universal),
            line(CMSG_MESSAGECHAT_ChatType::Dnd, Language::Universal),
            line(CMSG_MESSAGECHAT_ChatType::Guild, Language::Addon),
            line(CMSG_MESSAGECHAT_ChatType::Party, Language::Addon),
        ];
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        for n in 0..40 {
            let now = start + Duration::from_millis(10 * n);
            for message in &free {
                assert_eq!(limiter.judge(message, now, never_exempt, human_race), None);
            }
        }
        flood(&mut limiter, start + Duration::from_secs(1), 11);
        assert!(
            limiter
                .judge(
                    &say(),
                    start + Duration::from_secs(3),
                    never_exempt,
                    human_race
                )
                .is_some(),
            "the session is muted"
        );
        for message in &free {
            assert_eq!(
                limiter.judge(
                    message,
                    start + Duration::from_secs(3),
                    never_exempt,
                    human_race
                ),
                None
            );
        }
    }

    #[test]
    fn a_game_master_is_never_muted() {
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        for n in 0..40 {
            let now = start + Duration::from_millis(100 * n);
            assert_eq!(limiter.judge(&say(), now, || true, human_race), None);
        }
    }

    #[test]
    fn the_game_master_read_happens_only_when_a_line_would_mute() {
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        let reads = std::cell::Cell::new(0);
        for n in 0..11 {
            let now = start + Duration::from_millis(100 * n);
            limiter.judge(
                &say(),
                now,
                || {
                    reads.set(reads.get() + 1);
                    false
                },
                human_race,
            );
        }
        assert_eq!(reads.get(), 1);
    }

    /// cm:ChatHandler.cpp:100-111: the language check runs first and returns early, so a line the
    /// race does not know never reaches the mute check and never counts. It still reaches the
    /// Module, which answers "You don't know that language" on its own authority.
    #[test]
    fn a_line_in_an_unknown_language_does_not_count_toward_the_mute() {
        let orcish = || line(CMSG_MESSAGECHAT_ChatType::Say, Language::Orcish);
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        for n in 0..40 {
            let now = start + Duration::from_millis(100 * n);
            assert_eq!(
                limiter.judge(&orcish(), now, never_exempt, human_race),
                None
            );
        }
        assert!(
            limiter
                .judge(
                    &say(),
                    start + Duration::from_secs(5),
                    never_exempt,
                    human_race
                )
                .is_none(),
            "forty Orcish lines from a Human never muted the session"
        );
    }

    /// A read that fails, or finds no live entity, must not stop a line from counting: the Module's
    /// own language Gate is the fallback authority.
    #[test]
    fn an_unreadable_race_still_counts_toward_the_mute() {
        let no_race = || None;
        let start = Instant::now();
        let mut limiter = ChatFloodLimiter::default();
        let answers: Vec<_> = (0..12)
            .map(|n| {
                let now = start + Duration::from_millis(100 * n);
                notice(limiter.judge(&say(), now, never_exempt, no_race))
            })
            .collect();
        assert_eq!(answers[..11], vec![None; 11]);
        assert!(answers[11].is_some());
    }

    #[test]
    fn another_opcode_is_never_judged() {
        let mut limiter = ChatFloodLimiter::default();
        let start = Instant::now();
        flood(&mut limiter, start, 11);
        let ping = ClientOpcodeMessage::CMSG_PING(Default::default());
        assert_eq!(
            limiter.judge(
                &ping,
                start + Duration::from_secs(2),
                never_exempt,
                human_race
            ),
            None
        );
    }
}
