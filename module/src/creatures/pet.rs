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
//!   - `pass_pet`       : the tick branch (run in the sensing block) — owner-in-combat → arm a
//!     pet→owner-target melee row (the existing chase + swing tick do the rest); owner-idle → emit a
//!     follow RUN-leg toward the owner (the chase-pass geometry, "home = owner").
//!
//! Everything is GENERIC over `owner_guid != 0` + the `E_SUMMON_PET` kind — NO per-spell-id branch. The
//! Imp CASTS Firebolt (3110) via DATA, not code: the importer emits a `game_creature_cast` row keyed to
//! the Imp's entry (416) from cmangos `creature_template_spells`, and `pass_cast` (run right after
//! `pass_pet` in the same sense tick) fires it at the pet's melee target with ZERO pet-specific engine
//! code — the SAME engine + SAME cast table the wild caster mobs use. The Imp KEEPS its melee row
//! (armed by `pass_pet`) so it still chases/swings as a fallback; `ranged_spell_id` stays 0 (the cast
//! is independent of the wand/ranged-auto-attack field). [entity]

use lyracore_shared::{constants, spatial};
use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::helpers::entity_by_owner;
use crate::{
    game_creature_template, game_entity_motion, game_melee_attack, game_world_entity, MeleeAttack,
    WorldEntity,
};

use super::spawn::{build_creature_entity, CreatureSpawn};
use super::tick::emit_move_spline;

/// A pet that is FARTHER than this (yards, squared) from its owner walks toward the owner when idle —
/// the "follow band". Tuned so the pet trails a few yards behind (vanilla pets hover near the owner) and
/// the band sits comfortably outside `CHASE_MELEE_SQ` ((5 yd)²) so a following pet doesn't jitter on top
/// of the owner. (8 yd)².
const PET_FOLLOW_BAND_SQ: f32 = 64.0;

// ── Pet command bar (CMSG_PET_ACTION → stay/follow/attack/dismiss + passive/defensive/aggressive) ──
/// The player's pet-bar command/react state, in a MODULE-ONLY table (the gateway never reads it — it
/// only relays the resulting entity/melee changes, so no gateway binding is needed). Keyed by
/// `owner_guid` (one pet per owner). An ABSENT row IS the vanilla default (Follow + Defensive), which
/// is byte-for-byte the original derived `pass_pet` behavior — so every existing pet is unchanged
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
// SMSG_PET_SPELLS bar packing (build_pet_spells); do NOT reorder.
const CMD_STAY: u8 = 0;
const CMD_FOLLOW: u8 = 1;
const CMD_ATTACK: u8 = 2;
const CMD_DISMISS: u8 = 3;
// gtker PetReactState / mangos ReactStates.
const REACT_PASSIVE: u8 = 0;
const REACT_DEFENSIVE: u8 = 1;
const REACT_AGGRESSIVE: u8 = 2;
// The flag byte (high 8 bits) of the packed CMSG_PET_ACTION.data (mangos ActiveStates).
const ACT_COMMAND: u8 = 0x07;
const ACT_REACTION: u8 = 0x06;
/// How near (yd², squared) an AGGRESSIVE idle pet proactively seeks a hostile around ITSELF. (10 yd)².
const PET_AGGRO_SEEK_SQ: f32 = 100.0;

