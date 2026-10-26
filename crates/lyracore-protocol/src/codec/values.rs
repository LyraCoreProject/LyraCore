//! Finalize, dirty_reset, then reset changed fields to exclude OBJECT_FIELD_TYPE from VALUES.

use super::*;

pub(super) fn unit_values(guid: u64, apply: impl FnOnce(&mut UpdateUnit)) -> SMSG_UPDATE_OBJECT {
    let mut unit = UpdateUnit::builder().finalize();
    unit.dirty_reset();
    apply(&mut unit);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Unit(unit),
        }],
    }
}

fn player_values(guid: u64, apply: impl FnOnce(&mut UpdatePlayer)) -> SMSG_UPDATE_OBJECT {
    let mut player = UpdatePlayer::builder().finalize();
    player.dirty_reset();
    apply(&mut player);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Player(player),
        }],
    }
}

pub fn build_watched_faction_values(guid: u64, reputation_index: i32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_field_watched_faction_index(reputation_index);
    })
}

pub fn build_health_values(guid: u64, health: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_health(health as i32);
    })
}

pub fn build_player_flags_values(
    guid: u64,
    player_flags: u32,
    unit_bytes_1: u32,
) -> SMSG_UPDATE_OBJECT {
    let (b1a, b1b, b1c, b1d) = unpack4(unit_bytes_1);
    player_values(guid, |player| {
        player.set_player_flags(player_flags as i32);
        player.set_unit_bytes_1(b1a, b1b, b1c, b1d);
    })
}

pub fn build_bank_bag_slots_values(guid: u64, player_bytes_2: u32) -> SMSG_UPDATE_OBJECT {
    let (a, b, c, d) = unpack4(player_bytes_2);
    player_values(guid, |player| {
        player.set_player_bytes_2(a, b, c, d);
    })
}

pub const SMSG_UPDATE_OBJECT_OPCODE: u16 = 0x00A9;

pub fn build_values_update_raw(guid: u64, mask: &update_mask::UpdateMaskValues) -> (u16, Vec<u8>) {
    // OBJECT_FIELD_TYPE in partial VALUES can strip PLAYER and crash build 5875 at null+0x110.
    debug_assert!(
        mask.get(update_mask::idx::OBJECT_TYPE).is_none(),
        "raw VALUES update must NOT set OBJECT_FIELD_TYPE (5875 null+0x110 crash trap)"
    );
    let mut body = Vec::new();
    body.extend_from_slice(&1u32.to_le_bytes());
    body.push(0u8);
    body.push(0u8);
    write_packed_guid_u64(&mut body, guid);
    mask.write_to(&mut body);
    (SMSG_UPDATE_OBJECT_OPCODE, body)
}

pub fn build_guild_values(guid: u64, guild_id: u32, rank_id: u32) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_u32(update_mask::idx::PLAYER_GUILDID, guild_id);
    mask.set_u32(update_mask::idx::PLAYER_GUILDRANK, rank_id);
    build_values_update_raw(guid, &mask)
}

pub fn build_explored_zones_values(guid: u64, word_idx: u16, word_value: u32) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_u32(
        update_mask::idx::PLAYER_EXPLORED_ZONES_1 + word_idx,
        word_value,
    );
    build_values_update_raw(guid, &mask)
}

/// VALUES replaces the whole u32; preserve facial hair and the nonzero rest-state byte.
pub fn build_rest_state_values(guid: u64, player_bytes_2: u32) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_u32(update_mask::idx::PLAYER_BYTES_2, player_bytes_2);
    build_values_update_raw(guid, &mask)
}

/// The Module keeps PLAYER_FIELD_BYTES bytes 0, 2 and 3 at zero, so this sends only byte 1.
pub fn build_combo_points_values(guid: u64, target_guid: u64, points: u8) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_u64(update_mask::idx::PLAYER_FIELD_COMBO_TARGET, target_guid);
    mask.set_bytes(update_mask::idx::PLAYER_FIELD_BYTES, 0, points, 0, 0);
    build_values_update_raw(guid, &mask)
}

pub fn write_packed_guid_u64(out: &mut Vec<u8>, guid: u64) {
    let bytes = guid.to_le_bytes();
    let mut bit_pattern: u8 = 0;
    let mut data: Vec<u8> = Vec::with_capacity(8);
    for (i, &b) in bytes.iter().enumerate() {
        if b != 0 {
            bit_pattern |= 1 << i;
            data.push(b);
        }
    }
    out.push(bit_pattern);
    out.extend_from_slice(&data);
}

