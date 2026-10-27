use spacetimedb::{ReducerContext, Table};

use lyracore_package_delta::{
    is_package_loot_id, FieldValue, Operation, PrimaryKey, Table as ClaimTable, TracedRow,
};

use crate::loot::{
    game_fishing_loot, game_gameobject_loot, game_pickpocket_loot, game_skinning_loot,
    GameFishingLoot, GameObjectLoot, GamePickpocketLoot, GameSkinningLoot,
};
use crate::{game_creature_template, game_gameobject_template, game_item_template};

use super::{as_bool, as_u32, check_insert_is_whole, UpdateTarget};

pub(super) fn check_references(ctx: &ReducerContext, rows: &[TracedRow]) -> Result<(), String> {
    for row in rows {
        let item_entry = final_u32(ctx, row, "item_entry")?;
        if item_entry == 0
            || ctx
                .db
                .game_item_template()
                .entry()
                .find(item_entry)
                .is_none()
        {
            return Err(missing_reference(row, "item_entry", item_entry));
        }

        match row.table() {
            ClaimTable::PickpocketLoot => {
                let creature_entry = final_u32(ctx, row, "creature_entry")?;
                if creature_entry == 0
                    || ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(creature_entry)
                        .is_none()
                {
                    return Err(missing_reference(row, "creature_entry", creature_entry));
                }
            }
            ClaimTable::GameobjectLoot => {
                let loot_id = final_u32(ctx, row, "loot_id")?;
                let owned = loot_id != 0
                    && ctx
                        .db
                        .game_gameobject_template()
                        .iter()
                        .any(|template| template.type_id == 3 && template.data1 == loot_id);
                if !owned {
                    return Err(missing_reference(row, "loot_id", loot_id));
                }
            }
            ClaimTable::SkinningLoot => {
                let skin_loot_id = final_u32(ctx, row, "skin_loot_id")?;
                let owned = skin_loot_id != 0
                    && ctx
                        .db
                        .game_creature_template()
                        .iter()
                        .any(|template| template.skin_loot_id == skin_loot_id);
                if !owned {
                    return Err(missing_reference(row, "skin_loot_id", skin_loot_id));
                }
            }
            ClaimTable::FishingLoot => {
                let zone_id = final_u32(ctx, row, "zone_id")?;
                if zone_id == 0 {
                    return Err(missing_reference(row, "zone_id", zone_id));
                }
            }
            other => unreachable!("loot reference check received {other}"),
        }
    }
    Ok(())
}

fn final_u32(ctx: &ReducerContext, row: &TracedRow, field: &str) -> Result<u32, String> {
    if let Some(FieldValue::U32(value)) = row.fields().get(field).map(|claimed| &claimed.value) {
        return Ok(*value);
    }

    let value = match row.table() {
        ClaimTable::PickpocketLoot => {
            ctx.db
                .game_pickpocket_loot()
                .id()
                .find(row.row_id())
                .map(|loot| match field {
                    "creature_entry" => loot.creature_entry,
                    "item_entry" => loot.item_entry,
                    _ => 0,
                })
        }
        ClaimTable::GameobjectLoot => {
            ctx.db
                .game_gameobject_loot()
                .id()
                .find(row.row_id())
                .map(|loot| match field {
                    "loot_id" => loot.loot_id,
                    "item_entry" => loot.item_entry,
                    _ => 0,
                })
        }
        ClaimTable::SkinningLoot => {
            ctx.db
                .game_skinning_loot()
                .id()
                .find(row.row_id())
                .map(|loot| match field {
                    "skin_loot_id" => loot.skin_loot_id,
                    "item_entry" => loot.item_entry,
                    _ => 0,
                })
        }
        ClaimTable::FishingLoot => ctx
            .db
            .game_fishing_loot()
            .id()
            .find(row.row_id())
            .map(|loot| match field {
                "zone_id" => loot.zone_id,
                "item_entry" => loot.item_entry,
                _ => 0,
            }),
        other => unreachable!("loot value lookup received {other}"),
    };

    value.ok_or_else(|| {
        format!(
            "`{}` row {} vanished during preflight",
            row.table(),
            row.key()
        )
    })
}

fn missing_reference(row: &TracedRow, field: &str, value: u32) -> String {
    format!(
        "`{}` row {} references missing {field} {value}",
        row.table(),
        row.key()
    )
}

