//! Mail rules shared by the module and gateway.

pub const HAS_UNREAD_MAIL: f32 = 0.0;
pub const NO_UNREAD_MAIL: f32 = -86400.0;
pub const MAILBOX_GO_TYPE: u8 = 19;
pub const MAILBOX_RANGE_SQ: f32 = 100.0;
/// The client's inbox holds 50 mails, so the list stops there (cmangos `MailHandler.cpp:42,557`).
pub const INBOX_CAPACITY: usize = 50;
/// `MailCheckMask` bits (cmangos `Mails/Mail.h:65-73`). The client reads a mail's state from them.
/// READ is stored as `game_mail.was_read`; the other bits are stored in `game_mail.check_flags`.
pub const CHECK_MASK_READ: u32 = 0x01;
pub const CHECK_MASK_RETURNED: u32 = 0x02;
pub const CHECK_MASK_COPIED: u32 = 0x04;
pub const CHECK_MASK_COD_PAYMENT: u32 = 0x08;
pub const CHECK_MASK_HAS_BODY: u32 = 0x10;
/// `MailMessageType` codes (cmangos `Mails/Mail.h:54-61`), stored as `game_mail.sender_kind`.
pub const SENDER_KIND_CHARACTER: u8 = 0;
pub const SENDER_KIND_AUCTION: u8 = 2;
pub const SENDER_KIND_CREATURE: u8 = 3;
pub const SENDER_KIND_GAMEOBJECT: u8 = 4;
/// `Stationery.dbc` ids (cmangos `Mails/Mail.h:79-84`).
pub const STATIONERY_DEFAULT: u32 = 41;
pub const STATIONERY_AUCTION: u32 = 62;
/// Who a mail is from, as the client names it in the inbox. `game_mail` stores it as three
/// columns: `sender_kind`, `sender_guid` and `sender_entry`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MailSender {
    Character(u64),
    /// The `AuctionHouse.dbc` id (cmangos `Mails/Mail.cpp:81-84`).
    AuctionHouse(u32),
    /// The creature template entry (cmangos `Mails/Mail.cpp:48-75`).
    Creature(u32),
    /// The gameobject template entry.
    Gameobject(u32),
}

