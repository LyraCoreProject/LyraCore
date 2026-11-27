// vmangos `game_event_creature_data` on live creatures; read docs/schema.md#game-events first.

use spacetimedb::{log, ReducerContext};

use crate::game_events::{creature_data_worn, GameEventCreatureData};
use crate::{
    game_creature_spawn, game_creature_template, game_encounter_equip, game_world_entity,
    CreatureTemplate, WorldEntity,
};

/// The template a creature of its own entry `own_entry` takes while it wears `worn`, if another:
/// the first that a row names and the Realm holds.
fn worn_template(
    ctx: &ReducerContext,
    worn: &[GameEventCreatureData],
    own_entry: u32,
) -> Option<CreatureTemplate> {
    let template = worn
        .iter()
        .filter(|data| data.creature_entry != 0)
        .find_map(|data| {
            ctx.db
                .game_creature_template()
                .entry()
                .find(data.creature_entry)
        })?;
    (template.entry != own_entry).then_some(template)
}

fn worn_equipment(worn: &[GameEventCreatureData]) -> Option<[u32; 3]> {
    worn.iter().find_map(GameEventCreatureData::equipment)
}

/// Gives a creature about to enter the world the template of the event data its spawn wears, as
/// vmangos `Creature::LoadFromDB` does; `dress_spawned` finishes it once it is in the world.
pub(crate) fn wear_at_spawn(
    ctx: &ReducerContext,
    entity: &mut WorldEntity,
) -> Vec<GameEventCreatureData> {
    let Some(spawn) = ctx.db.game_creature_spawn().guid().find(entity.guid) else {
        return Vec::new();
    };
    let source_guid = if spawn.source_guid == 0 {
        spawn.guid
    } else {
        spawn.source_guid
    };
    let worn = creature_data_worn(ctx, source_guid);
    if let Some(template) = worn_template(ctx, &worn, entity.entry) {
        super::presentation::take_template(ctx, entity, &template);
    }
    worn
}

/// Gives a creature that just entered the world the equipment and start spells of `worn`.
pub(crate) fn dress_spawned(
    ctx: &ReducerContext,
    creature_guid: u64,
    worn: &[GameEventCreatureData],
) {
    if let Some(items) = worn_equipment(worn) {
        wear_equipment(ctx, creature_guid, items);
    }
    for data in worn {
        cast_on_itself(ctx, creature_guid, data.start_spell);
    }
}

/// vmangos `GameEventMgr::UpdateCreatureData` for `data` as its event starts (`running`) or stops,
/// with `worn` the spawn's rows of running events; returns the creatures it changed. A start spell
/// whose aura the creature already has, from its spawn, is not cast again.
pub(crate) fn follow_event(
    ctx: &ReducerContext,
    data: &GameEventCreatureData,
    running: bool,
    worn: &[GameEventCreatureData],
) -> usize {
    let spawns = ctx.db.game_creature_spawn();
    let creatures: Vec<(u64, u32)> = spawns
        .guid()
        .find(data.spawn_guid)
        .into_iter()
        .chain(spawns.by_source_guid().filter(data.spawn_guid))
        .map(|spawn| (spawn.guid, spawn.entry))
        .collect();
    creatures
        .into_iter()
        .filter(|&(guid, own_entry)| follow_one(ctx, guid, own_entry, data, running, worn))
        .count()
}

fn follow_one(
    ctx: &ReducerContext,
    creature_guid: u64,
    own_entry: u32,
    data: &GameEventCreatureData,
    running: bool,
    worn: &[GameEventCreatureData],
) -> bool {
    let Some(entity) = ctx.db.game_world_entity().guid().find(creature_guid) else {
        return false;
    };
    if entity.dead {
        return false;
    }
    let mut changed = false;
    let entry = worn_template(ctx, worn, own_entry).map_or(own_entry, |template| template.entry);
    if entity.entry != entry {
        if let Err(reason) = super::presentation::update_entry(ctx, creature_guid, entry) {
            log::warn!(
                "game events: creature {creature_guid} keeps entry {}: {reason}",
                entity.entry
            );
        }
        changed = true;
    }
    changed |= match worn_equipment(worn) {
        Some(items) => wear_equipment(ctx, creature_guid, items),
        None => data
            .equipment()
            .is_some_and(|items| take_off_equipment(ctx, creature_guid, items)),
    };
    if running {
        changed |= take_off_spell(ctx, creature_guid, data.end_spell);
        changed |= !has_aura(ctx, creature_guid, data.start_spell)
            && cast_on_itself(ctx, creature_guid, data.start_spell);
    } else {
        changed |= take_off_spell(ctx, creature_guid, data.start_spell);
        changed |= cast_on_itself(ctx, creature_guid, data.end_spell);
    }
    changed
}

fn shown_equipment(ctx: &ReducerContext, creature_guid: u64) -> Option<[u32; 3]> {
    ctx.db
        .game_encounter_equip()
        .creature_guid()
        .find(creature_guid)
        .map(|row| [row.main_hand, row.off_hand, row.ranged])
}

fn wear_equipment(ctx: &ReducerContext, creature_guid: u64, items: [u32; 3]) -> bool {
    let displays = match super::presentation::equipment_displays(ctx, items) {
        Ok(displays) => displays,
        Err(reason) => {
            log::warn!("game events: creature {creature_guid} keeps its equipment: {reason}");
            return false;
        }
    };
    if shown_equipment(ctx, creature_guid) == Some(displays) {
        return false;
    }
    let [main_hand, off_hand, ranged] = items;
    if let Err(reason) =
        super::presentation::equip_swap(ctx, creature_guid, main_hand, off_hand, ranged)
    {
        log::warn!("game events: creature {creature_guid} keeps its equipment: {reason}");
        return false;
    }
    true
}

/// Takes off the event's equipment `items`; equipment something else gave the creature stays.
fn take_off_equipment(ctx: &ReducerContext, creature_guid: u64, items: [u32; 3]) -> bool {
    let worn = super::presentation::equipment_displays(ctx, items)
        .is_ok_and(|displays| shown_equipment(ctx, creature_guid) == Some(displays));
    if worn {
        crate::EncounterEquip::delete(ctx, creature_guid);
    }
    worn
}

/// vmangos `CastSpell(this, spell, true)`: a triggered cast on itself.
fn cast_on_itself(ctx: &ReducerContext, creature_guid: u64, spell_id: u32) -> bool {
    if spell_id == 0 {
        return false;
    }
    let Some(level) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .map(|entity| entity.level)
    else {
        return false;
    };
    if let Err(reason) =
        crate::spell::cast_triggered(ctx, creature_guid, spell_id, level as u8, creature_guid)
    {
        log::warn!("game events: creature {creature_guid} cannot cast {spell_id}: {reason}");
        return false;
    }
    true
}

fn has_aura(ctx: &ReducerContext, creature_guid: u64, spell_id: u32) -> bool {
    spell_id != 0
        && crate::spell::auras_on(ctx, creature_guid).any(|aura| aura.spell_id == spell_id)
}

fn take_off_spell(ctx: &ReducerContext, creature_guid: u64, spell_id: u32) -> bool {
    if !has_aura(ctx, creature_guid, spell_id) {
        return false;
    }
    crate::spell::spellbook::strip_spell_auras(ctx, creature_guid, &[spell_id]);
    true
}
