//! Warlock PET system (Tier 3b) — a SUMMONED pet is a normal creature `game_world_entity` (no PLAYER
//! bit) with `owner_guid` set to the controlling player. It rides the EXISTING creature machinery: the
//! chase/melee/swing/relay passes carry it for free; the only pet-specific code is here —
//!   - `summon_pet`     : the E_SUMMON_PET effect handler — despawn the old pet, then spawn the new
//!     one at the caster (owner_guid = caster). ONE pet per owner.
//!   - `build_pet_entity`: wraps `build_creature_entity` (template → row) + stamps owner_guid +
//!     positions the pet at the owner. It inserts ONLY the live entity — NO `game_creature_spawn`
//!     row, so the respawn/decay/return-home passes never touch a pet.
//!   - `despawn_pets`   : delete the owner's pet (free its melee row + threat). Called on owner
//!     logout/death + on a re-summon.
//!
//! The PER-TICK decision (take the owner's foe, stand down, trail the owner, despawn) is NOT here —
//! it is the behavior cycle's pet phase (`creatures::cycle`), which reads this module through the
//! small `pub(crate)` queries below. What stays here is the pet's identity: its tables, its bar
//! state, its guid encoding, and summon/despawn.
//!
//! Everything is GENERIC over `owner_guid != 0` + the `E_SUMMON_PET` kind — NO per-spell-id branch. The
//! Imp CASTS Firebolt (3110) via DATA, not code: the importer emits a `game_creature_cast` row keyed to
//! the Imp's entry (416) from cmangos `creature_template_spells`, and the cycle's cast phase (run right
//! after the pet phase in the same sense tick) fires it at the pet's melee target with ZERO pet-specific
//! engine code — the SAME engine + SAME cast table the wild caster mobs use. The Imp KEEPS its melee row
//! (armed by the pet phase) so it still chases/swings as a fallback; `ranged_spell_id` stays 0 (the cast
//! is independent of the wand/ranged-auto-attack field). [entity]

use lyracore_shared::constants;
#[cfg(feature = "debug_reducers")]
use spacetimedb::reducer;
use spacetimedb::{table, ReducerContext, Table};

use crate::{
    game_creature_template, game_entity_motion, game_melee_attack, game_world_entity, WorldEntity,
};

use super::spawn::{build_creature_entity, CreatureSpawn};

// ── Pet command bar (CMSG_PET_ACTION → stay/follow/attack/dismiss + passive/defensive/aggressive) ──
/// The player's pet-bar command/react state, in a MODULE-ONLY table (the gateway never reads it — it
/// only relays the resulting entity/melee changes, so no gateway binding is needed). Keyed by
/// `owner_guid` (one pet per owner). An ABSENT row IS the vanilla default (Follow + Defensive), which
/// is byte-for-byte the derived behavior pets had before the bar — so every existing pet is unchanged
/// until the player presses a bar button. Ephemeral: cleared on despawn/logout with the pet (vanilla
/// pets persist their state; that's a durable-per-character follow-up, not v1). [entity]
#[table(accessor = game_pet_command)]
pub struct PetCommand {
    #[primary_key]
    pub owner_guid: u64,
    /// COMMAND_* — governs movement + explicit attack.
    pub command: u8,
    /// REACT_* — governs auto-engage.
    pub react: u8,
    /// the ATTACK command's target (0 otherwise).
    pub command_target: u64,
}

// Character-owned sweep: a pet-command row is ephemeral per-owner state keyed by the owner character's
// guid, removed with the character. Private/module-only (no `owner_identity`) → `delete` only.
crate::character_owned!(delete, fn sweep_delete_game_pet_command(ctx, character_guid) {
    ctx.db.game_pet_command().owner_guid().delete(character_guid);
});
// CROSS-DATABASE transport (issue #19): the row is EPHEMERAL bar state for a live pet ENTITY, and
// the pet entity does not cross the boundary (it is a `game_world_entity`, not character-owned) —
// it despawns with the owner and is re-summoned on the far side, which re-derives the default
// Follow+Defensive the absent row already means. Not transported, by decision.
crate::character_owned!(not_transported, fn sweep_transfer_game_pet_command());

