use lyracore_shared::constants;
use lyracore_shared::pet::pet_guid_for;
#[cfg(feature = "debug_reducers")]
use spacetimedb::reducer;
use spacetimedb::{table, ReducerContext, Table};

use crate::{
    game_creature_template, game_entity_motion, game_melee_attack, game_world_entity, WorldEntity,
};

use super::spawn::{build_creature_entity, CreatureSpawn};

#[table(accessor = game_pet_command)]
pub struct PetCommand {
    #[primary_key]
    pub owner_guid: u64,
    pub command: u8,
    pub react: u8,

    pub command_target: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_pet_command(ctx, character_guid) {
    ctx.db.game_pet_command().owner_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_pet_command());

// Match CMSG_PET_ACTION.data and SMSG_PET_SPELLS command numbering.
pub(crate) const CMD_STAY: u8 = 0;
const CMD_FOLLOW: u8 = 1;
pub(crate) const CMD_ATTACK: u8 = 2;
const CMD_DISMISS: u8 = 3;
pub(crate) const REACT_PASSIVE: u8 = 0;
const REACT_DEFENSIVE: u8 = 1;
pub(crate) const REACT_AGGRESSIVE: u8 = 2;
const ACT_COMMAND: u8 = 0x07;
const ACT_REACTION: u8 = 0x06;
const PET_AGGRO_SEEK_SQ: f32 = 100.0;

pub(crate) fn pet_command_state(ctx: &ReducerContext, owner_guid: u64) -> (u8, u8, u64) {
    ctx.db
        .game_pet_command()
        .owner_guid()
        .find(owner_guid)
        .map(|r| (r.command, r.react, r.command_target))
        .unwrap_or((CMD_FOLLOW, REACT_DEFENSIVE, 0))
}

fn set_pet_command(
    ctx: &ReducerContext,
    owner_guid: u64,
    command: u8,
    react: u8,
    command_target: u64,
) {
    let t = ctx.db.game_pet_command();
    let row = PetCommand {
        owner_guid,
        command,
        react,
        command_target,
    };
    if t.owner_guid().find(owner_guid).is_some() {
        t.owner_guid().update(row);
    } else {
        t.insert(row);
    }
}

fn is_valid_pet_target(
    ctx: &ReducerContext,
    owner: &WorldEntity,
    pet_guid: u64,
    target_guid: u64,
) -> bool {
    if target_guid == 0 || target_guid == pet_guid || target_guid == owner.guid {
        return false;
    }
    match ctx.db.game_world_entity().guid().find(target_guid) {
        Some(t) => {
            !t.dead
                && !t.is_player()
                && !crate::faction::is_friendly(ctx, owner.faction_template, t.faction_template)
        }
        None => false,
    }
}

fn nearest_hostile_near(
    ctx: &ReducerContext,
    owner: &WorldEntity,
    pet: &WorldEntity,
) -> Option<u64> {
    let mut best: Option<(u64, f32)> = None;
    for e in ctx.db.game_world_entity().iter() {
        if e.guid == pet.guid || e.owner_guid != 0 || e.is_player() || e.dead {
            continue;
        }
        if e.map_id != pet.map_id || e.instance_id != pet.instance_id {
            continue;
        }
        if crate::faction::is_friendly(ctx, owner.faction_template, e.faction_template) {
            continue;
        }
        let (dx, dy, dz) = (e.x - pet.x, e.y - pet.y, e.z - pet.z);
        let d2 = dx * dx + dy * dy + dz * dz;
        if d2 <= PET_AGGRO_SEEK_SQ && best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
            best = Some((e.guid, d2));
        }
    }
    best.map(|(g, _)| g)
}

