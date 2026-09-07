//! Build 5875 Spell columns shared by spell import and item enchantment import.
//!
//! Layout: cmangos/mangos-classic 8ec338a1704e7dcb1c0213eb7ed58f9231ade40f,
//! src/game/Server/DBCStructure.h, SpellEntry columns 0..172.
//! wow_dbc 0.3 omits InterruptFlags at column 21. Its equipment, reagent and effect
//! fields stay shifted until an extra unknown_flag resynchronizes the name at column 120.

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;

#[derive(Default, Debug)]
pub(crate) struct Spell {
    pub(crate) id: u32,
    pub(crate) school: u32,
    pub(crate) dispel_type: u32,
    pub(crate) mechanic: u32,
    pub(crate) attributes: u32,
    pub(crate) attributes_ex1: u32,
    pub(crate) attributes_ex2: u32,
    pub(crate) shapeshift_mask: u32,
    pub(crate) requires_spell_focus: u32,
    pub(crate) casting_time_index: u32,
    pub(crate) recovery_time: u32,
    pub(crate) category_recovery_time: u32,
    pub(crate) aura_interrupt_flags: u32,
    pub(crate) proc_flags: u32,
    pub(crate) proc_chance: u32,
    pub(crate) proc_charges: u32,
    pub(crate) max_level: u32,
    pub(crate) spell_level: u32,
    pub(crate) duration: u32,
    pub(crate) power_type: u32,
    pub(crate) mana_cost: u32,
    pub(crate) range: u32,
    pub(crate) stack_amount: u32,
    pub(crate) reagent: [i32; 8],
    pub(crate) reagent_count: [u32; 8],
    /// Class, subclass mask and inventory mask in source order.
    pub(crate) equipment: [u32; 3],
    pub(crate) effect: [i32; 3],
    pub(crate) effect_die_sides: [i32; 3],
    pub(crate) effect_base_dice: [u32; 3],
    pub(crate) effect_dice_per_level: [f32; 3],
    pub(crate) effect_real_points_per_level: [f32; 3],
    pub(crate) effect_base_points: [i32; 3],
    pub(crate) effect_mechanic: [u32; 3],
    pub(crate) implicit_target_a: [i32; 3],
    pub(crate) implicit_target_b: [i32; 3],
    pub(crate) effect_radius: [u32; 3],
    pub(crate) effect_aura: [u32; 3],
    pub(crate) effect_amplitude: [u32; 3],
    pub(crate) effect_multiple_values: [f32; 3],
    pub(crate) effect_chain_target: [u32; 3],
    pub(crate) effect_item_type: [i32; 3],
    pub(crate) effect_misc_value: [u32; 3],
    pub(crate) effect_trigger_spell: [u32; 3],
    pub(crate) effect_points_per_combo: [f32; 3],
    pub(crate) name: String,
    pub(crate) family_name: u32,
    pub(crate) family_flags: u64,
}

