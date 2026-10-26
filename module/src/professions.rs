use spacetimedb::ReducerContext;

use crate::game_creature_template;
use crate::game_fishing_loot;
use crate::game_item_template;
use crate::game_player_skill;
use crate::game_skinning_loot;
use crate::game_world_entity;
use crate::loot::LOOT_RANGE_SQ;

pub(crate) const LEATHER_ENTRY: u32 = 2318;
pub(crate) const LEATHER_COUNT: u32 = 1;

#[allow(clippy::too_many_arguments)]
pub(crate) fn can_skin(
    looter_dead: bool,
    corpse_is_player: bool,
    corpse_dead: bool,
    same_map: bool,
    dist_sq: f32,
    is_beast: bool,
    already_skinned: bool,
    learned_skinning: u32,
    creature_level: u32,
) -> Result<(), String> {
    if looter_dead {
        return Err("dead players cannot skin".to_string());
    }
    if corpse_is_player {
        return Err("cannot skin a player".to_string());
    }
    if !corpse_dead {
        return Err("target is not a corpse".to_string());
    }
    if !same_map {
        return Err("corpse on another map".to_string());
    }
    if dist_sq > LOOT_RANGE_SQ {
        return Err("corpse out of range".to_string());
    }
    if !is_beast {
        return Err("not a beast — cannot be skinned".to_string());
    }
    if already_skinned {
        return Err("already skinned".to_string());
    }
    if learned_skinning == 0 {
        return Err("you have not learned Skinning".to_string());
    }
    let required = creature_level.saturating_sub(1) * 10;
    if learned_skinning < required {
        return Err(format!(
            "your Skinning skill ({learned_skinning}) is too low — need {required}"
        ));
    }
    Ok(())
}

pub(crate) fn entry_is_beast(ctx: &ReducerContext, entry: u32) -> bool {
    ctx.db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_some_and(|t| t.creature_type == crate::spell::BEAST_TYPE)
}

pub(crate) fn skin_corpse(
    ctx: &ReducerContext,
    looter_guid: u64,
    corpse_guid: u64,
) -> Result<(), crate::Refusal> {
    let entities = ctx.db.game_world_entity();
    let looter = entities
        .guid()
        .find(looter_guid)
        .ok_or_else(|| "skinner not in world".to_string())?;
    let mut corpse = entities
        .guid()
        .find(corpse_guid)
        .ok_or_else(|| "no such corpse".to_string())?;
    crate::loot::corpse_access_gate(ctx, looter_guid, corpse_guid)?;

    let dist_sq = (corpse.x - looter.x).powi(2)
        + (corpse.y - looter.y).powi(2)
        + (corpse.z - looter.z).powi(2);
    let learned_skinning = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&looter_guid)
        .find(|s| s.skill_line == crate::skill::skill_line::SKINNING)
        .map(|s| s.current as u32)
        .unwrap_or(0);
    can_skin(
        looter.dead,
        corpse.is_player(),
        corpse.dead,
        corpse.map_id == looter.map_id && corpse.instance_id == looter.instance_id,
        dist_sq,
        entry_is_beast(ctx, corpse.entry),
        corpse.skinned,
        learned_skinning,
        corpse.level,
    )?;
    if !crate::loot::corpse_is_looted(ctx, corpse_guid, corpse.money) {
        return Err("corpse still has loot".into());
    }

    let skin_loot_id = ctx
        .db
        .game_creature_template()
        .entry()
        .find(corpse.entry)
        .map(|t| t.skin_loot_id)
        .unwrap_or(0);
    let rows: Vec<(u32, u32, u32, u32)> = if skin_loot_id != 0 {
        ctx.db
            .game_skinning_loot()
            .by_skin()
            .filter(&skin_loot_id)
            .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id))
            .collect()
    } else {
        Vec::new()
    };
    let winners = crate::loot::roll_loot_rows(ctx, rows);
    if winners.is_empty() {
        crate::items::grant_item(ctx, looter_guid, LEATHER_ENTRY, LEATHER_COUNT)?;
    } else {
        for (item_entry, count) in winners {
            crate::items::grant_item(ctx, looter_guid, item_entry, count)?;
        }
    }

    crate::skill::gain_profession_skill(ctx, looter_guid, crate::skill::skill_line::SKINNING, 1, 0);

    corpse.skinned = true;
    corpse.update(ctx);
    Ok(())
}

