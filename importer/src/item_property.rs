//! Derive Random Property catalogues from the Operator's client and ClassicDB dump.

use crate::{dbc, field, parse_table, push_insert, sql_text};
use anyhow::{bail, Context, Result};
use lyracore_shared::item_property as kind;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use wow_dbc::vanilla_tables::{
    item_random_properties::ItemRandomProperties,
    spell::{Spell, SpellRow},
    spell_item_enchantment::SpellItemEnchantment,
};
use wow_dbc::DbcTable;
use wow_mpq::PatchChain;

#[derive(Debug, PartialEq)]
struct Effect {
    index: u8,
    kind: u8,
    amount: i32,
    spell_id: u32,
    school_mask: u32,
}

fn resistance_kind(school: u32) -> Option<u8> {
    match school {
        0 => Some(kind::ARMOR),
        1..=6 => Some(kind::HOLY_RESISTANCE + school as u8 - 1),
        _ => None,
    }
}

/// Flat equip auras only. Conditional, scaling and percentage modifiers retain kind 0.
fn aura_kinds(aura: u32, misc: u32) -> Vec<(u8, u32)> {
    match (aura, misc) {
        (29, u32::MAX) => (kind::STRENGTH..=kind::SPIRIT)
            .map(|kind| (kind, 0))
            .collect(),
        (29, 0..=4) => vec![(kind::STRENGTH + misc as u8, 0)],
        (22, mask) if mask > 0 && mask & !0x7f == 0 => (0..7)
            .filter(|school| mask & (1 << school) != 0)
            .map(|school| (resistance_kind(school).unwrap(), 1 << school))
            .collect(),
        (34, _) => vec![(kind::HEALTH, 0)],
        (35, 0) => vec![(kind::MANA, 0)],
        (13, mask) if mask > 0 && mask & !0x7f == 0 => {
            let mut kinds = Vec::new();
            if mask & 1 != 0 {
                kinds.push((kind::WEAPON_DAMAGE, 1));
            }
            if mask & 0x7e != 0 {
                kinds.push((kind::SPELL_POWER, mask & 0x7e));
            }
            kinds
        }
        (135, _) => vec![(kind::HEALING_POWER, 0)],
        (84, _) => vec![(kind::HEALTH_PER_FIVE, 0)],
        (85, 0) => vec![(kind::MANA_PER_FIVE, 0)],
        (52, _) => vec![(kind::CRIT, 0)],
        (54, _) => vec![(kind::HIT, 0)],
        (30, 95) => vec![(kind::DEFENSE, 0)],
        (49, _) => vec![(kind::DODGE, 0)],
        (47, _) => vec![(kind::PARRY, 0)],
        (51, _) => vec![(kind::BLOCK, 0)],
        _ => vec![(kind::UNKNOWN, 0)],
    }
}

fn spell_effects(slot: u8, spell: &SpellRow) -> Result<Vec<Effect>> {
    let mut mapped = Vec::new();
    for subeffect in 0..3 {
        if spell.effect[subeffect] == 0 {
            continue;
        }
        let aura = spell.effect_aura[subeffect] as u32;
        let flat = spell.effect[subeffect] == 6
            && spell.effect_real_points_per_level[subeffect] == 0.0
            && spell.effect_dice_per_level[subeffect] == 0.0
            && spell.effect_points_per_combo[subeffect] == 0.0
            && spell.equipped_item_class.id == u32::MAX;
        let kinds = if flat {
            aura_kinds(aura, spell.effect_misc_value[subeffect])
        } else {
            vec![(kind::UNKNOWN, 0)]
        };
        let amount = spell.effect_base_points[subeffect]
            .checked_add(1)
            .context("enchantment spell amount overflows")?;
        for (expanded, (kind, school_mask)) in kinds.into_iter().enumerate() {
            let amount = match kind {
                kind::CRIT | kind::HIT | kind::DODGE | kind::PARRY | kind::BLOCK => amount
                    .checked_mul(100)
                    .context("enchantment basis points overflow")?,
                _ => amount,
            };
            mapped.push(Effect {
                index: slot * 64 + subeffect as u8 * 8 + expanded as u8,
                kind,
                amount,
                spell_id: spell.id.id as u32,
                school_mask,
            });
        }
    }
    if mapped.is_empty() {
        mapped.push(Effect {
            index: slot * 64,
            kind: kind::UNKNOWN,
            amount: 0,
            spell_id: spell.id.id as u32,
            school_mask: 0,
        });
    }
    Ok(mapped)
}