impl MailSender {
    /// `(sender_kind, sender_guid, sender_entry)`.
    pub fn columns(self) -> (u8, u64, u32) {
        match self {
            Self::Character(guid) => (SENDER_KIND_CHARACTER, guid, 0),
            Self::AuctionHouse(house) => (SENDER_KIND_AUCTION, 0, house),
            Self::Creature(entry) => (SENDER_KIND_CREATURE, 0, entry),
            Self::Gameobject(entry) => (SENDER_KIND_GAMEOBJECT, 0, entry),
        }
    }
    /// The inverse of [`Self::columns`]. The Module writes no other kind, so any other code reads
    /// as a Character mail. Every row written before the column existed is one.
    pub fn from_columns(kind: u8, guid: u64, entry: u32) -> Self {
        match kind {
            SENDER_KIND_AUCTION => Self::AuctionHouse(entry),
            SENDER_KIND_CREATURE => Self::Creature(entry),
            SENDER_KIND_GAMEOBJECT => Self::Gameobject(entry),
            _ => Self::Character(guid),
        }
    }
    pub fn stationery(self) -> u32 {
        match self {
            Self::AuctionHouse(_) => STATIONERY_AUCTION,
            _ => STATIONERY_DEFAULT,
        }
    }
}
const DAY_SECS: i64 = 86_400;
/// When a mail became visible to its recipient. A delivery instant of 0 means "when it was
/// created", which is true for every mail that was never delayed.
pub fn arrived_at_secs(created_secs: i64, deliver_secs: i64) -> i64 {
    created_secs.max(deliver_secs)
}
/// A mail expires 3 days after it arrives with a cash on delivery price and 30 days after it
/// arrives without one (cmangos `Mails/Mail.cpp:301-313`). The list countdown and the expiry both
/// read this, so they cannot drift apart.
pub fn expires_at_secs(created_secs: i64, deliver_secs: i64, cod: u32) -> i64 {
    let lifetime_days = if cod > 0 { 3 } else { 30 };
    arrived_at_secs(created_secs, deliver_secs).saturating_add(lifetime_days * DAY_SECS)
}
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
    // The payment keeps the letter's own subject. The client shows it as "COD Payment: <subject>"
    // because the letter carries CHECK_MASK_COD_PAYMENT (cmangos `MailHandler.cpp:475-477`).
    Some(CodSettlement {
        payer_guid: taker_guid,
        payee_guid: sender_guid,
        copper: cod,
        subject: subject.to_string(),
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
/// The days left before [`expires_at_secs`], as the list packet's countdown.
pub fn expiration_days(created_secs: i64, deliver_secs: i64, cod: u32, now_secs: i64) -> f32 {
    let left_secs = expires_at_secs(created_secs, deliver_secs, cod).saturating_sub(now_secs);
    (left_secs as f32 / DAY_SECS as f32).max(0.0)
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
pub const NO_SENDER_TO_RETURN_TO: &str = "mail: only a character's mail can be returned";
pub const ALREADY_RETURNED: &str = "mail: a returned mail cannot be returned again";
pub const COD_MAIL_UNDELETABLE: &str =
    "mail: a mail with a cash on delivery price cannot be deleted";

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
        assert_eq!(
            s.subject, "Your sword",
            "the client adds the \"COD Payment: \" prefix itself"
        );
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
    fn a_mail_expires_thirty_days_after_it_arrives_and_three_with_a_price() {
        assert_eq!(expires_at_secs(1_000, 0, 0), 1_000 + 2_592_000);
        assert_eq!(expires_at_secs(1_000, 0, 250), 1_000 + 259_200);
    }

    #[test]
    fn a_delivery_after_creation_starts_the_expiry_clock_at_delivery() {
        assert_eq!(expires_at_secs(1_000, 5_000, 0), 5_000 + 2_592_000);
        assert_eq!(
            expires_at_secs(5_000, 1_000, 0),
            5_000 + 2_592_000,
            "a delivery instant before creation cannot shorten the mail's life"
        );
    }

    #[test]
    fn the_expiry_countdown_runs_down_in_days_and_floors_at_zero() {
        assert_eq!(expiration_days(1_000, 0, 0, 1_000), 30.0);
        assert_eq!(expiration_days(0, 0, 0, 86_400), 29.0);
        assert_eq!(expiration_days(0, 0, 250, 86_400), 2.0);
        assert_eq!(expiration_days(0, 0, 0, 86_400 * 400), 0.0);
    }

    #[test]
    fn each_sender_is_stored_under_its_vanilla_message_type_code() {
        assert_eq!(MailSender::Character(42).columns(), (0, 42, 0));
        assert_eq!(MailSender::AuctionHouse(7).columns(), (2, 0, 7));
        assert_eq!(MailSender::Creature(11_811).columns(), (3, 0, 11_811));
        assert_eq!(MailSender::Gameobject(176_582).columns(), (4, 0, 176_582));
    }

    #[test]
    fn a_sender_reads_back_from_its_columns() {
        for sender in [
            MailSender::Character(42),
            MailSender::Character(0),
            MailSender::AuctionHouse(7),
            MailSender::Creature(11_811),
            MailSender::Gameobject(176_582),
        ] {
            let (kind, guid, entry) = sender.columns();
            assert_eq!(MailSender::from_columns(kind, guid, entry), sender);
        }
        assert_eq!(
            MailSender::from_columns(5, 42, 9),
            MailSender::Character(42),
            "a kind the Module never writes renders as the Character mail every legacy row is"
        );
    }

    #[test]
    fn only_auction_mail_is_written_on_auction_house_stationery() {
        assert_eq!(MailSender::AuctionHouse(7).stationery(), 62);
        assert_eq!(MailSender::Character(42).stationery(), 41);
        assert_eq!(MailSender::Creature(11_811).stationery(), 41);
        assert_eq!(MailSender::Gameobject(176_582).stationery(), 41);
    }
}
