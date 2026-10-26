use lyracore_protocol::messages::vanilla::{
    opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    CMSG_MESSAGECHAT_ChatType, Language, SMSG_NOTIFICATION,
};
use spacetimedb::{ReducerContext, SpacetimeType};

use super::{game_native_world_connection, NativeWorldConnection};

#[derive(SpacetimeType, Default)]
pub struct ChatFlood {
    fast_until_micros: i64,
    fast_lines: u32,
    muted_until_micros: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Counted,
    RefusedWhileMuted,
    Free,
}

fn rule(message: &ClientOpcodeMessage, race: u8) -> Rule {
    match message {
        ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) => {
            if matches!(
                chat.chat_type,
                CMSG_MESSAGECHAT_ChatType::Afk | CMSG_MESSAGECHAT_ChatType::Dnd
            ) || chat.language == Language::Addon
                || super::chat::kind(&chat.chat_type).is_some_and(|kind| {
                    lyracore_shared::chat::speakable_language(kind, race, chat.language.as_int())
                        .is_err()
                })
            {
                Rule::Free
            } else {
                Rule::Counted
            }
        }
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(_) | ClientOpcodeMessage::CMSG_EMOTE(_) => {
            Rule::RefusedWhileMuted
        }
        _ => Rule::Free,
    }
}

impl ChatFlood {
    fn admit(&mut self, rule: Rule, now_micros: i64, exempt: impl FnOnce() -> bool) -> Option<i64> {
        if rule == Rule::Free {
            return None;
        }
        if self.muted_until_micros > now_micros {
            return Some((self.muted_until_micros - now_micros + 999_999) / 1_000_000);
        }
        if rule == Rule::Counted {
            if now_micros < self.fast_until_micros {
                self.fast_lines += 1;
                if self.fast_lines >= 10 {
                    if !exempt() {
                        self.muted_until_micros = now_micros + 10_000_000;
                    }
                    self.fast_lines = 0;
                }
            } else {
                self.fast_lines = 0;
            }
            self.fast_until_micros = now_micros + 1_000_000;
        }
        None
    }
}

pub(super) fn judge(
    ctx: &ReducerContext,
    mut connection: NativeWorldConnection,
    speaker: &crate::WorldEntity,
    message: &ClientOpcodeMessage,
) -> Option<ServerOpcodeMessage> {
    let rule = rule(message, speaker.race());
    if rule == Rule::Free {
        return None;
    }
    let flood = connection.chat_flood.get_or_insert_with(ChatFlood::default);
    let seconds = flood.admit(rule, ctx.timestamp.to_micros_since_unix_epoch(), || {
        crate::helpers::character_by_guid(ctx, speaker.guid)
            .is_some_and(|c| crate::auth::gm_level(ctx, c.account_id) != 0)
    });
    if rule == Rule::Counted && seconds.is_none() {
        // The limiter belongs to the TCP connection and survives Character logout.
        ctx.db
            .game_native_world_connection()
            .connection_id()
            .update(connection);
    }
    seconds.map(|seconds| {
        ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
            notification: format!("You must wait {seconds} Second(s). before speaking again."),
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eleventh_fast_line_arms_ten_seconds_without_refusing_that_line() {
        let mut flood = ChatFlood::default();
        for i in 0..11 {
            assert_eq!(flood.admit(Rule::Counted, i * 10_000, || false), None);
        }
        assert_eq!(flood.admit(Rule::Counted, 100_001, || false), Some(10));
        assert_eq!(
            flood.admit(Rule::RefusedWhileMuted, 10_099_999, || false),
            Some(1)
        );
        assert_eq!(flood.admit(Rule::Counted, 10_100_000, || false), None);
    }

    #[test]
    fn exempt_lines_pass_a_mute_without_extending_it() {
        let mut flood = ChatFlood {
            muted_until_micros: 20_000_000,
            ..Default::default()
        };
        assert_eq!(flood.admit(Rule::Free, 15_000_000, || false), None);
        assert_eq!(flood.admit(Rule::Counted, 15_000_000, || false), Some(5));
        assert_eq!(flood.admit(Rule::Counted, 20_000_000, || false), None);
    }

    #[test]
    fn a_full_second_between_lines_resets_the_burst() {
        let mut flood = ChatFlood::default();
        for i in 0..30 {
            assert_eq!(flood.admit(Rule::Counted, i * 1_000_000, || false), None);
        }
    }

    #[test]
    fn gm_lines_do_not_arm_a_mute() {
        let mut flood = ChatFlood::default();
        for i in 0..30 {
            assert_eq!(flood.admit(Rule::Counted, i * 10_000, || true), None);
        }
    }
}
