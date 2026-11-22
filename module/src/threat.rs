use spacetimedb::{log, table, ReducerContext, Table};

#[cfg(feature = "debug_reducers")]
use spacetimedb::reducer;

pub const TAUNT_FORCED_MS: u64 = 3000;

#[table(accessor = game_taunt_lock)]
pub struct TauntLock {
    #[primary_key]
    pub creature_guid: u64,
    pub forced_target_guid: u64,
    pub forced_until_ms: u64,
}

use crate::spell::tables::{game_spell_threat, SpellThreat};
use crate::{game_melee_attack, game_spell, game_spell_effect, game_world_entity, WorldEntity};
use lyracore_shared::spell::taxonomy::{A_CONTROL, M_FEAR, M_POLY};

#[table(
    accessor = game_threat,
    public,
    index(accessor = by_creature, btree(columns = [creature_guid])),

    index(accessor = by_source, btree(columns = [source_guid]))
)]
pub struct ThreatEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub source_guid: u64,
    pub threat: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleSelectedThreat {
    pub creature_guid: u64,
    pub source_guid: u64,
    pub percent: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaleAllThreat {
    pub creature_guid: u64,
    pub percent: i32,
}

// vmangos Spell.cpp heal threat per point: a Paladin's direct heal draws half the usual.
const HEAL_THREAT_PER_POINT: f32 = 0.5;
const PALADIN_HEAL_THREAT_PER_POINT: f32 = 0.25;
const CLASS_PALADIN: u8 = 2;

/// Every effect index of a spell, for a target that all of its effects reach.
pub(crate) const EVERY_EFFECT: u8 = 0b111;

#[derive(Clone, Copy)]
pub(crate) enum HealDelivery {
    Direct,
    Periodic,
}

fn spell_threat(ctx: &ReducerContext, spell_id: u32) -> Option<SpellThreat> {
    let rows = ctx.db.game_spell_threat();
    let first = crate::spell::first_rank(ctx, spell_id);
    rank_threat(spell_id, first, rows.spell_id().find(spell_id), || {
        rows.spell_id().find(first)
    })
}

// vmangos SpellRankHelper refuses a later rank's own row that adds no flat threat.
fn rank_threat(
    spell_id: u32,
    first_rank: u32,
    own: Option<SpellThreat>,
    first_rank_row: impl FnOnce() -> Option<SpellThreat>,
) -> Option<SpellThreat> {
    match own {
        Some(row) if row.threat != 0 || first_rank == spell_id => Some(row),
        _ => first_rank_row(),
    }
}

fn threat_multiplier(ctx: &ReducerContext, spell_id: u32) -> f32 {
    if spell_id == 0 {
        return 1.0;
    }
    spell_threat(ctx, spell_id).map_or(1.0, |row| row.multiplier)
}

fn scaled_threat(amount: u32, multiplier: f32) -> f32 {
    amount as f32 * multiplier
}

/// `spell_id` zero is a white swing or a source with no spell.
pub(crate) fn threat_from_damage(ctx: &ReducerContext, spell_id: u32, damage: u32) -> i64 {
    scaled_threat(damage, threat_multiplier(ctx, spell_id)) as i64
}

fn heal_threat_per_point(healer_class: u8, delivery: HealDelivery) -> f32 {
    match (delivery, healer_class) {
        (HealDelivery::Direct, CLASS_PALADIN) => PALADIN_HEAL_THREAT_PER_POINT,
        _ => HEAL_THREAT_PER_POINT,
    }
}

fn threat_share(threat: f32, creatures: usize) -> i64 {
    if creatures == 0 {
        return 0;
    }
    (threat / creatures as f32) as i64
}

pub(crate) fn threat_after_reduction(current: i64, amount: i64) -> i64 {
    if amount <= 0 {
        return current;
    }
    (current - amount).max(0)
}

pub(crate) fn threat_after_percent_scale(current: i64, percent: i32) -> i64 {
    let multiplier = i64::from(percent).saturating_add(100).max(0);
    let scaled = i128::from(current.max(0)) * i128::from(multiplier) / 100;
    scaled.min(i128::from(i64::MAX)) as i64
}

pub(crate) fn add_threat(ctx: &ReducerContext, creature_guid: u64, source_guid: u64, amount: i64) {
    if amount < 0 {
        log::warn!("add_threat: negative amount {amount} for creature {creature_guid} source {source_guid} (upstream bug?)");
        return;
    }
    let threat_mult = (100
        + crate::modifiers::combat_field_bonus(ctx, source_guid, crate::spell::COMBAT_THREAT)
            as i64)
        .max(0);
    let amount = amount * threat_mult / 100;
    if amount > 0 {
        crate::loot::tag::record_first_threat(ctx, creature_guid, source_guid);
    }
    let threats = ctx.db.game_threat();
    if let Some(mut row) = threats
        .by_creature()
        .filter(&creature_guid)
        .find(|r| r.source_guid == source_guid)
    {
        row.threat = row.threat.saturating_add(amount);
        threats.id().update(row);
    } else {
        threats.insert(ThreatEntry {
            id: 0,
            creature_guid,
            source_guid,
            threat: amount,
        });
    }
}

pub(crate) fn scale_selected_threat(ctx: &ReducerContext, operation: ScaleSelectedThreat) {
    let threats = ctx.db.game_threat();
    if let Some(mut row) = threats
        .by_creature()
        .filter(&operation.creature_guid)
        .find(|row| row.source_guid == operation.source_guid)
    {
        row.threat = threat_after_percent_scale(row.threat, operation.percent);
        threats.id().update(row);
    }
}

pub(crate) fn scale_all_threat(ctx: &ReducerContext, operation: ScaleAllThreat) {
    let threats = ctx.db.game_threat();
    let rows: Vec<u64> = threats
        .by_creature()
        .filter(&operation.creature_guid)
        .map(|row| row.id)
        .collect();
    for id in rows {
        if let Some(mut row) = threats.id().find(id) {
            row.threat = threat_after_percent_scale(row.threat, operation.percent);
            threats.id().update(row);
        }
    }
}

pub(crate) fn threat_of(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) -> i64 {
    ctx.db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .find(|r| r.source_guid == source_guid)
        .map(|r| r.threat)
        .unwrap_or(0)
}

/// Orders `(source_guid, threat)` pairs highest threat first; the lower guid wins a tie.
pub(crate) fn rank(mut sources: Vec<(u64, i64)>) -> Vec<(u64, i64)> {
    sources.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sources
}

/// The creature's threat from living sources on its map and Instance, ranked.
pub(crate) fn ranked(ctx: &ReducerContext, creature_guid: u64) -> Vec<(u64, i64)> {
    let entities = ctx.db.game_world_entity();
    let Some(creature) = entities.guid().find(creature_guid) else {
        return Vec::new();
    };
    rank(
        ctx.db
            .game_threat()
            .by_creature()
            .filter(&creature_guid)
            .filter(|row| {
                entities.guid().find(row.source_guid).is_some_and(|source| {
                    !source.dead
                        && (source.map_id, source.instance_id)
                            == (creature.map_id, creature.instance_id)
                })
            })
            .map(|row| (row.source_guid, row.threat))
            .collect(),
    )
}

/// Where a threat source stands in victim selection, as vmangos `selectNextVictim` ranks it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceStanding {
    Free,
    LowPriority,
    Unattackable,
}

