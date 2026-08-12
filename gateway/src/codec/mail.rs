//! Mail wire: the mailbox window (`SMSG_MAIL_LIST_RESULT`), the client's periodic mail poll
//! (`MSG_QUERY_NEXT_MAIL_TIME`), and the letter body (`SMSG_ITEM_TEXT_QUERY_RESPONSE`).
//!
//! All gtker-typed vanilla — unlike the vendor list, this family needs no raw encoder. What is
//! decided here is only the field MAPPING; every rule the mapping applies (the unread float, the
//! `item_text_id`, the expiry stamp) lives in `lyracore_shared::mail`, because the module writes the
//! rows and the gateway renders them and the two must not each own a copy.

use lyracore_shared::mail as mail_rules;
use wow_world_messages::vanilla::{
    Gold, MSG_QUERY_NEXT_MAIL_TIME_Server, Mail, Mail_MailType, SMSG_SEND_MAIL_RESULT_MailAction,
    SMSG_SEND_MAIL_RESULT_MailResultTwo, SMSG_ITEM_TEXT_QUERY_RESPONSE, SMSG_MAIL_LIST_RESULT,
    SMSG_SEND_MAIL_RESULT,
};

/// One `game_mail` row, flattened for the codec. The attachment columns are carried even though
/// nothing writes them yet: the wire has exactly one item block per mail, and the list packet is
/// what a later slice's attachment must render through.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailView {
    pub id: u64,
    pub sender_guid: u64,
    pub subject: String,
    pub body: String,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub money: u32,
    pub cod: u32,
    pub was_read: bool,
    /// Unix seconds the row was written — the base of the client's expiry countdown.
    pub created_at_secs: i64,
}

/// Build `SMSG_MAIL_LIST_RESULT` — the mailbox window. An empty `mails` is a legitimate reply and
/// the one that tells "no mail" apart from "the server ignored me", so the caller always sends it.
///
/// `now_secs` is passed in rather than read here so the packet is a pure function of its inputs
/// (the expiry stamp is the only clock-dependent field and its rule is unit-tested on both sides).
pub fn build_mail_list(mails: &[MailView], now_secs: i64) -> SMSG_MAIL_LIST_RESULT {
    SMSG_MAIL_LIST_RESULT {
        mails: mails
            .iter()
            .map(|m| Mail {
                // The wire's message id is u32; the row id is the auto-inc u64 the client hands
                // back in `CMSG_ITEM_TEXT_QUERY.mail_id`, so both narrow the same way.
                message_id: m.id as u32,
                // Normal = written by a player, whose guid the client resolves by NAME_QUERY.
                // Auction/creature/gameobject senders have no producer and are out of scope.
                message_type: Mail_MailType::Normal {
                    sender: m.sender_guid.into(),
                },
                subject: m.subject.clone(),
                item_text_id: mail_rules::item_text_id_for(m.id, &m.body),
                unknown1: 0,
                // Stationery.dbc is not imported (licensing firewall) — vanilla's default parchment.
                stationery: 41,
                item: m.item_entry,
                item_enchant_id: m.item_enchant_id,
                item_random_property_id: 0,
                item_suffix_factor: 0,
                item_stack_size: m.item_stack_count.min(u8::MAX as u32) as u8,
                item_spell_charges: 0,
                max_durability: m.item_durability,
                durability: m.item_durability,
                money: Gold::new(m.money),
                cash_on_delivery_amount: m.cod,
                // cmangos/vmangos both send a timestamp here despite the field's name; 0 renders
                // as "never checked", which is what an unread mail is.
                checked_timestamp: 0,
                expiration_time: mail_rules::expiration_days(m.created_at_secs, now_secs),
                mail_template_id: 0,
            })
            .collect(),
    }
}

/// Answer the client's periodic "do I have new mail" poll — the minimap envelope.
pub fn build_next_mail_time(has_unread: bool) -> MSG_QUERY_NEXT_MAIL_TIME_Server {
    MSG_QUERY_NEXT_MAIL_TIME_Server {
        unread_mails: mail_rules::unread_mail_signal(has_unread),
    }
}

