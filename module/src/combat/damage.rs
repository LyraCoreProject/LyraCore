use spacetimedb::{table, ReducerContext, Table};

use crate::{game_world_entity, WorldEntity};

use super::*;

#[table(accessor = game_creature_lethal_damage_floor)]
pub struct CreatureLethalDamageFloor {
    #[primary_key]
    #[unique]
    pub creature_guid: u64,
    pub definition_revision: u64,
    pub notification_sent: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct FinalDamage {
    pub amount: u32,
    lethal_prevented: bool,
}

pub(crate) fn set_lethal_damage_floor(
    ctx: &ReducerContext,
    creature_guid: u64,
    definition_revision: u64,
    enabled: bool,
) -> bool {
    let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return false;
    };
    if creature.is_player() || creature.dead {
        return false;
    }
    let floors = ctx.db.game_creature_lethal_damage_floor();
    if !enabled {
        floors.creature_guid().delete(creature_guid);
        return true;
    }
    let row = CreatureLethalDamageFloor {
        creature_guid,
        definition_revision,
        notification_sent: false,
    };
    match floors.creature_guid().find(creature_guid) {
        Some(_) => {
            floors.creature_guid().update(row);
        }
        None => {
            floors.insert(row);
        }
    }
    true
}

pub(crate) fn clear_lethal_damage_floor(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_lethal_damage_floor()
        .creature_guid()
        .delete(creature_guid);
}

pub(crate) fn clear_stale_lethal_damage_floor(
    ctx: &ReducerContext,
    creature_guid: u64,
    current_revision: u64,
) {
    let floors = ctx.db.game_creature_lethal_damage_floor();
    if floors
        .creature_guid()
        .find(creature_guid)
        .is_some_and(|row| row.definition_revision != current_revision)
    {
        floors.creature_guid().delete(creature_guid);
    }
}

pub(crate) fn final_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    post_mitigation: u32,
) -> FinalDamage {
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return FinalDamage {
            amount: post_mitigation,
            lethal_prevented: false,
        };
    };
    lethal_floor_amount(
        target.health,
        post_mitigation,
        lethal_floor_protects(ctx, target_guid),
    )
}

pub(crate) fn lethal_floor_protects(ctx: &ReducerContext, target_guid: u64) -> bool {
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return false;
    };
    let floors = ctx.db.game_creature_lethal_damage_floor();
    !target.is_player()
        && !target.dead
        && floors.creature_guid().find(target_guid).is_some_and(|row| {
            let current = crate::creatures::current_definition_revision(ctx, target_guid);
            if row.definition_revision == current.value {
                true
            } else {
                floors.creature_guid().delete(target_guid);
                false
            }
        })
}

fn lethal_floor_amount(health: u32, post_mitigation: u32, protected: bool) -> FinalDamage {
    let lethal_prevented = protected && is_lethal(health, post_mitigation);
    FinalDamage {
        amount: if lethal_prevented {
            health.saturating_sub(1)
        } else {
            post_mitigation
        },
        lethal_prevented,
    }
}

