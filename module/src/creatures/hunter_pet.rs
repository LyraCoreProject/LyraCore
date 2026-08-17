//! Durable Hunter-pet identity and the wild-creature-to-live-pet transition.

use spacetimedb::{table, ReducerContext, Table, TimeDuration, Timestamp};

use crate::{
    game_creature_family, game_creature_spawn, game_creature_template, game_world_entity,
    WorldEntity,
};

const CLASS_HUNTER: u8 = 3;
const INITIAL_HAPPINESS: u32 = 166_500;
pub const PET_KIND_SUMMONED: u8 = 0;
pub const PET_KIND_HUNTER: u8 = 1;

/// The one durable Hunter-pet identity currently owned by a character. The table is module-private;
/// callers use the narrow owner/id lookups below.
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
}

/// Client-facing Hunter-pet state. This intentionally omits care timestamps, loyalty progress and
/// rename eligibility; the gateway needs only these fields to render the owner's live pet.
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

/// Explicit kind for a live owned creature. A missing row is the safe legacy default: any old
/// `owner_guid != 0` entity remains a summoned pet.
#[table(accessor = game_live_pet_kind)]
pub struct LivePetKind {
    #[primary_key]
    pub pet_guid: u64,
    /// Zero means summoned; a non-zero value names the durable Hunter identity.
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

pub fn hunter_pet_for_owner(ctx: &ReducerContext, owner_guid: u64) -> Option<HunterPet> {
    ctx.db
        .game_hunter_pet()
        .by_owner()
        .filter(&owner_guid)
        .next()
}

pub fn hunter_pet_by_id(ctx: &ReducerContext, pet_id: u64) -> Option<HunterPet> {
    ctx.db.game_hunter_pet().pet_id().find(pet_id)
}

/// Publish the bounded protocol projection after a durable identity write. Care/progression modules
/// call this at their identity-update seam so the gateway sees one owner-addressed delta.
pub fn publish_hunter_pet_protocol(ctx: &ReducerContext, pet: &HunterPet, live_pet_guid: u64) {
    let row = HunterPetProtocol {
        pet_id: pet.pet_id,
        owner_guid: pet.owner_guid,
        live_pet_guid,
        creature_entry: pet.creature_entry,
        name: pet.name.clone(),
        name_timestamp: (pet.created_at.to_micros_since_unix_epoch() / 1_000_000)
            .clamp(0, u32::MAX as i64) as u32,
        level: pet.level,
        pet_xp: pet.pet_xp,
        next_level_xp: super::pet_xp_to_next_level(pet.level),
        happiness: pet.happiness,
        loyalty_level: pet.loyalty_level,
    };
    let table = ctx.db.game_hunter_pet_protocol();
    if table.pet_id().find(row.pet_id).is_some() {
        table.pet_id().update(row);
    } else {
        table.insert(row);
    }
}

pub fn live_pet_kind(ctx: &ReducerContext, pet_guid: u64) -> PetKind {
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

/// Read-only tame gate used by spell completion before cost or any other write.
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

/// Convert the existing wild row into the Hunter's live pet and create its durable identity. A
/// durable row left by a despawned pet (logout, owner death, Dismiss, pet death) is replaced —
/// retaming is the slice's only recovery path. All validation is repeated immediately before the
/// first write, so a delayed completion cannot use stale target or owner state.
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

    // The preserved row retains entry/family-derived template, display, level and exact
    // current/max health ratio.
    target.owner_guid = caster.guid;
    target.faction_template = caster.faction_template;
    target.target_guid = 0;
    target.movement_flags = 0;
    target.leg_ends_ms = 0;
    target.wp_target = 0;
    target.combat_until_ms = 0;
    target.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;

    // Replace the wild guid with the existing owner-derived pet guid. Keep the authored spawn and
    // arm its next appearance in the future: taming retires this individual creature, not the world
    // spawn point that produced it. The live pet reaches the normal creature-create relay, including
    // summon fields and pet bar.
    let wild_guid = target.guid;
    let pet_guid = super::pet_guid_for(caster.guid);
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
    };
    // Implicit abandon on retame: drop the orphaned identity so the keyed insert cannot collide.
    ctx.db.game_hunter_pet().pet_id().delete(pet_id);
    let hunter_pet = ctx.db.game_hunter_pet().insert(hunter_pet);
    ctx.db.game_live_pet_kind().insert(LivePetKind {
        pet_guid,
        hunter_pet_id: pet_id,
    });
    publish_hunter_pet_protocol(ctx, &hunter_pet, pet_guid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn every_tame_gate_rejects_independently() {
        let cases: Vec<Box<dyn Fn(&mut TameEligibility)>> = vec![
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
    fn retame_after_despawn_replaces_the_orphaned_durable_identity() {
        // Logout, owner death, Dismiss and pet death all despawn the live pet but keep the durable
        // row. Recovery is a retame: the eligibility gates carry no durable-identity check, and
        // completion deletes the orphan before the keyed insert so the tame cannot collide.
        let fields =
            crate::test_scan::code_of(include_str!("hunter_pet.rs"), "struct TameEligibility");
        assert!(!fields.contains("durable"));
        let body = crate::test_scan::code_of(
            include_str!("hunter_pet.rs"),
            "pub(crate) fn tame_creature(",
        );
        let delete = body
            .find("game_hunter_pet().pet_id().delete(pet_id)")
            .expect("retame drops the orphaned durable identity");
        let insert = body
            .find("game_hunter_pet().insert(")
            .expect("tame inserts the fresh durable identity");
        assert!(delete < insert);
    }

    #[test]
    fn missing_and_explicit_zero_live_kind_are_summoned_not_hunter() {
        assert_eq!(classify_pet_kind(None), PetKind::Summoned);
        assert_eq!(classify_pet_kind(Some(0)), PetKind::Summoned);
        assert_eq!(classify_pet_kind(Some(42)), PetKind::Hunter { pet_id: 42 });
    }

    #[test]
    fn tame_retires_the_entity_but_preserves_and_rearms_its_authored_spawn() {
        let body = crate::test_scan::code_of(
            include_str!("hunter_pet.rs"),
            "pub(crate) fn tame_creature(",
        );
        assert!(body.contains("despawn_creature_entity(ctx, wild_guid)"));
        assert!(body.contains("game_creature_spawn().guid().update(spawn)"));
        assert!(!body.contains("game_creature_spawn().guid().delete(wild_guid)"));
    }

    #[test]
    fn protocol_projection_uses_the_pet_xp_curve() {
        let body = crate::test_scan::code_of(
            include_str!("hunter_pet.rs"),
            "pub fn publish_hunter_pet_protocol(",
        );
        assert!(body.contains("pet_xp_to_next_level(pet.level)"));
        assert!(!body.contains("xp::xp_to_next_level(pet.level)"));
    }
}
