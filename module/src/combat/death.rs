use spacetimedb::{ReducerContext, Table, TimeDuration};

use crate::{
    game_aura, game_creature_spawn, game_creature_template, game_spell, game_threat,
    game_world_entity, WorldEntity,
};

pub(crate) const RESPAWN_MICROS: i64 = 15_000_000;

pub(crate) const DRAIN_SOUL_SPELL_ID: u32 = 1120;
pub(crate) const SOUL_SHARD_ENTRY: u32 = 6265;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CreatureDeathAttribution {
    source_guid: Option<u64>,
    reward_guid: Option<u64>,
}

impl CreatureDeathAttribution {
    fn credited(killer_guid: Option<u64>) -> Self {
        Self {
            source_guid: killer_guid,
            reward_guid: killer_guid,
        }
    }

    fn suicide(creature_guid: u64) -> Self {
        Self {
            source_guid: Some(creature_guid),
            reward_guid: None,
        }
    }
}

pub(crate) fn force_creature_death(ctx: &ReducerContext, creature_guid: u64) -> bool {
    kill_creature_with_attribution(
        ctx,
        creature_guid,
        CreatureDeathAttribution::suicide(creature_guid),
    )
}

use super::*;

pub(crate) fn kill_creature(ctx: &ReducerContext, target_guid: u64, killer: Option<u64>) -> bool {
    kill_creature_with_attribution(ctx, target_guid, CreatureDeathAttribution::credited(killer))
}

pub(super) fn kill_creature_by(
    ctx: &ReducerContext,
    target_guid: u64,
    source_guid: Option<u64>,
    reward_guid: Option<u64>,
) -> bool {
    kill_creature_with_attribution(
        ctx,
        target_guid,
        CreatureDeathAttribution {
            source_guid,
            reward_guid,
        },
    )
}

fn kill_creature_with_attribution(
    ctx: &ReducerContext,
    target_guid: u64,
    attribution: CreatureDeathAttribution,
) -> bool {
    let killer = attribution.source_guid;
    let reward_source = attribution.reward_guid;
    let entities = ctx.db.game_world_entity();
    let Some(mut target) = entities.guid().find(target_guid) else {
        return false;
    };
    if target.is_player() || target.dead {
        return false;
    }
    clear_lethal_damage_floor(ctx, target_guid);
    if target.owner_guid != 0 {
        let guardian = crate::creatures::guardian::die(ctx, target_guid);
        crate::creatures::begin_death_dispatch(ctx, target_guid, killer);
        crate::creatures::on_pet_death(ctx, target_guid);
        disengage(ctx, target_guid);
        if !guardian {
            crate::WorldEntity::delete(ctx, target_guid);
        }
        crate::creatures::clear_live_pet_kind(ctx, target_guid);
        crate::creatures::distraction::clear(ctx, target_guid);
        crate::hooks::fire_on_death(
            ctx,
            &crate::hooks::DeathPayload {
                victim_guid: target_guid,
                killer_guid: killer.unwrap_or(0),
                victim_is_player: false,
            },
        );
        crate::creatures::finish_death_dispatch(ctx, target_guid, killer);
        if guardian {
            crate::loot::tag::clear(ctx, target_guid);
        } else {
            // After the hooks: the teardown deletes the summon origin `OnSummonedDeath` reads.
            crate::creatures::despawn_creature_entity(ctx, target_guid);
        }
        return true;
    }
    crate::creatures::leg::stop(ctx, &mut target);
    let victim_entry = target.entry;
    let victim_level = target.level;
    let victim_instance = target.instance_id;
    let current_target_guid = target.target_guid;
    // Resolve the Loot Tag while live; death hooks still need its reward ceiling.
    let entitlement = crate::loot::tag::death_entitlement(
        ctx,
        target_guid,
        target.x,
        target.y,
        target.map_id,
        target.instance_id,
    );
    let kill_recipients = entitlement
        .as_ref()
        .map(|entitlement| entitlement.recipients.as_slice())
        .unwrap_or_default();
    let in_party = entitlement
        .as_ref()
        .is_some_and(|entitlement| entitlement.group_id.is_some());
    award_tag_rewards(
        ctx,
        &target,
        target_guid,
        kill_recipients,
        in_party,
        reward_source,
    );
    roll_corpse_loot(ctx, &mut target, target_guid, entitlement.as_ref());
    target.health = 0;
    target.dead = true;
    target.update(ctx);
    crate::creatures::begin_death_dispatch(ctx, target_guid, killer);
    // Snapshot threat before disengage clears the ledger used by death hooks.
    let threat_snapshot: Vec<(u64, i64)> = ctx
        .db
        .game_threat()
        .iter()
        .filter(|t| t.creature_guid == target_guid)
        .map(|t| (t.source_guid, t.threat))
        .collect();
    disengage(ctx, target_guid);
    crate::combo::forget(ctx, target_guid);
    crate::threat::clear_taunt_lock(ctx, target_guid);
    crate::spell::break_channel(ctx, target_guid);
    crate::spell::remove_auras_on_death(ctx, target_guid);
    let spawns = ctx.db.game_creature_spawn();
    if let Some(mut spawn) = spawns.guid().find(target_guid) {
        spawn.despawn_at = crate::time::now(ctx)
            .checked_add(TimeDuration::from_micros(
                crate::creatures::CREATURE_CORPSE_DECAY_MICROS,
            ))
            .unwrap_or(crate::time::now(ctx));
        spawns.guid().update(spawn);
    }
    if let Some(killer_guid) = killer {
        if !is_engaged(ctx, killer_guid) {
            if let Some(mut k) = entities.guid().find(killer_guid) {
                k.combat_until_ms = crate::time::now_ms(ctx);
                k.update(ctx);
            }
        }
    }
    // Run hooks after death state is settled so handlers observe the complete transition.
    crate::hooks::fire_on_death(
        ctx,
        &crate::hooks::DeathPayload {
            victim_guid: target_guid,
            killer_guid: killer.unwrap_or(0),
            victim_is_player: false,
        },
    );
    crate::hooks::fire_on_creature_death(
        ctx,
        &crate::hooks::CreatureDeathPayload {
            creature_guid: target_guid,
            entry: victim_entry,
            instance_id: victim_instance,
            killer_guid: killer.unwrap_or(0),
            current_target_guid,
            threat_snapshot,
        },
    );
    for &killer_guid in kill_recipients {
        crate::hooks::fire_on_kill(
            ctx,
            &crate::hooks::KillPayload {
                killer_guid,
                victim_guid: target_guid,
                victim_entry,
                victim_level,
            },
        );
    }
    crate::creatures::finish_death_dispatch(ctx, target_guid, killer);
    // Clear the Loot Tag only after every synchronous death callback has observed it.
    crate::loot::tag::clear(ctx, target_guid);
    true
}