pub(crate) const FISH_POOL: &[u32] = &[6291, 6303, 6289];

pub(crate) fn pick_caught_fish(winner: Option<u32>, pool_idx: usize) -> u32 {
    winner.unwrap_or_else(|| FISH_POOL[pool_idx % FISH_POOL.len()])
}

pub(crate) fn apply_fish(ctx: &ReducerContext, guid: u64) -> Result<(), crate::Refusal> {
    let player =
        crate::helpers::live_entity(ctx, guid).map_err(|_| "fisher not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot fish".into());
    }
    crate::skill::learn_profession(
        ctx,
        guid,
        player.owner_identity,
        crate::skill::skill_line::FISHING,
        crate::skill::APPRENTICE_CAP,
    );
    let rows: Vec<(u32, u32, u32, u32)> =
        crate::terrain::zone_id_at(ctx, player.map_id, player.x, player.y)
            .map(|zone| {
                ctx.db
                    .game_fishing_loot()
                    .by_zone()
                    .filter(&zone)
                    .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id))
                    .collect()
            })
            .unwrap_or_default();
    let winner = crate::loot::roll_loot_rows(ctx, rows)
        .into_iter()
        .next()
        .map(|(item, _)| item);
    let fish = pick_caught_fish(winner, ctx.random::<usize>());
    crate::items::grant_item(ctx, guid, fish, 1)?;
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::FISHING, 1, 0);
    Ok(())
}

pub(crate) const DISENCHANT_MATS_ENTRY: u32 = 10940;
pub(crate) const DISENCHANT_MATS_COUNT: u32 = 1;

fn require_gear(ctx: &ReducerContext, entry: u32) -> Result<(), String> {
    const ITEM_CLASS_WEAPON: u8 = 2;
    const ITEM_CLASS_ARMOR: u8 = 4;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry)
        .ok_or_else(|| format!("no template for item {entry}"))?;
    if tmpl.class != ITEM_CLASS_WEAPON && tmpl.class != ITEM_CLASS_ARMOR {
        return Err(format!(
            "item {entry} is not enchantable (weapon/armor only)"
        ));
    }
    Ok(())
}

pub(crate) fn apply_disenchant(
    ctx: &ReducerContext,
    guid: u64,
    slot: u8,
) -> Result<(), crate::Refusal> {
    let player =
        crate::helpers::live_entity(ctx, guid).map_err(|_| "enchanter not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot disenchant".into());
    }
    let inst = crate::items::item_in_slot(ctx, guid, slot)
        .ok_or_else(|| format!("no item in slot {slot}"))?;
    if crate::trade::item_is_offered(ctx, guid, inst.guid) {
        return Err(crate::items::refused(lyracore_shared::item::ItemRefusal::NotRightNow).into());
    }
    require_gear(ctx, inst.entry)?;
    crate::skill::learn_profession(
        ctx,
        guid,
        player.owner_identity,
        crate::skill::skill_line::ENCHANTING,
        crate::skill::APPRENTICE_CAP,
    );
    // Grant materials before consuming the item so a full bag destroys nothing.
    crate::items::grant_item(ctx, guid, DISENCHANT_MATS_ENTRY, DISENCHANT_MATS_COUNT)?;
    crate::ItemInstance::delete(ctx, inst.guid);
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::ENCHANTING, 1, 0);
    if slot <= crate::items::equip_slot::END {
        crate::spell::recompute_vitals(ctx, guid);
        crate::spell::recompute_sheet(ctx, guid);
    }
    Ok(())
}