pub fn build_dynamic_flags_values(guid: u64, dynamic_flags: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_dynamic_flags(dynamic_flags as i32);
    })
}

pub fn build_unit_flags_values(guid: u64, unit_flags: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_flags(unit_flags as i32);
    })
}

pub fn build_taxi_presentation_values(
    guid: u64,
    mount_display_id: u32,
    unit_flags: u32,
) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_mountdisplayid(mount_display_id as i32);
        unit.set_unit_flags(unit_flags as i32);
    })
}

pub fn build_mount_display_values(guid: u64, mount_display_id: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_mountdisplayid(mount_display_id as i32);
    })
}

pub fn build_display_values(guid: u64, display_id: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_displayid(display_id as i32);
    })
}

pub fn build_sheath_values(guid: u64, unit_bytes_2: u32) -> SMSG_UPDATE_OBJECT {
    let (b2a, b2b, b2c, b2d) = unpack4(unit_bytes_2);
    unit_values(guid, |unit| {
        unit.set_unit_bytes_2(b2a, b2b, b2c, b2d);
    })
}

pub fn build_power_values(guid: u64, power_b: u8, power_cur: u32) -> SMSG_UPDATE_OBJECT {
    let cur = power_cur as i32;
    player_values(guid, |player| match power_b {
        1 => {
            player.set_unit_power2(cur);
        }
        2 => {
            player.set_unit_power3(cur);
        }
        3 => {
            player.set_unit_power4(cur);
        }
        4 => {
            player.set_unit_power5(cur);
        }
        _ => {
            player.set_unit_power1(cur);
        }
    })
}

pub fn build_target_values(guid: u64, target: u64) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_target(Guid::new(target));
    })
}

pub fn build_virtual_item_values(
    guid: u64,
    main_hand_display: u32,
    off_hand_display: u32,
    ranged_display: u32,
) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    let first = update_mask::idx::UNIT_VIRTUAL_ITEM_SLOT_DISPLAY;
    mask.set_u32(first, main_hand_display);
    mask.set_u32(first + 1, off_hand_display);
    mask.set_u32(first + 2, ranged_display);
    build_values_update_raw(guid, &mask)
}

pub fn build_max_vitals_values(
    guid: u64,
    max_health: u32,
    power_b: u8,
    max_power: u32,
) -> SMSG_UPDATE_OBJECT {
    let max_p = max_power as i32;
    unit_values(guid, |unit| {
        unit.set_unit_maxhealth(max_health as i32);
        match power_b {
            1 => {
                unit.set_unit_maxpower2(max_p);
            }
            2 => {
                unit.set_unit_maxpower3(max_p);
            }
            3 => {
                unit.set_unit_maxpower4(max_p);
            }
            4 => {
                unit.set_unit_maxpower5(max_p);
            }
            _ => {
                unit.set_unit_maxpower1(max_p);
            }
        }
    })
}

pub fn build_armor_values(guid: u64, total: [u32; 7], pos_buff: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |p| {
        p.set_unit_normal_resistance(total[0] as i32);
        p.set_unit_holy_resistance(total[1] as i32);
        p.set_unit_fire_resistance(total[2] as i32);
        p.set_unit_nature_resistance(total[3] as i32);
        p.set_unit_frost_resistance(total[4] as i32);
        p.set_unit_shadow_resistance(total[5] as i32);
        p.set_unit_arcane_resistance(total[6] as i32);
        p.set_player_field_resistancebuffmodspositive(pos_buff as i32);
    })
}

#[derive(PartialEq)]
pub struct SheetStatsValues {
    pub strength: u32,
    pub agility: u32,
    pub stamina: u32,
    pub intellect: u32,
    pub spirit: u32,
    pub str_bonus: i32,
    pub agi_bonus: i32,
    pub sta_bonus: i32,
    pub int_bonus: i32,
    pub spi_bonus: i32,
    pub attack_power: u32,
    pub ap_mods: i32,
    pub dmg_min: u32,
    pub dmg_max: u32,
    pub ranged_attack_power: u32,
    pub ranged_dmg_min: u32,
    pub ranged_dmg_max: u32,
    pub crit_pct: f32,
}