fn award_tag_rewards(
    ctx: &ReducerContext,
    target: &WorldEntity,
    target_guid: u64,
    recipients: &[u64],
    in_party: bool,
    reward_source: Option<u64>,
) {
    let template = ctx.db.game_creature_template().entry().find(target.entry);
    let rank = template.as_ref().map(|t| t.rank).unwrap_or(0);
    crate::xp::award_kill_xp(
        ctx,
        target_guid,
        target.level,
        rank,
        target.map_id,
        in_party,
        recipients,
    );
    for recipient in recipients {
        crate::quest::on_creature_killed(ctx, *recipient, target.entry);
    }
    let is_critter = template
        .as_ref()
        .map(|t| t.creature_type == crate::spell::CRITTER_TYPE)
        .unwrap_or(false);
    if let Some(reward_guid) = reward_source.filter(|guid| recipients.contains(guid)) {
        let reward_level = ctx
            .db
            .game_world_entity()
            .guid()
            .find(reward_guid)
            .map(|character| character.level)
            .unwrap_or(0);
        let yields_xp = !crate::xp::is_grey_kill(reward_level, target.level);
        // vmangos `spell_chain` ranks Drain Soul from 1120; every rank grants the shard.
        let draining_soul = ctx
            .db
            .game_aura()
            .by_target()
            .filter(&target_guid)
            .any(|a| {
                a.caster_guid == reward_guid
                    && crate::spell::first_rank(ctx, a.spell_id) == DRAIN_SOUL_SPELL_ID
            });
        if !is_critter && yields_xp && draining_soul {
            if let Err(refusal) =
                crate::items::request_grant_item(ctx, reward_guid, SOUL_SHARD_ENTRY, 1, None)
            {
                if refusal.kind != crate::actor::ActionRefusalKind::InventoryFull {
                    spacetimedb::log::warn!(
                        "soul shard for {reward_guid} after killing {target_guid} refused: {}",
                        refusal.detail
                    );
                }
            }
        }
    }
}

