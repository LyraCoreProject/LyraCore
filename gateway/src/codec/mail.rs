//! Vanilla mailbox packet mapping.

use lyracore_shared::mail::{self as mail_rules, MailSender};
use wow_world_messages::vanilla::{
    Gold, InventoryResult, MSG_QUERY_NEXT_MAIL_TIME_Server, Mail, Mail_MailType,
    SMSG_SEND_MAIL_RESULT_MailAction, SMSG_SEND_MAIL_RESULT_MailResult,
    SMSG_SEND_MAIL_RESULT_MailResultTwo, SMSG_ITEM_TEXT_QUERY_RESPONSE, SMSG_MAIL_LIST_RESULT,
    SMSG_RECEIVED_MAIL, SMSG_SEND_MAIL_RESULT,
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
    pub random_property_id: u32,
    pub sender_kind: u8,
    pub sender_entry: u32,
    /// `CHECK_MASK_*` bits other than READ, which is `was_read`.
    pub check_flags: u32,
    pub mail_template_id: u32,
    /// When the recipient can first see the mail, rounded up to the second. 0 means from creation.
    pub deliver_secs: i64,
}

impl MailView {
    pub fn sender(&self) -> MailSender {
        MailSender::from_columns(self.sender_kind, self.sender_guid, self.sender_entry)
    }
    pub fn is_delivered(&self, now_secs: i64) -> bool {
        self.deliver_secs <= now_secs
    }
}
/// The inbox the client shows: newest first, at most [`mail_rules::INBOX_CAPACITY`] mails
/// (cmangos `Player.cpp:15062`, `MailHandler.cpp:557`). A returned or delayed mail counts as new
/// from the moment it arrives.
pub fn build_mail_list(mails: &[MailView], now_secs: i64) -> SMSG_MAIL_LIST_RESULT {
    let mut newest_first: Vec<&MailView> = mails.iter().collect();
    newest_first.sort_by_key(|m| {
        std::cmp::Reverse((
            mail_rules::arrived_at_secs(m.created_at_secs, m.deliver_secs),
            m.id,
        ))
    });
    SMSG_MAIL_LIST_RESULT {
        mails: newest_first
            .into_iter()
            .take(mail_rules::INBOX_CAPACITY)
            .map(|m| Mail {
                message_id: m.id as u32,
                message_type: message_type(m.sender()),
                subject: m.subject.clone(),
                item_text_id: mail_rules::item_text_id_for(m.id, &m.body),
                unknown1: 0,
                stationery: m.sender().stationery(),
                item: m.item_entry,
                item_enchant_id: m.item_enchant_id,
                item_random_property_id: m.random_property_id,
                item_suffix_factor: 0,
                item_stack_size: m.item_stack_count.min(u8::MAX as u32) as u8,
                item_spell_charges: 0,
                max_durability: m.max_durability,
                durability: m.item_durability,
                money: Gold::new(m.money),
                cash_on_delivery_amount: m.cod,
                checked_timestamp: check_mask(m),
                expiration_time: mail_rules::expiration_days(
                    m.created_at_secs,
                    m.deliver_secs,
                    m.cod,
                    now_secs,
                ),
                mail_template_id: m.mail_template_id,
            })
            .collect(),
    }
}
/// cmangos writes the sender guid for a Character and the entry or house id for the rest
/// (`MailHandler.cpp:574-586`).
fn message_type(sender: MailSender) -> Mail_MailType {
    match sender {
        MailSender::Character(guid) => Mail_MailType::Normal {
            sender: guid.into(),
        },
        MailSender::AuctionHouse(house) => Mail_MailType::Auction { auction_id: house },
        MailSender::Creature(entry) => Mail_MailType::Creature { sender_id: entry },
        MailSender::Gameobject(entry) => Mail_MailType::Gameobject { sender_id: entry },
    }
}
/// The `checked` field the client reads read, returned, copied, COD-payment and has-body state
/// from (cmangos `MailHandler.cpp:610`).
fn check_mask(m: &MailView) -> u32 {
    let read = if m.was_read {
        mail_rules::CHECK_MASK_READ
    } else {
        0
    };
    m.check_flags | read
}
pub fn build_next_mail_time(has_unread: bool) -> MSG_QUERY_NEXT_MAIL_TIME_Server {
    MSG_QUERY_NEXT_MAIL_TIME_Server {
        unread_mails: mail_rules::unread_mail_signal(has_unread),
    }
}
/// A Mail Arrival. The packet carries one `u32` 0 (cmangos `Player.cpp:3071-3077`).
pub fn build_received_mail() -> SMSG_RECEIVED_MAIL {
    SMSG_RECEIVED_MAIL { unknown1: 0 }
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
        let with_body = &build_mail_list(&[view(7, "meet me at the gate")], 1_000).mails[0];
        assert_eq!(with_body.message_id, 7);
        assert_eq!(
            with_body.message_type,
            Mail_MailType::Normal {
                sender: 42u64.into()
            }
        );
        assert_eq!(with_body.item_text_id, 7);
        assert_eq!(
            build_mail_list(&[view(8, "")], 1_000).mails[0].item_text_id,
            0,
            "an empty body must advertise text id 0 — the client then never queries it"
        );
    }

    fn from(sender: MailSender) -> MailView {
        let (sender_kind, sender_guid, sender_entry) = sender.columns();
        MailView {
            sender_kind,
            sender_guid,
            sender_entry,
            ..view(1, "")
        }
    }

    #[test]
    fn each_sender_lists_under_its_vanilla_message_type_and_stationery() {
        let wire = |sender| {
            let m = &build_mail_list(&[from(sender)], 1_000).mails[0];
            (m.message_type, m.stationery)
        };
        assert_eq!(
            wire(MailSender::Character(42)),
            (
                Mail_MailType::Normal {
                    sender: 42u64.into()
                },
                41
            )
        );
        assert_eq!(
            wire(MailSender::AuctionHouse(7)),
            (Mail_MailType::Auction { auction_id: 7 }, 62)
        );
        assert_eq!(
            wire(MailSender::Creature(11_811)),
            (Mail_MailType::Creature { sender_id: 11_811 }, 41)
        );
        assert_eq!(
            wire(MailSender::Gameobject(176_582)),
            (Mail_MailType::Gameobject { sender_id: 176_582 }, 41)
        );
    }

    #[test]
    fn the_check_mask_is_the_stored_bits_plus_read() {
        let checked = |check_flags, was_read| {
            build_mail_list(
                &[MailView {
                    check_flags,
                    was_read,
                    ..view(1, "")
                }],
                1_000,
            )
            .mails[0]
                .checked_timestamp
        };
        assert_eq!(checked(0, false), 0, "a legacy unread row");
        assert_eq!(checked(0, true), 0x01, "a legacy read row now shows READ");
        assert_eq!(checked(0x10, true), 0x11, "HAS_BODY and READ");
        assert_eq!(checked(0x02, false), 0x02, "RETURNED");
        assert_eq!(checked(0x08, false), 0x08, "COD_PAYMENT");
    }

    #[test]
    fn the_template_id_rides_the_list() {
        let m = MailView {
            mail_template_id: 150,
            ..view(1, "")
        };
        assert_eq!(build_mail_list(&[m], 1_000).mails[0].mail_template_id, 150);
    }

    fn ids(list: &SMSG_MAIL_LIST_RESULT) -> Vec<u32> {
        list.mails.iter().map(|m| m.message_id).collect()
    }

    #[test]
    fn the_list_is_newest_first() {
        let at = |id, created_at_secs| MailView {
            created_at_secs,
            ..view(id, "")
        };
        assert_eq!(
            ids(&build_mail_list(
                &[at(1, 1_000), at(2, 3_000), at(3, 2_000)],
                5_000
            )),
            vec![2, 3, 1]
        );
        assert_eq!(
            ids(&build_mail_list(&[at(1, 1_000), at(2, 1_000)], 5_000)),
            vec![2, 1],
            "two mails from the same second list by id, newest first"
        );
    }

    #[test]
    fn a_mail_that_arrived_after_it_was_written_lists_from_its_arrival() {
        let returned = MailView {
            created_at_secs: 1_000,
            deliver_secs: 4_000,
            ..view(1, "")
        };
        let newer = MailView {
            created_at_secs: 3_000,
            ..view(2, "")
        };
        assert_eq!(ids(&build_mail_list(&[newer, returned], 5_000)), vec![1, 2]);
    }

    #[test]
    fn the_list_holds_the_fifty_newest_mails() {
        let mails: Vec<MailView> = (1..=60)
            .map(|id| MailView {
                created_at_secs: 1_000 + id as i64,
                ..view(id, "")
            })
            .collect();
        let listed = ids(&build_mail_list(&mails, 5_000));
        assert_eq!(listed.len(), 50);
        assert_eq!(listed.first(), Some(&60));
        assert_eq!(listed.last(), Some(&11));
    }

    #[test]
    fn a_priced_mail_counts_down_three_days_from_its_arrival() {
        let priced = MailView {
            cod: 250,
            item_entry: 5_090_001,
            created_at_secs: 1_000,
            deliver_secs: 1_000 + 86_400,
            ..view(1, "")
        };
        assert_eq!(
            build_mail_list(&[priced], 1_000 + 86_400).mails[0].expiration_time,
            3.0
        );
    }

    #[test]
    fn an_empty_mailbox_still_builds_a_list_packet() {
        assert!(build_mail_list(&[], 0).mails.is_empty());
    }

    #[test]
    fn the_expiry_stamp_counts_days_down_from_the_rows_age() {
        let list = build_mail_list(&[view(1, "x")], 1_000 + 86_400);
        assert_eq!(list.mails[0].expiration_time, 29.0);
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

    #[test]
    fn a_mail_arrival_is_opcode_0x0285_with_one_zero_u32() {
        use wow_world_messages::Message;
        let mut body = Vec::new();
        build_received_mail()
            .write_into_vec(&mut body)
            .expect("a four-byte body writes");
        assert_eq!(SMSG_RECEIVED_MAIL::OPCODE, 0x0285);
        assert_eq!(body, [0, 0, 0, 0], "cmangos Player.cpp:3074-3075");
    }
}