pub fn build_sheet_stats_values(guid: u64, s: &SheetStatsValues) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |p| {
        p.set_unit_strength(s.strength as i32);
        p.set_unit_agility(s.agility as i32);
        p.set_unit_stamina(s.stamina as i32);
        p.set_unit_intellect(s.intellect as i32);
        p.set_unit_spirit(s.spirit as i32);
        p.set_player_field_posstat0(s.str_bonus.max(0));
        p.set_player_field_posstat1(s.agi_bonus.max(0));
        p.set_player_field_posstat2(s.sta_bonus.max(0));
        p.set_player_field_posstat3(s.int_bonus.max(0));
        p.set_player_field_posstat4(s.spi_bonus.max(0));
        p.set_player_field_negstat0(s.str_bonus.min(0));
        p.set_player_field_negstat1(s.agi_bonus.min(0));
        p.set_player_field_negstat2(s.sta_bonus.min(0));
        p.set_player_field_negstat3(s.int_bonus.min(0));
        p.set_player_field_negstat4(s.spi_bonus.min(0));
        p.set_unit_attack_power(s.attack_power as i32);
        // ATTACK_POWER_MODS packs unsigned positive and negative magnitudes into two u16 fields.
        p.set_unit_attack_power_mods(s.ap_mods.max(0) as u16, (-s.ap_mods).max(0) as u16);
        p.set_unit_mindamage(s.dmg_min as f32);
        p.set_unit_maxdamage(s.dmg_max as f32);
        p.set_unit_ranged_attack_power(s.ranged_attack_power as i32);
        p.set_unit_ranged_attack_power_mods(0, 0);
        p.set_unit_minrangeddamage(s.ranged_dmg_min as f32);
        p.set_unit_maxrangeddamage(s.ranged_dmg_max as f32);
        p.set_player_crit_percentage(s.crit_pct);
    })
}

pub fn build_resistance_values(guid: u64, total: [u32; 7]) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_normal_resistance(total[0] as i32);
        unit.set_unit_holy_resistance(total[1] as i32);
        unit.set_unit_fire_resistance(total[2] as i32);
        unit.set_unit_nature_resistance(total[3] as i32);
        unit.set_unit_frost_resistance(total[4] as i32);
        unit.set_unit_shadow_resistance(total[5] as i32);
        unit.set_unit_arcane_resistance(total[6] as i32);
    })
}

pub fn build_coinage_values(guid: u64, money: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_field_coinage(money as i32);
    })
}

pub fn build_player_ammo_id_values(guid: u64, ammo_entry: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_ammo_id(ammo_entry as i32);
    })
}

pub fn build_self_res_spell_values(guid: u64, spell_id: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_self_res_spell(spell_id as i32);
    })
}

pub fn build_owner_summon_values(owner_guid: u64, pet_guid: u64) -> SMSG_UPDATE_OBJECT {
    player_values(owner_guid, |player| {
        player.set_unit_summon(Guid::new(pet_guid));
    })
}

pub fn build_skill_values(
    guid: u64,
    slot: u8,
    skill: Skill,
    cur: u16,
    max: u16,
) -> Option<SMSG_UPDATE_OBJECT> {
    let idx = SkillInfoIndex::try_from(slot).ok()?;
    Some(player_values(guid, |player| {
        player.set_player_skill_info(SkillInfo::new(skill, 0, cur, max, 0, 0), idx);
    }))
}

pub fn build_talent_points_values(guid: u64, points: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_character_points1(points as i32);
    })
}

pub fn build_player_xp_values(guid: u64, xp: u32, next_level_xp: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_xp(xp as i32);
        player.set_player_next_level_xp(next_level_xp as i32);
    })
}

pub fn build_aura_duration(slot: u8, duration_ms: u32) -> SMSG_UPDATE_AURA_DURATION {
    SMSG_UPDATE_AURA_DURATION {
        aura_slot: slot,
        aura_duration: duration_ms,
    }
}

pub fn build_inv_slot_values(
    player_guid: u64,
    slot: u8,
    item_guid: u64,
) -> Option<SMSG_UPDATE_OBJECT> {
    let s = ItemSlot::try_from(slot).ok()?;
    Some(player_values(player_guid, |player| {
        player.set_player_field_inv(s, Guid::new(item_guid));
    }))
}