pub(crate) fn catalogue_sql(chain: &mut PatchChain) -> Result<(Vec<String>, BTreeSet<u32>)> {
    let properties: ItemRandomProperties = dbc::read_table(chain)?;
    let enchantments: SpellItemEnchantment = dbc::read_table(chain)?;
    let spells: Spell = dbc::read_table(chain)?;
    catalogue_rows_sql(&properties, &enchantments, &spells)
}

fn catalogue_rows_sql(
    properties: &ItemRandomProperties,
    enchantments: &SpellItemEnchantment,
    spells: &Spell,
) -> Result<(Vec<String>, BTreeSet<u32>)> {
    for (name, count) in [
        ("ItemRandomProperties.dbc", properties.rows().len()),
        ("SpellItemEnchantment.dbc", enchantments.rows().len()),
        ("Spell.dbc", spells.rows().len()),
    ] {
        if count == 0 {
            bail!("{name} contains no rows; item catalogues were not changed");
        }
    }
    let spells: HashMap<_, _> = spells
        .rows()
        .iter()
        .map(|row| (row.id.id as u32, row))
        .collect();
    let enchant_ids: BTreeSet<_> = enchantments
        .rows()
        .iter()
        .map(|row| row.id.id as u32)
        .collect();
    for (id, _, _) in kind::COMPATIBILITY_ENCHANTMENTS {
        if enchant_ids.contains(&id) {
            bail!("client enchantment {id} collides with an authored compatibility entry");
        }
    }
    let mut property_rows = BTreeMap::new();
    for property in properties.rows() {
        let id = property.id.id as u32;
        if id == 0 {
            bail!("Random Property ID zero is reserved for plain items");
        }
        if property.spell_item_enchantment[3..]
            .iter()
            .any(|id| *id != 0)
        {
            bail!("Random Property {id} has more than three enchantments");
        }
        for enchant in property
            .spell_item_enchantment
            .into_iter()
            .filter(|id| *id != 0)
        {
            if !enchant_ids.contains(&enchant) {
                bail!("Random Property {id} references missing enchantment {enchant}");
            }
        }
        let [first, second, third, _, _] = property.spell_item_enchantment;
        if property_rows
            .insert(
                id,
                format!(
                    "({id},{first},{second},{third},{})",
                    sql_text(&property.suffix.en_gb)
                ),
            )
            .is_some()
        {
            bail!("duplicate Random Property {id}");
        }
    }
    let mut enchant_rows = BTreeMap::new();
    for enchant in enchantments.rows() {
        let enchant_id = enchant.id.id as u32;
        let mut effects = Vec::new();
        for slot in 0..3 {
            let source_kind = enchant.enchantment_type[slot];
            if source_kind == 0 {
                continue;
            }
            let argument = enchant.effect_arg[slot];
            if source_kind == 3 {
                if let Some(spell) = spells.get(&(argument as u32)) {
                    effects.extend(spell_effects(slot as u8, spell)?);
                    continue;
                }
            }
            let kind = match source_kind {
                2 => kind::WEAPON_DAMAGE,
                4 => resistance_kind(argument as u32).unwrap_or(kind::UNKNOWN),
                5 => match argument {
                    0 => kind::MANA,
                    1 => kind::HEALTH,
                    3 => kind::AGILITY,
                    4 => kind::STRENGTH,
                    5 => kind::INTELLECT,
                    6 => kind::SPIRIT,
                    7 => kind::STAMINA,
                    _ => kind::UNKNOWN,
                },
                _ => kind::UNKNOWN,
            };
            effects.push(Effect {
                index: slot as u8 * 64,
                kind,
                amount: enchant.effect_points_min[slot],
                spell_id: if matches!(source_kind, 1 | 3) {
                    argument as u32
                } else {
                    0
                },
                school_mask: 0,
            });
        }
        if effects.is_empty() {
            effects.push(Effect {
                index: 0,
                kind: kind::UNKNOWN,
                amount: 0,
                spell_id: 0,
                school_mask: 0,
            });
        }
        for effect in effects {
            insert_effect(&mut enchant_rows, enchant_id, effect)?;
        }
    }
    for (id, kind, amount) in kind::COMPATIBILITY_ENCHANTMENTS {
        insert_effect(
            &mut enchant_rows,
            id,
            Effect {
                index: 0,
                kind,
                amount,
                spell_id: 0,
                school_mask: 0,
            },
        )?;
    }
    eprintln!("Random Properties: {} properties, {} normalized enchantment effects, including two authored compatibility entries", property_rows.len(), enchant_rows.len());
    let ids = property_rows.keys().copied().collect();
    let mut sql = vec![
        "DELETE FROM game_item_random_property WHERE property_id > 0".into(),
        "DELETE FROM game_item_enchantment WHERE id > 0".into(),
    ];
    push_insert(
        &mut sql,
        "game_item_random_property",
        "property_id,enchant_id_1,enchant_id_2,enchant_id_3,suffix",
        &property_rows.into_values().collect::<Vec<_>>(),
    );
    push_insert(
        &mut sql,
        "game_item_enchantment",
        "id,enchant_id,effect_index,kind,amount,spell_id,school_mask",
        &enchant_rows.into_values().collect::<Vec<_>>(),
    );
    Ok((sql, ids))
}

