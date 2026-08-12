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

/// Postage for one letter, in copper — vanilla's flat 30, the same constant mangoszero charges
/// per attachment slot. Charged at SEND, so a refused send costs nothing and a delivered one is
/// paid for before the row exists.
pub const POSTAGE_COPPER: u32 = 30;

/// What the post office charges to carry one letter. A function rather than a bare constant read
/// at the call sites, so one place owns the rule.
///
/// Vanilla's formula is `30 * max(1, attachments)` and the wire carries at most ONE attachment, so
/// a letter with an item costs the same 30 as a text-only one. The multiplier stays out of the
/// signature until something can send more than one item.
pub fn postage() -> u32 {
    POSTAGE_COPPER
}

/// The copper that leaves the sender's purse for one mail: the postage, plus any coin travelling
/// WITH the letter. Text-only mail passes 0; the money-attachment slice is what makes the argument
/// non-zero, and it must debit one total rather than charging twice.
///
/// Saturating, so a crafted `money` near `u32::MAX` cannot wrap the total down to something the
/// sender can afford.
pub fn total_cost(attached_money: u32) -> u32 {
    postage().saturating_add(attached_money)
}

/// The sender cannot pay [`total_cost`]. The ONE refusal the module owns on the send path — every
/// other gate is answered by the gateway, which is the only party that can see the whole realm.
pub const NOT_ENOUGH_MONEY: &str = "mail: not enough money for postage";

/// No character on the realm answers to the typed name.
pub fn no_recipient_named(name: &str) -> String {
    format!("mail: no character named {name}")
}

/// Vanilla's rule: a letter to yourself is refused.
pub const CANNOT_SEND_TO_SELF: &str = "mail: cannot send to yourself";

/// Vanilla's rule: mail does not cross the faction line.
pub const NOT_YOUR_TEAM: &str = "mail: recipient is not your team";

/// The typed name resolves to SEVERAL characters and nothing can choose between them.
///
/// Character names are unique per DATABASE, not per realm (`party::resolve_all_by_name`), so a name
/// can name two people at once. A whisper disambiguates by picking the one that is online; mail
/// cannot, because reaching an offline character is the whole point of it. Refusing is deliberate —
/// see the call site in `gateway/src/world/mail.rs`.
pub fn ambiguous_recipient(name: &str) -> String {
    format!("mail: {name} names more than one character on the realm")
}

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

/// The mail holds no copper — a second `CMSG_MAIL_TAKE_MONEY` on a mail already emptied, or a
/// text-only letter. Refused rather than answered `Ok`, so "taking twice credits once" is a visible
/// outcome instead of a silent one, and the row stays readable either way.
pub const NOTHING_TO_TAKE: &str = "mail: nothing to take from that mail";

/// The sender named an item instance that does not exist or is not theirs. Merged, like
/// [`NOT_YOUR_MAIL`]: an item guid is client-supplied, and a refusal must not tell a crafted packet
/// which guids are real.
pub const NOT_YOUR_ITEM: &str = "mail: that item is not yours to send";

/// The instance has bound to its owner. Vanilla refuses it at send with its own line, and the item
/// stays in the sender's bags — an UNWORN bind-on-equip item is unbound and mails fine.
pub const ITEM_IS_SOULBOUND: &str = "mail: soulbound items cannot be mailed";

/// The item module's own full-bag refusal, verbatim (`items::ops`). Named here because the gateway
/// maps exactly this text onto `ErrEquipError` — the variant that tells the player to make room —
/// and a drift would silently become the generic internal error instead.
pub const INVENTORY_FULL: &str = "inventory full";

/// A mark-as-read or delete named a `mail_id` that either does not exist or is not the caller's —
/// deliberately the SAME text for both, the way `letter_body`'s read already answers `None` for
/// either: a crafted id must not be a way to learn which mail ids belong to somebody else.
pub const NOT_YOUR_MAIL: &str = "mail: not addressed to you";

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

    /// Postage is vanilla's flat 30 copper, and a text-only letter costs exactly that.
    #[test]
    fn a_text_only_letter_costs_the_flat_postage() {
        assert_eq!(postage(), 30);
        assert_eq!(total_cost(0), 30);
    }

    /// Attached coin rides on TOP of the postage — one debit, not two — and the total saturates
    /// rather than wrapping, so a crafted amount cannot make an unaffordable mail look affordable.
    #[test]
    fn attached_money_adds_to_the_postage_and_the_total_saturates() {
        assert_eq!(total_cost(100), 130);
        assert_eq!(total_cost(u32::MAX), u32::MAX);
    }

    /// The countdown starts at the full stamp and never goes negative.
    #[test]
    fn the_expiry_countdown_runs_down_in_days_and_floors_at_zero() {
        assert_eq!(expiration_days(1_000, 1_000), EXPIRY_DAYS);
        assert_eq!(expiration_days(0, 86_400), EXPIRY_DAYS - 1.0);
        assert_eq!(expiration_days(0, 86_400 * 400), 0.0);
    }
}