/// vmangos `IsValidAttackTarget` and `IsSecondaryThreatTarget`. The Module summons no Totems.
pub(crate) fn source_standing(
    ctx: &ReducerContext,
    creature: &WorldEntity,
    source: &WorldEntity,
) -> SourceStanding {
    if !crate::combat::may_harm(ctx, creature, source) {
        SourceStanding::Unattackable
    } else if holds_low_priority_aura(ctx, source.guid) {
        SourceStanding::LowPriority
    } else {
        SourceStanding::Free
    }
}

fn holds_low_priority_aura(ctx: &ReducerContext, guid: u64) -> bool {
    let spells = ctx.db.game_spell();
    crate::spell::auras_on(ctx, guid).any(|aura| {
        spells.spell_id().find(aura.spell_id).is_some_and(|spell| {
            makes_low_priority(
                spell.negative(),
                spell.aura_interrupt,
                aura.eff_kind,
                aura.eff_p0,
            )
        })
    })
}

// vmangos `IsTargetSecondaryThreatFocus`; confuse, pacify and charm import as one control class.
fn makes_low_priority(harmful: bool, aura_interrupt: u16, kind: u8, control: i32) -> bool {
    harmful
        && (crate::spell::breaks_on_damage(aura_interrupt)
            || (kind == A_CONTROL && matches!(control, M_FEAR | M_POLY)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Contender {
    pub source_guid: u64,
    pub threat: i64,
    pub in_melee_reach: bool,
    pub standing: SourceStanding,
}

/// vmangos `selectNextVictim` over `ranked`, highest threat first. Among free contenders, keep
/// `current` unless one has more than 110% of its threat in melee reach or more than 130% outside
/// it. With no free contender, take the top low-priority one. Never take an unattackable one.
/// vmangos tests the margin before the victim's own priority; a low-priority victim yields here.
pub(crate) fn select_victim(current: u64, ranked: &[Contender]) -> Option<u64> {
    let free: Vec<Contender> = ranked
        .iter()
        .filter(|c| c.standing == SourceStanding::Free)
        .copied()
        .collect();
    if free.is_empty() {
        return ranked
            .iter()
            .find(|c| c.standing == SourceStanding::LowPriority)
            .map(|c| c.source_guid);
    }
    keep_or_overtake(current, &free)
}

fn keep_or_overtake(current: u64, ranked: &[Contender]) -> Option<u64> {
    let Some(current_threat) = ranked
        .iter()
        .find(|c| c.source_guid == current)
        .map(|c| i128::from(c.threat))
    else {
        return ranked.first().map(|c| c.source_guid);
    };
    for contender in ranked {
        let threat = i128::from(contender.threat);
        if contender.source_guid == current || threat * 10 <= current_threat * 11 {
            return Some(current);
        }
        if threat * 10 > current_threat * 13 || contender.in_melee_reach {
            return Some(contender.source_guid);
        }
    }
    None
}

pub(crate) fn top_threat_target(ctx: &ReducerContext, creature_guid: u64) -> Option<u64> {
    ranked(ctx, creature_guid)
        .first()
        .map(|&(source, _)| source)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PartyThreatFacts {
    pub top_target_guid: Option<u64>,
    pub party_has_threat: bool,
}

pub(crate) fn party_threat_facts(
    ctx: &ReducerContext,
    creature_guid: u64,
    party_guids: &std::collections::BTreeSet<u64>,
    limit: usize,
) -> Result<PartyThreatFacts, ()> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .ok_or(())?;
    let rows = ctx
        .db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .take(limit.saturating_add(1))
        .count();
    if rows > limit {
        return Err(());
    }
    let ranked = ranked(ctx, creature_guid);
    Ok(PartyThreatFacts {
        top_target_guid: ranked.first().map(|&(source, _)| source),
        party_has_threat: ranked
            .iter()
            .any(|(source, _)| party_guids.contains(source)),
    })
}

pub(crate) fn taunt(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) {
    let max = ctx
        .db
        .game_threat()
        .by_creature()
        .filter(&creature_guid)
        .map(|r| r.threat)
        .max()
        .unwrap_or(0);
    // The forced-target window still holds taunt when threat cannot exceed i64::MAX.
    let target = max.saturating_add(1);
    let current = threat_of(ctx, creature_guid, source_guid);
    if current < target {
        add_threat(ctx, creature_guid, source_guid, target - current);
    }
    let now_ms = crate::time::now_ms(ctx);
    let locks = ctx.db.game_taunt_lock();
    let row = TauntLock {
        creature_guid,
        forced_target_guid: source_guid,
        forced_until_ms: now_ms + TAUNT_FORCED_MS,
    };
    if locks.creature_guid().find(creature_guid).is_some() {
        locks.creature_guid().update(row);
    } else {
        locks.insert(row);
    }
}

pub(crate) fn forced_target(ctx: &ReducerContext, creature_guid: u64, now_ms: u64) -> Option<u64> {
    let locks = ctx.db.game_taunt_lock();
    let lock = locks.creature_guid().find(creature_guid)?;
    let entities = ctx.db.game_world_entity();
    let taunter_valid = entities.guid().find(creature_guid).is_some_and(|c| {
        entities
            .guid()
            .find(lock.forced_target_guid)
            .is_some_and(|t| !t.dead && t.map_id == c.map_id && t.instance_id == c.instance_id)
    });
    if now_ms < lock.forced_until_ms && taunter_valid {
        Some(lock.forced_target_guid)
    } else {
        locks.creature_guid().delete(creature_guid);
        None
    }
}

pub(crate) fn clear_taunt_lock(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_taunt_lock()
        .creature_guid()
        .delete(creature_guid);
}

/// A direct heal from no spell, such as a Runtime Script's.
pub(crate) fn add_heal_threat(
    ctx: &ReducerContext,
    healer_guid: u64,
    healed_guid: u64,
    effective_heal: u32,
) {
    add_spell_heal_threat(
        ctx,
        healer_guid,
        healed_guid,
        effective_heal,
        0,
        HealDelivery::Direct,
    );
}

pub(crate) fn add_spell_heal_threat(
    ctx: &ReducerContext,
    healer_guid: u64,
    healed_guid: u64,
    effective_heal: u32,
    spell_id: u32,
    delivery: HealDelivery,
) {
    if effective_heal == 0 {
        return;
    }
    let Some(healer) = ctx.db.game_world_entity().guid().find(healer_guid) else {
        return;
    };
    let threat = scaled_threat(
        effective_heal,
        heal_threat_per_point(healer.class(), delivery) * threat_multiplier(ctx, spell_id),
    );
    add_assist_threat(ctx, healer_guid, healed_guid, threat);
}

/// vmangos `HostileRefManager::threatAssist`: the creatures fighting `helped_guid` share `threat`.
fn add_assist_threat(ctx: &ReducerContext, helper_guid: u64, helped_guid: u64, threat: f32) {
    let entities = ctx.db.game_world_entity();
    if !entities
        .guid()
        .find(helper_guid)
        .is_some_and(|helper| helper.is_player())
    {
        return;
    }
    let mut creatures: Vec<u64> = ctx
        .db
        .game_melee_attack()
        .by_target()
        .filter(&helped_guid)
        .map(|a| a.attacker_guid)
        .chain(
            ctx.db
                .game_threat()
                .by_source()
                .filter(&helped_guid)
                .map(|entry| entry.creature_guid),
        )
        .filter(|g| {
            entities
                .guid()
                .find(g)
                .map(|e| !e.is_player())
                .unwrap_or(false)
        })
        .collect();
    creatures.sort_unstable();
    creatures.dedup();
    let each = threat_share(threat, creatures.len());
    for creature in creatures {
        add_threat(ctx, creature, helper_guid, each);
    }
}

/// vmangos `Spell::HandleThreatSpells`; `landed` pairs each target with its landed effect mask.
pub(crate) fn add_table_threat(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    landed: &[(u64, u8)],
) {
    let Some(row) = spell_threat(ctx, spell_id).filter(|row| row.threat != 0) else {
        return;
    };
    let Some(spell) = ctx.db.game_spell().spell_id().find(spell_id) else {
        return;
    };
    let effects: Vec<_> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .collect();
    let harmful = effects
        .iter()
        .filter(|effect| crate::spell::effect_is_harmful(&spell, effect))
        .count();
    if harmful != 0 && harmful != effects.len() {
        return;
    }
    let entities = ctx.db.game_world_entity();
    for &(target_guid, effect_mask) in landed {
        if (effect_mask & !row.inverse_effect_mask) == 0 {
            continue;
        }
        let Some(target) = entities.guid().find(target_guid).filter(|t| !t.dead) else {
            continue;
        };
        if harmful == 0 {
            add_assist_threat(ctx, caster_guid, target_guid, f32::from(row.threat));
        } else if !target.is_player()
            && crate::loot::tag::controlling_character(ctx, caster_guid).is_some()
        {
            add_threat(ctx, target_guid, caster_guid, i64::from(row.threat));
        }
    }
}

pub(crate) fn clear_for_unit(ctx: &ReducerContext, guid: u64) {
    let threats = ctx.db.game_threat();
    let stale: Vec<u64> = threats
        .by_creature()
        .filter(&guid)
        .chain(threats.by_source().filter(&guid))
        .map(|r| r.id)
        .collect();
    for id in stale {
        threats.id().delete(id);
    }
}

pub(crate) fn clear_all(ctx: &ReducerContext) {
    let threats = ctx.db.game_threat();
    let all: Vec<u64> = threats.iter().map(|r| r.id).collect();
    for id in all {
        threats.id().delete(id);
    }
}

pub(crate) fn reduce_threat_from_source(ctx: &ReducerContext, source_guid: u64, amount: i64) {
    if amount <= 0 {
        return;
    }
    let threats = ctx.db.game_threat();
    let hits: Vec<u64> = threats
        .by_source()
        .filter(&source_guid)
        .map(|r| r.id)
        .collect();
    for id in hits {
        if let Some(mut row) = threats.id().find(id) {
            row.threat = threat_after_reduction(row.threat, amount);
            threats.id().update(row);
        }
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_add_threat(
    ctx: &ReducerContext,
    creature_guid: u64,
    source_guid: u64,
    amount: i64,
) -> Result<(), String> {
    add_threat(ctx, creature_guid, source_guid, amount);
    Ok(())
}

pub(crate) fn engage_without_threat(ctx: &ReducerContext, creature_guid: u64, source_guid: u64) {
    let table = ctx.db.game_threat();
    if !table
        .by_creature()
        .filter(&creature_guid)
        .any(|row| row.source_guid == source_guid)
    {
        table.insert(ThreatEntry {
            id: 0,
            creature_guid,
            source_guid,
            threat: 0,
        });
    }
    crate::combat::enter_combat(ctx, creature_guid);
    crate::combat::enter_combat(ctx, source_guid);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contender(source_guid: u64, threat: i64, in_melee_reach: bool) -> Contender {
        Contender {
            source_guid,
            threat,
            in_melee_reach,
            standing: SourceStanding::Free,
        }
    }

    #[test]
    fn a_contender_in_melee_reach_needs_more_than_110_percent() {
        let at = |threat| [contender(2, threat, true), contender(1, 100, true)];
        assert_eq!(select_victim(1, &at(110)), Some(1));
        assert_eq!(select_victim(1, &at(111)), Some(2));
    }

    #[test]
    fn a_contender_at_range_needs_more_than_130_percent() {
        let at = |threat| [contender(2, threat, false), contender(1, 100, true)];
        assert_eq!(select_victim(1, &at(130)), Some(1));
        assert_eq!(select_victim(1, &at(131)), Some(2));
    }

    #[test]
    fn a_lower_contender_in_melee_reach_takes_over_from_a_higher_one_at_range() {
        let ranked = [
            contender(3, 120, false),
            contender(2, 115, true),
            contender(1, 100, true),
        ];
        assert_eq!(select_victim(1, &ranked), Some(2));
    }

    #[test]
    fn a_creature_without_a_victim_picks_the_top_contender() {
        let ranked = [contender(2, 5, false), contender(1, 4, true)];
        assert_eq!(select_victim(0, &ranked), Some(2));
        assert_eq!(select_victim(0, &[]), None);
    }

    #[test]
    fn a_victim_at_zero_threat_yields_to_any_contender_with_threat() {
        assert_eq!(
            select_victim(1, &[contender(2, 0, true), contender(1, 0, true)]),
            Some(1)
        );
        assert_eq!(
            select_victim(1, &[contender(2, 1, false), contender(1, 0, true)]),
            Some(2)
        );
    }

    #[test]
    fn a_victim_missing_from_the_ranking_yields_to_the_top_contender() {
        let ranked = [contender(2, 0, false), contender(3, 0, true)];
        assert_eq!(select_victim(1, &ranked), Some(2));
    }

    #[test]
    fn margins_survive_saturated_threat() {
        let ranked = [contender(2, i64::MAX, true), contender(1, i64::MAX, true)];
        assert_eq!(select_victim(1, &ranked), Some(1));
    }

    #[test]
    fn a_harmful_aura_that_fears_confuses_or_breaks_on_damage_makes_its_target_low_priority() {
        const BREAKS_ON_DAMAGE: u16 = 0x1;
        let stun = (A_CONTROL, lyracore_shared::spell::taxonomy::M_STUN);
        let fear = (A_CONTROL, M_FEAR);
        let confuse = (A_CONTROL, M_POLY);
        let low = |harmful, interrupt, (kind, control)| {
            makes_low_priority(harmful, interrupt, kind, control)
        };
        assert!(low(true, 0, fear), "fear");
        assert!(low(true, 0, confuse), "confuse");
        assert!(
            low(true, BREAKS_ON_DAMAGE, stun),
            "a stun that breaks on damage"
        );
        assert!(!low(true, 0, stun), "a stun that holds through damage");
        assert!(!low(false, BREAKS_ON_DAMAGE, fear), "a helpful aura");
    }

    #[test]
    fn a_threat_multiplier_scales_damage_and_truncates() {
        // vmangos spell_threat: Revenge 2.25, Hamstring 1.25, Holy Nova 0.
        assert_eq!(scaled_threat(100, 2.25) as i64, 225);
        assert_eq!(scaled_threat(7, 1.25) as i64, 8);
        assert_eq!(scaled_threat(500, 0.0) as i64, 0);
        assert_eq!(scaled_threat(12345, 1.0) as i64, 12345);
    }

    fn threat_row(spell_id: u32, threat: u16) -> SpellThreat {
        SpellThreat {
            spell_id,
            threat,
            multiplier: 1.0,
            inverse_effect_mask: 0,
        }
    }

    fn chosen_rank(own: Option<SpellThreat>) -> Option<u32> {
        let (first, later) = (10, 11);
        let spell_id = own.as_ref().map_or(later, |row| row.spell_id);
        rank_threat(spell_id, first, own, || Some(threat_row(first, 45))).map(|row| row.spell_id)
    }

    #[test]
    fn a_later_rank_with_its_own_flat_threat_keeps_its_row() {
        assert_eq!(chosen_rank(Some(threat_row(11, 99))), Some(11));
    }

    #[test]
    fn a_later_rank_whose_own_row_has_no_flat_threat_takes_its_first_ranks_row() {
        assert_eq!(chosen_rank(Some(threat_row(11, 0))), Some(10));
        assert_eq!(chosen_rank(None), Some(10));
    }

    #[test]
    fn a_first_rank_keeps_its_own_row_without_flat_threat() {
        assert_eq!(chosen_rank(Some(threat_row(10, 0))), Some(10));
    }

    #[test]
    fn a_paladins_direct_heal_draws_a_quarter_threat_a_point() {
        let warrior = 1;
        assert_eq!(
            heal_threat_per_point(CLASS_PALADIN, HealDelivery::Direct),
            0.25
        );
        assert_eq!(
            heal_threat_per_point(CLASS_PALADIN, HealDelivery::Periodic),
            0.5
        );
        assert_eq!(heal_threat_per_point(warrior, HealDelivery::Direct), 0.5);
    }

    #[test]
    fn heal_threat_is_split_among_the_creatures_and_truncated() {
        assert_eq!(threat_share(50.0, 1), 50);
        assert_eq!(threat_share(50.0, 2), 25);
        assert_eq!(threat_share(50.0, 4), 12);
        assert_eq!(threat_share(50.0, 0), 0);
        assert_eq!(threat_share(0.0, 3), 0);
    }

    #[test]
    fn feint_reduction_subtracts_and_floors_at_zero() {
        assert_eq!(threat_after_reduction(100, 30), 70);
        assert_eq!(threat_after_reduction(20, 50), 0);
        assert_eq!(threat_after_reduction(40, 40), 0);
        assert_eq!(threat_after_reduction(100, 0), 100);
        assert_eq!(threat_after_reduction(100, -10), 100);
        assert!(threat_after_reduction(1000, 100) >= threat_after_reduction(1000, 200));
    }

    #[test]
    fn percent_scales_use_fixed_point_rounding_and_keep_zero() {
        assert_eq!(threat_after_percent_scale(5, 50), 7);
        assert_eq!(threat_after_percent_scale(5, -50), 2);
        assert_eq!(threat_after_percent_scale(5, -100), 0);
        assert_eq!(threat_after_percent_scale(i64::MAX, 100), i64::MAX);
    }
}