pub(crate) fn apply_pet_command(
    ctx: &ReducerContext,
    owner_guid: u64,
    data: u32,
    target_guid: u64,
) -> Result<(), crate::Refusal> {
    pet_of(ctx, owner_guid).ok_or_else(|| "no pet".to_string())?;
    let flag = ((data >> 24) & 0xFF) as u8;
    let id = (data & 0x00FF_FFFF) as u8;
    let (cmd, react, cmd_target) = pet_command_state(ctx, owner_guid);
    match flag {
        ACT_COMMAND => match id {
            CMD_STAY => set_pet_command(ctx, owner_guid, CMD_STAY, react, 0),
            CMD_FOLLOW => set_pet_command(ctx, owner_guid, CMD_FOLLOW, react, 0),
            CMD_ATTACK => set_pet_command(ctx, owner_guid, CMD_ATTACK, react, target_guid),
            CMD_DISMISS => despawn_pets(ctx, owner_guid),
            other => return Err(format!("unknown pet command id {other}").into()),
        },
        ACT_REACTION => {
            if id > REACT_AGGRESSIVE {
                return Err(format!("unknown pet react id {id}").into());
            }
            set_pet_command(ctx, owner_guid, cmd, id, cmd_target);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_pet_command(
    ctx: &ReducerContext,
    owner_guid: u64,
    data: u32,
    target_guid: u64,
) -> Result<(), String> {
    Ok(apply_pet_command(ctx, owner_guid, data, target_guid)?)
}

pub(crate) fn pet_of(ctx: &ReducerContext, owner_guid: u64) -> Option<WorldEntity> {
    if owner_guid == 0 {
        return None;
    }
    ctx.db
        .game_world_entity()
        .guid()
        .find(pet_guid_for(owner_guid))
        .filter(|pet| pet.owner_guid == owner_guid)
}

// Clear combat and threat before motion, entity and command state.
fn despawn_pet_entity(ctx: &ReducerContext, pet: WorldEntity) {
    crate::combat::disengage(ctx, pet.guid);
    crate::motion::drop_pending(ctx, pet.guid);
    ctx.db.game_entity_motion().guid().delete(pet.guid);
    crate::WorldEntity::delete(ctx, pet.guid);
    super::clear_live_pet_kind(ctx, pet.guid);
    super::distraction::clear(ctx, pet.guid);
}

pub(crate) fn despawn_pets(ctx: &ReducerContext, owner_guid: u64) {
    if let Some(pet) = pet_of(ctx, owner_guid) {
        despawn_pet_entity(ctx, pet);
    }
    ctx.db.game_pet_command().owner_guid().delete(owner_guid);
}

fn build_pet_entity(ctx: &ReducerContext, owner: &WorldEntity, entry: u32) -> Option<WorldEntity> {
    let tmpl = ctx.db.game_creature_template().entry().find(entry)?;
    let pet_guid = pet_guid_for(owner.guid);
    let spawn = CreatureSpawn {
        guid: pet_guid,
        entry,
        map_id: owner.map_id,
        x: owner.x,
        y: owner.y,
        z: owner.z,
        orientation: owner.orientation,
        respawn_at: ctx.timestamp,
        despawn_at: ctx.timestamp,
        movement_type: 0,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: owner.instance_id,
    };
    let mut pet = build_creature_entity(&spawn, &tmpl, ctx.random(), owner.instance_id);
    pet.owner_guid = owner.guid;
    Some(pet)
}

pub(crate) fn summon_pet(ctx: &ReducerContext, caster_guid: u64, entry: u32) {
    if entry == 0 {
        spacetimedb::log::info!(
            "E_SUMMON_PET with no creature entry (p0=0) for {caster_guid} — skipped"
        );
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(caster) = entities.guid().find(caster_guid) else {
        return;
    };
    despawn_pets(ctx, caster_guid);
    match build_pet_entity(ctx, &caster, entry) {
        Some(pet) => {
            let pet_guid = pet.guid;
            super::spawn::insert_creature_entity(ctx, pet);
            super::mark_summoned_pet(ctx, pet_guid);
        }
        None => spacetimedb::log::info!(
            "E_SUMMON_PET: creature template {entry} not loaded — no pet summoned for {caster_guid}"
        ),
    }
}

pub(crate) fn owner_combat_target(ctx: &ReducerContext, owner: &WorldEntity) -> u64 {
    if let Some(row) = ctx.db.game_melee_attack().attacker_guid().find(owner.guid) {
        return row.target_guid;
    }
    if owner.unit_flags & constants::unit_flags::IN_COMBAT != 0 {
        return owner.target_guid;
    }
    0
}

pub(crate) fn may_attack(
    ctx: &ReducerContext,
    owner_guid: u64,
    pet_guid: u64,
    target_guid: u64,
) -> bool {
    ctx.db
        .game_world_entity()
        .guid()
        .find(owner_guid)
        .is_some_and(|owner| is_valid_pet_target(ctx, &owner, pet_guid, target_guid))
}

pub(crate) fn nearest_hostile_for(
    ctx: &ReducerContext,
    owner_guid: u64,
    pet_guid: u64,
) -> Option<u64> {
    let entities = ctx.db.game_world_entity();
    let owner = entities.guid().find(owner_guid)?;
    let pet = entities.guid().find(pet_guid)?;
    nearest_hostile_near(ctx, &owner, &pet)
}

pub(crate) fn cancel_attack_order(ctx: &ReducerContext, owner_guid: u64) {
    let (_, react, _) = pet_command_state(ctx, owner_guid);
    set_pet_command(ctx, owner_guid, CMD_FOLLOW, react, 0);
}

pub(crate) fn despawn_pet(ctx: &ReducerContext, pet_guid: u64) {
    let Some(pet) = ctx.db.game_world_entity().guid().find(pet_guid) else {
        return;
    };
    if pet.owner_guid == 0 || pet.guid != pet_guid_for(pet.owner_guid) {
        spacetimedb::log::info!("despawn_pet: refusing malformed pet row guid={pet_guid}");
        return;
    }
    let owner_guid = pet.owner_guid;
    despawn_pet_entity(ctx, pet);
    ctx.db.game_pet_command().owner_guid().delete(owner_guid);
}
