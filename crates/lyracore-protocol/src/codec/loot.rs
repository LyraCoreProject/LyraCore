use super::*;
use lyracore_shared::loot_roll::vote_kind;
use wow_world_messages::vanilla::{
    LootMethodError, SMSG_LOOT_RESPONSE_LootMethod, SMSG_LOOT_ALL_PASSED, SMSG_LOOT_RESPONSE,
};

const SMSG_LOOT_RESPONSE_OPCODE: u16 = 0x0160;

pub type LootItemView = (u8, u32, u32, u32, u32);

/// gtker emits 6 bytes per loot item; build 5875 reads 22, including four extra u32 fields.
pub fn build_loot_response_raw(guid: u64, money: u32, items: &[LootItemView]) -> (u16, Vec<u8>) {
    let items = &items[..items.len().min(255)];
    let mut body = Vec::with_capacity(14 + items.len() * 22);
    body.extend_from_slice(&guid.to_le_bytes()); // Full guid, not packed.
    body.push(1u8); // LootMethod::Corpse
    body.extend_from_slice(&money.to_le_bytes());
    body.push(items.len() as u8);
    for &(slot, item_id, count, display_id, random_property_id) in items {
        body.push(slot);
        body.extend_from_slice(&item_id.to_le_bytes());
        body.extend_from_slice(&count.to_le_bytes());
        body.extend_from_slice(&display_id.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes()); // No random suffix.
        body.extend_from_slice(&random_property_id.to_le_bytes());
        body.push(0u8); // TypeAllowLoot
    }
    (SMSG_LOOT_RESPONSE_OPCODE, body)
}

pub fn build_loot_error_response(guid: u64, loot_error: LootMethodError) -> SMSG_LOOT_RESPONSE {
    SMSG_LOOT_RESPONSE {
        guid: Guid::new(guid),
        loot_method: SMSG_LOOT_RESPONSE_LootMethod::ErrorX { loot_error },
        gold: Gold::new(0),
        items: Vec::new(),
    }
}

pub fn build_loot_removed(slot: u8) -> SMSG_LOOT_REMOVED {
    SMSG_LOOT_REMOVED { slot }
}

pub fn build_loot_money_notify(amount: u32) -> SMSG_LOOT_MONEY_NOTIFY {
    SMSG_LOOT_MONEY_NOTIFY { amount }
}

pub fn build_loot_release_response(guid: u64) -> SMSG_LOOT_RELEASE_RESPONSE {
    SMSG_LOOT_RELEASE_RESPONSE {
        guid: Guid::new(guid),
        unknown1: 1,
    }
}

fn wire_vote(vote: u8) -> RollVote {
    RollVote::try_from(vote).unwrap_or(RollVote::Pass)
}

pub fn build_loot_start_roll(
    corpse_guid: u64,
    loot_slot: u8,
    item_entry: u32,
    countdown_ms: u32,
    random_property_id: u32,
) -> SMSG_LOOT_START_ROLL {
    SMSG_LOOT_START_ROLL {
        creature: Guid::new(corpse_guid),
        loot_slot: loot_slot as u32,
        item: item_entry,
        item_random_suffix: 0,
        item_random_property_id: random_property_id,
        countdown_time: std::time::Duration::from_millis(countdown_ms as u64),
    }
}

pub fn wire_roll_number(vote: u8, rolled: u8) -> u8 {
    if vote == vote_kind::PASS {
        128
    } else {
        rolled
    }
}

pub fn build_loot_roll(
    corpse_guid: u64,
    loot_slot: u8,
    voter_guid: u64,
    item_entry: u32,
    rolled: u8,
    vote: u8,
    random_property_id: u32,
) -> SMSG_LOOT_ROLL {
    SMSG_LOOT_ROLL {
        creature: Guid::new(corpse_guid),
        loot_slot: loot_slot as u32,
        player: Guid::new(voter_guid),
        item: item_entry,
        item_random_suffix: 0,
        item_random_property_id: random_property_id,
        roll_number: wire_roll_number(vote, rolled),
        vote: wire_vote(vote),
    }
}

pub fn build_loot_roll_won(
    corpse_guid: u64,
    loot_slot: u8,
    item_entry: u32,
    winning_player: u64,
    winning_roll: u8,
    winning_vote: u8,
    random_property_id: u32,
) -> SMSG_LOOT_ROLL_WON {
    SMSG_LOOT_ROLL_WON {
        looted_target: Guid::new(corpse_guid),
        loot_slot: loot_slot as u32,
        item: item_entry,
        item_random_suffix: 0,
        item_random_property_id: random_property_id,
        winning_player: Guid::new(winning_player),
        winning_roll,
        vote: wire_vote(winning_vote),
    }
}

pub fn build_loot_master_list(eligible: &[u64]) -> SMSG_LOOT_MASTER_LIST {
    SMSG_LOOT_MASTER_LIST {
        guids: eligible.iter().map(|&g| Guid::new(g)).collect(),
    }
}

pub fn build_loot_all_passed(
    corpse_guid: u64,
    loot_slot: u8,
    item_entry: u32,
    random_property_id: u32,
) -> SMSG_LOOT_ALL_PASSED {
    SMSG_LOOT_ALL_PASSED {
        looted_target: Guid::new(corpse_guid),
        loot_slot: loot_slot as u32,
        item: item_entry,
        item_random_property_id: random_property_id,
        item_random_suffix_id: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_passed_is_the_corpse_slot_item_property_and_suffix_under_its_header() {
        let reply = build_loot_all_passed(0xF130_0000_0000_0007, 3, 1234, 117);
        let mut wire = Vec::new();
        ServerOpcodeMessage::SMSG_LOOT_ALL_PASSED(Box::new(reply))
            .write_unencrypted_server(&mut wire)
            .unwrap();
        assert_eq!(
            wire,
            [
                0x00, 0x1A, 0x9E, 0x02, // size 26 (big endian), opcode 0x029E
                0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, // corpse guid
                0x03, 0x00, 0x00, 0x00, // loot slot
                0xD2, 0x04, 0x00, 0x00, // item entry 1234
                0x75, 0x00, 0x00, 0x00, // random property id 117
                0x00, 0x00, 0x00, 0x00, // random suffix id
            ]
        );
    }
}
