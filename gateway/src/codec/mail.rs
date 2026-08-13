//! Vanilla mailbox packet mapping.

use lyracore_shared::mail as mail_rules;
use wow_world_messages::vanilla::{
    Gold, InventoryResult, MSG_QUERY_NEXT_MAIL_TIME_Server, Mail, Mail_MailType,
    SMSG_SEND_MAIL_RESULT_MailAction, SMSG_SEND_MAIL_RESULT_MailResult,
    SMSG_SEND_MAIL_RESULT_MailResultTwo, SMSG_ITEM_TEXT_QUERY_RESPONSE, SMSG_MAIL_LIST_RESULT,
    SMSG_SEND_MAIL_RESULT,
};
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailView {
    pub id: u64,
    pub sender_guid: u64,
    pub subject: String,
    pub body: String,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub max_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub money: u32,
    pub cod: u32,
    pub was_read: bool,
    pub created_at_secs: i64,
}
pub fn build_mail_list(mails: &[MailView], now_secs: i64) -> SMSG_MAIL_LIST_RESULT {
    SMSG_MAIL_LIST_RESULT {
        mails: mails
            .iter()
            .map(|m| Mail {
                message_id: m.id as u32,
                message_type: Mail_MailType::Normal {
                    sender: m.sender_guid.into(),
                },
                subject: m.subject.clone(),
                item_text_id: mail_rules::item_text_id_for(m.id, &m.body),
                unknown1: 0,
                stationery: 41,
                item: m.item_entry,
                item_enchant_id: m.item_enchant_id,
                item_random_property_id: 0,
                item_suffix_factor: 0,
                item_stack_size: m.item_stack_count.min(u8::MAX as u32) as u8,
                item_spell_charges: 0,
                max_durability: m.max_durability,
                durability: m.item_durability,
                money: Gold::new(m.money),
                cash_on_delivery_amount: m.cod,
                checked_timestamp: 0,
                expiration_time: mail_rules::expiration_days(m.created_at_secs, now_secs),
                mail_template_id: 0,
            })
            .collect(),
    }
}
pub fn build_next_mail_time(has_unread: bool) -> MSG_QUERY_NEXT_MAIL_TIME_Server {
    MSG_QUERY_NEXT_MAIL_TIME_Server {
        unread_mails: mail_rules::unread_mail_signal(has_unread),
    }
}
pub fn build_item_text_response(item_text_id: u32, text: String) -> SMSG_ITEM_TEXT_QUERY_RESPONSE {
    SMSG_ITEM_TEXT_QUERY_RESPONSE { item_text_id, text }
}
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
pub fn build_mail_return_result(mail_id: u32, ok: bool) -> SMSG_SEND_MAIL_RESULT {
    SMSG_SEND_MAIL_RESULT {
        mail_id,
        action: SMSG_SEND_MAIL_RESULT_MailAction::ReturnedToSender {
            result2: if ok {
                SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok
            } else {
                SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
            },
        },
    }
}
pub fn build_mail_take_money_result(mail_id: u32, ok: bool) -> SMSG_SEND_MAIL_RESULT {
    SMSG_SEND_MAIL_RESULT {
        mail_id,
        action: SMSG_SEND_MAIL_RESULT_MailAction::MoneyTaken {
            result2: if ok {
                SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok
            } else {
                SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
            },
        },
    }
}
pub fn build_mail_take_item_result(
    mail_id: u32,
    taken: Result<(u32, u32), MailTakeItemError>,
) -> SMSG_SEND_MAIL_RESULT {
    SMSG_SEND_MAIL_RESULT {
        mail_id,
        action: SMSG_SEND_MAIL_RESULT_MailAction::ItemTaken {
            result: match taken {
                Ok((item, item_count)) => SMSG_SEND_MAIL_RESULT_MailResult::Ok { item, item_count },
                Err(MailTakeItemError::BagsFull) => {
                    SMSG_SEND_MAIL_RESULT_MailResult::ErrEquipError {
                        equip_error: u32::from(InventoryResult::InventoryFull.as_int()),
                    }
                }
                Err(MailTakeItemError::NotEnoughMoney) => {
                    SMSG_SEND_MAIL_RESULT_MailResult::ErrNotEnoughMoney {
                        item: 0,
                        item_count: 0,
                    }
                }
                Err(MailTakeItemError::Other) => {
                    SMSG_SEND_MAIL_RESULT_MailResult::ErrInternalError {
                        item: 0,
                        item_count: 0,
                    }
                }
            },
        },
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MailTakeItemError {
    BagsFull,
    NotEnoughMoney,
    Other,
}
pub fn build_mail_send_result(
    result2: SMSG_SEND_MAIL_RESULT_MailResultTwo,
) -> SMSG_SEND_MAIL_RESULT {
    SMSG_SEND_MAIL_RESULT {
        mail_id: 0,
        action: SMSG_SEND_MAIL_RESULT_MailAction::Send { result2 },
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

    #[test]
    fn an_empty_mailbox_still_builds_a_list_packet() {
        assert!(build_mail_list(&[], 0).mails.is_empty());
    }

    #[test]
    fn the_expiry_stamp_counts_days_down_from_the_rows_age() {
        let list = build_mail_list(&[view(1, "x")], 1_000 + 86_400);
        assert_eq!(
            list.mails[0].expiration_time,
            lyracore_shared::mail::EXPIRY_DAYS - 1.0
        );
    }

    #[test]
    fn the_mail_poll_packet_carries_the_shared_unread_signal() {
        assert_eq!(build_next_mail_time(true).unread_mails, 0.0);
        assert!(build_next_mail_time(false).unread_mails < 0.0);
    }

    #[test]
    fn a_send_result_carries_the_gates_own_verdict_on_the_send_action() {
        for want in [
            SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok,
            SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrRecipientNotFound,
            SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrCannotSendToSelf,
            SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrNotYourTeam,
            SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrNotEnoughMoney,
        ] {
            let packet = build_mail_send_result(want);
            assert_eq!(packet.mail_id, 0, "a send names no existing mail");
            match packet.action {
                SMSG_SEND_MAIL_RESULT_MailAction::Send { result2 } => assert_eq!(result2, want),
                other => panic!("expected the Send action, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_take_money_result_answers_on_the_money_taken_action() {
        match build_mail_take_money_result(7, true).action {
            SMSG_SEND_MAIL_RESULT_MailAction::MoneyTaken { result2 } => {
                assert_eq!(result2, SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok)
            }
            other => panic!("expected the MoneyTaken action, got {other:?}"),
        }
        let refused = build_mail_take_money_result(7, false);
        assert_eq!(refused.mail_id, 7);
        match refused.action {
            SMSG_SEND_MAIL_RESULT_MailAction::MoneyTaken { result2 } => assert_eq!(
                result2,
                SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
            ),
            other => panic!("expected the MoneyTaken action, got {other:?}"),
        }
    }

    #[test]
    fn the_list_packet_carries_a_mails_attached_copper() {
        let mut m = view(1, "here you go");
        m.money = 130;
        assert_eq!(build_mail_list(&[m], 1_000).mails[0].money, Gold::new(130));
    }

    #[test]
    fn the_list_packet_carries_a_mails_cash_on_delivery_price() {
        let mut m = view(1, "yours for 250");
        m.item_entry = 5_090_001;
        m.cod = 250;
        assert_eq!(
            build_mail_list(&[m], 1_000).mails[0].cash_on_delivery_amount,
            250
        );
    }

    #[test]
    fn the_list_packet_carries_a_mails_attached_item() {
        let mut m = view(1, "here you go");
        m.item_entry = 5_090_001;
        m.item_stack_count = 12;
        m.item_durability = 42;
        m.item_enchant_id = 7;
        let wire = &build_mail_list(&[m], 1_000).mails[0];
        assert_eq!(wire.item, 5_090_001);
        assert_eq!(wire.item_stack_size, 12);
        assert_eq!(wire.durability, 42);
        assert_eq!(
            wire.item_enchant_id, 7,
            "an enchant dropped here is an enchant the recipient cannot see they are owed"
        );
    }

    #[test]
    fn a_damaged_attachment_previews_with_its_templates_max_durability_not_its_current_one() {
        let mut m = view(1, "well-used");
        m.item_entry = 5_090_001;
        m.item_durability = 10;
        m.max_durability = 40;
        let wire = &build_mail_list(&[m], 1_000).mails[0];
        assert_eq!(
            wire.durability, 10,
            "the current durability rides unchanged"
        );
        assert_eq!(
            wire.max_durability, 40,
            "max_durability must be the TEMPLATE's, not a copy of the current value"
        );
    }

    #[test]
    fn a_take_item_result_names_what_the_client_just_gained() {
        match build_mail_take_item_result(7, Ok((5_090_001, 12))).action {
            SMSG_SEND_MAIL_RESULT_MailAction::ItemTaken { result } => assert_eq!(
                result,
                SMSG_SEND_MAIL_RESULT_MailResult::Ok {
                    item: 5_090_001,
                    item_count: 12
                }
            ),
            other => panic!("expected the ItemTaken action, got {other:?}"),
        }
    }

    #[test]
    fn a_full_bag_answers_the_equip_error_variant_and_everything_else_the_generic_one() {
        match build_mail_take_item_result(7, Err(MailTakeItemError::BagsFull)).action {
            SMSG_SEND_MAIL_RESULT_MailAction::ItemTaken { result } => assert_eq!(
                result,
                SMSG_SEND_MAIL_RESULT_MailResult::ErrEquipError {
                    equip_error: u32::from(InventoryResult::InventoryFull.as_int())
                }
            ),
            other => panic!("expected the ItemTaken action, got {other:?}"),
        }
        match build_mail_take_item_result(7, Err(MailTakeItemError::NotEnoughMoney)).action {
            SMSG_SEND_MAIL_RESULT_MailAction::ItemTaken { result } => assert_eq!(
                result,
                SMSG_SEND_MAIL_RESULT_MailResult::ErrNotEnoughMoney {
                    item: 0,
                    item_count: 0
                },
                "a COD price the taker cannot pay must read as 'bring gold', not as a broken \
                 mailbox"
            ),
            other => panic!("expected the ItemTaken action, got {other:?}"),
        }
        match build_mail_take_item_result(7, Err(MailTakeItemError::Other)).action {
            SMSG_SEND_MAIL_RESULT_MailAction::ItemTaken { result } => assert_eq!(
                result,
                SMSG_SEND_MAIL_RESULT_MailResult::ErrInternalError {
                    item: 0,
                    item_count: 0
                }
            ),
            other => panic!("expected the ItemTaken action, got {other:?}"),
        }
    }

    #[test]
    fn a_return_result_carries_the_mail_id_and_the_generic_error_on_refusal() {
        match build_mail_return_result(7, true).action {
            SMSG_SEND_MAIL_RESULT_MailAction::ReturnedToSender { result2 } => {
                assert_eq!(result2, SMSG_SEND_MAIL_RESULT_MailResultTwo::Ok)
            }
            other => panic!("expected the ReturnedToSender action, got {other:?}"),
        }
        let refused = build_mail_return_result(7, false);
        assert_eq!(refused.mail_id, 7);
        match refused.action {
            SMSG_SEND_MAIL_RESULT_MailAction::ReturnedToSender { result2 } => assert_eq!(
                result2,
                SMSG_SEND_MAIL_RESULT_MailResultTwo::ErrInternalError
            ),
            other => panic!("expected the ReturnedToSender action, got {other:?}"),
        }
    }

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
