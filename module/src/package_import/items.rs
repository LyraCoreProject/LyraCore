//! The items Import Family's half of a Package Delta apply: the setters for `game_item_template`,
//! where a Package item lives, and how to find one row.
//!
//! The shared shell in the parent module decides everything a family does not: what the plan is,
//! what refuses it, the order of the durable pass, and the provenance. What is here is only what is
//! true of items.
//!
//! The Claim schema and the setter below are one contract; the test at the bottom fails if a
//! claimable column has no setter.
//!
//! Unlike the spell family, items own exactly one table, so `row.key()` reaching this module is
//! always `PrimaryKey::Item` — `check_claims_belong_to` refuses anything else before the family
//! dispatch in the parent module ever calls here.

use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{is_package_item_id, FieldValue, Operation, PrimaryKey, TracedRow};

use crate::items::{game_item_template, ItemTemplate};

use super::{as_bool, as_f32, as_i32, as_str, as_u32, as_u8, check_insert_is_whole, UpdateTarget};

/// What this shard holds for the row an `update` claim names.
pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let present = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry_of(row))
        .is_some();
    if present {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

/// True when a traced update would land on a Package-range row that no enabled Package invents.
///
/// The Package item range is cleared on every apply, so such a row is gone by the time the write
/// pass runs: the Package that owns it is not enabled, and the one tuning it is claiming a row that
/// does not exist. A base item is different — the base import puts it there.
fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_item_id(entry_of(row))
}