pub(super) fn update_target(ctx: &ReducerContext, row: &TracedRow) -> UpdateTarget {
    if updates_an_uninvented_package_row(row) {
        return UpdateTarget::Uninvented;
    }
    let present = match row.table() {
        ClaimTable::PickpocketLoot => ctx
            .db
            .game_pickpocket_loot()
            .id()
            .find(row.row_id())
            .is_some(),
        ClaimTable::GameobjectLoot => ctx
            .db
            .game_gameobject_loot()
            .id()
            .find(row.row_id())
            .is_some(),
        ClaimTable::SkinningLoot => ctx
            .db
            .game_skinning_loot()
            .id()
            .find(row.row_id())
            .is_some(),
        ClaimTable::FishingLoot => ctx.db.game_fishing_loot().id().find(row.row_id()).is_some(),
        other => unreachable!(
            "`check_claims_belong_to` refuses a non-loot row before the loot family's dispatch \
             runs, found {other}"
        ),
    };
    if present {
        UpdateTarget::Present
    } else {
        UpdateTarget::Absent
    }
}

fn updates_an_uninvented_package_row(row: &TracedRow) -> bool {
    row.operation() == Operation::Update && is_package_loot_id(row.row_id())
}

pub(super) fn clear_package_range(ctx: &ReducerContext) {
    let pickpocket = ctx.db.game_pickpocket_loot();
    let stale_pickpocket: Vec<u64> = pickpocket
        .iter()
        .filter(|r| is_package_loot_id(r.id))
        .map(|r| r.id)
        .collect();
    for id in stale_pickpocket {
        pickpocket.id().delete(id);
    }

    let gameobject = ctx.db.game_gameobject_loot();
    let stale_gameobject: Vec<u64> = gameobject
        .iter()
        .filter(|r| is_package_loot_id(r.id))
        .map(|r| r.id)
        .collect();
    for id in stale_gameobject {
        gameobject.id().delete(id);
    }

    let skinning = ctx.db.game_skinning_loot();
    let stale_skinning: Vec<u64> = skinning
        .iter()
        .filter(|r| is_package_loot_id(r.id))
        .map(|r| r.id)
        .collect();
    for id in stale_skinning {
        skinning.id().delete(id);
    }

    let fishing = ctx.db.game_fishing_loot();
    let stale_fishing: Vec<u64> = fishing
        .iter()
        .filter(|r| is_package_loot_id(r.id))
        .map(|r| r.id)
        .collect();
    for id in stale_fishing {
        fishing.id().delete(id);
    }
}

