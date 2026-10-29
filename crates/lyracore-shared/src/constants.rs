// Build-5875 object_type_id byte in update blocks.
pub mod type_id {
    pub const OBJECT: u8 = 0;
    pub const ITEM: u8 = 1;
    pub const CONTAINER: u8 = 2;
    pub const UNIT: u8 = 3;
    pub const PLAYER: u8 = 4;
    pub const GAMEOBJECT: u8 = 5;
    pub const DYNAMICOBJECT: u8 = 6;
    pub const CORPSE: u8 = 7;
}

// OBJECT_FIELD_TYPE contains 1 << TypeId for every inherited type.
pub mod type_mask {
    use super::type_id;
    pub const OBJECT: u32 = 1 << type_id::OBJECT;
    pub const UNIT: u32 = 1 << type_id::UNIT;
    pub const PLAYER_BIT: u32 = 1 << type_id::PLAYER;
    pub const PLAYER: u32 = OBJECT | UNIT | PLAYER_BIT;
    pub const CREATURE: u32 = OBJECT | UNIT;
}

pub mod unit_dynamic_flags {
    pub const LOOTABLE: u32 = 0x0001;
    pub const TAPPED: u32 = 0x0004;
    // Viewer-relative bit; never store it on game_world_entity.
    pub const TAPPED_BY_PLAYER: u32 = 0x0008;
    // This bit means feign death; real corpses use UNIT_FIELD_HEALTH = 0.
    pub const DEAD: u32 = 0x0020;
}

pub mod player_flags {
    pub const AFK: u32 = 0x0002;
    pub const DND: u32 = 0x0004;

    pub const GHOST: u32 = 0x0010;
}

pub mod npc_flags {
    pub const GOSSIP: u32 = 0x0000_0001;
    // Vanilla VENDOR is 0x4; later expansions use a different numbering.
    pub const VENDOR: u32 = 0x0000_0004;
    pub const TAXI: u32 = 0x0000_0008;
    pub const TRAINER: u32 = 0x0000_0010;
    pub const SPIRITHEALER: u32 = 0x0000_0020;
    // Vanilla INNKEEPER is 0x80.
    pub const INNKEEPER: u32 = 0x0000_0080;
    pub const BANKER: u32 = 0x0000_0100;
    pub const PETITIONER: u32 = 0x0000_0200;
    pub const TABARDDESIGNER: u32 = 0x0000_0400;
    pub const AUCTIONEER: u32 = 0x0000_1000;
    // Vanilla REPAIR is 0x4000; 0x1000 is AUCTIONEER.
    pub const REPAIR: u32 = 0x0000_4000;
}

pub mod go_type {
    pub const QUESTGIVER: u8 = 2;
    pub const CHEST: u8 = 3;
    pub const MO_TRANSPORT: u8 = 15;
    // Meeting Stones send CMSG_MEETINGSTONE_JOIN, not CMSG_GAMEOBJ_USE.
    pub const MEETINGSTONE: u8 = 23;
}

// Build 5875 stores visibility flags in byte 3 of UNIT_FIELD_BYTES_1.
pub mod unit_vis_flags {
    pub const GHOST: u32 = 0x0100_0000;
}

// UNIT_FIELD_BYTES_2 byte 0 is sheath state; item_template.sheath is the stow location.
pub mod sheath_state {
    pub const UNARMED: u8 = 0;
    pub const MELEE: u8 = 1;
    pub const RANGED: u8 = 2;

    pub fn is_valid(state: u8) -> bool {
        matches!(state, UNARMED | MELEE | RANGED)
    }

    pub fn packed_with(unit_bytes_2: u32, state: u8) -> u32 {
        (unit_bytes_2 & 0xFFFF_FF00) | state as u32
    }
}

pub mod unit_flags {
    pub const REMOVE_CLIENT_CONTROL: u32 = 0x0000_0004;
    pub const PLAYER_CONTROLLED: u32 = 0x0000_0008;
    pub const NOT_SELECTABLE: u32 = 0x0200_0000;
    pub const IN_COMBAT: u32 = 0x0008_0000;
    pub const TAXI_FLIGHT: u32 = 0x0010_0000;
}

