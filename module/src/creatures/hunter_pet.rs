use lyracore_shared::pet::{is_valid_pet_name, pet_guid_for, PET_NAME_INVALID};
use spacetimedb::{reducer, table, ReducerContext, Table, TimeDuration, Timestamp};

use crate::{
    game_creature_family, game_creature_spawn, game_creature_template, game_world_entity,
    WorldEntity,
};

const CLASS_HUNTER: u8 = 3;
const INITIAL_HAPPINESS: u32 = 166_500;
// In 1.12, Rename Pet eligibility uses UNIT_FIELD_FLAGS rather than UNIT_FIELD_BYTES_2.
const UNIT_FLAG_PET_RENAME: u32 = 0x0000_0010;

#[derive(Clone)]
#[table(
    accessor = game_hunter_pet,
    index(accessor = by_owner, btree(columns = [owner_guid]))
)]
pub struct HunterPet {
    #[primary_key]
    pub pet_id: u64,
    pub owner_guid: u64,
    pub creature_entry: u32,
    pub creature_family: u32,
    pub name: String,
    pub rename_allowed: bool,
    pub level: u32,
    pub pet_xp: u32,
    pub loyalty_level: u8,
    pub loyalty_progress: u32,
    pub happiness: u32,
    pub created_at: Timestamp,
    pub care_updated_at: Timestamp,
    pub loyalty_updated_at: Timestamp,
    /// Name-cache stamp in Unix seconds; zero falls back to `created_at` for existing rows.
    #[default(0u32)]
    pub name_timestamp: u32,
}

#[derive(PartialEq, Eq)]
#[table(
    accessor = game_hunter_pet_protocol,
    public,
    index(accessor = by_owner, btree(columns = [owner_guid]))
)]
pub struct HunterPetProtocol {
    #[primary_key]
    pub pet_id: u64,
    pub owner_guid: u64,
    pub live_pet_guid: u64,
    pub creature_entry: u32,
    pub name: String,
    pub name_timestamp: u32,
    pub level: u32,
    pub pet_xp: u32,
    pub next_level_xp: u32,
    pub happiness: u32,
    pub loyalty_level: u8,
}