fn roll_corpse_loot(
    ctx: &ReducerContext,
    target: &mut WorldEntity,
    target_guid: u64,
    entitlement: Option<&crate::loot::tag::DeathEntitlement>,
) {
    crate::loot::purge_corpse_residue(ctx, target_guid);
    let Some(entitlement) = entitlement else {
        target.money = 0;
        target.dynamic_flags &= !lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        return;
    };
    crate::loot::tag::record_corpse_eligibility(ctx, target_guid, &entitlement.recipients);
    let loot = ctx
        .db
        .game_creature_template()
        .entry()
        .find(target.entry)
        .map(|template| {
            crate::loot::scale_money_for_rank(
                roll_money(ctx, template.money_min, template.money_max),
                template.rank,
            )
        })
        .unwrap_or(0);
    if loot > 0 {
        target.money = loot;
    }
    let dropped = crate::loot::roll_creature_loot(ctx, target.entry, target_guid);
    if let Some(group_id) = entitlement.group_id {
        crate::loot::apply_group_loot_rules(ctx, target_guid, group_id);
    }
    if loot > 0 || dropped {
        target.dynamic_flags |= lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
    }
}

// vmangos SPELL_ATTR_EX3_NO_DURABILITY_LOSS, kept in `attributes_ex_3`.
const SPELL_ATTR_EX3_NO_DURABILITY_LOSS: u32 = 0x0000_0020;

/// A Character dies to a hit. vmangos `Unit::Kill` spares the death durability loss when a Character
/// dealt or controls the hit, the victim included, or the spell has NO_DURABILITY_LOSS.
pub(crate) fn kill_player_by_hit(
    ctx: &ReducerContext,
    victim_guid: u64,
    attacker_guid: u64,
    spell_id: u32,
) -> bool {
    let character_hit = ctx
        .db
        .game_world_entity()
        .guid()
        .find(attacker_guid)
        .is_some_and(|attacker| crate::pvp::affecting_character(ctx, &attacker).is_some());
    let spell_spares = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .is_some_and(|spell| spell.attributes_ex_3 & SPELL_ATTR_EX3_NO_DURABILITY_LOSS != 0);
    let durability_loss = if character_hit || spell_spares {
        DurabilityLoss::Spared
    } else {
        DurabilityLoss::Applied
    };
    kill_character(ctx, victim_guid, attacker_guid, durability_loss)
}

/// A Character dies to the environment or without a killer, and loses durability, as vmangos
/// `Player::EnvironmentalDamage` adds the loss that `Unit::Kill` spares a self-inflicted death.
pub(crate) fn kill_player(ctx: &ReducerContext, victim_guid: u64, killer_guid: u64) -> bool {
    kill_character(ctx, victim_guid, killer_guid, DurabilityLoss::Applied)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DurabilityLoss {
    Applied,
    Spared,
}

fn kill_character(
    ctx: &ReducerContext,
    victim_guid: u64,
    killer_guid: u64,
    durability_loss: DurabilityLoss,
) -> bool {
    let entities = ctx.db.game_world_entity();
    if !entities
        .guid()
        .find(victim_guid)
        .is_some_and(|victim| victim.is_player() && !victim.dead)
    {
        return false;
    }
    crate::duel::interrupt_duel_on_death(ctx, victim_guid);
    // Ending the Duel stops the victim's combat, which rewrites its entity row.
    let Some(mut victim) = entities.guid().find(victim_guid) else {
        return false;
    };
    victim.health = 0;
    victim.dead = true;
    crate::drunk::sober_on_death(&mut victim);
    super::queued_strike::forget(&mut victim);
    victim.combat_until_ms = crate::time::now_ms(ctx);
    victim.update(ctx);
    let killer = (killer_guid != 0).then_some(killer_guid);
    crate::creatures::begin_death_dispatch(ctx, victim_guid, killer);
    disengage(ctx, victim_guid);
    crate::combo::forget(ctx, victim_guid);
    crate::spell::break_channel(ctx, victim_guid);
    crate::spell::record_self_resurrect_option(ctx, victim_guid);
    crate::spell::remove_auras_on_death(ctx, victim_guid);
    crate::trade::cancel_trade_for(ctx, victim_guid);
    crate::native::loot::close_window(ctx, victim_guid);
    if durability_loss == DurabilityLoss::Applied
        && crate::items::apply_death_durability_loss(ctx, victim_guid)
    {
        crate::group::push_event(
            ctx,
            victim_guid,
            lyracore_shared::group::event_kind::DEATH_DURABILITY,
            0,
            String::new(),
        );
    }
    crate::hooks::fire_on_death(
        ctx,
        &crate::hooks::DeathPayload {
            victim_guid,
            killer_guid,
            victim_is_player: true,
        },
    );
    crate::creatures::finish_death_dispatch(ctx, victim_guid, killer);
    true
}

#[cfg(test)]
mod tests {
    use super::CreatureDeathAttribution;

    #[test]
    fn forced_death_names_self_without_granting_kill_rewards() {
        let attribution = CreatureDeathAttribution::suicide(77);
        assert_eq!(attribution.source_guid, Some(77));
        assert_eq!(attribution.reward_guid, None);
    }
}