pub mod tracer_spell {
    pub const SPELL_ID: u32 = 6673;
    pub const AURA_SLOT: u8 = 0;
    pub const AURA_SLOTS: u8 = 48; // Total UNIT_FIELD_AURA slots.
                                   // Slots 0..32 render buffs; slots 32..48 render debuffs.
    pub const BUFF_SLOT_COUNT: u8 = 32;
    pub const AURA_FLAGS: u8 = 0x09;
    pub const AURA_DURATION_MICROS: i64 = 30_000_000;
}

pub mod starter_item {
    pub const ENTRY: u32 = 25;
    // HIGHGUID_ITEM occupies bits 48..63 of an item GUID.
    pub const HIGHGUID_ITEM: u64 = 0x4000;
    // Vanilla inventory slots: equipment 0..18, bags 19..22, backpack 23..38.
    pub const BACKPACK_SLOT_0: u8 = 23;
    pub const MAINHAND_SLOT: u8 = 15;
    pub const CLASS_WEAPON: u8 = 2;
    pub const SUBCLASS_SWORD_1H: u8 = 7;
    pub const QUALITY_POOR: u8 = 0;
    pub const INVTYPE_WEAPON_MAINHAND: u8 = 21;
    pub const ITEM_LEVEL: u8 = 2;
    pub const REQUIRED_LEVEL: u8 = 1;
    pub const MAX_DURABILITY: u32 = 20;
    pub const BUY_PRICE: u32 = 35; // Copper.
    pub const SELL_PRICE: u32 = 7; // Copper.
    pub const DISPLAY_ID: u32 = 1542;
    pub const DAMAGE_MIN: f32 = 1.0;
    pub const DAMAGE_MAX: f32 = 3.0;
    pub const DELAY_MS: u32 = 1900;

    pub const HEARTHSTONE_ENTRY: u32 = 6948;
    // Last backpack slot avoids the class outfit, which fills upward from slot 23.
    pub const HEARTHSTONE_SLOT: u8 = 38;
}

pub mod dual_wield {
    pub const SPELL_ID: u32 = 674;
}

// The learned passive grants proficiency; the trainer wrapper only teaches it.
pub mod armor_proficiency {
    pub const PLATE_PASSIVE_SPELL_ID: u32 = 750;
    pub const PLATE_TRAINER_SPELL_ID: u32 = 7109;
    pub const MAIL_PASSIVE_SPELL_ID: u32 = 8737;
    pub const MAIL_TRAINER_SPELL_ID: u32 = 8738;
}

// Movement speeds are in yards per second; vanilla has no flight speed fields.
pub mod speeds {
    pub const WALK: f32 = 2.5;
    pub const RUN: f32 = 7.0;
    pub const CHARGE: f32 = 25.0;
    pub const RUN_BACK: f32 = 4.5;
    pub const SWIM: f32 = 4.722_222;
    pub const SWIM_BACK: f32 = 2.5;
    pub const TURN_RATE: f32 = std::f32::consts::PI;
}

pub mod start_human_warrior {
    pub const RACE: u8 = 1;
    pub const CLASS: u8 = 1;
    pub const MAP_ID: u32 = 0;
    pub const ZONE_ID: u32 = 12;
    pub const X: f32 = -8949.95;
    pub const Y: f32 = -132.493;
    pub const Z: f32 = 83.5312;
    pub const ORIENTATION: f32 = 0.0;
}

// Taxi node ids are one-based bit positions in a fixed eight-word mask.
pub mod taxi_protocol {
    pub const NODE_MASK_WORDS: u32 = 8;
    pub const CLIENT_NODE_ID_MIN: u32 = 1;
    pub const CLIENT_NODE_ID_MAX: u32 = NODE_MASK_WORDS * u32::BITS;
    pub const REPLY_STATUS: u8 = 1;
    pub const REPLY_OPEN: u8 = 2;
    pub const REPLY_ACTIVATE: u8 = 3;