/// The owner's stored pet command/react state, or the vanilla default (Follow + Defensive, no target)
/// when no row exists — the absent-row default is exactly the original `pass_pet` behavior. [entity]
fn pet_command_state(ctx: &ReducerContext, owner_guid: u64) -> (u8, u8, u64) {
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

/// Handle a pet command-bar action (`CMSG_PET_ACTION`). `data` is the mangos-packed action
/// (`flag << 24 | id`): flag `0x07` = a command (Stay/Follow/Attack/Dismiss), flag `0x06` = a react
/// state (Passive/Defensive/Aggressive). `target_guid` is the ATTACK target (ignored otherwise). The
/// caller is the pet's owner (resolved from `ctx.sender`); a pet-less caller is a no-op error. All pet
/// policy lives here (the gateway passes the raw wire `data` through). [entity]
#[reducer]
pub fn pet_command(ctx: &ReducerContext, data: u32, target_guid: u64) -> Result<(), String> {
    let owner =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "caller not in world".to_string())?;
    apply_pet_command(ctx, owner.guid, data, target_guid)
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

/// The owner's pet, if any — the single live `game_world_entity` whose `owner_guid == owner_guid`. A
/// full-table scan (the module's established idiom; pets are rare — one per online warlock). `owner_guid`
/// must be non-zero (0 = "not a pet", which would match every wild creature) — callers always pass a real
/// player guid. [entity]
pub fn pet_of(ctx: &ReducerContext, owner_guid: u64) -> Option<WorldEntity> {
    if owner_guid == 0 {
        return None;
    }
    ctx.db
        .game_world_entity()
        .iter()
        .find(|e| e.owner_guid == owner_guid)
}

/// Despawn the player's pet (if any): free its melee engagement + threat (`disengage`), then DELETE the
/// live entity. The `on_delete` relay fires `SMSG_DESTROY_OBJECT` for free (exactly like creature decay),
/// so the pet vanishes for every client. Called on owner logout/disconnect (`remove_from_world`), owner
/// death (`repop`), and at the top of `summon_pet` (a re-summon replaces the old pet). Idempotent — a
/// no-op when the owner has no pet. [entity]
pub fn despawn_pets(ctx: &ReducerContext, owner_guid: u64) {
    if let Some(pet) = pet_of(ctx, owner_guid) {
        crate::combat::disengage(ctx, pet.guid); // free its MeleeAttack row + threat
        ctx.db.game_entity_motion().guid().delete(pet.guid); // motion row dies with the entity (2.1)
        ctx.db.game_world_entity().guid().delete(pet.guid);
    }
    // Clear any pet-bar command state (the pet is ephemeral — no stale row across re-summon/logout;
    // a re-summon thus resets to the Follow + Defensive default, matching vanilla).
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
    pet.owner_guid = owner.guid; // THIS is what makes it a pet (pass_pet keys on owner_guid != 0)
    Some(pet)
}

/// HIGHGUID_UNIT (0xF130) — the high 16 bits of every creature guid. The 5875 client classifies an object
/// (Unit vs other) from its guid's HIGHGUID type, so a pet MUST carry it or the client can't treat it as a
/// unit (broken nameplate/targeting/pet-bar), exactly like the importer `world_guid` / `seed` / `debug`.
const HIGHGUID_UNIT: u64 = 0xF130;

/// The deterministic pet guid for an owner: `HIGHGUID_UNIT` in the high bits (so the client treats it as a
/// Unit) + the owner guid in the low 48. The owner is a small player guid (< 2^24); a real creature's low is
/// `(entry << 24) | db_guid` (always >= 1<<24 since entry >= 1), so the pet's low never collides with a
/// creature. Stable so a re-summon's `pet_of` scan + the despawn delete agree + the `on_delete` relay is
/// clean. Pure.
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

/// PET AI pass (run in the SENSING block of `tick_creatures`, right after aggro/assist + before chase): for
/// each live pet (`owner_guid != 0`, alive, not CC'd) decide its action keyed on the OWNER's state:
///   - OWNER IN COMBAT (engaged, or has a live enemy target) → arm a pet→that-target `MeleeAttack` row +
///     point the pet at the target. The EXISTING chase pass closes the gap and the swing tick swings — no
///     new combat/movement code. (Idempotent: re-pointing keeps `last_swing_ms` cadence.)
///   - OWNER IDLE (not in combat) → drop any pet melee row (stop fighting) and, if the pet has drifted
///     beyond the follow band, emit a RUN follow-leg toward the owner (the return-pass geometry with
///     "home = the owner's live position").
///
/// A dead owner / an owner that left the world → despawn the pet (it never outlives its owner). Snapshot
/// the work first (collect-then-mutate), like every other pass. [entity]
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — a pet's owner is a PLAYER by
/// definition, so `entities.iter().filter(|e| e.owner_guid != 0)` is already bounded by the pet-owning
/// player count (small), never the ~2500-creature wild population this item exists to stop scanning; a
/// pet is inherently tethered to an always-active player anyway.
///
/// Work-item 233 rejected an `owner_guid` btree index on `game_world_entity` — soundly: that table is
/// the hottest write target in the tick (every movement leg writes it), so paying index maintenance
/// every 0.5s to shave a read that happens every ~4s over a tiny result set is a bad trade. But "no
/// index" never meant "must full-scan": perf catalog 1.10 takes the third option and harvests the
/// `owner_guid != 0` guids from `active_cell_creatures`' scan, which already visits every entity row
/// every tick for the active-cell set. The dedicated scan is gone, the write path is untouched, and
/// the candidate set + visit order are identical (table order, same predicate).
///
/// Work-item 229: gated per PET on `scope.covers(pet.instance_id)` — the PET's own instance, not the
/// owner's, so a pet momentarily left behind by an owner's cross-instance teleport is still ticked by
/// whichever row covers where the PET is (and its Follow snap then moves it into the owner's
/// instance, whose row picks it up next sense tick — never stranded). With only the catch-all row
/// this admits every pet (equivalence: ai.rs `tick_scope_default_config_…`). Returns covered pets
/// visited.
pub(crate) fn pass_pet(
    ctx: &ReducerContext,
    now_ms: u32,
    scope: &super::TickScope,
    interval_micros: i64,
    // Perf catalog 1.10: the `owner_guid != 0` guids, harvested from `active_cell_creatures`' scan —
    // which visits every entity row every tick anyway. In table order, so the visit order (and the
    // resulting move-event ids) match the old dedicated scan exactly.
    pet_guids: &[u64],
) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();
    let melee = ctx.db.game_melee_attack();

    // The owner's "combat target": the enemy the pet should engage. The owner is fighting when it has a
    // live `game_melee_attack` row (a player auto-attacking) — use that row's target; else fall back to the
    // owner's `target_guid` ONLY when the owner is flagged IN_COMBAT (so a pet doesn't attack a peacefully
    // selected NPC). Returns the target guid, or 0 = the owner is idle (follow).
    let owner_combat_target = |owner: &WorldEntity| -> u64 {
        if let Some(row) = melee.attacker_guid().find(owner.guid) {
            return row.target_guid;
        }
        if owner.unit_flags & constants::unit_flags::IN_COMBAT != 0 {
            return owner.target_guid;
        }
        0
    };

    // Snapshot: (pet_guid, action). Action = Engage(target) | Follow(dest x,y,z,dur) | Despawn | Idle.
    enum Act {
        Engage(u64),
        Follow {
            map_id: u32,
            instance_id: u64, // work-item 190 slice 1: follow the owner's instance too (always 0)
            dest: (f32, f32, f32),
            start: (f32, f32, f32),
            duration_ms: u32, // 0 = a cross-map snap (no move-leg emitted; the position jumps to the owner)
        },
        Despawn,
        Disengage, // owner went idle while the pet held a melee row — drop it (then follow next tick)
    }
    let mut work: Vec<(u64, Act)> = Vec::new();
    for pet in pet_guids.iter().filter_map(|g| entities.guid().find(g)) {
        // Work-item 229: only this firing's covered instances (see the fn doc).
        if !scope.covers(pet.instance_id) {
            continue;
        }
        visited += 1;
        if pet.dead {
            // A dead pet has no spawn row, so the decay pass never reaps it. kill_creature now deletes a
            // combat-killed pet outright; this catches any OTHER dead-pet path (e.g. a debug kill) so a dead
            // Imp never lingers as a stale corpse.
            work.push((pet.guid, Act::Despawn));
            continue;
        }
        // CC: a frozen/feared pet can't act/move this tick (the existing gates own that). The pet's own
        // chase/swing already respect CC; the follow leg below must too.
        if crate::spell::is_self_movement_suppressed(ctx, pet.guid) {
            continue;
        }
        // The owner — gone (logout) or dead → the pet despawns (it never outlives the owner).
        let Some(owner) = entities.guid().find(pet.owner_guid) else {
            work.push((pet.guid, Act::Despawn));
            continue;
        };
        if owner.dead {
            work.push((pet.guid, Act::Despawn));
            continue;
        }
        // Pet command bar (CMSG_PET_ACTION). An ABSENT row is the vanilla default (Follow + Defensive)
        // = the original derived behavior, so an unmanaged pet is unchanged.
        let (cmd, react, cmd_target) = pet_command_state(ctx, owner.guid);
        // Resolve who to engage. An explicit ATTACK command wins; otherwise the react state governs
        // auto-assist (PASSIVE never engages; DEFENSIVE/AGGRESSIVE assist the owner's combat target;
        // AGGRESSIVE also seeks a nearby hostile when the owner is idle). The faction/self guards live
        // in `is_valid_pet_target` (the pet's swing path has no faction gate, so this is load-bearing).
        let engage: Option<u64> = if cmd == CMD_ATTACK {
            if is_valid_pet_target(ctx, &owner, pet.guid, cmd_target) {
                Some(cmd_target)
            } else {
                // The commanded target died/vanished → clear the stale ATTACK so the pet resumes
                // following/assisting instead of standing on a dead foe.
                set_pet_command(ctx, owner.guid, CMD_FOLLOW, react, 0);
                None
            }
        } else {
            None
        };
        let engage = engage.or_else(|| {
            if react == REACT_PASSIVE {
                return None; // passive pets never auto-engage
            }
            let t = owner_combat_target(&owner);
            if is_valid_pet_target(ctx, &owner, pet.guid, t) {
                Some(t) // defensive + aggressive assist the owner's target
            } else if react == REACT_AGGRESSIVE {
                nearest_hostile_near(ctx, &owner, &pet) // aggressive also proactively seeks
            } else {
                None
            }
        });
        if let Some(target_guid) = engage {
            work.push((pet.guid, Act::Engage(target_guid)));
            continue;
        }
        // Not engaging. If the pet currently holds a melee row, drop it (stop fighting a stale foe).
        if melee.attacker_guid().find(pet.guid).is_some() {
            work.push((pet.guid, Act::Disengage));
            continue;
        }
        // STAY: hold position — skip the follow-the-owner legs below. (The pet still fights when it
        // engages above; STAY only stops it trailing the owner around.)
        if cmd == CMD_STAY {
            continue;
        }
        // Follow the owner when drifted beyond the follow band (and on the same map). Reuse the chase
        // geometry with the OWNER as the destination ("home = the owner's live position").
        if pet.map_id != owner.map_id || pet.instance_id != owner.instance_id {
            // Owner zoned away (e.g. a teleport) OR changed instance (work-item 190 slice 1) — re-stage
            // the pet at the owner (a snap, not a move-leg: SMSG_MONSTER_MOVE can't cross maps; the
            // pet's CREATE/DESTROY on the AOI/map relay handles it).
            work.push((
                pet.guid,
                Act::Follow {
                    map_id: owner.map_id,
                    instance_id: owner.instance_id,
                    dest: (owner.x, owner.y, owner.z),
                    start: (pet.x, pet.y, pet.z),
                    duration_ms: 0,
                },
            ));
            continue;
        }
        let (dx, dy, dz) = (owner.x - pet.x, owner.y - pet.y, owner.z - pet.z);
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq <= PET_FOLLOW_BAND_SQ {
            continue; // close enough — hold position
        }
        let run = crate::combat::effective_move_speed(ctx, pet.guid, constants::speeds::RUN);
        // Step ONE SENSE PERIOD of run toward the owner, stopping ~4 yd short (just inside the follow
        // band so the pet settles a few yd behind, not on top). `pass_pet` only runs on the sense tick,
        // so the leg must span until the NEXT one — a one-tick step would let a moving owner outrun the
        // pet. The span is the ROW'S OWN sense period, not a hardcoded 4s (229 review: a 2.5s dedicated
        // row senses EVERY firing, so a 4s leg re-emitted every 2.5s overlapped — ~1.6× closing speed
        // with client jump-ahead jitter). At the default 500ms row this is exactly the old 4s leg.
        let follow_step = run * super::ai::sense_period_secs_for_interval(interval_micros);
        let (nx, ny) = crate::nav::nav_step(
            ctx,
            pet.map_id,
            (pet.x, pet.y),
            (owner.x, owner.y),
            follow_step,
            4.0,
        );
        if nx == pet.x && ny == pet.y {
            continue; // no-op step (already within the stop band) — skip the zero-length leg
        }
        let (ndx, ndy) = (nx - pet.x, ny - pet.y);
        let dist = (ndx * ndx + ndy * ndy).sqrt();
        let duration_ms = ((dist / run) * 1000.0) as u32;
        work.push((
            pet.guid,
            Act::Follow {
                map_id: owner.map_id,
                instance_id: owner.instance_id,
                dest: (nx, ny, owner.z),
                start: (pet.x, pet.y, pet.z),
                duration_ms,
            },
        ));
    }

    for (pet_guid, act) in work {
        match act {
            Act::Despawn => despawn_pets(ctx, pet_guid_owner_of(ctx, pet_guid)),
            Act::Engage(target_guid) => {
                // Arm the pet→target melee row (idempotent — same shape as the aggro pass).
                if melee.attacker_guid().find(pet_guid).is_none() {
                    melee.insert(MeleeAttack {
                        attacker_guid: pet_guid,
                        target_guid,
                        last_swing_ms: 0,   // swing on the next melee tick
                        ranged_spell_id: 0, // 0 = melee/no wand auto-shot; Firebolt is cast via pass_cast (game_creature_cast 416→3110), NOT this field
                        last_offhand_swing_ms: 0,
                    });
                } else if let Some(mut row) = melee.attacker_guid().find(pet_guid) {
                    // Re-point at the owner's current target WITHOUT resetting the swing cadence.
                    if row.target_guid != target_guid {
                        row.target_guid = target_guid;
                        melee.attacker_guid().update(row);
                    }
                }
                if let Some(mut pet) = entities.guid().find(pet_guid) {
                    if pet.target_guid != target_guid {
                        pet.target_guid = target_guid;
                        entities.guid().update(pet);
                    }
                }
            }
            Act::Disengage => {
                crate::combat::break_own_attacks(ctx, pet_guid); // drop the pet's own attack + threat
            }
            Act::Follow {
                map_id,
                instance_id,
                dest: (nx, ny, nz),
                start: (sx, sy, sz),
                duration_ms,
            } => {
                if duration_ms > 0 {
                    // ONE relay path (perf 2.3) — see `creatures::tick::emit_move_spline`. This arm
                    // kept writing the unsubscribed `game_creature_move_event` table after the gateway
                    // dropped its subscription, so a following pet's leg moved the server and nothing
                    // else (#357) — every sense tick, forever, on a live shard.
                    emit_move_spline(
                        ctx,
                        pet_guid,
                        (sx, sy, sz),
                        (nx, ny, nz),
                        duration_ms,
                        true, // a pet follows at a run
                        now_ms,
                        map_id,
                        instance_id,
                        spatial::grid_cell(sx, sy), // leg START cell, same convention as the chase pass
                    );
                }
                if let Some(mut pet) = entities.guid().find(pet_guid) {
                    let (gx, gy) = spatial::grid_cell(nx, ny);
                    pet.map_id = map_id; // follow the owner's map (only changes on a cross-map snap)
                    pet.instance_id = instance_id; // follow the owner's instance (190 slice 1, always 0)
                    pet.x = nx;
                    pet.y = ny;
                    pet.z = nz;
                    pet.grid_x = gx;
                    pet.grid_y = gy;
                    pet.last_move_ms = now_ms;
                    entities.guid().update(pet);
                }
            }
        }
    }
    visited
}

/// Resolve the OWNER guid for a pet guid so `Act::Despawn` can reuse `despawn_pets(owner)` (which scans by
/// owner). A pet's guid encodes the owner in its low 48 bits (`pet_guid_for`), so mask the HIGHGUID off;
/// verify against the live row in case the encoding ever changes. Pure-ish (one find). [entity]
fn pet_guid_owner_of(ctx: &ReducerContext, pet_guid: u64) -> u64 {
    ctx.db
        .game_world_entity()
        .guid()
        .find(pet_guid)
        .map(|p| p.owner_guid)
        .unwrap_or(pet_guid & 0x0000_FFFF_FFFF_FFFF)
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
    fn follow_band_sits_outside_melee_range() {
        // The follow band ((8yd)²) must be larger than the chase melee range ((5yd)²) so a following pet
        // settles a few yards behind the owner instead of piling onto it / jittering at the melee boundary.
        const {
            assert!(
                PET_FOLLOW_BAND_SQ > super::super::ai::CHASE_MELEE_SQ,
                "follow band must be outside melee range"
            )
        };
    }
}
