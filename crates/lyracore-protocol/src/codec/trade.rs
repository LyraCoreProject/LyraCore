use lyracore_shared::item_property::client_enchantment_id;
use wow_world_messages::vanilla::{opcodes::ServerOpcodeMessage, SMSG_TRADE_STATUS};

pub fn build_trade_event(kind: u8, other_guid: u64, payload: &str) -> Option<ServerOpcodeMessage> {
    use lyracore_shared::trade::event_kind as kind;
    if kind == kind::OFFER_SELF || kind == kind::OFFER_PARTNER {
        // The misnamed self_player byte is 1 for the partner's offer and 0 for one's own.
        return trade_offer_extended(kind == kind::OFFER_PARTNER, payload)
            .map(|offer| ServerOpcodeMessage::SMSG_TRADE_STATUS_EXTENDED(Box::new(offer)));
    }
    let status = match kind {
        kind::BEGIN_TRADE => Some(SMSG_TRADE_STATUS::BeginTrade {
            unknown1: wow_world_messages::Guid::new(other_guid),
        }),
        kind::OPEN_WINDOW => Some(SMSG_TRADE_STATUS::OpenWindow),
        kind::TRADE_CANCELED => Some(SMSG_TRADE_STATUS::TradeCanceled),
        kind::BUSY => Some(SMSG_TRADE_STATUS::Busy),
        kind::NO_TARGET => Some(SMSG_TRADE_STATUS::NoTarget),
        kind::TARGET_TO_FAR => Some(SMSG_TRADE_STATUS::TargetToFar),
        kind::WRONG_FACTION => Some(SMSG_TRADE_STATUS::WrongFaction),
        kind::YOU_DEAD => Some(SMSG_TRADE_STATUS::YouDead),
        kind::TARGET_DEAD => Some(SMSG_TRADE_STATUS::TargetDead),
        kind::IGNORE_YOU => Some(SMSG_TRADE_STATUS::IgnoreYou),
        kind::TRADE_ACCEPT => Some(SMSG_TRADE_STATUS::TradeAccept),
        kind::BACK_TO_TRADE => Some(SMSG_TRADE_STATUS::BackToTrade),
        kind::TRADE_COMPLETE => Some(SMSG_TRADE_STATUS::TradeComplete),
        kind::INV_FULL_SELF => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::InventoryFull,
            item_limit_category_id: 0,
            target_error: false,
        }),
        kind::INV_FULL_PARTNER => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::InventoryFull,
            item_limit_category_id: 0,
            target_error: true,
        }),
        kind::GOLD_FAIL_SELF => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::NotEnoughMoney,
            item_limit_category_id: 0,
            target_error: false,
        }),
        kind::GOLD_FAIL_PARTNER => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::NotEnoughMoney,
            item_limit_category_id: 0,
            target_error: true,
        }),
        _ => None,
    }?;
    Some(ServerOpcodeMessage::SMSG_TRADE_STATUS(Box::new(status)))
}

fn trade_offer_extended(
    describes_partner: bool,
    payload: &str,
) -> Option<wow_world_messages::vanilla::SMSG_TRADE_STATUS_EXTENDED> {
    use wow_world_messages::vanilla::{TradeSlot, SMSG_TRADE_STATUS_EXTENDED};
    let (gold, views) = lyracore_shared::trade::decode_offer(payload)?;
    let mut trade_slots = [TradeSlot::default(); 7];
    for v in views {
        let slot = trade_slots.get_mut(v.trade_slot as usize)?;
        *slot = TradeSlot {
            trade_slot_number: v.trade_slot,
            item: v.entry,
            display_id: v.display_id,
            stack_count: v.stack_count,
            enchantment: client_enchantment_id(v.enchantment),
            item_random_properties_id: v.random_property_id,
            max_durability: v.max_durability,
            durability: v.durability,
            ..TradeSlot::default()
        };
    }
    Some(SMSG_TRADE_STATUS_EXTENDED {
        self_player: describes_partner,
        trade_slot_count1: 7,
        trade_slot_count2: 7,
        money_in_trade: wow_world_messages::vanilla::Gold::new(gold),
        spell_on_lowest_slot: 0,
        trade_slots,
    })
}
