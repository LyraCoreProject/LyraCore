use crate::codec::item::ItemInstanceView;
use crate::codec::values::SheetStatsValues;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::SMSG_SET_PROFICIENCY;

pub(super) fn ranged_sheet_stats(
    ranged_attack_power: u32,
    ranged_dmg_min: u32,
    ranged_dmg_max: u32,
) -> SheetStatsValues {
    SheetStatsValues {
        strength: 20,
        agility: 20,
        stamina: 20,
        intellect: 20,
        spirit: 20,
        str_bonus: 0,
        agi_bonus: 0,
        sta_bonus: 0,
        int_bonus: 0,
        spi_bonus: 0,
        attack_power: 50,
        ap_modifiers: 0,
        ranged_ap_modifiers: 0,
        dmg_min: 11,
        dmg_max: 13,
        ranged_attack_power,
        ranged_dmg_min: ranged_dmg_min as f32,
        ranged_dmg_max: ranged_dmg_max as f32,
        crit_pct: 5.0,
        ranged_crit_pct: 5.0,
    }
}

pub(super) fn worn_item(slot: u8, guid: u64, entry: u32, enchantment: u32) -> ItemInstanceView {
    ItemInstanceView {
        guid,
        entry,
        slot,
        enchantment,
        ..Default::default()
    }
}

pub(super) fn proficiency(msg: &ServerOpcodeMessage) -> &SMSG_SET_PROFICIENCY {
    match msg {
        ServerOpcodeMessage::SMSG_SET_PROFICIENCY(p) => p,
        other => panic!("expected SMSG_SET_PROFICIENCY, got {other}"),
    }
}