crate::character_owned!(delete, fn sweep_delete_game_hunter_pet(ctx, character_guid) {
    if let Some(row) = ctx.db.game_hunter_pet().by_owner().filter(&character_guid).next() {
        ctx.db.game_hunter_pet().pet_id().delete(row.pet_id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_hunter_pet(ctx, character_guid, io) {
    table = game_hunter_pet,
    by = by_owner,
    keep_key,
});
crate::character_owned!(delete, fn sweep_delete_game_hunter_pet_protocol(ctx, character_guid) {
    if let Some(row) = ctx.db.game_hunter_pet_protocol().by_owner().filter(&character_guid).next() {
        ctx.db.game_hunter_pet_protocol().pet_id().delete(row.pet_id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_hunter_pet_protocol(ctx, character_guid, io) {
    table = game_hunter_pet_protocol,
    by = by_owner,
    keep_key,
});

#[table(accessor = game_live_pet_kind)]
pub struct LivePetKind {
    #[primary_key]
    pub pet_guid: u64,
    pub hunter_pet_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetKind {
    Summoned,
    Hunter { pet_id: u64 },
}

fn classify_pet_kind(hunter_pet_id: Option<u64>) -> PetKind {
    match hunter_pet_id {
        Some(id) if id != 0 => PetKind::Hunter { pet_id: id },
        _ => PetKind::Summoned,
    }
}

pub(crate) fn hunter_pet_for_owner(ctx: &ReducerContext, owner_guid: u64) -> Option<HunterPet> {
    ctx.db
        .game_hunter_pet()
        .by_owner()
        .filter(&owner_guid)
        .next()
}

pub(crate) fn publish_hunter_pet_protocol(
    ctx: &ReducerContext,
    pet: &HunterPet,
    live_pet_guid: u64,
) {
    if let Some(mut live) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(live_pet_guid)
        .filter(|live| {
            live.owner_guid == pet.owner_guid
                && live_pet_kind(ctx, live.guid) == (PetKind::Hunter { pet_id: pet.pet_id })
        })
    {
        let flags = if pet.rename_allowed {
            live.unit_flags | UNIT_FLAG_PET_RENAME
        } else {
            live.unit_flags & !UNIT_FLAG_PET_RENAME
        };
        if flags != live.unit_flags {
            live.unit_flags = flags;
            live.update(ctx);
        }
    }
    let row = HunterPetProtocol {
        pet_id: pet.pet_id,
        owner_guid: pet.owner_guid,
        live_pet_guid,
        creature_entry: pet.creature_entry,
        name: pet.name.clone(),
        name_timestamp: effective_name_timestamp(pet),
        level: pet.level,
        pet_xp: pet.pet_xp,
        next_level_xp: super::pet_xp_to_next_level(pet.level),
        happiness: pet.happiness,
        loyalty_level: pet.loyalty_level,
    };
    let table = ctx.db.game_hunter_pet_protocol();
    let previous = table.pet_id().find(row.pet_id);
    if previous.as_ref() == Some(&row) {
        return;
    }
    let row = if previous.is_some() {
        table.pet_id().update(row)
    } else {
        table.insert(row)
    };
    crate::native::hunter_pet_changed(ctx, previous.as_ref(), &row);
}

pub(crate) fn live_pet_kind(ctx: &ReducerContext, pet_guid: u64) -> PetKind {
    classify_pet_kind(
        ctx.db
            .game_live_pet_kind()
            .pet_guid()
            .find(pet_guid)
            .map(|row| row.hunter_pet_id),
    )
}

pub(crate) fn mark_summoned_pet(ctx: &ReducerContext, pet_guid: u64) {
    let table = ctx.db.game_live_pet_kind();
    table.pet_guid().delete(pet_guid);
    table.insert(LivePetKind {
        pet_guid,
        hunter_pet_id: 0,
    });
}

pub(crate) fn clear_live_pet_kind(ctx: &ReducerContext, pet_guid: u64) {
    ctx.db.game_live_pet_kind().pet_guid().delete(pet_guid);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TameEligibility {
    caster_is_player: bool,
    caster_owned: bool,
    caster_class: u8,
    caster_dead: bool,
    target_exists: bool,
    target_is_player: bool,
    target_dead: bool,
    target_owned: bool,
    target_level: u32,
    owner_level: u32,
    has_live_pet: bool,
    family_present: bool,
    family_tameable: bool,
}

impl TameEligibility {
    fn validate(&self) -> Result<(), &'static str> {
        if !self.caster_is_player || self.caster_class != CLASS_HUNTER {
            return Err("only a Hunter can tame a creature");
        }
        if !self.caster_owned {
            return Err("the Hunter has no matching durable owner");
        }
        if self.caster_dead {
            return Err("a dead Hunter cannot tame a creature");
        }
        if !self.target_exists {
            return Err("tame target is not in world");
        }
        if self.target_is_player {
            return Err("players cannot be tamed");
        }
        if self.target_dead {
            return Err("a dead creature cannot be tamed");
        }
        if self.target_owned {
            return Err("an owned or charmed creature cannot be tamed");
        }
        if self.target_level > self.owner_level {
            return Err("the creature is higher level than the Hunter");
        }
        if self.has_live_pet {
            return Err("the Hunter already has an active pet");
        }
        if !self.family_present {
            return Err("the creature has no pet family");
        }
        if !self.family_tameable {
            return Err("the creature family is not tameable");
        }
        Ok(())
    }
}

fn eligibility(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target: Option<&WorldEntity>,
) -> TameEligibility {
    let template = target.and_then(|row| ctx.db.game_creature_template().entry().find(row.entry));
    let family = template
        .as_ref()
        .filter(|row| row.creature_family != 0)
        .and_then(|row| {
            ctx.db
                .game_creature_family()
                .family_id()
                .find(row.creature_family as u32)
        });
    TameEligibility {
        caster_is_player: caster.is_player(),
        caster_owned: crate::helpers::character_by_guid(ctx, caster.guid)
            .is_some_and(|row| row.owner_identity == caster.owner_identity),
        caster_class: caster.class(),
        caster_dead: caster.dead,
        target_exists: target.is_some(),
        target_is_player: target.is_some_and(WorldEntity::is_player),
        target_dead: target.is_some_and(|row| row.dead),
        target_owned: target.is_some_and(|row| row.owner_guid != 0),
        target_level: target.map_or(0, |row| row.level),
        owner_level: caster.level,
        has_live_pet: super::pet_of(ctx, caster.guid).is_some(),
        family_present: family.is_some(),
        family_tameable: family.is_some_and(|row| row.pet_talent_type >= 0),
    }
}

pub(crate) fn validate_tame(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target_guid: u64,
) -> Result<(), String> {
    let target = ctx.db.game_world_entity().guid().find(target_guid);
    eligibility(ctx, caster, target.as_ref())
        .validate()
        .map_err(str::to_string)
}

pub(crate) fn tame_creature(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let caster = entities
        .guid()
        .find(caster_guid)
        .ok_or_else(|| "tame caster is not in world".to_string())?;
    let mut target = entities
        .guid()
        .find(target_guid)
        .ok_or_else(|| "tame target is not in world".to_string())?;
    validate_tame(ctx, &caster, target_guid)?;

    let template = ctx
        .db
        .game_creature_template()
        .entry()
        .find(target.entry)
        .ok_or_else(|| "tame target has no creature template".to_string())?;
    let family_id = template.creature_family as u32;
    let pet_id = caster.guid;

    target.owner_guid = caster.guid;
    target.faction_template = caster.faction_template;
    target.target_guid = 0;
    target.movement_flags = 0;
    target.leg_ends_ms = 0;
    target.wp_target = 0;
    target.combat_until_ms = 0;
    target.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;
    target.unit_flags |= UNIT_FLAG_PET_RENAME;

    // Taming retires this wild life; preserve its authored spawn and arm its future appearance.
    let wild_guid = target.guid;
    let pet_guid = pet_guid_for(caster.guid);
    let creature_entry = target.entry;
    let level = target.level;
    super::despawn_creature_entity(ctx, wild_guid);
    if let Some(mut spawn) = ctx.db.game_creature_spawn().guid().find(wild_guid) {
        let delay_micros = if spawn.respawn_secs == 0 {
            crate::combat::RESPAWN_MICROS
        } else {
            i64::from(spawn.respawn_secs).saturating_mul(1_000_000)
        };
        spawn.respawn_at = ctx
            .timestamp
            .checked_add(TimeDuration::from_micros(delay_micros))
            .unwrap_or_else(|| super::timer_never(ctx));
        spawn.despawn_at = super::timer_never(ctx);
        ctx.db.game_creature_spawn().guid().update(spawn);
    }
    target.guid = pet_guid;
    super::spawn::insert_creature_entity(ctx, target);

    let now = ctx.timestamp;
    let hunter_pet = HunterPet {
        pet_id,
        owner_guid: caster.guid,
        creature_entry,
        creature_family: family_id,
        name: template.name,
        rename_allowed: true,
        level,
        pet_xp: 0,
        loyalty_level: 1,
        loyalty_progress: 0,
        happiness: INITIAL_HAPPINESS,
        created_at: now,
        care_updated_at: now,
        loyalty_updated_at: now,
        name_timestamp: unix_seconds(now),
    };
    ctx.db.game_hunter_pet().pet_id().delete(pet_id);
    let hunter_pet = ctx.db.game_hunter_pet().insert(hunter_pet);
    ctx.db.game_live_pet_kind().insert(LivePetKind {
        pet_guid,
        hunter_pet_id: pet_id,
    });
    publish_hunter_pet_protocol(ctx, &hunter_pet, pet_guid);
    Ok(())
}

fn unix_seconds(at: Timestamp) -> u32 {
    (at.to_micros_since_unix_epoch() / 1_000_000).clamp(0, i64::from(u32::MAX)) as u32
}

fn effective_name_timestamp(pet: &HunterPet) -> u32 {
    if pet.name_timestamp == 0 {
        unix_seconds(pet.created_at)
    } else {
        pet.name_timestamp
    }
}

/// Advance the name-cache timestamp even for a same-second rename, up to the protocol limit.
fn next_name_timestamp(previous: u32, now: u32) -> u32 {
    now.max(previous.saturating_add(1))
}

/// See docs/benilla-compatibility.md, Combat feedback, for rename Gates and client replies.
pub(crate) fn rename_hunter_pet(
    ctx: &ReducerContext,
    owner_guid: u64,
    pet_guid: u64,
    name: &str,
) -> Result<(), String> {
    let live = super::pet_of(ctx, owner_guid)
        .filter(|pet| pet.guid == pet_guid)
        .ok_or_else(|| format!("pet rename: {owner_guid} has no live pet {pet_guid}"))?;
    let PetKind::Hunter { pet_id } = live_pet_kind(ctx, live.guid) else {
        return Err(format!("pet rename: {pet_guid} is not a Hunter pet"));
    };
    let mut pet = ctx
        .db
        .game_hunter_pet()
        .pet_id()
        .find(pet_id)
        .filter(|pet| pet.owner_guid == owner_guid)
        .ok_or_else(|| format!("pet rename: {owner_guid} does not own Hunter pet {pet_id}"))?;
    if !pet.rename_allowed {
        return Err(format!(
            "pet rename: Hunter pet {pet_id} was already renamed"
        ));
    }
    if !is_valid_pet_name(name) {
        return Err(PET_NAME_INVALID.to_string());
    }

    pet.name = name.to_string();
    pet.rename_allowed = false;
    pet.name_timestamp =
        next_name_timestamp(effective_name_timestamp(&pet), unix_seconds(ctx.timestamp));
    let pet = ctx.db.game_hunter_pet().pet_id().update(pet);
    publish_hunter_pet_protocol(ctx, &pet, pet_guid);
    Ok(())
}

#[reducer]
pub fn gw_pet_rename(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    pet_guid: u64,
    name: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let owner_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    crate::helpers::acting_entity_by_guid(ctx, owner_guid)
        .ok_or_else(|| "mover not in world".to_string())?;
    rename_hunter_pet(ctx, owner_guid, pet_guid, &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_name_timestamp_advances_until_the_protocol_limit() {
        assert_eq!(next_name_timestamp(100, 250), 250);
        assert_eq!(next_name_timestamp(250, 250), 251);
        assert_eq!(next_name_timestamp(300, 250), 301);
        assert_eq!(next_name_timestamp(u32::MAX, 250), u32::MAX);
    }

    fn eligible() -> TameEligibility {
        TameEligibility {
            caster_is_player: true,
            caster_owned: true,
            caster_class: CLASS_HUNTER,
            caster_dead: false,
            target_exists: true,
            target_is_player: false,
            target_dead: false,
            target_owned: false,
            target_level: 9,
            owner_level: 10,
            has_live_pet: false,
            family_present: true,
            family_tameable: true,
        }
    }

    #[test]
    fn valid_tame_passes_every_gate() {
        assert_eq!(eligible().validate(), Ok(()));
    }

    type BreakGate = Box<dyn Fn(&mut TameEligibility)>;

    #[test]
    fn every_tame_gate_rejects_independently() {
        let cases: Vec<BreakGate> = vec![
            Box::new(|v| v.caster_class = 9),
            Box::new(|v| v.caster_is_player = false),
            Box::new(|v| v.caster_owned = false),
            Box::new(|v| v.caster_dead = true),
            Box::new(|v| v.target_exists = false),
            Box::new(|v| v.target_is_player = true),
            Box::new(|v| v.target_dead = true),
            Box::new(|v| v.target_owned = true),
            Box::new(|v| v.target_level = v.owner_level + 1),
            Box::new(|v| v.has_live_pet = true),
            Box::new(|v| v.family_present = false),
            Box::new(|v| v.family_tameable = false),
        ];
        for mutate in cases {
            let mut value = eligible();
            mutate(&mut value);
            assert!(
                value.validate().is_err(),
                "invalid tame unexpectedly passed: {value:?}"
            );
        }
    }

    #[test]
    fn missing_and_explicit_zero_live_kind_are_summoned_not_hunter() {
        assert_eq!(classify_pet_kind(None), PetKind::Summoned);
        assert_eq!(classify_pet_kind(Some(0)), PetKind::Summoned);
        assert_eq!(classify_pet_kind(Some(42)), PetKind::Hunter { pet_id: 42 });
    }
}