pub(crate) fn commit_death_prevention(
    ctx: &ReducerContext,
    creature_guid: u64,
    attacker_guid: u64,
) {
    let floors = ctx.db.game_creature_lethal_damage_floor();
    let Some(mut row) = floors.creature_guid().find(creature_guid) else {
        return;
    };
    if row.notification_sent {
        return;
    }
    row.notification_sent = true;
    floors.creature_guid().update(row);
    crate::hooks::fire_on_death_prevented(
        ctx,
        &crate::hooks::DeathPreventedPayload {
            creature_guid,
            attacker_guid,
        },
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HitSource {
    MainHand,
    OffHand,
    Ranged,
    MeleeSpell,
    Spell,
    Triggered,
}

impl HitSource {
    pub(crate) fn is_weapon(self) -> bool {
        matches!(self, Self::MainHand | Self::OffHand | Self::Ranged)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Hit {
    pub source: HitSource,
    pub spell_id: u32,
    pub crit: bool,
}

impl Hit {
    pub(crate) fn weapon(source: HitSource, crit: bool) -> Self {
        Self {
            source,
            spell_id: 0,
            crit,
        }
    }

    pub(crate) fn spell(source: HitSource, spell_id: u32, crit: bool) -> Self {
        Self {
            source,
            spell_id,
            crit,
        }
    }

    pub(crate) fn triggered() -> Self {
        Self {
            source: HitSource::Triggered,
            spell_id: 0,
            crit: false,
        }
    }
}

pub(crate) struct HitOutcome {
    pub killed: bool,
    pub duel_completed: bool,
}

impl HitOutcome {
    pub(crate) fn combat_ended(self) -> bool {
        self.killed || self.duel_completed
    }
}

pub(crate) fn is_lethal(health: u32, dmg: u32) -> bool {
    dmg > 0 && health <= dmg
}

/// A lethal hit between active Duel opponents ends the Duel instead: `apply_hit` floors the loser.
pub(crate) fn duel_takes_lethal_hit(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target: &WorldEntity,
) -> bool {
    target.is_player() && crate::duel::active_opponents(ctx, attacker_guid, target.guid)
}

pub(crate) fn is_killing_blow(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target: &WorldEntity,
    dmg: u32,
) -> bool {
    is_lethal(target.health, dmg) && !duel_takes_lethal_hit(ctx, attacker_guid, target)
}

// Absorb before lethal checks and godmode so even a zeroed hit drains shields.
pub(crate) fn fold_incoming_damage(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    rolled: u32,
) -> (u32, u32) {
    let entities = ctx.db.game_world_entity();
    let attacker_stance = entities
        .guid()
        .find(attacker_guid)
        .map(|a| a.stance)
        .unwrap_or(0);
    let outgoing_pct = crate::spell::stance_damage_done_pct(attacker_stance)
        + crate::modifiers::combat_field_bonus(ctx, attacker_guid, crate::spell::COMBAT_DMG_DONE);
    let after_outgoing = crate::spell::apply_damage_pct(rolled, outgoing_pct);
    let incoming = crate::spell::apply_damage_pct(
        after_outgoing,
        crate::spell::damage_taken_bonus(ctx, target_guid),
    );
    let dealt = crate::spell::absorb_incoming(ctx, target_guid, incoming);
    let dealt = landed_amount(entities.guid().find(target_guid).as_ref(), dealt);
    (dealt, incoming - dealt)
}

// A godmode target takes no damage; the rule applies when damage lands.
fn landed_amount(target: Option<&WorldEntity>, amount: u32) -> u32 {
    if target.is_some_and(|target| target.godmode) {
        0
    } else {
        amount
    }
}

// vmangos rewards a Character taken rage for health lost to another unit, weapon or not.
fn reward_taken_rage(target: &mut WorldEntity, attacker_guid: u64, health_before: u32) {
    if attacker_guid != target.guid && target.is_player() && is_rage_user(target) {
        let lost = health_before.saturating_sub(target.health);
        target.power = (target.power + rage_from_damage(lost, false)).min(target.max_power);
    }
}

pub(crate) fn apply_hit(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    damage: FinalDamage,
    hit: Hit,
) -> HitOutcome {
    let miss = HitOutcome {
        killed: false,
        duel_completed: false,
    };
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return miss;
    };
    if target.dead || (damage.amount == 0 && !damage.lethal_prevented) {
        return miss;
    }
    let dmg = damage.amount;
    let weapon = hit.source.is_weapon();
    let attacker = entities.guid().find(attacker_guid);
    if attacker
        .as_ref()
        .is_some_and(|attacker| !crate::combat::may_harm(ctx, attacker, &target))
    {
        return miss;
    }
    let attacker_is_player = attacker.as_ref().map(|a| a.is_player()).unwrap_or(false);
    let reward_source = crate::loot::tag::controlling_character(ctx, attacker_guid);
    let target_is_player = target.is_player();
    if damage.lethal_prevented && dmg == 0 {
        // A lethal floor still refreshes pursuit so the scripted fight cannot evade.
        refresh_leash(ctx, attacker_guid, target_guid);
        commit_death_prevention(ctx, target_guid, attacker_guid);
        return miss;
    }
    if !target_is_player {
        crate::loot::tag::record_first_threat(ctx, target_guid, attacker_guid);
    }

    if weapon {
        if let Some(a) = &attacker {
            // vmangos rewards dealt rage for white swings only, not for a queued strike.
            if hit.spell_id == 0 && is_rage_user(a) {
                if let Some(mut fresh) = entities.guid().find(attacker_guid) {
                    fresh.power = (fresh.power + rage_from_damage(dmg, true)).min(fresh.max_power);
                    fresh.update(ctx);
                }
            }
            if attacker_is_player && !target_is_player {
                crate::skill::gain_weapon_skill(ctx, a);
            }
        }
    }

    if is_lethal(target.health, dmg) && duel_takes_lethal_hit(ctx, attacker_guid, &target) {
        let before = target.health;
        target.health = 1;
        reward_taken_rage(&mut target, attacker_guid, before);
        target.update(ctx);
        if let Some(duel) = crate::duel::active_duel_between(ctx, attacker_guid, target_guid) {
            crate::duel::complete_duel(
                ctx,
                duel.id,
                crate::duel::completion_kind::WON,
                attacker_guid,
                target_guid,
            );
        }
        crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, true);
        return HitOutcome {
            killed: false,
            duel_completed: true,
        };
    }
    let killed = is_lethal(target.health, dmg) && (weapon || !target_is_player);
    if killed {
        if target_is_player {
            kill_player(ctx, target_guid, attacker_guid);
        } else {
            super::death::kill_creature_by(
                ctx,
                target_guid,
                attacker.as_ref().map(|_| attacker_guid),
                reward_source,
            );
        }
        crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, false);
        return HitOutcome {
            killed: true,
            duel_completed: false,
        };
    }
    let accrues_threat = !target_is_player && reward_source.is_some();
    if weapon && target_is_player {
        crate::skill::gain_defense_skill(ctx, &target);
    }
    let before = target.health;
    target.health = crate::spell::damaged_value(target.health, dmg as i32);
    reward_taken_rage(&mut target, attacker_guid, before);
    target.update(ctx);
    if damage.lethal_prevented {
        commit_death_prevention(ctx, target_guid, attacker_guid);
    }
    refresh_leash(ctx, attacker_guid, target_guid);
    engage_passive_creature(ctx, target_guid, attacker_guid);
    if !weapon {
        enter_combat(ctx, target_guid);
    }

    crate::spell::break_auras_on_damage(
        ctx,
        target_guid,
        if weapon { attacker_guid } else { 0 },
        false,
    );
    if accrues_threat {
        crate::threat::add_threat(
            ctx,
            target_guid,
            attacker_guid,
            crate::threat::threat_from_damage(dmg),
        );
        if !weapon {
            crate::spell::arm_spell_retaliation(ctx, attacker_guid, target_guid);
        }
    }
    crate::spell::proc::run_proc_pass(ctx, attacker_guid, target_guid, &hit, true);
    HitOutcome {
        killed: false,
        duel_completed: false,
    }
}

/// Damage after outgoing and taken modifiers, absorbs, godmode and the Lethal Damage Floor.
#[derive(Clone, Copy)]
pub(crate) struct ResolvedDamage {
    damage: FinalDamage,
    absorbed: u32,
}

impl ResolvedDamage {
    pub(crate) fn amount(self) -> u32 {
        self.damage.amount
    }

    pub(crate) fn land(
        self,
        ctx: &ReducerContext,
        attacker_guid: u64,
        target_guid: u64,
        hit: Hit,
    ) -> HitOutcome {
        apply_hit(ctx, attacker_guid, target_guid, self.damage, hit)
    }
}

pub(crate) fn resolve_damage(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    rolled: u32,
) -> ResolvedDamage {
    let (dealt, absorbed) = fold_incoming_damage(ctx, attacker_guid, target_guid, rolled);
    ResolvedDamage {
        damage: final_damage(ctx, target_guid, dealt),
        absorbed,
    }
}

/// A projectile's damage, folded and absorbed at launch; godmode and the floor apply at impact.
#[derive(Clone, Copy)]
pub(crate) struct FrozenDamage(u32);

impl FrozenDamage {
    pub(crate) fn launch(
        ctx: &ReducerContext,
        attacker_guid: u64,
        target_guid: u64,
        rolled: u32,
    ) -> Self {
        Self(fold_incoming_damage(ctx, attacker_guid, target_guid, rolled).0)
    }

    pub(crate) fn stored(amount: u32) -> Self {
        Self(amount)
    }

    pub(crate) fn amount(self) -> u32 {
        self.0
    }

    pub(crate) fn resolve(self, ctx: &ReducerContext, target: &WorldEntity) -> ResolvedDamage {
        ResolvedDamage {
            damage: final_damage(ctx, target.guid, landed_amount(Some(target), self.0)),
            absorbed: 0,
        }
    }
}

/// Spell and triggered damage: a dead or unharmable target takes nothing. Returns the damage
/// dealt and the amount absorbed.
pub(crate) fn deal_damage(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    basis: i32,
    hit: Hit,
) -> (u32, u32) {
    let entities = ctx.db.game_world_entity();
    let Some(target) = entities.guid().find(target_guid) else {
        return (0, 0);
    };
    if target.dead {
        return (0, 0);
    }
    if entities
        .guid()
        .find(attacker_guid)
        .is_some_and(|attacker| !may_harm(ctx, &attacker, &target))
    {
        return (0, 0);
    }
    let resolved = resolve_damage(ctx, attacker_guid, target_guid, basis.max(0) as u32);
    if resolved.amount() == 0 && !resolved.damage.lethal_prevented {
        return (0, resolved.absorbed);
    }
    // Enter combat before applying a lethal hit, which may remove the target.
    enter_combat(ctx, attacker_guid);
    resolved.land(ctx, attacker_guid, target_guid, hit);
    (resolved.amount(), resolved.absorbed)
}

#[cfg(feature = "debug_reducers")]
mod fixtures;

#[cfg(test)]
mod lethality_tests {
    use super::{is_lethal, lethal_floor_amount, HitOutcome};

    #[test]
    fn a_zero_damage_hit_is_never_lethal() {
        assert!(!is_lethal(1, 0));
        assert!(!is_lethal(0, 0));
    }

    #[test]
    fn a_hit_at_or_above_remaining_health_is_lethal() {
        assert!(is_lethal(10, 10));
        assert!(is_lethal(10, 11));
        assert!(!is_lethal(10, 9));
    }

    #[test]
    fn lethal_floor_changes_only_protected_lethal_damage() {
        assert_eq!(lethal_floor_amount(10, 4, true).amount, 4);
        assert_eq!(lethal_floor_amount(10, 10, false).amount, 10);
        assert_eq!(lethal_floor_amount(10, 10, true).amount, 9);
        assert_eq!(lethal_floor_amount(1, 20, true).amount, 0);
    }

    #[test]
    fn duel_completion_is_a_combat_terminal_without_becoming_a_death() {
        let outcome = HitOutcome {
            killed: false,
            duel_completed: true,
        };
        assert!(outcome.combat_ended());
    }
}
