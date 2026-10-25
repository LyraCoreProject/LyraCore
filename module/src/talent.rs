use lyracore_package_delta::packed_spell_effect_id;
use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::{game_character, game_spell, game_spell_effect, game_world_entity, Spell, SpellEffect};

#[table(
    accessor = game_talent,
    public,
    index(accessor = by_tree, btree(columns = [tree_id])),
    index(accessor = by_tab, btree(columns = [tab_id]))
)]
pub struct Talent {
    #[primary_key]
    pub talent_id: u32,
    pub name: String,
    pub tree_id: u8,
    pub tier: u8,
    pub column: u8,
    pub max_rank: u8,
    pub spell_id: u32,
    pub required_talent_id: u32,
    pub required_points_in_tree: u32,
    #[default(0)]
    pub grant_spell_id: u32,
    #[default(0)]
    pub tab_id: u32,
    #[default(0)]
    pub rank_spell_2: u32,
    #[default(0)]
    pub rank_spell_3: u32,
    #[default(0)]
    pub rank_spell_4: u32,
    #[default(0)]
    pub rank_spell_5: u32,
    #[default(0)]
    pub required_talent_rank: u8,
    #[default(0)]
    pub required_spell_id: u32,
}

#[table(
    accessor = game_talent_tab,
    public,
    index(accessor = by_order, btree(columns = [order_index]))
)]
pub struct TalentTab {
    #[primary_key]
    pub tab_id: u32,
    pub name: String,
    pub class_mask: u32,
    pub race_mask: u32,
    pub order_index: u8,
}

#[table(accessor = game_character_talent, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct CharacterTalent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub talent_id: u32,
    pub rank: u8,
}