fn insert_effect(rows: &mut BTreeMap<u64, String>, enchant_id: u32, effect: Effect) -> Result<()> {
    let id = (u64::from(enchant_id) << 8) | u64::from(effect.index);
    if rows
        .insert(
            id,
            format!(
                "({id},{enchant_id},{},{},{},{},{})",
                effect.index, effect.kind, effect.amount, effect.spell_id, effect.school_mask
            ),
        )
        .is_some()
    {
        bail!("duplicate enchantment effect {id}");
    }
    Ok(())
}

/// ClassicDB chance has at most two decimal places. Keep hundredths exactly; reject extra precision.
fn decimal_weight(value: &str) -> Result<u32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        bail!("invalid property weight {value}");
    }
    let whole: u32 = whole.parse()?;
    let fraction: u32 = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>()? * if fraction.len() == 1 { 10 } else { 1 }
    };
    let weight = whole
        .checked_mul(100)
        .and_then(|n| n.checked_add(fraction))
        .context("property weight overflows")?;
    if weight == 0 {
        bail!("property weight must be positive");
    }
    Ok(weight)
}

pub(crate) fn template_pool(row: &[String]) -> Result<u32> {
    field(row, crate::it::RANDOM_PROPERTY)
        .parse::<u32>()
        .with_context(|| {
            format!(
                "item {} has an invalid Property Pool ID",
                field(row, crate::it::ENTRY)
            )
        })
}