// Wire numbering — gtker PetCommandState / mangos CommandStates. MUST match CMSG_PET_ACTION.data + the
// SMSG_PET_SPELLS bar packing (build_pet_spells); do NOT reorder. `pub(crate)` for the behavior
// cycle's pet phase, which reads a stored id back into its own command/react enums.
pub(crate) const CMD_STAY: u8 = 0;
const CMD_FOLLOW: u8 = 1;
pub(crate) const CMD_ATTACK: u8 = 2;
const CMD_DISMISS: u8 = 3;
// gtker PetReactState / mangos ReactStates.
pub(crate) const REACT_PASSIVE: u8 = 0;
const REACT_DEFENSIVE: u8 = 1;
pub(crate) const REACT_AGGRESSIVE: u8 = 2;
// The flag byte (high 8 bits) of the packed CMSG_PET_ACTION.data (mangos ActiveStates).
const ACT_COMMAND: u8 = 0x07;
const ACT_REACTION: u8 = 0x06;
/// How near (yd², squared) an AGGRESSIVE idle pet proactively seeks a hostile around ITSELF. (10 yd)².
const PET_AGGRO_SEEK_SQ: f32 = 100.0;

/// The owner's stored pet command/react state, or the vanilla default (Follow + Defensive, no target)
/// when no row exists — the absent-row default is exactly the derived behavior pets had before the
/// bar existed. Read by the behavior cycle's pet phase. [entity]
pub(crate) fn pet_command_state(ctx: &ReducerContext, owner_guid: u64) -> (u8, u8, u64) {
    ctx.db
        .game_pet_command()
        .owner_guid()
        .find(owner_guid)
        .map(|r| (r.command, r.react, r.command_target))
        .unwrap_or((CMD_FOLLOW, REACT_DEFENSIVE, 0))
}

/// Upsert the owner's pet command/react state.
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

/// A valid thing for a pet to attack: exists, alive, not the pet/owner/a player, and not friendly to
/// the owner. Mirrors the inline guard the original auto-assist used. [entity]
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