crate::character_owned!(delete, fn sweep_delete_game_character_talent(ctx, character_guid) {
    let talents = ctx.db.game_character_talent();
    for r in talents.by_character().filter(&character_guid).collect::<Vec<_>>() {
        talents.id().delete(r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_character_talent(ctx, character_guid, io) {
    table = game_character_talent,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_character_talent(ctx, character_guid, identity) {
    let talents = ctx.db.game_character_talent();
    for mut r in talents.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            talents.id().update(r);
        }
    }
});

pub(crate) fn talent_points_available(level: u32, spent: u32) -> u32 {
    lyracore_shared::talent::points_available(level, spent)
}

pub(crate) fn validate_learn(
    current_rank: u8,
    max_rank: u8,
    available: u32,
    points_in_tree: u32,
    required_points_in_tree: u32,
    prereq_ok: bool,
) -> Result<u8, String> {
    if available == 0 {
        return Err("no talent points available".to_string());
    }
    if current_rank >= max_rank {
        return Err("talent already at max rank".to_string());
    }
    if points_in_tree < required_points_in_tree {
        return Err("tier locked: not enough points spent in this tree".to_string());
    }
    if !prereq_ok {
        return Err("prerequisite talent not learned".to_string());
    }
    Ok(current_rank + 1)
}

pub(crate) fn prereq_satisfied(
    required_talent_id: u32,
    learned_prereq_rank: u8,
    required_talent_rank: u8,
) -> bool {
    required_talent_id == 0 || learned_prereq_rank >= required_talent_rank.max(1)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
fn learned_rank(ctx: &ReducerContext, guid: u64, talent_id: u32) -> u8 {
    ctx.db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .find(|t| t.talent_id == talent_id)
        .map(|t| t.rank)
        .unwrap_or(0)
}

fn rank_in(learned: &[CharacterTalent], talent_id: u32) -> u8 {
    learned
        .iter()
        .find(|row| row.talent_id == talent_id)
        .map_or(0, |row| row.rank)
}

fn same_talent_tree(candidate: &Talent, selected: &Talent) -> bool {
    if selected.tab_id == 0 {
        candidate.tab_id == 0 && candidate.tree_id == selected.tree_id
    } else {
        candidate.tab_id == selected.tab_id
    }
}

fn points_in_tree(
    ctx: &ReducerContext,
    guid: u64,
    learned: &[CharacterTalent],
    selected: &Talent,
) -> u32 {
    let talents = ctx.db.game_talent();
    learned
        .iter()
        .filter_map(|t| match talents.talent_id().find(t.talent_id) {
            Some(def) => Some((def, t.rank)),
            None => {
                spacetimedb::log::info!(
                    "points_in_tree: character {guid} has orphaned talent {} (no game_talent row)",
                    t.talent_id
                );
                None
            }
        })
        .filter(|(def, _)| same_talent_tree(def, selected))
        .map(|(_, rank)| rank as u32)
        .sum()
}

fn mask_admits(mask: u32, id: u8) -> bool {
    id != 0 && (id as u32) <= u32::BITS && mask & (1u32 << (id - 1)) != 0
}

fn talent_tab_admits(tab: &TalentTab, race: u8, class: u8) -> bool {
    mask_admits(tab.class_mask, class) && (tab.race_mask == 0 || mask_admits(tab.race_mask, race))
}

fn admitted_talent_rank(
    ctx: &ReducerContext,
    guid: u64,
    entity: &crate::WorldEntity,
    learned: &[CharacterTalent],
    talent: &Talent,
) -> Result<u8, String> {
    let spent_in_tree = points_in_tree(ctx, guid, learned, talent);
    admitted_talent_rank_with_points(ctx, guid, entity, learned, talent, spent_in_tree)
}

fn admitted_talent_rank_with_points(
    ctx: &ReducerContext,
    guid: u64,
    entity: &crate::WorldEntity,
    learned: &[CharacterTalent],
    talent: &Talent,
    spent_in_tree: u32,
) -> Result<u8, String> {
    if talent.tab_id == 0 {
        if entity.class() != 1 || ctx.db.game_talent_tab().count() != 0 {
            return Err(format!(
                "demo talent {} is not available to class {} with the current catalogue",
                talent.talent_id,
                entity.class()
            ));
        }
    } else {
        let tab = ctx
            .db
            .game_talent_tab()
            .tab_id()
            .find(talent.tab_id)
            .ok_or_else(|| format!("unknown talent tab {}", talent.tab_id))?;
        if !talent_tab_admits(&tab, entity.race(), entity.class()) {
            return Err(format!(
                "talent {} is not available to race {} class {}",
                talent.talent_id,
                entity.race(),
                entity.class()
            ));
        }
    }
    if talent.required_spell_id != 0
        && !crate::spell::knows_spell(ctx, guid, talent.required_spell_id)
    {
        return Err(format!(
            "talent {} requires spell {}",
            talent.talent_id, talent.required_spell_id
        ));
    }

    let spent = learned.iter().map(|row| u32::from(row.rank)).sum();
    let available = talent_points_available(entity.level, spent);
    let current = rank_in(learned, talent.talent_id);
    let prereq_ok = prereq_satisfied(
        talent.required_talent_id,
        rank_in(learned, talent.required_talent_id),
        talent.required_talent_rank,
    );
    validate_learn(
        current,
        talent.max_rank,
        available,
        spent_in_tree,
        talent.required_points_in_tree,
        prereq_ok,
    )
}

pub(crate) fn do_learn_talent(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    talent_id: u32,
) -> Result<u8, crate::Refusal> {
    let talent = ctx
        .db
        .game_talent()
        .talent_id()
        .find(talent_id)
        .ok_or_else(|| format!("unknown talent {talent_id}"))?;
    let entity = crate::helpers::live_entity(ctx, guid)
        .map_err(|_| format!("no live entity for guid {guid} (must be in world to learn)"))?;
    let learned: Vec<_> = ctx
        .db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .collect();
    let next = admitted_talent_rank(ctx, guid, &entity, &learned, &talent)?;

    let talents = ctx.db.game_character_talent();
    if let Some(mut row) = talents
        .by_character()
        .filter(&guid)
        .find(|t| t.talent_id == talent_id)
    {
        row.rank = next;
        talents.id().update(row);
    } else {
        talents.insert(CharacterTalent {
            id: 0,
            character_guid: guid,
            owner_identity: owner,
            talent_id,
            rank: next,
        });
    }

    apply_talent_rank(ctx, &talent, guid, owner, next, entity.level as u8);
    crate::native::talent_points(ctx, guid, entity.level);
    Ok(next)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn reconcile_profile_talent(
    ctx: &ReducerContext,
    guid: u64,
    talent_id: u32,
) -> Result<bool, crate::actor::ActionRefusal> {
    let owner = crate::helpers::live_entity(ctx, guid)
        .map_err(|_| {
            crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::MissingActor,
                format!("no live entity for guid {guid} (must be in world to learn)"),
            )
        })?
        .owner_identity;
    let before = learned_rank(ctx, guid, talent_id);
    let after = do_learn_talent(ctx, guid, owner, talent_id).map_err(|detail| {
        crate::actor::ActionRefusal::new(crate::actor::ActionRefusalKind::Other, detail)
    })?;
    Ok(after != before)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
const PROFILE_TALENT_LIMIT: usize = 64;
#[cfg_attr(not(has_packages), allow(dead_code))]
const PROFILE_TALENT_TAB_LIMIT: usize = 16;

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn select_profile_talent(
    ctx: &ReducerContext,
    guid: u64,
    preferred_tree: u8,
) -> Result<Option<u32>, crate::actor::ActionRefusal> {
    let entity = crate::helpers::live_entity(ctx, guid).map_err(|_| {
        crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::MissingActor,
            format!("learner {guid} not in world"),
        )
    })?;
    let learned: Vec<_> = ctx
        .db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .take(PROFILE_TALENT_LIMIT + 1)
        .collect();
    if learned.len() > PROFILE_TALENT_LIMIT {
        return Err(crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::ProfileLimit,
            format!("character {guid} has more than {PROFILE_TALENT_LIMIT} learned talent rows"),
        ));
    }
    let imported = ctx.db.game_talent_tab().count() != 0;
    let tabs: Vec<_> = ctx
        .db
        .game_talent_tab()
        .by_order()
        .filter(&preferred_tree)
        .take(PROFILE_TALENT_TAB_LIMIT + 1)
        .collect();
    if tabs.len() > PROFILE_TALENT_TAB_LIMIT {
        return Err(crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::ProfileLimit,
            format!("talent order {preferred_tree} has more than {PROFILE_TALENT_TAB_LIMIT} tabs"),
        ));
    }
    let tab = tabs
        .into_iter()
        .filter(|tab| talent_tab_admits(tab, entity.race(), entity.class()))
        .map(|tab| tab.tab_id)
        .min();
    if imported && tab.is_none() {
        return Ok(None);
    }
    let mut candidates: Vec<_> = if let Some(tab_id) = tab {
        ctx.db
            .game_talent()
            .by_tab()
            .filter(&tab_id)
            .take(PROFILE_TALENT_LIMIT + 1)
            .collect()
    } else if entity.class() == 1 {
        let raw_tree: Vec<_> = ctx
            .db
            .game_talent()
            .by_tree()
            .filter(&preferred_tree)
            .take(PROFILE_TALENT_LIMIT + 1)
            .collect();
        if raw_tree.len() > PROFILE_TALENT_LIMIT {
            return Err(crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::ProfileLimit,
                format!(
                    "talent tree {preferred_tree} has more than {PROFILE_TALENT_LIMIT} raw rows"
                ),
            ));
        }
        raw_tree
            .into_iter()
            .filter(|talent| talent.tab_id == 0)
            .collect()
    } else {
        vec![]
    };
    if candidates.len() > PROFILE_TALENT_LIMIT {
        return Err(crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::ProfileLimit,
            format!("talent tab has more than {PROFILE_TALENT_LIMIT} rows"),
        ));
    }
    candidates.sort_by_key(|talent| (talent.tier, talent.column, talent.talent_id));
    let spent_in_tree = candidates
        .first()
        .map_or(0, |talent| points_in_tree(ctx, guid, &learned, talent));
    Ok(candidates.into_iter().find_map(|talent| {
        admitted_talent_rank_with_points(ctx, guid, &entity, &learned, &talent, spent_in_tree)
            .is_ok()
            .then_some(talent.talent_id)
    }))
}

