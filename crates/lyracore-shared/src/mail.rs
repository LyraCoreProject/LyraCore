//! Mailbox rules shared by the module and the gateway.
//!
//! The mail ROW lives in the module (`module/src/mail.rs`), but every decision the wire needs is
//! made in the gateway, which builds `SMSG_MAIL_LIST_RESULT` from a cache read rather than from a
//! reducer return value. Putting the rules here is what makes "the realm plane and the
//! single-database fallback answer the same list" a structural fact instead of two copies that
//! agree today: both planes reach the same functions below.

/// `MSG_QUERY_NEXT_MAIL_TIME_Server.unread_mails` when the player HAS unread mail — the minimap
/// envelope lights up.
pub const HAS_UNREAD_MAIL: f32 = 0.0;

/// The same field with no unread mail. mangoszero sends `-86400.0`, vmangos `-1.0`; the client
/// accepts either, and mangoszero's is the one whose sign the 1.12 UI was observed against.
pub const NO_UNREAD_MAIL: f32 = -86400.0;

/// cmangos `GAMEOBJECT_TYPE_MAILBOX`. The 1.12 client opens the mail frame ITSELF on a type-19
/// gameobject and then sends `CMSG_GET_MAIL_LIST` — there is no `SMSG_SHOW_MAILBOX` in vanilla — so
/// this type is the whole server-side meaning of "that guid is a mailbox".
pub const MAILBOX_GO_TYPE: u8 = 19;

/// Squared interaction range for a mailbox, in yards² — the same 10 yd the module's gameobject
/// `use` gate applies (`gameobject::USE_RANGE_SQ`). A mailbox is clicked, so it is the same walk-up
/// distance as a chest.
pub const MAILBOX_RANGE_SQ: f32 = 100.0;

/// Days a mail is stamped to live. Nothing expires a row — the field exists so the client's own
/// countdown renders. The design takes "no expiry reaper" deliberately: nothing is silently
/// destroyed.
pub const EXPIRY_DAYS: f32 = 30.0;

/// The float the client's periodic mail poll expects.
pub fn unread_mail_signal(has_unread: bool) -> f32 {
    if has_unread {
        HAS_UNREAD_MAIL
    } else {
        NO_UNREAD_MAIL
    }
}

/// `Mail.expiration_time` — DAYS remaining, as vanilla sends it (mangoszero divides the remaining
/// seconds by one day). Clamped at 0 so a mail older than the stamp renders as "expiring", never as
/// a negative countdown.
pub fn expiration_days(created_at_secs: i64, now_secs: i64) -> f32 {
    let elapsed_days = (now_secs - created_at_secs) as f32 / 86_400.0;
    (EXPIRY_DAYS - elapsed_days).max(0.0)
}

/// The `item_text_id` a mail advertises in `SMSG_MAIL_LIST_RESULT`.
///
/// The letter body does not ride the list packet: the client fetches it with
/// `CMSG_ITEM_TEXT_QUERY` on this id. The mail's OWN id doubles as it, so no text table exists —
/// and 0 means "no body", which the client never queries. An id past `u32` is unreachable in
/// practice (ids are per-database auto-inc) and answers 0 rather than a wrapped id that would
/// address a different mail.
pub fn item_text_id_for(mail_id: u64, body: &str) -> u32 {
    if body.is_empty() {
        return 0;
    }
    u32::try_from(mail_id).unwrap_or(0)
}

/// The sender has no live entity — every mail opcode is refused at character select and mid-hop.
pub const NOT_IN_WORLD: &str = "mail: not in world";

/// The guid the client named is not a mailbox the sender can reach (wrong map, wrong instance, out
/// of range, or not a mailbox at all).
pub fn not_at_mailbox(mailbox_guid: u64) -> String {
    format!("mail: not at mailbox {mailbox_guid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The poll answer, pinned by value: 0.0 is the "you have mail" the envelope reads, and the
    /// no-mail answer must be NEGATIVE — a positive or zero value there lights the indicator for a
    /// player with an empty mailbox.
    #[test]
    fn the_mail_poll_answers_zero_with_unread_mail_and_a_negative_value_without() {
        assert_eq!(unread_mail_signal(true), 0.0);
        assert!(unread_mail_signal(false) < 0.0);
        assert_eq!(unread_mail_signal(false), -86400.0);
    }

    /// An empty body sends id 0 (the client then never queries it); a non-empty body sends the
    /// mail's own id, which is what `CMSG_ITEM_TEXT_QUERY` comes back with.
    #[test]
    fn the_text_id_is_the_mails_own_id_and_zero_for_an_empty_body() {
        assert_eq!(item_text_id_for(7, "hello"), 7);
        assert_eq!(item_text_id_for(7, ""), 0);
        assert_eq!(item_text_id_for(u64::from(u32::MAX) + 1, "hello"), 0);
    }

    /// The countdown starts at the full stamp and never goes negative.
    #[test]
    fn the_expiry_countdown_runs_down_in_days_and_floors_at_zero() {
        assert_eq!(expiration_days(1_000, 1_000), EXPIRY_DAYS);
        assert_eq!(expiration_days(0, 86_400), EXPIRY_DAYS - 1.0);
        assert_eq!(expiration_days(0, 86_400 * 400), 0.0);
    }
}