pub fn build_visible_item_values(
    player_guid: u64,
    slot: u8,
    item: &ItemInstanceView,
) -> Option<SMSG_UPDATE_OBJECT> {
    let vi_index = super::entity::visible_item_index(slot)?;
    Some(player_values(player_guid, |player| {
        player.set_player_visible_item(super::entity::visible_item_of(item), vi_index);
    }))
}

pub fn build_item_values(guid: u64, stack_count: u32, durability: u32) -> SMSG_UPDATE_OBJECT {
    let mut item = UpdateItem::builder().finalize();
    item.dirty_reset();
    item.set_item_stack_count(stack_count.max(1) as i32);
    item.set_item_durability(durability as i32);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Item(item),
        }],
    }
}

pub fn build_item_enchantment_values(guid: u64, client_enchant_id: u32) -> SMSG_UPDATE_OBJECT {
    let mut item = UpdateItem::builder().finalize();
    item.dirty_reset();
    item.set_item_enchantment(client_enchant_id as i32);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Item(item),
        }],
    }
}

pub fn build_container_slot_values(
    bag_guid: u64,
    slot_in_bag: u8,
    item_guid: u64,
) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    let field = update_mask::idx::CONTAINER_FIELD_SLOT_1 + u16::from(slot_in_bag) * 2;
    mask.set_u64(field, item_guid);
    build_values_update_raw(bag_guid, &mask)
}

pub fn build_level_values(guid: u64, level: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_unit_level(level as i32);
    })
}

#[cfg(test)]
mod lint_tests {
    use super::*;
    use wow_world_messages::vanilla::ServerMessage;

    #[test]
    fn every_values_builder_serializes_without_the_object_type_bit() {
        let g = 0x9u64;
        let msgs: Vec<(&str, SMSG_UPDATE_OBJECT)> = vec![
            ("health", build_health_values(g, 41)),
            ("player_flags", build_player_flags_values(g, 0x10, 0)),
            ("dynamic_flags", build_dynamic_flags_values(g, 1)),
            ("unit_flags", build_unit_flags_values(g, 0x80000)),
            (
                "taxi_presentation",
                build_taxi_presentation_values(g, 1147, 0x0010_0000),
            ),
            ("mount_display", build_mount_display_values(g, 1147)),
            ("display", build_display_values(g, 1825)),
            ("power", build_power_values(g, 0, 55)),
            ("target", build_target_values(g, 0xF130_0000_0000_0001)),
            ("max_vitals", build_max_vitals_values(g, 100, 0, 200)),
            ("armor", build_armor_values(g, [60, 0, 0, 0, 0, 0, 0], 20)),
            (
                "resistance",
                build_resistance_values(g, [40, 0, 0, 0, 0, 0, 0]),
            ),
            ("coinage", build_coinage_values(g, 900)),
            ("ammo", build_player_ammo_id_values(g, 2512)),
            (
                "owner_summon",
                build_owner_summon_values(g, 0xF130_0000_0000_0009),
            ),
            (
                "skill",
                build_skill_values(g, 3, Skill::Fishing, 100, 150).unwrap(),
            ),
            ("talent_points", build_talent_points_values(g, 2)),
            ("xp", build_player_xp_values(g, 400, 1000)),
            (
                "inv_slot",
                build_inv_slot_values(g, 15, 0x4000_0000_0000_0001).unwrap(),
            ),
            (
                "visible_item",
                build_visible_item_values(
                    g,
                    15,
                    &ItemInstanceView {
                        entry: 25,
                        random_property_id: 117,
                        enchantment: 823,
                        ..Default::default()
                    },
                )
                .unwrap(),
            ),
            ("item", build_item_values(0x4000_0000_0000_0001, 5, 70)),
            (
                "item_enchantment",
                build_item_enchantment_values(0x4000_0000_0000_0001, 823),
            ),
        ];
        for (name, m) in msgs {
            let mut buf = Vec::new();
            m.write_unencrypted_server(&mut buf).unwrap();
            let body = &buf[4..];
            let updates = lyracore_shared::values_mask::parse_values_updates(body);
            assert!(
                !updates.is_empty(),
                "{name}: no VALUES block decoded — framing drift?"
            );
            for u in &updates {
                assert!(
                    !u.fields.iter().any(|&(idx, _)| idx == 2),
                    "{name}: OBJECT_FIELD_TYPE (bit 2) leaked into a VALUES partial — dirty_reset missing (5875 crash class)"
                );
                assert!(!u.fields.is_empty(), "{name}: empty VALUES mask");
            }
        }
    }
}