pub(crate) fn talent_rank_spell(t: &Talent, rank: u8) -> u32 {
    pick_rank_spell(
        rank,
        t.spell_id,
        t.rank_spell_2,
        t.rank_spell_3,
        t.rank_spell_4,
        t.rank_spell_5,
    )
}

fn pick_rank_spell(rank: u8, r1: u32, r2: u32, r3: u32, r4: u32, r5: u32) -> u32 {
    lyracore_shared::talent::rank_spell(rank, [r1, r2, r3, r4, r5])
}

fn apply_talent_rank(
    ctx: &ReducerContext,
    talent: &Talent,
    guid: u64,
    owner: Identity,
    rank: u8,
    level: u8,
) {
    let rank_spell = talent_rank_spell(talent, rank);
    if rank_spell != 0 {
        if ctx.db.game_spell().spell_id().find(rank_spell).is_some() {
            crate::spell::learn_spell(ctx, guid, owner, rank_spell);
            let stack = if talent.tab_id != 0 { 1 } else { rank };
            crate::spell::apply_spell_auras(ctx, rank_spell, guid, level, stack);
            let mut revitalize = false;
            for prev_rank in 1..rank {
                let prev = talent_rank_spell(talent, prev_rank);
                if prev == 0 || prev == rank_spell {
                    continue;
                }
                crate::spell::forget_spell(ctx, guid, prev);
                revitalize |= crate::spell::strip_spell_auras(ctx, guid, prev);
            }
            if revitalize {
                crate::spell::recompute_vitals(ctx, guid);
            }
            crate::spell::recompute_sheet(ctx, guid);
        } else {
            spacetimedb::log::info!(
                "talent {} rank {rank}: rank spell {rank_spell} not imported yet",
                talent.talent_id
            );
        }
    }
    if talent.grant_spell_id != 0 && rank == 1 {
        crate::spell::learn_spell(ctx, guid, owner, talent.grant_spell_id);
    }
}