pub(super) fn write_row(ctx: &ReducerContext, row: &TracedRow) -> Result<(), String> {
    match (row.table(), row.operation()) {
        (ClaimTable::PickpocketLoot, Operation::Insert) => {
            ctx.db
                .game_pickpocket_loot()
                .try_insert(built_pickpocket_loot(row)?)
                .map_err(|e| {
                    format!(
                        "`game_pickpocket_loot` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::PickpocketLoot, Operation::Update) => {
            let rows = ctx.db.game_pickpocket_loot();
            let mut loot = rows.id().find(row.row_id()).ok_or_else(|| {
                format!(
                    "`game_pickpocket_loot` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_pickpocket_loot_field(&mut loot, field, &claimed.value)?;
            }
            rows.id().update(loot);
        }
        (ClaimTable::GameobjectLoot, Operation::Insert) => {
            ctx.db
                .game_gameobject_loot()
                .try_insert(built_gameobject_loot(row)?)
                .map_err(|e| {
                    format!(
                        "`game_gameobject_loot` row {} did not insert: {e}",
                        row.key()
                    )
                })?;
        }
        (ClaimTable::GameobjectLoot, Operation::Update) => {
            let rows = ctx.db.game_gameobject_loot();
            let mut loot = rows.id().find(row.row_id()).ok_or_else(|| {
                format!(
                    "`game_gameobject_loot` row {} vanished mid-apply",
                    row.key()
                )
            })?;
            for (field, claimed) in row.fields() {
                apply_gameobject_loot_field(&mut loot, field, &claimed.value)?;
            }
            rows.id().update(loot);
        }
        (ClaimTable::SkinningLoot, Operation::Insert) => {
            ctx.db
                .game_skinning_loot()
                .try_insert(built_skinning_loot(row)?)
                .map_err(|e| {
                    format!("`game_skinning_loot` row {} did not insert: {e}", row.key())
                })?;
        }
        (ClaimTable::SkinningLoot, Operation::Update) => {
            let rows = ctx.db.game_skinning_loot();
            let mut loot = rows.id().find(row.row_id()).ok_or_else(|| {
                format!("`game_skinning_loot` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_skinning_loot_field(&mut loot, field, &claimed.value)?;
            }
            rows.id().update(loot);
        }
        (ClaimTable::FishingLoot, Operation::Insert) => {
            ctx.db
                .game_fishing_loot()
                .try_insert(built_fishing_loot(row)?)
                .map_err(|e| {
                    format!("`game_fishing_loot` row {} did not insert: {e}", row.key())
                })?;
        }
        (ClaimTable::FishingLoot, Operation::Update) => {
            let rows = ctx.db.game_fishing_loot();
            let mut loot = rows.id().find(row.row_id()).ok_or_else(|| {
                format!("`game_fishing_loot` row {} vanished mid-apply", row.key())
            })?;
            for (field, claimed) in row.fields() {
                apply_fishing_loot_field(&mut loot, field, &claimed.value)?;
            }
            rows.id().update(loot);
        }
        (other, _) => unreachable!(
            "`check_claims_belong_to` refuses a non-loot row before the loot family's dispatch \
             runs, found {other}"
        ),
    }
    Ok(())
}

fn id_of(row: &TracedRow) -> u64 {
    match row.key() {
        PrimaryKey::PickpocketLoot { id }
        | PrimaryKey::GameobjectLoot { id }
        | PrimaryKey::SkinningLoot { id }
        | PrimaryKey::FishingLoot { id } => id,
        _ => unreachable!(
            "`check_claims_belong_to` refuses a non-loot row before the loot family's dispatch runs"
        ),
    }
}

fn blank_pickpocket_loot(id: u64) -> GamePickpocketLoot {
    GamePickpocketLoot {
        id,
        creature_entry: 0,
        item_entry: 0,
        chance_bp: 0,
        count: 0,
        group_id: 0,
        quest_only: false,
    }
}

fn apply_pickpocket_loot_field(
    loot: &mut GamePickpocketLoot,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "creature_entry" => loot.creature_entry = as_u32(field, value)?,
        "item_entry" => loot.item_entry = as_u32(field, value)?,
        "chance_bp" => loot.chance_bp = as_u32(field, value)?,
        "count" => loot.count = as_u32(field, value)?,
        "group_id" => loot.group_id = as_u32(field, value)?,
        "quest_only" => loot.quest_only = as_bool(field, value)?,
        other => {
            return Err(format!(
                "`game_pickpocket_loot` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_pickpocket_loot(row: &TracedRow) -> Result<GamePickpocketLoot, String> {
    check_insert_is_whole(row)?;
    let mut loot = blank_pickpocket_loot(id_of(row));
    for (field, claimed) in row.fields() {
        apply_pickpocket_loot_field(&mut loot, field, &claimed.value)?;
    }
    Ok(loot)
}

fn blank_gameobject_loot(id: u64) -> GameObjectLoot {
    GameObjectLoot {
        id,
        loot_id: 0,
        item_entry: 0,
        chance_bp: 0,
        count: 0,
        group_id: 0,
        quest_only: false,
    }
}

fn apply_gameobject_loot_field(
    loot: &mut GameObjectLoot,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "loot_id" => loot.loot_id = as_u32(field, value)?,
        "item_entry" => loot.item_entry = as_u32(field, value)?,
        "chance_bp" => loot.chance_bp = as_u32(field, value)?,
        "count" => loot.count = as_u32(field, value)?,
        "group_id" => loot.group_id = as_u32(field, value)?,
        "quest_only" => loot.quest_only = as_bool(field, value)?,
        other => {
            return Err(format!(
                "`game_gameobject_loot` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_gameobject_loot(row: &TracedRow) -> Result<GameObjectLoot, String> {
    check_insert_is_whole(row)?;
    let mut loot = blank_gameobject_loot(id_of(row));
    for (field, claimed) in row.fields() {
        apply_gameobject_loot_field(&mut loot, field, &claimed.value)?;
    }
    Ok(loot)
}

fn blank_skinning_loot(id: u64) -> GameSkinningLoot {
    GameSkinningLoot {
        id,
        skin_loot_id: 0,
        item_entry: 0,
        chance_bp: 0,
        count: 0,
        group_id: 0,
    }
}

fn apply_skinning_loot_field(
    loot: &mut GameSkinningLoot,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "skin_loot_id" => loot.skin_loot_id = as_u32(field, value)?,
        "item_entry" => loot.item_entry = as_u32(field, value)?,
        "chance_bp" => loot.chance_bp = as_u32(field, value)?,
        "count" => loot.count = as_u32(field, value)?,
        "group_id" => loot.group_id = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_skinning_loot` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_skinning_loot(row: &TracedRow) -> Result<GameSkinningLoot, String> {
    check_insert_is_whole(row)?;
    let mut loot = blank_skinning_loot(id_of(row));
    for (field, claimed) in row.fields() {
        apply_skinning_loot_field(&mut loot, field, &claimed.value)?;
    }
    Ok(loot)
}

fn blank_fishing_loot(id: u64) -> GameFishingLoot {
    GameFishingLoot {
        id,
        zone_id: 0,
        item_entry: 0,
        chance_bp: 0,
        count: 0,
        group_id: 0,
    }
}

fn apply_fishing_loot_field(
    loot: &mut GameFishingLoot,
    field: &str,
    value: &FieldValue,
) -> Result<(), String> {
    match field {
        "zone_id" => loot.zone_id = as_u32(field, value)?,
        "item_entry" => loot.item_entry = as_u32(field, value)?,
        "chance_bp" => loot.chance_bp = as_u32(field, value)?,
        "count" => loot.count = as_u32(field, value)?,
        "group_id" => loot.group_id = as_u32(field, value)?,
        other => {
            return Err(format!(
                "`game_fishing_loot` has no claimable column `{other}`"
            ))
        }
    }
    Ok(())
}

fn built_fishing_loot(row: &TracedRow) -> Result<GameFishingLoot, String> {
    check_insert_is_whole(row)?;
    let mut loot = blank_fishing_loot(id_of(row));
    for (field, claimed) in row.fields() {
        apply_fishing_loot_field(&mut loot, field, &claimed.value)?;
    }
    Ok(loot)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{
        artifact, assert_each_column_sets_its_own_field, pickpocket_loot_claim, plan, PACKAGE_LOOT,
        REAL_PICKPOCKET_LOOT, WHOLE_PICKPOCKET_LOOT_ROW,
    };
    use super::*;

    #[test]
    fn tuning_a_package_loot_row_no_enabled_package_inserts_is_refused() {
        let plan = plan(&[artifact(
            "example.tuner",
            &pickpocket_loot_claim(
                PACKAGE_LOOT,
                "update",
                r#"{"chance_bp":{"type":"u32","value":2500}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn tuning_a_real_loot_row_is_not_an_uninvented_package_row() {
        let plan = plan(&[artifact(
            "example.tuner",
            &pickpocket_loot_claim(
                REAL_PICKPOCKET_LOOT,
                "update",
                r#"{"chance_bp":{"type":"u32","value":2500}}"#,
            ),
        )])
        .expect("plan builds");

        assert!(!updates_an_uninvented_package_row(&plan.rows[0]));
    }

    #[test]
    fn an_inserted_loot_row_carries_every_claimed_value_onto_the_row() {
        let plan = plan(&[artifact(
            "example.bolt",
            &pickpocket_loot_claim(PACKAGE_LOOT, "insert", WHOLE_PICKPOCKET_LOOT_ROW),
        )])
        .expect("plan builds");

        let loot = built_pickpocket_loot(&plan.rows[0]).expect("row builds");

        assert_eq!(loot.id, PACKAGE_LOOT);
        assert_eq!(loot.creature_entry, 6);
        assert_eq!(loot.item_entry, 25);
        assert_eq!(loot.chance_bp, 5000);
    }

    #[test]
    fn every_claimable_pickpocket_loot_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::PickpocketLoot,
            || blank_pickpocket_loot(PACKAGE_LOOT),
            apply_pickpocket_loot_field,
        );
    }

    #[test]
    fn every_claimable_gameobject_loot_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::GameobjectLoot,
            || blank_gameobject_loot(PACKAGE_LOOT),
            apply_gameobject_loot_field,
        );
    }

    #[test]
    fn every_claimable_skinning_loot_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::SkinningLoot,
            || blank_skinning_loot(PACKAGE_LOOT),
            apply_skinning_loot_field,
        );
    }

    #[test]
    fn every_claimable_fishing_loot_column_sets_its_own_field() {
        assert_each_column_sets_its_own_field(
            ClaimTable::FishingLoot,
            || blank_fishing_loot(PACKAGE_LOOT),
            apply_fishing_loot_field,
        );
    }

    #[test]
    fn a_column_claimed_as_the_wrong_type_is_refused_rather_than_narrowed() {
        let mut loot = blank_pickpocket_loot(PACKAGE_LOOT);

        let refusal = apply_pickpocket_loot_field(&mut loot, "chance_bp", &FieldValue::U8(9))
            .expect_err("the setter refuses");

        assert!(refusal.contains("chance_bp"), "{refusal}");
    }
}
