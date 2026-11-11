use lyracore_shared::loot_roll::vote_kind;
use wow_world_messages::vanilla::{
    Gold, LootMethodError, RollVote, SMSG_LOOT_RESPONSE_LootMethod, SMSG_LOOT_ALL_PASSED,
    SMSG_LOOT_MASTER_LIST, SMSG_LOOT_MONEY_NOTIFY, SMSG_LOOT_RELEASE_RESPONSE, SMSG_LOOT_REMOVED,
    SMSG_LOOT_RESPONSE, SMSG_LOOT_ROLL, SMSG_LOOT_ROLL_WON, SMSG_LOOT_START_ROLL,
};
use wow_world_messages::Guid;

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
    use lyracore_shared::loot_roll::vote_kind;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::{
        Gold, RollVote, SMSG_LOOT_RESPONSE_LootMethod, SMSG_LOOT_RESPONSE,
    };
    use wow_world_messages::Guid;

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

    #[test]
    fn loot_response_envelope_matches_gtker() {
        let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
        let (opcode, body) = build_loot_response_raw(guid, 13, &[]);
        let gtker = ServerOpcodeMessage::SMSG_LOOT_RESPONSE(Box::new(SMSG_LOOT_RESPONSE {
            guid: Guid::new(guid),
            loot_method: SMSG_LOOT_RESPONSE_LootMethod::Corpse,
            gold: Gold::new(13),
            items: Vec::new(),
        }));
        let mut framed = Vec::new();
        gtker.write_unencrypted_server(&mut framed).unwrap();
        let gtker_opcode = u16::from_le_bytes([framed[2], framed[3]]);
        assert_eq!(
            opcode, gtker_opcode,
            "raw opcode must match gtker's SMSG_LOOT_RESPONSE"
        );
        assert_eq!(
            body,
            framed[4..],
            "raw envelope must byte-match gtker's money-only loot"
        );
    }

    #[test]
    fn loot_response_item_layout_is_full_vanilla() {
        let guid = 0xF130_0000_0000_0001;
        let (_op, body) = build_loot_response_raw(guid, 5, &[(0, 25, 2, 1542, 117)]);
        assert_eq!(
            body.len(),
            14 + 22,
            "one item must add 22 bytes (full vanilla layout)"
        );
        assert_eq!(body[13], 1, "amount_of_items == 1");
        let it = &body[14..];
        assert_eq!(it[0], 0, "loot slot index");
        assert_eq!(
            u32::from_le_bytes([it[1], it[2], it[3], it[4]]),
            25,
            "item id (entry)"
        );
        assert_eq!(
            u32::from_le_bytes([it[5], it[6], it[7], it[8]]),
            2,
            "stack count"
        );
        assert_eq!(
            u32::from_le_bytes([it[9], it[10], it[11], it[12]]),
            1542,
            "display id"
        );
        assert_eq!(u32::from_le_bytes(it[17..21].try_into().unwrap()), 117);
        assert_eq!(it[21], 0, "slot_type = TypeAllowLoot");
    }

    #[test]
    fn loot_start_roll_carries_the_countdown_and_item() {
        let m = build_loot_start_roll(0xF130_0000_0000_0007, 3, 1234, 60_000, 117);
        assert_eq!(m.creature.guid(), 0xF130_0000_0000_0007);
        assert_eq!(m.loot_slot, 3);
        assert_eq!(m.item, 1234);
        assert_eq!(m.item_random_property_id, 117);
        assert_eq!(m.countdown_time, std::time::Duration::from_millis(60_000));
        let msg = ServerOpcodeMessage::SMSG_LOOT_START_ROLL(Box::new(m));
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn wire_roll_number_signals_pass_as_128_and_passes_a_real_roll_through() {
        assert_eq!(wire_roll_number(vote_kind::PASS, 0), 128);
        assert_eq!(
            wire_roll_number(vote_kind::NEED, 87),
            87,
            "a real roll passes through unchanged"
        );
        assert_eq!(wire_roll_number(vote_kind::GREED, 42), 42);
    }

    #[test]
    fn loot_roll_carries_the_voter_item_and_vote() {
        let m = build_loot_roll(0xF130_0000_0000_0007, 3, 55, 1234, 87, vote_kind::NEED, 117);
        assert_eq!(m.creature.guid(), 0xF130_0000_0000_0007);
        assert_eq!(m.loot_slot, 3);
        assert_eq!(m.player.guid(), 55);
        assert_eq!(m.item, 1234);
        assert_eq!(m.roll_number, 87);
        assert_eq!(m.item_random_property_id, 117);
        assert_eq!(m.vote, RollVote::Need);
        let pass = build_loot_roll(0xF130_0000_0000_0007, 3, 55, 1234, 0, vote_kind::PASS, 117);
        assert_eq!(pass.roll_number, 128);
        assert_eq!(pass.vote, RollVote::Pass);
    }

    #[test]
    fn loot_roll_won_carries_the_winner_roll_and_winning_tier() {
        let m = build_loot_roll_won(0xF130_0000_0000_0007, 3, 1234, 55, 96, vote_kind::NEED, 0);
        assert_eq!(m.looted_target.guid(), 0xF130_0000_0000_0007);
        assert_eq!(m.loot_slot, 3);
        assert_eq!(m.item, 1234);
        assert_eq!(m.winning_player.guid(), 55);
        assert_eq!(m.winning_roll, 96);
        assert_eq!(m.vote, RollVote::Need);
        let g = build_loot_roll_won(0xF130_0000_0000_0007, 3, 1234, 55, 42, vote_kind::GREED, 0);
        assert_eq!(g.vote, RollVote::Greed);
    }

    #[test]
    fn loot_master_list_carries_every_eligible_guid() {
        let m = build_loot_master_list(&[10, 20, 30]);
        assert_eq!(m.guids.len(), 3);
        assert_eq!(m.guids[0].guid(), 10);
        assert_eq!(m.guids[1].guid(), 20);
        assert_eq!(m.guids[2].guid(), 30);
        let msg = ServerOpcodeMessage::SMSG_LOOT_MASTER_LIST(Box::new(m));
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn loot_money_notify_carries_the_share_not_a_hardcoded_total() {
        let share = lyracore_shared::loot_roll::decode_money_share(
            &lyracore_shared::loot_roll::encode_money_share(17),
        )
        .unwrap();
        let m = build_loot_money_notify(share);
        assert_eq!(m.amount, 17);
        let msg = ServerOpcodeMessage::SMSG_LOOT_MONEY_NOTIFY(m);
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }
}