pub(crate) fn apply_learned_talents(ctx: &ReducerContext, guid: u64, owner: Identity, level: u32) {
    let talents = ctx.db.game_talent();
    let learned: Vec<(u32, u8)> = ctx
        .db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .filter(|t| t.rank >= 1)
        .map(|t| (t.talent_id, t.rank))
        .collect();
    for (talent_id, rank) in learned {
        let Some(def) = talents.talent_id().find(talent_id) else {
            spacetimedb::log::warn!("apply_learned_talents: character {guid} knows talent {talent_id} rank {rank} but no such talent exists (replaced by an import?) — skipping; respec clears it");
            continue;
        };
        apply_talent_rank(ctx, &def, guid, owner, rank, level as u8);
    }
}

pub(crate) use lyracore_shared::talent::respec_cost_copper;

pub(crate) fn do_reset_talents(
    ctx: &ReducerContext,
    character_guid: u64,
    trainer_guid: u64,
) -> Result<u32, String> {
    let entities = ctx.db.game_world_entity();
    let entity = entities
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    crate::trainer::validate_trainer_interaction(ctx, &entity, trainer_guid)?;

    let chars = ctx.db.game_character();
    let mut character = chars
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no character {character_guid}"))?;
    let cost = respec_cost_copper(character.respec_count);
    if entity.money < cost {
        return Err(format!("not enough money (need {cost})"));
    }

    let talents = ctx.db.game_talent();
    let learned = ctx.db.game_character_talent();
    let mut passives: Vec<u32> = Vec::new();
    let mut granted: Vec<u32> = Vec::new();
    for t in learned.by_character().filter(&character_guid) {
        let Some(def) = talents.talent_id().find(t.talent_id) else {
            continue;
        };
        for r in 1..=t.rank {
            let s = talent_rank_spell(&def, r);
            if s != 0 && !passives.contains(&s) {
                passives.push(s);
            }
        }
        if def.grant_spell_id != 0 {
            granted.push(def.grant_spell_id);
        }
    }

    for row in learned
        .by_character()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        learned.id().delete(row.id);
    }

    // Cancelling a passive Aura through the client request is forbidden.
    let mut revitalize = false;
    for spell_id in &passives {
        revitalize |= crate::spell::strip_spell_auras(ctx, character_guid, *spell_id);
    }
    if revitalize {
        crate::spell::recompute_vitals(ctx, character_guid);
    }
    crate::spell::recompute_sheet(ctx, character_guid);

    for spell_id in granted.iter().chain(passives.iter()) {
        crate::spell::forget_spell(ctx, character_guid, *spell_id);
    }

    if let Some(mut entity) = entities.guid().find(character_guid) {
        entity.money = entity.money.saturating_sub(cost);
        crate::native::talent_points(ctx, character_guid, entity.level);
        entity.update(ctx);
    }
    character.respec_count = character.respec_count.saturating_add(1);
    chars.guid().update(character);
    Ok(cost)
}

