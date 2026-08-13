//! Mail rules shared by the module and gateway.

pub const HAS_UNREAD_MAIL: f32 = 0.0;
pub const NO_UNREAD_MAIL: f32 = -86400.0;
pub const MAILBOX_GO_TYPE: u8 = 19;
pub const MAILBOX_RANGE_SQ: f32 = 100.0;
pub const EXPIRY_DAYS: f32 = 30.0;
pub fn escrow_range_mark(gateway_id: &str) -> u64 {
    const OFFSET: u64 = 10_000;
    const SLOTS: u64 = 50_000;
    let hash = gateway_id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    (OFFSET + hash % SLOTS) * 1_000_000_000
}
pub fn resume_escrow_range(
    base: u64,
    size: u64,
    seen: impl Iterator<Item = u64>,
) -> Option<(u64, u64)> {
    let end = base.checked_add(size)?;
    let next = seen
        .filter(|id| *id >= base && *id < end)
        .max()
        .map_or(base.max(1), |id| id.saturating_add(1));
    (next < end).then_some((next, end))
}
pub const POSTAGE_COPPER: u32 = 30;
pub fn postage() -> u32 {
    POSTAGE_COPPER
}
pub fn total_cost(attached_money: u32) -> u32 {
    postage().saturating_add(attached_money)
}
pub const NOT_ENOUGH_MONEY: &str = "mail: not enough money for postage";
pub const COD_NOT_AFFORDABLE: &str = "mail: not enough money for the cash on delivery";
pub fn cod_at_send(cod: u32, has_attachment: bool) -> u32 {
    if has_attachment {
        cod
    } else {
        0
    }
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CodSettlement {
    pub payer_guid: u64,
    pub payee_guid: u64,
    pub copper: u32,
    pub subject: String,
}
pub fn cod_settlement(
    cod: u32,
    sender_guid: u64,
    subject: &str,
    taker_guid: u64,
) -> Option<CodSettlement> {
    if cod == 0 || sender_guid == taker_guid {
        return None;
    }
    Some(CodSettlement {
        payer_guid: taker_guid,
        payee_guid: sender_guid,
        copper: cod,
        subject: format!("COD Payment: {subject}"),
    })
}
pub fn no_recipient_named(name: &str) -> String {
    format!("mail: no character named {name}")
}
pub const CANNOT_SEND_TO_SELF: &str = "mail: cannot send to yourself";
pub const NOT_YOUR_TEAM: &str = "mail: recipient is not your team";
pub fn ambiguous_recipient(name: &str) -> String {
    format!("mail: {name} names more than one character on the realm")
}
pub fn unread_mail_signal(has_unread: bool) -> f32 {
    if has_unread {
        HAS_UNREAD_MAIL
    } else {
        NO_UNREAD_MAIL
    }
}
pub fn expiration_days(created_at_secs: i64, now_secs: i64) -> f32 {
    let elapsed_days = (now_secs - created_at_secs) as f32 / 86_400.0;
    (EXPIRY_DAYS - elapsed_days).max(0.0)
}
pub fn item_text_id_for(mail_id: u64, body: &str) -> u32 {
    if body.is_empty() {
        return 0;
    }
    u32::try_from(mail_id).unwrap_or(0)
}
pub const NOT_IN_WORLD: &str = "mail: not in world";
pub fn not_at_mailbox(mailbox_guid: u64) -> String {
    format!("mail: not at mailbox {mailbox_guid}")
}
pub const NOTHING_TO_TAKE: &str = "mail: nothing to take from that mail";
pub const NOT_YOUR_ITEM: &str = "mail: that item is not yours to send";
pub const ITEM_IS_SOULBOUND: &str = "mail: soulbound items cannot be mailed";
pub const INVENTORY_FULL: &str = "inventory full";
pub const NOT_YOUR_MAIL: &str = "mail: not addressed to you";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mail_poll_answers_zero_with_unread_mail_and_a_negative_value_without() {
        assert_eq!(unread_mail_signal(true), 0.0);
        assert!(unread_mail_signal(false) < 0.0);
        assert_eq!(unread_mail_signal(false), -86400.0);
    }

    #[test]
    fn escrow_range_choice_is_stable_and_separate_for_gateway_identities() {
        assert_eq!(
            escrow_range_mark("gateway-a"),
            escrow_range_mark("gateway-a")
        );
        assert_ne!(
            escrow_range_mark("gateway-a"),
            escrow_range_mark("gateway-b")
        );
        assert!(escrow_range_mark("gateway-a") >= 10_000_000_000_000);
    }

    #[test]
    fn escrow_range_resume_ignores_other_ranges_and_refuses_exhaustion() {
        assert_eq!(
            resume_escrow_range(100, 10, [3, 102, 999].into_iter()),
            Some((103, 110))
        );
        assert_eq!(resume_escrow_range(100, 10, [109].into_iter()), None);
        assert_eq!(resume_escrow_range(0, 10, [].into_iter()), Some((1, 10)));
    }

    #[test]
    fn the_text_id_is_the_mails_own_id_and_zero_for_an_empty_body() {
        assert_eq!(item_text_id_for(7, "hello"), 7);
        assert_eq!(item_text_id_for(7, ""), 0);
        assert_eq!(item_text_id_for(u64::from(u32::MAX) + 1, "hello"), 0);
    }

    #[test]
    fn a_text_only_letter_costs_the_flat_postage() {
        assert_eq!(postage(), 30);
        assert_eq!(total_cost(0), 30);
    }

    #[test]
    fn attached_money_adds_to_the_postage_and_the_total_saturates() {
        assert_eq!(total_cost(100), 130);
        assert_eq!(total_cost(u32::MAX), u32::MAX);
    }

    #[test]
    fn a_cod_take_debits_the_taker_and_pays_the_letters_sender() {
        let s = cod_settlement(250, 11, "Your sword", 22).expect("a priced letter settles");
        assert_eq!(s.payer_guid, 22, "the taker pays");
        assert_eq!(s.payee_guid, 11, "the seller is paid");
        assert_eq!(s.copper, 250, "exactly the price, no fee either way");
        assert_eq!(s.subject, "COD Payment: Your sword");
    }

    #[test]
    fn a_free_letter_and_a_self_addressed_one_settle_nothing() {
        assert_eq!(cod_settlement(0, 11, "Your sword", 22), None);
        assert_eq!(cod_settlement(250, 11, "Your sword", 11), None);
    }

    #[test]
    fn a_cod_price_needs_an_attachment_to_survive_the_send() {
        assert_eq!(cod_at_send(250, true), 250);
        assert_eq!(cod_at_send(250, false), 0);
    }

    #[test]
    fn the_expiry_countdown_runs_down_in_days_and_floors_at_zero() {
        assert_eq!(expiration_days(1_000, 1_000), EXPIRY_DAYS);
        assert_eq!(expiration_days(0, 86_400), EXPIRY_DAYS - 1.0);
        assert_eq!(expiration_days(0, 86_400 * 400), 0.0);
    }
}