/// For an AGGRESSIVE pet whose owner is idle: the nearest alive hostile (non-player, non-pet,
/// non-friendly) creature within `PET_AGGRO_SEEK_SQ` of the PET, on the pet's map/instance. `None` if
/// nothing is in range. A full-table scan (pets are rare — one per online warlock). [entity]
fn nearest_hostile_near(
    ctx: &ReducerContext,
    owner: &WorldEntity,
    pet: &WorldEntity,
) -> Option<u64> {
    let mut best: Option<(u64, f32)> = None;
    for e in ctx.db.game_world_entity().iter() {
        if e.guid == pet.guid || e.owner_guid != 0 || e.is_player() || e.dead {
            continue; // skip the pet itself, other pets, players, corpses
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

/// Shared core (also driven by `debug_pet_command` for headless tests): decode + apply one pet-bar
/// action for `owner_guid`. Errors (no pet, unknown id) are transient — the gateway swallows them.
pub(crate) fn apply_pet_command(
    ctx: &ReducerContext,
    owner_guid: u64,
    data: u32,
    target_guid: u64,
) -> Result<(), String> {
    // Ownership gate: the caller must actually have a pet.
    pet_of(ctx, owner_guid).ok_or_else(|| "no pet".to_string())?;
    let flag = ((data >> 24) & 0xFF) as u8;
    let id = (data & 0x00FF_FFFF) as u8; // command/react ids are small (0..3)
    let (cmd, react, cmd_target) = pet_command_state(ctx, owner_guid);
    match flag {
        ACT_COMMAND => match id {
            CMD_STAY => set_pet_command(ctx, owner_guid, CMD_STAY, react, 0),
            CMD_FOLLOW => set_pet_command(ctx, owner_guid, CMD_FOLLOW, react, 0),
            CMD_ATTACK => set_pet_command(ctx, owner_guid, CMD_ATTACK, react, target_guid),
            CMD_DISMISS => despawn_pets(ctx, owner_guid), // despawns the pet + clears the row
            other => return Err(format!("unknown pet command id {other}")),
        },
        ACT_REACTION => {
            if id > REACT_AGGRESSIVE {
                return Err(format!("unknown pet react id {id}"));
            }
            // Change react; keep the current command + its attack target.
            set_pet_command(ctx, owner_guid, cmd, id, cmd_target);
        }
        // A bar spell-cast / autocast toggle (ACT_ENABLED/DISABLED/PASSIVE) — v1 no-op (the Imp casts
        // Firebolt via the data-driven game_creature_cast path, not the bar).
        _ => {}
    }
    Ok(())
}

/// Headless drive of a pet-bar action for `owner_guid` — the machine-test counterpart to the
/// `pet_command` reducer (which is `ctx.sender`-bound and so undrivable from the CLI identity). `data`
/// packed values (decimal): STAY=117440512, FOLLOW=117440513, ATTACK=117440514 (+target),
/// DISMISS=117440515, PASSIVE=100663296, DEFENSIVE=100663297, AGGRESSIVE=100663298. [entity]
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_pet_command(
    ctx: &ReducerContext,
    owner_guid: u64,
    data: u32,
    target_guid: u64,
) -> Result<(), String> {
    apply_pet_command(ctx, owner_guid, data, target_guid)
}

/// Does the keyed candidate name the requested non-zero owner? Kept pure so the fail-closed half of
/// the lookup can be tested without constructing a `ReducerContext`.
fn pet_owner_matches(owner_guid: u64, candidate_owner: Option<u64>) -> bool {
    owner_guid != 0 && candidate_owner == Some(owner_guid)
}

/// The owner's pet, if any. Pet GUIDs are deterministic, so this is one primary-key read followed by
/// an owner check. A missing row or a row that names another owner fails closed. `owner_guid == 0`
/// returns before looking in the wild-creature namespace. [entity]
pub fn pet_of(ctx: &ReducerContext, owner_guid: u64) -> Option<WorldEntity> {
    if owner_guid == 0 {
        return None;
    }
    ctx.db
        .game_world_entity()
        .guid()
        .find(pet_guid_for(owner_guid))
        .filter(|pet| pet_owner_matches(owner_guid, Some(pet.owner_guid)))
}

/// Remove a live pet row that has already been resolved. Cleanup order is load-bearing: combat and
/// threat first, then staged motion, persisted motion, and finally the entity deletion relay.
fn despawn_pet_entity(ctx: &ReducerContext, pet: WorldEntity) {
    crate::combat::disengage(ctx, pet.guid); // free its MeleeAttack row + threat
    crate::motion::drop_pending(ctx, pet.guid); // the staged payload dies with it too
    ctx.db.game_entity_motion().guid().delete(pet.guid); // motion row dies with the entity (2.1)
    ctx.db.game_world_entity().guid().delete(pet.guid);
}

/// Despawn the player's pet (if any): free its melee engagement + threat (`disengage`), then DELETE the
/// live entity. The `on_delete` relay fires `SMSG_DESTROY_OBJECT` for free (exactly like creature decay),
/// so the pet vanishes for every client. Called on owner logout/disconnect (`remove_from_world`), owner
/// death (`repop`), and at the top of `summon_pet` (a re-summon replaces the old pet). Idempotent — a
/// no-op when the owner has no pet. [entity]
pub fn despawn_pets(ctx: &ReducerContext, owner_guid: u64) {
    if let Some(pet) = pet_of(ctx, owner_guid) {
        despawn_pet_entity(ctx, pet);
    }
    // Clear any pet-bar command state (the pet is ephemeral — no stale row across re-summon/logout;
    // a re-summon thus resets to the Follow + Defensive default, matching vanilla). This also covers
    // an owner-keyed no-op despawn after the live row has already disappeared.
    ctx.db.game_pet_command().owner_guid().delete(owner_guid);
}

/// Build the live pet `game_world_entity` for `entry`, owned by `owner`, positioned AT the owner. Wraps
/// `build_creature_entity` against a TRANSIENT spawn (the owner's position) so the pet reuses the full
/// creature build (faction/level/health roll/display/swing time) — then stamps `owner_guid` and a fresh
/// guid. NOTE: the transient `CreatureSpawn` is NEVER inserted (a pet has no durable spawn row — that
/// drives respawn/decay/return-home, which a despawned-on-logout pet must not do); it exists only to feed
/// the builder. Returns `None` if the creature template isn't loaded (the caller logs + no-ops). [entity]
fn build_pet_entity(ctx: &ReducerContext, owner: &WorldEntity, entry: u32) -> Option<WorldEntity> {
    let tmpl = ctx.db.game_creature_template().entry().find(entry)?;
    let pet_guid = pet_guid_for(owner.guid);
    // Transient spawn at the owner's position — feeds the builder, never inserted (no respawn/decay row).
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
        movement_type: 0, // IDLE — irrelevant (a pet has no spawn row, so the wander/return passes skip it)
        respawn_secs: 0, // irrelevant — this transient spawn is never inserted, so no decay/respawn pass ever reads it
    };
    let mut pet = build_creature_entity(&spawn, &tmpl, ctx.random(), 0);
    pet.owner_guid = owner.guid; // THIS is what makes it a pet (the cycle's pet phase keys on owner_guid != 0)
    Some(pet)
}

/// HIGHGUID_UNIT (0xF130) — the high 16 bits of every creature guid. The 5875 client classifies an object
/// (Unit vs other) from its guid's HIGHGUID type, so a pet MUST carry it or the client can't treat it as a
/// unit (broken nameplate/targeting/pet-bar), exactly like the importer `world_guid` / `seed` / `debug`.
const HIGHGUID_UNIT: u64 = 0xF130;

/// The deterministic pet guid for an owner: `HIGHGUID_UNIT` in the high bits (so the client treats it as a
/// Unit) + the owner guid in the low 48. The owner is a small player guid (< 2^24); a real creature's low is
/// `(entry << 24) | db_guid` (always >= 1<<24 since entry >= 1), so the pet's low never collides with a
/// creature. Stable so the keyed `pet_of` read, despawn delete, and `on_delete` relay agree. Pure.
fn pet_guid_for(owner_guid: u64) -> u64 {
    (HIGHGUID_UNIT << 48) | (owner_guid & 0x0000_FFFF_FFFF_FFFF)
}

/// E_SUMMON_PET handler: summon a persistent pet `entry` owned by `caster_guid` (Summon Imp → an Imp).
/// Despawn any existing pet FIRST (one pet per owner — a re-summon replaces it), then build + insert the
/// new pet at the caster. No-op if the caster left the world or the creature template isn't loaded (logs).
/// Generic over the kind — `entry` is the effect's `p0` (a game_creature_template entry); the engine never
/// names Summon Imp. [entity]
pub fn summon_pet(ctx: &ReducerContext, caster_guid: u64, entry: u32) {
    if entry == 0 {
        spacetimedb::log::info!(
            "E_SUMMON_PET with no creature entry (p0=0) for {caster_guid} — skipped"
        );
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(caster) = entities.guid().find(caster_guid) else {
        return; // caster left the world mid-cast
    };
    // Re-summon: replace any existing pet of this owner (vanilla — one pet at a time).
    despawn_pets(ctx, caster_guid);
    match build_pet_entity(ctx, &caster, entry) {
        Some(pet) => {
            super::spawn::insert_creature_entity(ctx, pet);
        }
        None => spacetimedb::log::info!(
            "E_SUMMON_PET: creature template {entry} not loaded — no pet summoned for {caster_guid}"
        ),
    }
}

/// The enemy `owner` is fighting, or 0 when it is idle — what a DEFENSIVE or AGGRESSIVE pet piles
/// onto. A live `game_melee_attack` row IS the fight, so its target wins; a bare selection counts
/// only while the owner carries the in-combat flag, so a pet does not jump a peacefully targeted
/// NPC. [entity]
pub(crate) fn owner_combat_target(ctx: &ReducerContext, owner: &WorldEntity) -> u64 {
    if let Some(row) = ctx.db.game_melee_attack().attacker_guid().find(owner.guid) {
        return row.target_guid;
    }
    if owner.unit_flags & constants::unit_flags::IN_COMBAT != 0 {
        return owner.target_guid;
    }
    0
}

/// May this pet attack `target_guid` on its owner's behalf? Resolves the owner, then applies the
/// exists/alive/not-a-player/not-friendly guard — the pet's swing path has no faction gate of its
/// own, so this is load-bearing. [entity]
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

/// The nearest hostile an AGGRESSIVE pet seeks out around ITSELF, within `PET_AGGRO_SEEK_SQ`.
/// `None` when either row is gone or nothing is in range. [entity]
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

/// Drop a stale ATTACK order — its foe died or vanished — keeping the react stance, so the pet
/// resumes following instead of standing over a corpse. [entity]
pub(crate) fn cancel_attack_order(ctx: &ReducerContext, owner_guid: u64) {
    let (_, react, _) = pet_command_state(ctx, owner_guid);
    set_pet_command(ctx, owner_guid, CMD_FOLLOW, react, 0);
}

/// Despawn one pet BY ITS OWN GUID — the cycle's despawn action. Resolve the row once, then validate
/// its owner and deterministic identity before cleanup. A wild or malformed row fails closed. [entity]
pub(crate) fn despawn_pet(ctx: &ReducerContext, pet_guid: u64) {
    let Some(pet) = ctx.db.game_world_entity().guid().find(pet_guid) else {
        return;
    };
    if pet.owner_guid == 0 || pet.guid != pet_guid_for(pet.owner_guid) {
        return;
    }
    let owner_guid = pet.owner_guid;
    despawn_pet_entity(ctx, pet);
    ctx.db.game_pet_command().owner_guid().delete(owner_guid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_guid_is_highguid_unit_and_reversible() {
        let owner = 0x0000_0000_0000_002A_u64; // a normal low-range player guid
        let pet = pet_guid_for(owner);
        assert_eq!(
            pet >> 48,
            HIGHGUID_UNIT,
            "pet guid carries HIGHGUID_UNIT so the client treats it as a Unit"
        );
        assert_eq!(
            pet & 0x0000_FFFF_FFFF_FFFF,
            owner,
            "the low 48 bits recover the owner"
        );
        assert_ne!(pet, owner, "pet guid is distinct from the owner");
    }

    #[test]
    fn pet_guid_uses_only_the_owner_low_48_bits() {
        let owner = 0xABCD_1234_5678_9ABC_u64;
        assert_eq!(
            pet_guid_for(owner),
            (HIGHGUID_UNIT << 48) | 0x0000_1234_5678_9ABC
        );
    }

    #[test]
    fn pet_selection_fails_closed() {
        assert!(
            !pet_owner_matches(42, None),
            "a missing keyed row is no pet"
        );
        assert!(
            !pet_owner_matches(42, Some(7)),
            "a keyed row naming another owner is no pet"
        );
        assert!(
            !pet_owner_matches(0, Some(0)),
            "owner zero is never a pet owner"
        );
        assert!(
            pet_owner_matches(42, Some(42)),
            "the keyed row for the requested owner is selected"
        );
    }
}