pub(crate) fn dump_sql(dump: &str, dbc_dir: Option<&str>) -> Result<Vec<String>> {
    let raw = parse_table(dump, "item_enchantment_template");
    let required: BTreeSet<u32> = parse_table(dump, "item_template")
        .iter()
        .map(|row| template_pool(row))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|id| *id != 0)
        .collect();
    if raw.is_empty() && required.is_empty() {
        return Ok(Vec::new());
    }
    let dir = dbc_dir.context(
        "Random Properties require --dbc to check pool and enchantment references before apply",
    )?;
    let mut chain = dbc::open_chain(std::path::Path::new(dir))?;
    let (mut sql, properties) = catalogue_sql(&mut chain)?;
    let mut rows = BTreeMap::new();
    let mut pools = BTreeSet::new();
    for row in raw {
        let pool: u32 = field(&row, 0).parse().context("invalid Property Pool ID")?;
        let property: u32 = field(&row, 1)
            .parse()
            .context("invalid Random Property ID")?;
        if pool == 0 || !properties.contains(&property) {
            bail!("pool {pool} references missing Random Property {property}");
        }
        let weight = decimal_weight(field(&row, 2))?;
        let id = (u64::from(pool) << 32) | u64::from(property);
        if rows
            .insert(id, format!("({id},{pool},{property},{weight})"))
            .is_some()
        {
            bail!("duplicate property {property} in pool {pool}");
        }
        pools.insert(pool);
    }
    for pool in required.difference(&pools) {
        bail!("item template references missing Property Pool {pool}");
    }
    eprintln!(
        "Random Properties: {} members in {} pools",
        rows.len(),
        pools.len()
    );
    sql.push("DELETE FROM game_item_property_weight WHERE id > 0".into());
    push_insert(
        &mut sql,
        "game_item_property_weight",
        "id,pool_id,property_id,weight",
        &rows.into_values().collect::<Vec<_>>(),
    );
    Ok(sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Authored zero records let tests exercise the real DBC parser without client bytes.
    fn synthetic_table<T: DbcTable>(fields: u32) -> T {
        let mut bytes = b"WDBC".to_vec();
        for value in [1, fields, fields * 4, 1] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.resize(20 + fields as usize * 4 + 1, 0);
        T::read(&mut std::io::Cursor::new(bytes)).unwrap()
    }

    fn catalogues() -> (ItemRandomProperties, SpellItemEnchantment, Spell) {
        let mut properties: ItemRandomProperties = synthetic_table(16);
        properties.rows[0].id.id = 509_0001;
        properties.rows[0].spell_item_enchantment = [509_0002, 0, 0, 0, 0];
        properties.rows[0].suffix.en_gb = "of the Fixture".to_owned();
        let mut enchantments: SpellItemEnchantment = synthetic_table(24);
        enchantments.rows[0].id.id = 509_0002;
        enchantments.rows[0].enchantment_type = [5, 5, 77];
        enchantments.rows[0].effect_arg = [7, 4, 0];
        enchantments.rows[0].effect_points_min = [7, 9, 11];
        (properties, enchantments, synthetic_table(173))
    }

    #[test]
    fn catalogue_keeps_all_effect_slots_unknowns_and_compatibility_amounts() {
        let (properties, enchantments, spells) = catalogues();
        let (sql, ids) = catalogue_rows_sql(&properties, &enchantments, &spells).unwrap();
        let sql = sql.join("\n");
        assert_eq!(ids, BTreeSet::from([509_0001]));
        for row in [
            "(5090001,5090002,0,0,'of the Fixture')",
            "(1303040512,5090002,0,3,7,0,0)",
            "(1303040576,5090002,64,1,9,0,0)",
            "(1303040640,5090002,128,0,11,0,0)",
            "(1982720,7745,0,1,3,0,0)",
            "(1983488,7748,0,3,3,0,0)",
        ] {
            assert!(sql.contains(row), "missing {row}: {sql}");
        }
    }

    #[test]
    fn absent_linked_spells_retain_the_source_reference_as_unknown() {
        let (properties, mut enchantments, spells) = catalogues();
        enchantments.rows[0].enchantment_type = [1, 3, 0];
        enchantments.rows[0].effect_arg = [509_0098, 509_0099, 0];
        let (sql, _) = catalogue_rows_sql(&properties, &enchantments, &spells).unwrap();
        let sql = sql.join("\n");
        assert!(sql.contains("(1303040512,5090002,0,0,7,5090098,0)"));
        assert!(sql.contains("(1303040576,5090002,64,0,9,5090099,0)"));
    }

    #[test]
    fn empty_or_inconsistent_catalogues_produce_no_import_statements() {
        for missing in 0..3 {
            let (mut properties, mut enchantments, mut spells) = catalogues();
            match missing {
                0 => properties.rows.clear(),
                1 => enchantments.rows.clear(),
                _ => spells.rows.clear(),
            }
            assert!(catalogue_rows_sql(&properties, &enchantments, &spells).is_err());
        }
        let (mut properties, mut enchantments, spells) = catalogues();
        properties.rows[0].spell_item_enchantment[1] = 509_0099;
        assert!(catalogue_rows_sql(&properties, &enchantments, &spells).is_err());
        properties.rows[0].spell_item_enchantment[1] = 0;
        enchantments.rows[0].id.id = 7745;
        assert!(catalogue_rows_sql(&properties, &enchantments, &spells)
            .unwrap_err()
            .to_string()
            .contains("collides"));
    }

    #[test]
    fn equip_spell_expands_stats_and_retains_unmapped_effects() {
        let mut spells: Spell = synthetic_table(173);
        let spell = &mut spells.rows[0];
        spell.id.id = 509_0003;
        spell.equipped_item_class.id = u32::MAX;
        spell.effect = [6, 6, 2];
        spell.effect_aura[0] = 29u32.try_into().unwrap();
        spell.effect_misc_value[0] = u32::MAX;
        spell.effect_base_points = [6, 8, 10];
        let mapped = spell_effects(1, spell).unwrap();
        assert_eq!(mapped.len(), 7);
        assert_eq!(
            mapped.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 0, 0]
        );
        assert_eq!(
            mapped.iter().map(|e| e.amount).collect::<Vec<_>>(),
            [7, 7, 7, 7, 7, 9, 11]
        );
        assert_eq!(
            mapped.iter().map(|e| e.index).collect::<Vec<_>>(),
            [64, 65, 66, 67, 68, 72, 80]
        );
        assert!(mapped.iter().all(|e| e.spell_id == 509_0003));
    }

    #[test]
    fn decimal_weights_keep_exact_hundredths() {
        assert_eq!(decimal_weight("0.01").unwrap(), 1);
        assert_eq!(decimal_weight("99.52").unwrap(), 9952);
        assert_eq!(decimal_weight("131.01").unwrap(), 13101);
        assert_eq!(decimal_weight("1.2").unwrap(), 120);
        for value in ["0", "-1", "NaN", "1.001", "1e2"] {
            assert!(decimal_weight(value).is_err());
        }
    }
    #[test]
    fn primary_stats_and_multischool_resistance_keep_all_contributions() {
        assert_eq!(aura_kinds(29, 2), vec![(kind::STAMINA, 0)]);
        assert_eq!(
            aura_kinds(29, u32::MAX),
            vec![(1, 0), (2, 0), (3, 0), (4, 0), (5, 0)]
        );
        assert_eq!(
            aura_kinds(22, 0x7e),
            vec![(8, 2), (9, 4), (10, 8), (11, 16), (12, 32), (13, 64)]
        );
        assert_eq!(aura_kinds(13, 16), vec![(kind::SPELL_POWER, 16)]);
        assert_eq!(aura_kinds(999, 0), vec![(kind::UNKNOWN, 0)]);
    }
    #[test]
    fn authored_enchants_keep_existing_stored_meanings() {
        assert_eq!(
            kind::COMPATIBILITY_ENCHANTMENTS,
            [(7745, kind::STRENGTH, 3), (7748, kind::STAMINA, 3)]
        );
    }
}