/// Read only the supported fixed-width layout. Client bytes remain in memory.
pub(crate) fn read_spells(bytes: &[u8]) -> Result<Vec<Spell>> {
    if bytes.len() < 20 || &bytes[..4] != b"WDBC" {
        bail!("Spell.dbc has no WDBC header");
    }
    let header = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let (count, fields, size, strings) = (header(4), header(8), header(12), header(16));
    if count == 0 {
        bail!("Spell.dbc contains no rows");
    }
    if fields != 173 || size != 692 {
        bail!("Spell.dbc requires the build 5875 layout: 173 fields, 692 bytes per row");
    }
    let records_end = count.checked_mul(size).and_then(|n| n.checked_add(20));
    if records_end.and_then(|n| n.checked_add(strings)) != Some(bytes.len()) {
        bail!("Spell.dbc record and string lengths do not match its header");
    }
    // The length check proves both the record boundary and every fixed column below fit.
    let records_end = records_end.unwrap();
    let strings = &bytes[records_end..];
    let mut ids = BTreeSet::new();
    bytes[20..records_end]
        .chunks_exact(size)
        .map(|row| {
            let column =
                |at: usize| u32::from_le_bytes(row[at * 4..at * 4 + 4].try_into().unwrap());
            let array = |at: usize| std::array::from_fn(|i| column(at + i));
            let id = column(0);
            if !ids.insert(id) {
                bail!("duplicate Spell.dbc row {id}");
            }
            let name = strings.get(column(120) as usize..).with_context(|| {
                format!("Spell.dbc row {id} name offset exceeds the string block")
            })?;
            let end = name
                .iter()
                .position(|&b| b == 0)
                .with_context(|| format!("Spell.dbc row {id} name has no terminator"))?;
            let name = std::str::from_utf8(&name[..end])
                .with_context(|| format!("Spell.dbc row {id} name is not UTF-8"))?;
            let spell = Spell {
                id: column(0),
                school: column(1),
                dispel_type: column(4),
                mechanic: column(5),
                attributes: column(6),
                attributes_ex1: column(7),
                attributes_ex2: column(8),
                shapeshift_mask: column(11),
                requires_spell_focus: column(15),
                casting_time_index: column(18),
                recovery_time: column(19),
                category_recovery_time: column(20),
                aura_interrupt_flags: column(22),
                proc_flags: column(24),
                proc_chance: column(25),
                proc_charges: column(26),
                max_level: column(27),
                spell_level: column(29),
                duration: column(30),
                power_type: column(31),
                mana_cost: column(32),
                range: column(36),
                stack_amount: column(39),
                reagent: std::array::from_fn(|i| column(42 + i) as i32),
                reagent_count: std::array::from_fn(|i| column(50 + i)),
                equipment: array(58),
                effect: array(61).map(|n| n as i32),
                effect_die_sides: array(64).map(|n| n as i32),
                effect_base_dice: array(67),
                effect_dice_per_level: array(70).map(f32::from_bits),
                effect_real_points_per_level: array(73).map(f32::from_bits),
                effect_base_points: array(76).map(|n| n as i32),
                effect_mechanic: array(79),
                implicit_target_a: array(82).map(|n| n as i32),
                implicit_target_b: array(85).map(|n| n as i32),
                effect_radius: array(88),
                effect_aura: array(91),
                effect_amplitude: array(94),
                effect_multiple_values: array(97).map(f32::from_bits),
                effect_chain_target: array(100),
                effect_item_type: array(103).map(|n| n as i32),
                effect_misc_value: array(106),
                effect_trigger_spell: array(109),
                effect_points_per_combo: array(112).map(f32::from_bits),
                name: name.to_owned(),
                family_name: column(160),
                family_flags: u64::from(column(161)) | (u64::from(column(162)) << 32),
            };
            if spell
                .effect_dice_per_level
                .iter()
                .chain(&spell.effect_real_points_per_level)
                .chain(&spell.effect_multiple_values)
                .chain(&spell.effect_points_per_combo)
                .any(|n| !n.is_finite())
            {
                bail!("Spell.dbc row {id} contains a non-finite effect magnitude");
            }
            Ok(spell)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_spell() -> Vec<u8> {
        let mut bytes = b"WDBC".to_vec();
        for n in [1u32, 173, 692, 1] {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        bytes.resize(713, 0);
        bytes
    }

    #[test]
    fn malformed_layout_strings_and_magnitudes_return_errors() {
        for bytes in [vec![], vec![0; 20]] {
            assert!(read_spells(&bytes).is_err());
        }
        for (at, value) in [
            (4, u32::MAX),
            (8, 172),
            (12, 688),
            (16, 2),
            (20 + 120 * 4, 2),
            (20 + 73 * 4, f32::NAN.to_bits()),
        ] {
            let mut bytes = empty_spell();
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(read_spells(&bytes).is_err(), "byte {at}");
        }
        let mut bytes = empty_spell();
        bytes[4..8].copy_from_slice(&0u32.to_le_bytes());
        bytes.truncate(21);
        assert!(read_spells(&bytes)
            .unwrap_err()
            .to_string()
            .contains("no rows"));
        let mut bytes = empty_spell();
        bytes[712] = b'x';
        assert!(read_spells(&bytes).is_err());
        bytes[16..20].copy_from_slice(&2u32.to_le_bytes());
        bytes[712] = 0xff;
        bytes.push(0);
        assert!(read_spells(&bytes).is_err());
    }

    #[test]
    fn duplicate_spell_ids_return_an_error() {
        let mut bytes = empty_spell();
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
        bytes.resize(20 + 2 * 692 + 1, 0);
        assert!(read_spells(&bytes)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
    }
}