pub(crate) fn seed_talents(ctx: &ReducerContext) {
    const TALENT_DURATION_MS: u32 = u32::MAX;

    let talent = |talent_id: u32,
                  name: &str,
                  tree_id: u8,
                  tier: u8,
                  column: u8,
                  max_rank: u8,
                  spell_id: u32,
                  required_talent_id: u32,
                  required_points_in_tree: u32,
                  grant_spell_id: u32| {
        if ctx.db.game_talent().talent_id().find(talent_id).is_none() {
            ctx.db.game_talent().insert(Talent {
                talent_id,
                name: name.to_string(),
                tree_id,
                tier,
                column,
                max_rank,
                spell_id,
                required_talent_id,
                required_points_in_tree,
                grant_spell_id,
                tab_id: 0,
                rank_spell_2: 0,
                rank_spell_3: 0,
                rank_spell_4: 0,
                rank_spell_5: 0,
                required_talent_rank: 0,
                required_spell_id: 0,
            });
        }
    };
    let talent_spell =
        |spell_id: u32, name: &str, kind: u8, per_rank: i32, p0: i32, p0_kind: u8| {
            if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
                ctx.db.game_spell().insert(Spell {
                    spell_id,
                    family_name: 0,
                    family_flags: 0,
                    name: name.to_string(),
                    power_type: 0,
                    cost: 0,
                    cast_time_ms: 0,
                    gcd_ms: 0,
                    cooldown_ms: 0,
                    range_yd: 0,
                    duration_ms: TALENT_DURATION_MS,
                    school_mask: 1,
                    dispel_type: 0,
                    mechanic: 0,
                    max_stacks: 1,
                    aura_interrupt: 0,
                    attributes: crate::spell::SPELL_ATTR_PASSIVE,
                    spell_level: 0,
                    max_level: 0,
                    is_negative: false,
                    cast_flags: 0,
                    stances: 0,
                    proc_flags: 0,
                    proc_chance: 0,
                    proc_charges: 0,
                });
                let effect_index: u8 = 0;
                ctx.db.game_spell_effect().insert(SpellEffect {
                    id: packed_spell_effect_id(spell_id, effect_index),
                    spell_id,
                    effect_index,
                    kind,
                    base_points: per_rank,
                    die_sides: 0,
                    per_level: 0.0,
                    period_ms: 0,
                    target: 0,
                    radius_yd: 0.0,
                    chain_targets: 0,
                    trigger_spell: 0,
                    effect_mechanic: 0,
                    p0,
                    p0_kind,
                    p1: 0,
                    script_id: 0,
                    enters_combat: false,
                });
            }
        };

    #[allow(clippy::too_many_arguments)]
    let ability_spell = |spell_id: u32,
                         name: &str,
                         power_type: u8,
                         cost: u32,
                         range_yd: u32,
                         cooldown_ms: u32,
                         duration_ms: u32,
                         is_negative: bool,
                         kind: u8,
                         amount: i32,
                         target: u8,
                         p0: i32,
                         p0_kind: u8| {
        if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
            ctx.db.game_spell().insert(Spell {
                spell_id,
                family_name: 0,
                family_flags: 0,
                name: name.to_string(),
                power_type,
                cost,
                cast_time_ms: 0,
                gcd_ms: 1500,
                cooldown_ms,
                range_yd,
                duration_ms,
                school_mask: 1,
                dispel_type: 0,
                mechanic: 0,
                max_stacks: 1,
                aura_interrupt: 0,
                attributes: 0,
                spell_level: 0,
                max_level: 0,
                is_negative,
                cast_flags: 0,
                stances: 0,
                proc_flags: 0,
                proc_chance: 0,
                proc_charges: 0,
            });
            ctx.db.game_spell_effect().insert(SpellEffect {
                id: packed_spell_effect_id(spell_id, 0),
                spell_id,
                effect_index: 0,
                kind,
                base_points: amount,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target,
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0,
                p0_kind,
                p1: 0,
                script_id: 0,
                enters_combat: false,
            });
        }
    };

    talent(1, "Cruelty", 1, 0, 0, 5, 51000, 0, 0, 0);
    talent_spell(51000, "Cruelty", 0xA3, 100, 1, 5);

    talent(2, "Toughness", 2, 0, 0, 5, 51100, 0, 0, 0);
    talent_spell(51100, "Toughness", 0xA1, 240, 1, 2);

    talent(3, "Death Wish", 1, 0, 2, 1, 0, 0, 0, 51300);
    ability_spell(
        51300,
        "Death Wish",
        0,
        0,
        0,
        180000,
        30000,
        false,
        0xA3,
        50,
        0,
        0,
        5,
    );

    talent(4, "Deflection", 0, 0, 0, 5, 51200, 0, 0, 0);
    talent_spell(51200, "Deflection", 0xA3, 100, 6, 5);
    talent(5, "Shield Specialization", 2, 0, 1, 5, 51110, 0, 0, 0);
    talent_spell(51110, "Shield Specialization", 0xA3, 100, 8, 5);
    talent(6, "Anticipation", 2, 0, 2, 5, 51120, 0, 0, 0);
    talent_spell(51120, "Anticipation", 0xA3, 2, 9, 5);
    talent(7, "Defiance", 2, 0, 3, 3, 51130, 0, 0, 0);
    talent_spell(51130, "Defiance", 0xA3, 5, 10, 5);

    talent(8, "Mortal Strike", 0, 1, 0, 1, 0, 0, 5, 51210);
    ability_spell(
        51210,
        "Mortal Strike",
        1,
        30,
        5,
        6000,
        0,
        true,
        0x08,
        85,
        1,
        0,
        0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_spell_picks_the_imported_per_rank_spell_or_falls_back_to_base() {
        assert_eq!(pick_rank_spell(1, 20257, 20258, 20259, 20260, 20261), 20257);
        assert_eq!(pick_rank_spell(5, 20257, 20258, 20259, 20260, 20261), 20261);
        assert_eq!(pick_rank_spell(1, 26573, 0, 0, 0, 0), 26573);
        assert_eq!(pick_rank_spell(3, 500, 0, 0, 0, 0), 500);
        assert_eq!(pick_rank_spell(0, 20257, 20258, 20259, 20260, 20261), 0);
    }

    #[test]
    fn points_available_is_level_minus_nine_minus_spent() {
        assert_eq!(talent_points_available(1, 0), 0);
        assert_eq!(talent_points_available(9, 0), 0);
        assert_eq!(talent_points_available(10, 0), 1);
        assert_eq!(talent_points_available(60, 0), 51);
        assert_eq!(talent_points_available(20, 5), 6);
        assert_eq!(talent_points_available(10, 5), 0);
        assert_eq!(talent_points_available(20, 30), 0);
    }

    #[test]
    fn validate_learn_gates_in_order() {
        assert!(validate_learn(0, 5, 0, 0, 0, true).is_err());
        assert!(validate_learn(5, 5, 10, 0, 0, true).is_err());
        assert!(validate_learn(0, 5, 10, 5, 10, true).is_err());
        assert_eq!(validate_learn(0, 5, 10, 10, 10, true), Ok(1));
        assert!(validate_learn(0, 5, 10, 0, 0, false).is_err());
        assert_eq!(validate_learn(0, 5, 1, 0, 0, true), Ok(1));
        assert_eq!(validate_learn(4, 5, 1, 0, 0, true), Ok(5));
        assert_eq!(validate_learn(0, 5, 3, 0, 0, true), Ok(1));
    }

    #[test]
    fn prereq_satisfied_gates_on_required_rank() {
        assert!(prereq_satisfied(0, 0, 0));
        assert!(prereq_satisfied(0, 0, 5));
        assert!(!prereq_satisfied(4, 0, 0));
        assert!(prereq_satisfied(4, 1, 0));
        assert!(prereq_satisfied(4, 5, 0));
        assert!(!prereq_satisfied(4, 1, 2));
        assert!(prereq_satisfied(4, 2, 2));
        assert!(prereq_satisfied(4, 3, 2));
    }

    #[test]
    fn respec_cost_escalates_then_caps() {
        assert_eq!(respec_cost_copper(0), 10_000);
        assert_eq!(respec_cost_copper(1), 50_000);
        assert_eq!(respec_cost_copper(2), 100_000);
        assert_eq!(respec_cost_copper(3), 150_000);
        assert_eq!(respec_cost_copper(4), 200_000);
        assert_eq!(respec_cost_copper(9), 450_000);
        assert_eq!(respec_cost_copper(10), 500_000);
        assert_eq!(respec_cost_copper(11), 500_000);
        assert_eq!(respec_cost_copper(1000), 500_000);
        assert_eq!(respec_cost_copper(u32::MAX), 500_000);
    }
}