    // Activation results match vanilla ActivateTaxiReply discriminants.
    pub const ACTIVATE_OK: u8 = 0;
    pub const ACTIVATE_UNSPECIFIED_SERVER_ERROR: u8 = 1;
    pub const ACTIVATE_NO_SUCH_PATH: u8 = 2;
    pub const ACTIVATE_NOT_ENOUGH_MONEY: u8 = 3;
    pub const ACTIVATE_TOO_FAR_AWAY: u8 = 4;
    pub const ACTIVATE_NO_VENDOR_NEARBY: u8 = 5;
    pub const ACTIVATE_NOT_VISITED: u8 = 6;
    pub const ACTIVATE_PLAYER_BUSY: u8 = 7;
    pub const ACTIVATE_PLAYER_ALREADY_MOUNTED: u8 = 8;
    pub const ACTIVATE_PLAYER_SHAPE_SHIFTED: u8 = 9;
    pub const ACTIVATE_PLAYER_MOVING: u8 = 10;
    pub const ACTIVATE_SAME_NODE: u8 = 11;
    pub const ACTIVATE_NOT_STANDING: u8 = 12;
}

// Reserved storage ids survive DBC import; client ids must fit the vanilla taxi mask.
pub mod taxi_fixture {
    pub const STORAGE_ID_FLOOR: u32 = 5_090_000;

    pub const SOURCE_NODE_STORAGE_ID: u32 = 5_090_100;
    pub const DESTINATION_NODE_STORAGE_ID: u32 = 5_090_101;
    pub const PATH_ID: u32 = 5_090_102;
    pub const POINT_IDS: [u32; 3] = [5_090_103, 5_090_104, 5_090_105];

    pub const SOURCE_CLIENT_NODE_ID: u32 = super::taxi_protocol::CLIENT_NODE_ID_MAX - 1;
    pub const DESTINATION_CLIENT_NODE_ID: u32 = super::taxi_protocol::CLIENT_NODE_ID_MAX;

    pub const MAP_ID: u32 = super::start_human_warrior::MAP_ID;
    pub const SOURCE_X: f32 = super::start_human_warrior::X + 5.0;
    pub const SOURCE_Y: f32 = super::start_human_warrior::Y - 5.0;
    pub const SOURCE_Z: f32 = super::start_human_warrior::Z;
    pub const DESTINATION_X: f32 = SOURCE_X + 60.0;
    pub const DESTINATION_Y: f32 = SOURCE_Y;
    pub const DESTINATION_Z: f32 = SOURCE_Z;
    pub const MIDPOINT_X: f32 = SOURCE_X + 30.0;
    pub const MIDPOINT_Y: f32 = SOURCE_Y;
    pub const MIDPOINT_Z: f32 = SOURCE_Z + 12.0;

    pub const SOURCE_NAME: &str = "Northshire Test Flight";
    pub const DESTINATION_NAME: &str = "Northshire Test Landing";
    pub const FARE: u32 = 25;
    // TaxiNodes.dbc stores the Horde display id before the Alliance display id.
    pub const MOUNT_DISPLAY_HORDE: u32 = 295;
    pub const MOUNT_DISPLAY_ALLIANCE: u32 = 1147;

    pub const FLIGHT_MASTER_ENTRY: u32 = 51_006;
    pub const FLIGHT_MASTER_GUID: u64 =
        (0xF130_u64 << 48) | ((FLIGHT_MASTER_ENTRY as u64) << 24) | 1;
}

pub mod gossip_option {
    pub const GOSSIP: u32 = 1;
    pub const QUESTGIVER: u32 = 2;
    pub const VENDOR: u32 = 3;
    pub const TAXI: u32 = 4;
    pub const TRAINER: u32 = 5;
    pub const SPIRITHEALER: u32 = 6;
    pub const INNKEEPER: u32 = 8;
    pub const BANKER: u32 = 9;
    pub const PETITIONER: u32 = 10;
    pub const TABARDDESIGNER: u32 = 11;
    // The dump encodes talent reset as GOSSIP; the importer reclassifies its option text.
    pub const UNLEARNTALENTS: u32 = 16;
}