/// Answer `CMSG_ITEM_TEXT_QUERY` with a letter's body. An unknown or unreadable id answers with an
/// empty text rather than silence: the client opened the letter and waits for this packet.
pub fn build_item_text_response(item_text_id: u32, text: String) -> SMSG_ITEM_TEXT_QUERY_RESPONSE {
    SMSG_ITEM_TEXT_QUERY_RESPONSE { item_text_id, text }
}

/// Answer `CMSG_MAIL_DELETE` with `SMSG_SEND_MAIL_RESULT` — the wire's only ack for this opcode, so
/// both a successful delete AND a refused one (not the caller's mail, or a stale gate) reply through
/// it rather than through a family of specific `MailResultTwo` variants: none of them names "not your
/// mail", and `ErrInternalError` is mangoszero's own generic bucket for exactly that ("Mail database
/// error"). `CMSG_MAIL_MARK_AS_READ` gets no wire reply at all — vanilla sends none, the client
/// already flipped its own display — so there is no sibling builder for it.
pub fn build_mail_delete_result(mail_id: u32, ok: bool) -> SMSG_SEND_MAIL_RESULT {
    SMSG_SEND_MAIL_RESULT {
        mail_id,
        action: SMSG_SEND_MAIL_RESULT_MailAction::Deleted {
            result2: if ok {
                SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok
            } else {
                SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: u64, body: &str) -> MailView {
        MailView {
            id,
            sender_guid: 42,
            subject: "Greetings".into(),
            body: body.into(),
            created_at_secs: 1_000,
            ..Default::default()
        }
    }

    /// The field-mapping decisions worth pinning: the sender rides the NORMAL arm as a guid, the
    /// mail's own id doubles as the `item_text_id`, and an empty body advertises 0 (so the client
    /// never queries a letter that has no text).
    #[test]
    fn a_mail_row_maps_onto_the_wire_with_its_own_id_as_the_text_id() {
        let list = build_mail_list(&[view(7, "meet me at the gate"), view(8, "")], 1_000);
        assert_eq!(list.mails[0].message_id, 7);
        assert_eq!(
            list.mails[0].message_type,
            Mail_MailType::Normal {
                sender: 42u64.into()
            }
        );
        assert_eq!(list.mails[0].item_text_id, 7);
        assert_eq!(
            list.mails[1].item_text_id, 0,
            "an empty body must advertise text id 0 — the client then never queries it"
        );
    }

    /// An empty mailbox is a real packet with zero rows, not a dropped reply.
    #[test]
    fn an_empty_mailbox_still_builds_a_list_packet() {
        assert!(build_mail_list(&[], 0).mails.is_empty());
    }

    /// The expiry stamp is what makes the client's countdown render, and it counts DAYS down from
    /// the row's own age.
    #[test]
    fn the_expiry_stamp_counts_days_down_from_the_rows_age() {
        let list = build_mail_list(&[view(1, "x")], 1_000 + 86_400);
        assert_eq!(
            list.mails[0].expiration_time,
            lyracore_shared::mail::EXPIRY_DAYS - 1.0
        );
    }

    /// The poll's two answers, at the wire.
    #[test]
    fn the_mail_poll_packet_carries_the_shared_unread_signal() {
        assert_eq!(build_next_mail_time(true).unread_mails, 0.0);
        assert!(build_next_mail_time(false).unread_mails < 0.0);
    }

    /// A successful delete answers `Ok`; a refused one answers `ErrInternalError` — the generic
    /// bucket, because none of `MailResultTwo`'s named variants means "not your mail". Both carry
    /// the mail id back so the client's window drops the right row either way.
    #[test]
    fn a_delete_result_carries_the_mail_id_and_the_generic_error_on_refusal() {
        match build_mail_delete_result(7, true).action {
            SMSG_SEND_MAIL_RESULT_MailAction::Deleted { result2 } => {
                assert_eq!(result2, SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok)
            }
            other => panic!("expected the Deleted action, got {other:?}"),
        }
        let refused = build_mail_delete_result(7, false);
        assert_eq!(refused.mail_id, 7);
        match refused.action {
            SMSG_SEND_MAIL_RESULT_MailAction::Deleted { result2 } => assert_eq!(
                result2,
                SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
            ),
            other => panic!("expected the Deleted action, got {other:?}"),
        }
    }
}