pub(crate) fn apply_enchant_item(
    ctx: &ReducerContext,
    guid: u64,
    target_slot: u8,
    enchant_id: u32,
) -> Result<(), crate::Refusal> {
    let player =
        crate::helpers::live_entity(ctx, guid).map_err(|_| "enchanter not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot enchant".into());
    }
    if !crate::items::is_known_enchant(ctx, enchant_id) {
        return Err(format!("unknown enchant {enchant_id}").into());
    }
    let mut inst = crate::items::item_in_slot(ctx, guid, target_slot)
        .ok_or_else(|| format!("no item in slot {target_slot}"))?;
    require_gear(ctx, inst.entry)?;
    crate::items::remove_items(ctx, guid, DISENCHANT_MATS_ENTRY, DISENCHANT_MATS_COUNT)?;

    inst.enchant_id = enchant_id;
    inst.update(ctx);
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::ENCHANTING, 1, 0);
    if target_slot <= crate::items::equip_slot::END {
        crate::spell::recompute_vitals(ctx, guid);
        crate::spell::recompute_sheet(ctx, guid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_gate_admits_only_a_fresh_in_range_beast_corpse() {
        let learned: u32 = 1;
        let lvl: u32 = 1;

        assert!(can_skin(false, false, true, true, 0.0, true, false, learned, lvl).is_ok());
        assert!(can_skin(
            false,
            false,
            true,
            true,
            LOOT_RANGE_SQ,
            true,
            false,
            learned,
            lvl
        )
        .is_ok());

        assert_eq!(
            can_skin(true, false, true, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "dead players cannot skin"
        );
        assert_eq!(
            can_skin(false, true, true, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "cannot skin a player"
        );
        assert_eq!(
            can_skin(false, false, false, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "target is not a corpse"
        );
        assert_eq!(
            can_skin(false, false, true, false, 0.0, true, false, learned, lvl).unwrap_err(),
            "corpse on another map"
        );
        assert_eq!(
            can_skin(
                false,
                false,
                true,
                true,
                LOOT_RANGE_SQ + 1.0,
                true,
                false,
                learned,
                lvl
            )
            .unwrap_err(),
            "corpse out of range"
        );
        assert_eq!(
            can_skin(false, false, true, true, 0.0, false, false, learned, lvl).unwrap_err(),
            "not a beast — cannot be skinned"
        );
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, true, learned, lvl).unwrap_err(),
            "already skinned"
        );
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 0, lvl).unwrap_err(),
            "you have not learned Skinning"
        );
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 5, 2).unwrap_err(),
            "your Skinning skill (5) is too low — need 10"
        );
        assert!(can_skin(false, false, true, true, 0.0, true, false, 10, 2).is_ok());
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 19, 3).unwrap_err(),
            "your Skinning skill (19) is too low — need 20"
        );
        assert!(can_skin(false, false, true, true, 0.0, true, false, 20, 3).is_ok());
    }

    #[test]
    fn skin_leather_constants_are_the_real_light_leather_2318_flat_one() {
        assert_eq!(LEATHER_ENTRY, 2318);
        assert_eq!(LEATHER_COUNT, 1);
    }

    #[test]
    fn fish_pool_is_nonempty_and_lists_the_real_low_level_fish_entries() {
        assert!(
            !FISH_POOL.is_empty(),
            "fishing must have a pool to roll (an empty pool divides by zero)"
        );
        assert_eq!(
            FISH_POOL,
            &[6291, 6303, 6289],
            "the three real low-level fish entries"
        );
    }

    #[test]
    fn pick_caught_fish_prefers_the_winner_else_floors_to_the_pool() {
        assert_eq!(
            pick_caught_fish(Some(6291), 0),
            6291,
            "a real winner is never overridden by the pool"
        );
        assert_eq!(pick_caught_fish(None, 0), FISH_POOL[0]);
        assert_eq!(
            pick_caught_fish(None, FISH_POOL.len()),
            FISH_POOL[0],
            "wraps via %, never panics OOB"
        );
        assert_eq!(pick_caught_fish(None, 1), FISH_POOL[1]);
    }

    #[test]
    fn disenchant_mats_are_the_real_strange_dust_entry() {
        assert_eq!(
            DISENCHANT_MATS_ENTRY, 10940,
            "Strange Dust (real imported enchanting reagent)"
        );
        assert_eq!(
            DISENCHANT_MATS_COUNT, 1,
            "the alpha mints a flat 1x Strange Dust"
        );
    }
}