pub const MIN_TALENT_LEVEL: u8 = 10;

// Persisted encodings: retain values 0–3 when adding condition kinds.
pub mod gossip_condition {
    pub const NONE: u32 = 0;
    /// An active quest: value2 is 0 for either state, 1 incomplete, or 2 complete.
    pub const QUEST_TAKEN: u32 = 1;
    pub const QUEST_REWARDED: u32 = 2;
    /// An explicitly unavailable option, including unsupported inline source conditions.
    pub const NEVER: u32 = 3;
    /// value1 is the level; value2 is 0 equal, 1 at least, or 2 at most.
    pub const LEVEL: u32 = 4;
    /// value1 is the source team id, 469 Alliance or 67 Horde.
    pub const TEAM: u32 = 5;
    /// value1 and value2 are race and class masks; a zero mask is unrestricted.
    pub const RACE_CLASS: u32 = 6;
    /// value1 names a retained source row in game_gossip_condition.
    pub const REFERENCE: u32 = 7;
    pub const QUEST_NONE: u32 = 8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_type_mask_is_0x19() {
        assert_eq!(type_mask::PLAYER, 0x19);
    }

    #[test]
    fn packing_a_sheath_state_preserves_the_other_three_bytes() {
        let neighbours = 0xAB_CD_EF_00_u32;
        assert_eq!(
            sheath_state::packed_with(neighbours, sheath_state::MELEE),
            0xAB_CD_EF_01
        );
        assert_eq!(
            sheath_state::packed_with(0xAB_CD_EF_02, sheath_state::UNARMED),
            0xAB_CD_EF_00
        );
    }

    #[test]
    fn sheath_state_validation_rejects_out_of_range_bytes() {
        for ok in [
            sheath_state::UNARMED,
            sheath_state::MELEE,
            sheath_state::RANGED,
        ] {
            assert!(sheath_state::is_valid(ok), "{ok} is a real sheath state");
        }
        for bad in [3u8, 4, 17, 255] {
            assert!(!sheath_state::is_valid(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn creature_type_mask_is_0x9() {
        assert_eq!(type_mask::CREATURE, 0x9);
    }

    #[test]
    fn vanilla_loot_tag_flags_use_build_5875_values() {
        assert_eq!(unit_dynamic_flags::TAPPED, 0x0004);
        assert_eq!(unit_dynamic_flags::TAPPED_BY_PLAYER, 0x0008);
    }

    #[test]
    fn vanilla_auction_vocabulary_uses_the_build_5875_values() {
        assert_eq!(npc_flags::AUCTIONEER, 0x1000);
    }

    #[test]
    fn go_type_questgiver_is_cmangos_type_2() {
        assert_eq!(go_type::QUESTGIVER, 2);
    }

    #[test]
    fn go_type_meetingstone_is_cmangos_type_23() {
        assert_eq!(go_type::MEETINGSTONE, 23);
    }

    #[test]
    fn gossip_option_actions_are_distinct() {
        use gossip_option::*;
        let mut codes = [
            GOSSIP,
            QUESTGIVER,
            BANKER,
            VENDOR,
            TAXI,
            TRAINER,
            SPIRITHEALER,
            INNKEEPER,
        ];
        codes.sort_unstable();
        assert_eq!(
            codes.windows(2).filter(|w| w[0] == w[1]).count(),
            0,
            "gossip_option action codes must not collide"
        );
        assert_eq!(
            (
                GOSSIP,
                QUESTGIVER,
                VENDOR,
                TAXI,
                TRAINER,
                SPIRITHEALER,
                INNKEEPER,
                BANKER
            ),
            (1, 2, 3, 4, 5, 6, 8, 9)
        );
    }

    #[test]
    fn gossip_condition_keeps_existing_persisted_values() {
        assert_eq!(
            (
                gossip_condition::NONE,
                gossip_condition::QUEST_TAKEN,
                gossip_condition::QUEST_REWARDED,
                gossip_condition::NEVER
            ),
            (0, 1, 2, 3),
        );
    }
}