/// Removes every row a Package invented. A whole-table pass over `game_item_template`, once per
/// import, right after the base import rewrote it anyway.
pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let items = ctx.db.game_item_template();
    let stale: Vec<u32> = items
        .iter()
        .filter(|item| is_package_item_id(item.entry))
        .map(|item| item.entry)
        .collect();
    for entry in stale {
        items.entry().delete(entry);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match row.operation() {
        Operation::Insert => {
            ctx.db
                .game_item_template()
                .try_insert(built_item(row)?)
                .map_err(|e| {
                    format!("`game_item_template` row {} did not insert: {e}", row.key())
                })?;
        }
        Operation::Update => {
            let items = ctx.db.game_item_template();
            let mut item = items.entry().find(entry_of(row)).ok_or_else(|| {
                format!("`game_item_template` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_item_field(&mut item, field, &claimed.value)?;
            }
            items.entry().update(item);
        }
    }
    Ok(())
}

// ===========================================================================================
//  Row building — pure.
// ===========================================================================================

/// The `entry` an items-family row names. Total, not a `Result`: `check_claims_belong_to` has
/// already refused a foreign-family row before this module's dispatch runs, so `row.key()` is
/// always `PrimaryKey::Item` here.
fn entry_of(row: &TracedRow) -> u32 {
    let PrimaryKey::Item { entry } = row.key() else {
        unreachable!("`check_claims_belong_to` refuses a non-item row before the items family's dispatch runs")
    };
    entry
}

/// A Package item before any claim has been applied to it. Every claimable column is overwritten by
/// the insert that follows, so these values never survive; they exist because the row's shape has no
/// `Default`.
fn blank_item(entry: u32) -> ItemTemplate {
    ItemTemplate {
        entry,
        class: 0,
        subclass: 0,
        name: String::new(),
        display_id: 0,
        quality: 0,
        inventory_type: 0,
        item_level: 0,
        required_level: 0,
        max_durability: 0,
        buy_price: 0,
        sell_price: 0,
        max_stack: 0,
        damage_min: 0.0,
        damage_max: 0.0,
        delay_ms: 0,
        stat_strength: 0,
        stat_agility: 0,
        stat_stamina: 0,
        stat_intellect: 0,
        stat_spirit: 0,
        stat_crit: 0,
        stat_hit: 0,
        stat_armor: 0,
        block_value: 0,
        restores_power: false,
        spellid_1: 0,
        spelltrigger_1: 0,
        spellid_2: 0,
        spelltrigger_2: 0,
        container_slots: 0,
        sheath: 0,
        bonding: 0,
        holy_res: 0,
        fire_res: 0,
        nature_res: 0,
        frost_res: 0,
        shadow_res: 0,
        arcane_res: 0,
        spellid_3: 0,
        spelltrigger_3: 0,
        spellid_4: 0,
        spelltrigger_4: 0,
        spellid_5: 0,
        spelltrigger_5: 0,
        required_skill: 0,
        required_skill_rank: 0,
        required_reputation_faction: 0,
        required_reputation_rank: 0,
        max_count: 0,
        item_flags: 0,
        page_text: 0,
        start_quest: 0,
        bag_family: 0,
        buy_count: 0,
        food_type: 0,
        allowed_class: 0,
        allowed_race: 0,
    }
}

fn apply_item_field(
    item: &mut ItemTemplate,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "class" => item.class = as_u8(field, value)?,
        "subclass" => item.subclass = as_u8(field, value)?,
        "name" => item.name = as_str(field, value)?,
        "display_id" => item.display_id = as_u32(field, value)?,
        "quality" => item.quality = as_u8(field, value)?,
        "inventory_type" => item.inventory_type = as_u8(field, value)?,
        "item_level" => item.item_level = as_u8(field, value)?,
        "required_level" => item.required_level = as_u8(field, value)?,
        "max_durability" => item.max_durability = as_u32(field, value)?,
        "buy_price" => item.buy_price = as_u32(field, value)?,
        "sell_price" => item.sell_price = as_u32(field, value)?,
        "max_stack" => item.max_stack = as_u32(field, value)?,
        "damage_min" => item.damage_min = as_f32(field, value)?,
        "damage_max" => item.damage_max = as_f32(field, value)?,
        "delay_ms" => item.delay_ms = as_u32(field, value)?,
        "stat_strength" => item.stat_strength = as_i32(field, value)?,
        "stat_agility" => item.stat_agility = as_i32(field, value)?,
        "stat_stamina" => item.stat_stamina = as_i32(field, value)?,
        "stat_intellect" => item.stat_intellect = as_i32(field, value)?,
        "stat_spirit" => item.stat_spirit = as_i32(field, value)?,
        "stat_crit" => item.stat_crit = as_i32(field, value)?,
        "stat_hit" => item.stat_hit = as_i32(field, value)?,
        "stat_armor" => item.stat_armor = as_i32(field, value)?,
        "block_value" => item.block_value = as_i32(field, value)?,
        "restores_power" => item.restores_power = as_bool(field, value)?,
        "spellid_1" => item.spellid_1 = as_u32(field, value)?,
        "spelltrigger_1" => item.spelltrigger_1 = as_u8(field, value)?,
        "spellid_2" => item.spellid_2 = as_u32(field, value)?,
        "spelltrigger_2" => item.spelltrigger_2 = as_u8(field, value)?,
        "container_slots" => item.container_slots = as_u8(field, value)?,
        "sheath" => item.sheath = as_u8(field, value)?,
        "bonding" => item.bonding = as_u8(field, value)?,
        "holy_res" => item.holy_res = as_i32(field, value)?,
        "fire_res" => item.fire_res = as_i32(field, value)?,
        "nature_res" => item.nature_res = as_i32(field, value)?,
        "frost_res" => item.frost_res = as_i32(field, value)?,
        "shadow_res" => item.shadow_res = as_i32(field, value)?,
        "arcane_res" => item.arcane_res = as_i32(field, value)?,
        "spellid_3" => item.spellid_3 = as_u32(field, value)?,
        "spelltrigger_3" => item.spelltrigger_3 = as_u8(field, value)?,
        "spellid_4" => item.spellid_4 = as_u32(field, value)?,
        "spelltrigger_4" => item.spelltrigger_4 = as_u8(field, value)?,
        "spellid_5" => item.spellid_5 = as_u32(field, value)?,
        "spelltrigger_5" => item.spelltrigger_5 = as_u8(field, value)?,
        "required_skill" => item.required_skill = as_u32(field, value)?,
        "required_skill_rank" => item.required_skill_rank = as_u32(field, value)?,
        "required_reputation_faction" => item.required_reputation_faction = as_u32(field, value)?,
        "required_reputation_rank" => item.required_reputation_rank = as_u32(field, value)?,
        "max_count" => item.max_count = as_u32(field, value)?,
        "item_flags" => item.item_flags = as_u32(field, value)?,
        "page_text" => item.page_text = as_u32(field, value)?,
        "start_quest" => item.start_quest = as_u32(field, value)?,
        "bag_family" => item.bag_family = as_u32(field, value)?,
        "buy_count" => item.buy_count = as_u32(field, value)?,
        "food_type" => item.food_type = as_u8(field, value)?,
        "allowed_class" => item.allowed_class = as_u32(field, value)?,
        "allowed_race" => item.allowed_race = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_item_template` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_item(row: &TracedRow) -> Result<ItemTemplate, String> {
    check_insert_is_whole(row)?;
    let mut item = blank_item(entry_of(row));
    for (field, claimed) in row.fields() {
        apply_item_field(&mut item, field, &claimed.value)?;
    }
    Ok(item)
}

// ===========================================================================================
//  Tests — the pure half. Row WRITING needs a live ReducerContext, which a native test has no way
//  to build (same limit `import_meta`'s tests note); the wire suite covers that rung.
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, item_claim, plan, some_value, PACKAGE_ITEM, REAL_ITEM, WHOLE_ITEM_ROW,
    };
    use super::*;
    use lyracore_package_delta::Table as ClaimTable;

    /// The Package item range is cleared on every apply, so tuning a Package item nobody enables is
    /// a plan that names a row which will not exist.
    #[test]
    fn tuning_a_package_item_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &item_claim(
                PACKAGE_ITEM,
                "update",
                r#"{"buy_price":{"type":"u32","value":100}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_base_item_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &item_claim(
                REAL_ITEM,
                "update",
                r#"{"buy_price":{"type":"u32","value":100}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_item_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &item_claim(PACKAGE_ITEM, "insert", WHOLE_ITEM_ROW),
        )])
        .expect("plan builds");

        let item = built_item(&plan.rows[0]).expect("row builds");

        assert_eq!(item.entry, PACKAGE_ITEM);
        assert_eq!(item.name, "Kindled Blade");
        assert_eq!(item.buy_price, 500);
        assert_eq!(item.allowed_class, 1503);
        assert!((item.damage_max - 9.0).abs() < f32::EPSILON);
    }

    /// The Claim schema and the setter above are one contract. A column the schema declares but no
    /// setter names would fail an apply against a live shard and nowhere else.
    #[test]
    fn every_claimable_item_column_has_a_setter() {
        let mut item = blank_item(PACKAGE_ITEM);
        for column in ClaimTable::Item.columns() {
            apply_item_field(&mut item, column.name, &some_value(*column))
                .unwrap_or_else(|e| panic!("`game_item_template` column `{}`: {e}", column.name));
        }
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut item = blank_item(PACKAGE_ITEM);

        let refusal = apply_item_field(&mut item, "buy_price", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("buy_price"), "{refusal}");
    }
}
