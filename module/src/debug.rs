//! Layer 1 — server-side **test-only** reducers that pilot the world for the client-automation
//! harness. Each puts the world in an exact precondition
//! (teleport / set HP / spawn-at-feet / force-cast / set level / clear creatures) so most ⭐ tests
//! lose their blind 3D-navigation step and become deterministic.
//!
//! # PROD-SAFETY GATE
//! The whole module is behind `#![cfg(feature = "debug_reducers")]` — with the feature OFF (the
//! default, so a plain `spacetime build` / `cargo build` and any production publish) this file
//! compiles to nothing and the reducers don't exist in the module at all. The test build / the
//! automation harness opt in explicitly:
//!
//! ```text
//! spacetime publish -s local --build-options='--features=debug_reducers' lyracore
//! ```
//!
//! (`--build-options` is forwarded verbatim to `cargo build --target=wasm32-… --release`, so the
//! feature reaches the wasm compile — verified against the 2.5 CLI.)
//!
//! # CRITICAL identity caveat (docs §6 / HANDOFF §10.2)
//! A `spacetime call` runs as the **CLI identity**, which is NOT a player's bound identity. So every
//! reducer here takes an **explicit `character_guid`/`guid`** and operates on that row directly — it
//! MUST NOT use `entity_by_owner(ctx, ctx.sender())` (that only resolves the gateway's per-player
//! connection identity). This is the single most important difference from the player reducers.
// NOTE: no `#![cfg(feature = "debug_reducers")]` here — `lib.rs` already gates `mod debug;` on that
// exact feature, so repeating it inside the file is a duplicated `cfg` (clippy::duplicated_attributes)
// that reads as a second, independent gate while adding nothing. lib.rs:175 is THE gate; if it ever
// loses its `#[cfg]`, this whole file compiles into production — that is what preflight check 1
// (`cargo check --features=debug_reducers`) and the prod-safety note above lib.rs's `mod debug;` guard.

use lyracore_shared::{constants, spatial};
use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{
    build_creature_entity,
    // Catalogue-fingerprint families only (issue #82) — see `debug_catalogue_fingerprint` below.
    // (`game_catalogue_fingerprint` itself is defined further down in THIS file, so it needs no import.)
    game_area,
    game_area_trigger,
    game_areatrigger_teleport,
    game_aura_schedule,
    // `game_faction` is deliberately absent here: #85 added it to the accessor import above, and
    // naming it twice compiles clean on each branch alone but not once both land (E0252).
    game_char_base_info,
    game_character,
    game_class_level_stats,
    game_config,
    game_createinfo_action,
    game_createinfo_spell,
    game_creature_family,
    game_creature_move_event,
    game_creature_move_schedule,
    game_creature_spawn,
    game_creature_template,
    game_faction,
    game_faction_template,
    game_gameobject,
    game_gameobject_pool,
    game_gameobject_pool_member,
    game_gameobject_template,
    game_gameobject_unlocked,
    game_graveyard,
    game_graveyard_zone,
    game_ground_area,
    game_ground_area_schedule,
    game_instance,
    game_instance_reaper_schedule,
    game_item_instance,
    game_item_template,
    game_level_stats,
    game_lock,
    game_melee_attack,
    game_quest_template,
    game_race_info,
    game_skill_ability,
    game_skill_availability,
    game_skill_line,
    game_spell,
    game_spell_chain,
    game_spell_effect,
    game_spell_learn,
    game_spell_reagent,
    game_start_item,
    game_start_position,
    game_talent,
    game_talent_tab,
    game_world_entity,
    AuraSchedule,
    CreatureMoveSchedule,
    CreatureSpawn,
    GroundArea,
    GroundAreaSchedule,
    ItemInstance,
    ServerConfig,
};
// `game_spell_group`/`game_spell_group_rule` live in `spell::stacking`, not re-exported at crate root.
use crate::spell::stacking::{game_spell_group, game_spell_group_rule};

/// Teleport `character_guid` to `(map_id, x, y, z, o)` via the shared `world::teleport_player` core: it
/// updates the LIVE entity (position + grid cell) AND the durable `game_character` AND emits the
/// `game_teleport_event` that makes the gateway send `MSG_MOVE_TELEPORT_ACK` so the player's camera
/// actually follows. Keyed off the explicit guid (the CLI identity owns no entity). Errors if not in world.
#[reducer]
pub fn debug_teleport(
    ctx: &ReducerContext,
    character_guid: u64,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> Result<(), String> {
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_none()
    {
        return Err(format!("no live entity for guid {character_guid}"));
    }
    crate::world::teleport_player(ctx, character_guid, map_id, 0, x, y, z, o);
    Ok(())
}

/// Set `guid`'s live health (clamped to `max_health`). `health == 0` drives the SAME death path the
/// combat killing blow uses (`combat::tick_melee`): a PLAYER enters the release/ghost path
/// (`health = 0` + `dead = true`, then the client shows Release Spirit — vanilla 1.12 has no death
/// opcode, `UNIT_FIELD_HEALTH == 0` is the sole signal); a CREATURE becomes a corpse (`dead = true`,
/// disengaged, decay timer armed, loot rolled onto the corpse).
///
/// NOTE: the combat module has no reusable kill `fn` — the kill logic is inline in `tick_melee` over local
/// swing state. This is the minimal faithful re-expression of that path (drop combat via `disengage`,
/// set the same fields, arm the corpse decay timer like the kill does), NOT a copy of the swing loop.
/// Deal `amount` DIRECT damage to a live entity through the REAL damage side-effect path
/// (`break_auras_on_damage`: pushback, CC break, on-damage hooks) — the deterministic damage
/// source for harness tests (testing-hardening: cast_interrupt's pushback used to depend on a mob
/// swing landing inside a 1.7s window, a per-run lottery). Unlike `debug_set_health` (a raw field
/// write, NO side effects), this behaves like being hit. Never lethal: clamps to 1 HP — a harness
/// poke must not trip the kill path (use `debug_kill_nearest` for that).
#[reducer]
pub fn debug_apply_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    amount: u32,
    // The attributed attacker (0 = anonymous): threads through to `on_damage_taken` so
    // attacker-reactive systems (the playerbots defend hook, A_PROC_ON_HIT) see a real source.
    attacker_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut e = entities
        .guid()
        .find(target_guid)
        .ok_or_else(|| format!("no live entity for guid {target_guid}"))?;
    if e.dead {
        return Err("target is dead".to_string());
    }
    e.health = e.health.saturating_sub(amount).max(1);
    entities.guid().update(e);
    crate::spell::break_auras_on_damage(ctx, target_guid, attacker_guid, false);
    Ok(())
}

#[reducer]
pub fn debug_set_health(ctx: &ReducerContext, guid: u64, health: u32) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut e = entities
        .guid()
        .find(guid)
        .ok_or_else(|| format!("no live entity for guid {guid}"))?;

    let clamped = health.min(e.max_health);
    if clamped > 0 {
        e.health = clamped;
        // A revive-by-HP from a dead state: clear the death flag so the unit is alive again (the
        // client comes back from death purely on health > 0). Keep it simple — no ghost/corpse here.
        e.dead = false;
        entities.guid().update(e);
        return Ok(());
    }

    // health == 0 → death. Branch exactly like the combat killing blow (combat::tick_melee).
    if e.is_player() {
        // PLAYER death through the SHARED kill path (combat::kill_player) so a debug kill is
        // byte-identical to a real killing blow, including channel teardown and combat-deadline
        // zeroing. No killer on this path.
        crate::combat::kill_player(ctx, guid, 0);
        log::info!("debug_set_health: player {guid} killed (dead, awaiting release)");
        return Ok(());
    }

    // CREATURE death: route through the SHARED kill path so a debug kill produces the exact same
    // corpse + money/item loot + decay as a real killing blow (no drift between the paths). No XP
    // credit — there's no player killer here (`award_xp` needs an attacker).
    crate::combat::kill_creature(ctx, guid, None);
    log::info!("debug_set_health: creature {guid} killed (corpse via kill_creature)");
    Ok(())
}

/// Drive Release Spirit by explicit guid (the `CMSG_REPOP_REQUEST` / `repop` path) — the CLI identity
/// owns no entity, so this calls the shared `world::do_repop` against the row directly. Leaves a corpse,
/// becomes a ghost (health 1, GHOST flags), and teleports to the Northshire graveyard, exactly like a
/// real Release click. Lets the parent drive die → release → spirit-heal headless. Errors if not dead.
#[reducer]
pub fn debug_repop(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::actor::repop(ctx, character_guid)
}

/// Spirit-Healer resurrect by explicit guid (the `CMSG_SPIRIT_HEALER_ACTIVATE` / `spirit_healer_res`
/// path) — drives the new graveyard-res reducer's body against the row directly (the CLI identity owns
/// no entity), so the parent can verify the feature without the mouse-only spirit-healer dialog. Gates
/// on the entity being a ghost; res's in place at 50% health + 50% mana and lands Resurrection Sickness.
#[reducer]
pub fn debug_spirit_healer_res(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::actor::spirit_res(ctx, character_guid)
}

/// Answer a pending resurrect offer by explicit guid (`CMSG_RESURRECT_RESPONSE` / `resurrect_response`
/// path) — drives the shared `do_resurrect_response` core against the row directly (the CLI
/// identity owns no entity), so the parent can verify the accept-prompt handshake headlessly. Errors if
/// `character_guid` has no pending offer.
#[reducer]
pub fn debug_resurrect_response(
    ctx: &ReducerContext,
    character_guid: u64,
    accept: bool,
) -> Result<(), String> {
    crate::spell::do_resurrect_response(ctx, character_guid, accept)
}

/// Spawn a creature of `creature_entry` at the player's feet (`player.x + offset`, same y/z/map) so a
/// combat test doesn't have to walk to a mob. Reads the player's live entity, looks up the template,
/// builds the live `game_world_entity` via the shared `build_creature_entity` (same construction
/// `seed` and the creature-respawn pass use) with a fresh unique guid, and inserts a matching `game_creature_spawn` so the normal
/// decay/respawn lifecycle owns it. The reducer can't return a value to the CLI, so the new guid is
/// logged (`grep` the module log, or read it back from `game_world_entity` by entry).
#[reducer]
pub fn debug_spawn_at_feet(
    ctx: &ReducerContext,
    character_guid: u64,
    creature_entry: u32,
    offset: f32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let player = entities
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let tmpl = ctx
        .db
        .game_creature_template()
        .entry()
        .find(creature_entry)
        .ok_or_else(|| format!("no creature template for entry {creature_entry}"))?;

    // A unique creature guid carrying HIGHGUID_UNIT (0xF130) + the entry in bits 24..47 (like the
    // seed's chicken), with a low part one past the current max so it never collides with an existing
    // spawn of the same entry.
    const HIGHGUID_UNIT: u64 = 0xF130;
    let entry_hi = (creature_entry as u64) << 24;
    // Max over BOTH the spawn rows AND the live entities of this entry: an operator can delete a
    // spawn row (test cleanup) while its entity still lingers (corpse decay), and a spawn-rows-only
    // scan then re-allocates that entity's guid → duplicate-unique-column PANIC (live find 2026-07-10).
    let spawn_max = ctx
        .db
        .game_creature_spawn()
        .iter()
        .filter(|s| s.entry == creature_entry)
        .map(|s| s.guid & 0x00FF_FFFF)
        .max()
        .unwrap_or(0);
    let entity_max = entities
        .iter()
        .filter(|e| e.entry == creature_entry && !e.is_player())
        .map(|e| e.guid & 0x00FF_FFFF)
        .max()
        .unwrap_or(0);
    let next_low = spawn_max.max(entity_max) + 1;
    let new_guid = (HIGHGUID_UNIT << 48) | entry_hi | next_low;

    // Place the creature `offset` yards in the player's FACING direction (not a
    // fixed +X). The harness drives targeting via the client's TargetNearestEnemy /
    // Tab / `/target`, which are FACING-CONE limited — a creature spawned behind the
    // player is unselectable, so a fixed +X offset made combat tests flaky. Spawning
    // along the player's orientation puts it directly in front (client facing == this
    // server orientation once the player has turned), so the harness can reliably
    // select + melee it. WoW orientation 0 = +X, so (cos, sin) is the facing vector.
    let (sin_o, cos_o) = player.orientation.sin_cos();
    let spawn = CreatureSpawn {
        guid: new_guid,
        entry: creature_entry,
        map_id: player.map_id,
        x: player.x + offset * cos_o,
        y: player.y + offset * sin_o,
        z: player.z,
        orientation: player.orientation,
        // NOT-ARMED (`creatures::timer_never`): the live entity is inserted immediately below, so a
        // past stamp would only keep this row in every due-time range scan forever.
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        // IDLE so a debug test creature holds where it's spawned (predictable for combat readback; the
        // wander pass interfered with verification — see the harness memory).
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: 0, // not imported — falls back to the flat legacy respawn timer (a debug spawn's death is a one-off harness action)
    };
    // Build the live entity from a reference (fixed roll 0 → deterministic min level/health) BEFORE
    // moving the spawn into its table, so we never depend on the row type deriving Clone. The
    // entity insert fires the gateway's on_insert → CREATE_OBJECT so the creature appears at feet.
    let entity = build_creature_entity(&spawn, &tmpl, 0, 0);
    ctx.db.game_creature_spawn().insert(spawn);
    crate::creatures::insert_creature_entity(ctx, entity);
    log::info!(
        "debug_spawn_at_feet: spawned entry {creature_entry} as guid {new_guid} for player {character_guid}"
    );
    Ok(())
}

/// Materialize a PLAYER's live `game_world_entity` from its durable `game_character` row — SERVER-SIDE,
/// with no game client. This is the harness keystone for the items/equipment/progression chain: a
/// schema-changing `publish` tears down the in-world player entity (the live `game_world_entity`), and
/// the client can't be driven headless to log back in, so reducers that require a live entity
/// (`debug_compute_swing`, `debug_equip_weapon`, the item/XP/durability paths) have nothing to act on.
/// This rebuilds that entity from the persisted character so those reducers are testable again.
///
/// Idempotent: if a live entity for the guid already exists it's left untouched and we return Ok (a
/// real session may own it — we never clobber a live player). Otherwise we build + insert the entity
/// through the SAME shared `build_player_entity` `world::player_login` uses (the player counterpart to
/// `build_creature_entity`), so the stat curve, packing, and display/faction resolution are identical
/// by construction. Position/level/appearance/xp/money come from the character row; vitals are filled
/// to the new max.
///
/// CLI-identity caveat (file header §): a `spacetime call` runs as the CLI identity, which owns no
/// player binding, so we key off the explicit `character_guid` and stamp the entity's `owner_identity`
/// to the character's persisted `owner_identity` (NOT `ctx.sender()`) — the per-owner RLS that scopes
/// items to a connection then matches what a real login would have set.
#[reducer]
pub fn debug_spawn_player_entity(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    // Idempotent: a live entity already exists (a real session, or a prior call) — never clobber it.
    if entities.guid().find(character_guid).is_some() {
        log::info!("debug_spawn_player_entity: guid {character_guid} already live (no-op)");
        return Ok(());
    }

    // REFUSE verdict (issue #30). This is `player_login`'s RE-MATERIALISATION path wearing a harness
    // hat: it builds a live entity straight off the durable row, so without the fence it is a second
    // way to put a live copy back on a shard the character has left — the dual-liveness dupe the
    // escrow exists to prevent. `player_login` fences it; so must this.
    let character = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or_else(|| format!("no such character: {character_guid}"))?;

    // Build the live entity from the durable character row — the SAME construction
    // `world::player_login` runs, factored into the shared `build_player_entity`. The ONLY difference:
    // `owner_identity` is the character's PERSISTED binding (NOT `ctx.sender()` — a `spacetime call`
    // runs as the CLI identity, which owns no player binding), so per-owner RLS matches a real login.
    let entity = crate::build_player_entity(ctx, &character, character.owner_identity);
    let (level, max_health) = (entity.level, entity.max_health);
    entities.insert(entity);
    log::info!(
        "debug_spawn_player_entity: materialized player guid {character_guid} (lvl {level}, hp {max_health})"
    );
    Ok(())
}

/// Force-apply the tracer aura on `character_guid` (the ⭐ aura wire-check without needing rage / the
/// spellbook UI). Reuses `spell::apply_tracer_aura` — the exact aura-upsert + `SpellCastEvent` that
/// `cast_spell` runs — keyed off the explicit guid instead of `ctx.sender()`. Accepts ANY `spell_id`
/// (a debug actuator; the player-facing `cast_spell` still gates on the one tracer spell). The client
/// renders whatever icon its `Spell.dbc` has for the id, so passing several distinct ids stacks
/// several auras in distinct slots — the multi-aura wire test for the raw-send path.
#[reducer]
pub fn debug_force_cast(
    ctx: &ReducerContext,
    character_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    // Route through the SAME resolved-cast path the player reducer uses, so the debug cast enforces
    // the spell's mana cost and runs its effect dispatch (self-aura vs heal-self) — not just the aura.
    crate::spell::resolve_cast(ctx, character_guid, spell_id, e.level as u8)
}

/// Like `debug_force_cast` but directs the cast at an explicit `target_guid` instead of self —
/// used to test targeted interrupts (e.g. Kick: caster A casts Kick on target B who is mid-cast).
/// For interrupt testing the `lockout_ms` value is read from `game_spell.duration_ms` for the
/// interrupting spell; change that value to verify the lockout window changes with zero engine change.
#[reducer]
pub fn debug_force_cast_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    // Actor verb: sources the caster level from the live entity and resolves synchronously.
    crate::actor::cast_at(ctx, caster_guid, spell_id, target_guid)
}

/// Set `character_guid`'s level and recompute `max_health`/`max_power` from the real stat curve
/// (`stats::max_health_for`/`max_power_for`, the SAME helpers `player_login` uses — not reimplemented).
/// Health/power are refilled to the new max for the test. Also recomputes the five base attributes so
/// the character sheet stays consistent with the new level. Thin wrapper over the shared
/// `stats::set_character_level` core (work-item 223 factored it out so `gm::gm_command`'s `.level`
/// reuses the SAME recompute — see that fn's doc).
#[reducer]
pub fn debug_set_level(
    ctx: &ReducerContext,
    character_guid: u64,
    level: u32,
) -> Result<(), String> {
    // REFUSE verdict (issue #30) — the `debug_set_money` sibling. `set_character_level` writes
    // `Character.level` (an `ExportBlob` field) and `Character.xp` on the DURABLE row and needs no
    // live entity to do it (it errors only when neither an entity nor a character row exists), so
    // by-guid it reaches an escrowed character. Fenced here, not in `stats`: `gm_command`'s caller
    // resolves through `entity_by_owner` already, and the core also serves guids with no character
    // row at all.
    crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or_else(|| format!("no character {character_guid}"))?;
    crate::stats::set_character_level(ctx, character_guid, level)
}

/// Delete every CREATURE-type live `game_world_entity` on `map_id` (clean slate before a combat test).
/// Never touches a PLAYER (the PLAYER bit gates it), mirroring `import_creature_spawns`' reset pass.
/// The persistent `game_creature_spawn` rows are left intact, so the normal respawn pass will re-create
/// seeded creatures later — this clears the live roster, it doesn't unseed the world.
#[reducer]
pub fn debug_clear_creatures(ctx: &ReducerContext, map_id: u32) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let guids: Vec<u64> = entities
        .iter()
        .filter(|e| e.map_id == map_id && !e.is_player())
        .map(|e| e.guid)
        .collect();
    let n = guids.len();
    for g in &guids {
        // Free any combat the creature was in so no orphan engagement lingers, then delete it.
        crate::combat::disengage(ctx, *g);
        entities.guid().delete(g);
    }
    log::info!("debug_clear_creatures: removed {n} creatures on map {map_id}");
    Ok(())
}

/// Seed the aura-expiry schedule row if it is MISSING (idempotent). A DB provisioned
/// before the `game_aura_schedule` insert was added to `seed::init` never gets it — `init` runs only
/// on first publish, NOT on an auto-migrate republish — so `tick_auras` never fires and expired auras
/// never clear (the "buff clears after ~30s" behavior silently breaks while apply still works). The
/// reaper logic itself is correct; only the schedule was absent. This patches such a live DB without a
/// destructive `-c` re-provision. (The proper project fix is migration-safe schedule seeding; a fresh
/// `publish -c` already seeds it.) Matches the interval in `seed::init` (1s).
#[reducer]
pub fn debug_ensure_aura_schedule(ctx: &ReducerContext) -> Result<(), String> {
    if ctx.db.game_aura_schedule().iter().next().is_some() {
        return Ok(()); // already scheduled — no-op
    }
    ctx.db.game_aura_schedule().insert(AuraSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(1_000_000)),
    });
    log::info!("debug_ensure_aura_schedule: seeded the missing aura-expiry schedule (1s)");
    Ok(())
}

/// Stamp a `game_ground_area` at `caster_guid`'s position from `spell_id`'s area effect (118 test hook):
/// looks up the spell header + its periodic-area effect (the one with a radius + period) and inserts the
/// zone, exactly as an `E_PERSISTENT_AREA` cast would — so the tick engine can be verified server-side
/// without a live client cast. `tick_ground_areas` then damages hostiles inside for `duration_ms`.
#[reducer]
pub fn debug_spawn_ground_area(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let caster = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .ok_or("caster not in world")?;
    let hdr = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or("unknown spell")?;
    let eff = ctx
        .db
        .game_spell_effect()
        .iter()
        .find(|e| e.spell_id == spell_id && e.period_ms > 0 && e.radius_yd > 0.0)
        .ok_or("spell has no periodic-area effect (radius + period)")?;
    let now = ctx.timestamp;
    let expires_at = now
        .checked_add(TimeDuration::from_micros(
            (hdr.duration_ms.max(1) as i64) * 1000,
        ))
        .ok_or("duration overflow")?;
    // Perf catalog 1.20: the tick schedule is demand-armed (it disarms when the area table drains).
    crate::arm_ground_area_tick(ctx);
    ctx.db.game_ground_area().insert(GroundArea {
        id: 0,
        spell_id,
        caster_guid,
        map_id: caster.map_id,
        instance_id: caster.instance_id,
        x: caster.x,
        y: caster.y,
        z: caster.z,
        radius_yd: eff.radius_yd,
        amount: eff.base_points,
        school_mask: hdr.school_mask,
        period_ms: eff.period_ms,
        next_tick_micros: now.to_micros_since_unix_epoch() + (eff.period_ms as i64) * 1000,
        expires_at,
    });
    log::info!(
        "debug_spawn_ground_area: {spell_id} at ({:.1},{:.1}) r={} amount={}/{}ms for {}ms",
        caster.x,
        caster.y,
        eff.radius_yd,
        eff.base_points,
        eff.period_ms,
        hdr.duration_ms
    );
    Ok(())
}

/// Seed the ground-AoE tick schedule row if MISSING (idempotent) — the 118 twin of
/// `debug_ensure_aura_schedule`. A DB provisioned before `game_ground_area_schedule` was added to
/// `seed::init` never gets it on a plain (auto-migrate) publish, so `tick_ground_areas` never fires and
/// Consecration/Blizzard patches sit inert. Call once after publishing 118 to a live DB. Matches the
/// 500ms interval in `seed::init`.
#[reducer]
pub fn debug_ensure_ground_area_schedule(ctx: &ReducerContext) -> Result<(), String> {
    if ctx.db.game_ground_area_schedule().iter().next().is_some() {
        return Ok(()); // already scheduled — no-op
    }
    ctx.db
        .game_ground_area_schedule()
        .insert(GroundAreaSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(500_000)),
        });
    log::info!(
        "debug_ensure_ground_area_schedule: seeded the missing ground-AoE tick schedule (500ms)"
    );
    Ok(())
}

/// A read-only scratchpad the combat-readback harness reads via SQL (the keystone that flips
/// combat-stat features from "needs a finicky live kill" to machine-solo). `debug_compute_swing`
/// writes the deterministic swing profile here; the harness asserts on it without a fight. Public so
/// `spacetime sql` can read it; debug-only (the whole module is behind `debug_reducers`). [server]
#[table(accessor = game_debug_readout, public)]
pub struct DebugReadout {
    #[primary_key]
    pub key: String, // a single "swing" slot today; keyed so future readouts can coexist
    pub base_min: u32,
    pub base_max: u32,
    pub mitigation_pct: u32,
    pub final_min: u32,
    pub final_max: u32,
    pub note: String,
    /// The target's chance (basis points) to dodge the attacker's swing, from its EFFECTIVE agility
    /// (base + any A_MOD_STAT(AGI) buff) — the avoidance readback. END-appended + defaulted so adding
    /// it auto-migrates the existing row.
    #[default(0)]
    pub dodge_bp: u32,
    /// The attacker's EFFECTIVE crit chance (basis points): `CRIT_BP` + any A_MOD_COMBAT(CRIT) aura —
    /// the crit-rating readback. END-appended + defaulted so adding it auto-migrates the existing row.
    #[default(0)]
    pub crit_bp: u32,
    /// The attacker's EFFECTIVE miss chance (basis points) vs the target: level-derived `miss_chance_bp`
    /// MINUS any A_MOD_COMBAT(HIT) aura (hit rating reduces miss) — the hit-rating readback. END-appended
    /// + defaulted so adding it auto-migrates the existing row.
    #[default(0)]
    pub hit_miss_bp: u32,
    /// The target's chance (basis points) to PARRY the attacker's swing: `PARRY_BP` floor plus the
    /// defense-vs-weapon-skill term (a higher-level defender parries more), agility-independent — the
    /// parry-scaling readback. END-appended + defaulted so adding it auto-migrates the existing row.
    #[default(0)]
    pub parry_bp: u32,
    /// The attacker's EFFECTIVE swing interval (ms): `base_attack_time_ms` adjusted by any
    /// `A_MOD_SPEED(SPEED_SWING)` aura (haste shortens, an attack-speed slow lengthens) — the swing-speed
    /// readback. END-appended + defaulted.
    #[default(0)]
    pub attack_time_ms: u32,
    /// SPELL crit readbacks (written by `debug_compute_spell` to the "spell" row). The caster's spell
    /// crit chance (bp) = `SPELL_CRIT_BP` + any A_MOD_COMBAT(CRIT) aura. END-appended + defaulted.
    #[default(0)]
    pub spell_crit_bp: u32,
    /// The spell's NON-crit damage (post spell-power scaling + magic resistance) — the baseline hit.
    /// END-appended + defaulted.
    #[default(0)]
    pub spell_hit_normal: u32,
    /// The spell's CRIT damage (×1.5 of the scaled base, then magic resistance) — proves the crit fold.
    /// END-appended + defaulted.
    #[default(0)]
    pub spell_hit_crit: u32,
    /// The TARGET's incoming-damage modifier (signed %, A_MOD_DAMAGE_TAKEN) folded into the swing's
    /// final_min/final_max — so a defensive cooldown like Shield Wall (−75) shows the swing landing for
    /// 25% here, with no fight. 0 = no such aura (final_min/max are armor-only). END-appended + defaulted.
    #[default(0)]
    pub damage_taken_pct: i32,
    /// The TARGET's chance (basis points) to BLOCK the attacker's swing: 0 unless it has a shield
    /// equipped, else `block_chance_bp` (the defense-vs-weapon-skill band). The block-chance readback,
    /// the avoidance/mitigation twin of `parry_bp`. END-appended + defaulted (auto-migrates). [server]
    #[default(0)]
    pub block_bp: u32,
    /// The TARGET's flat shield BLOCK VALUE (the damage a blocked swing absorbs): the shield's base
    /// `block_value` + Str/20, or 0 if unshielded — so a shield's effect is verifiable with no fight.
    /// END-appended + defaulted (auto-migrates). [server]
    #[default(0)]
    pub block_value: u32,
    /// The ATTACKER's effective weapon skill (equipped weapon's line, else level*5) and the TARGET's
    /// effective defense skill (else level*5). Their difference drives the miss/dodge/parry/block bands —
    /// the skill readback, so a below-cap skill set via `debug_set_skill` is verifiable with no fight.
    /// END-appended + defaulted (auto-migrates). [server]
    #[default(0)]
    pub attacker_weapon_skill: u32,
    #[default(0)]
    pub defender_defense_skill: u32,
}

/// Compute the SERVER's swing profile for `attacker` vs `target` (the exact `swing_range_ctx` +
/// `armor_mitigation_pct` live combat uses) and write it to `game_debug_readout` — so combat-stat
/// features (functional armor, weapon damage, …) are verifiable via SQL, with NO live kill. Keyed by
/// "swing" (each call overwrites). No-op if either guid is absent.
#[reducer]
pub fn debug_compute_swing(ctx: &ReducerContext, attacker_guid: u64, target_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let (Some(attacker), Some(target)) = (
        entities.guid().find(attacker_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    let (bmin, bmax) = crate::combat::swing_range_ctx(ctx, &attacker);
    // EFFECTIVE armor (base + any A_MOD_RESISTANCE(armor) aura) — the exact value live combat mitigates
    // against, so an armor buff shows up in mitigation_pct here without a fight.
    let eff_armor = crate::combat::effective_armor(ctx, &target);
    let p = crate::combat::swing_profile(bmin, bmax, eff_armor, attacker.level);
    // EFFECTIVE crit/miss (base + any A_MOD_COMBAT(CRIT)/(HIT) aura on the attacker) — the exact bands
    // roll_swing builds the attack table from, so a crit/hit-rating buff shows up here without a fight.
    let crit_bp = crate::combat::effective_crit_bp(ctx, &attacker);
    // The skill difference (defender defense skill − attacker weapon skill) feeding the skill-based bands,
    // exactly as roll_swing computes it; plus the raw weapon/defense skill values for the readback. The
    // defense skill now folds A_MOD_COMBAT(COMBAT_DEFENSE) auras (Anticipation), so it shifts sd here.
    let sd = crate::skill::skill_diff_ctx(ctx, &attacker, &target);
    let attacker_weapon_skill = crate::skill::effective_weapon_skill(ctx, &attacker);
    let defender_defense_skill = crate::skill::effective_defense_skill(ctx, &target);
    let hit_miss_bp = crate::combat::effective_miss_bp(ctx, &attacker, sd);
    // Defender dodge/parry/block bands — the EXACT effective bands roll_swing uses (the agility/skill base
    // PLUS any A_MOD_COMBAT(DODGE/PARRY/BLOCK) talent aura), so a Deflection/Shield-Spec buff shows up here
    // without a fight. Block is shielded-only.
    let dodge_bp = crate::combat::effective_dodge_bp(ctx, &target, sd);
    let parry_bp = crate::combat::effective_parry_bp(ctx, &target, sd);
    let block_value = crate::combat::effective_block_value(ctx, &target);
    let block_bp = crate::combat::effective_block_bp(ctx, &target, sd, block_value);
    // Effective swing interval (ms): base attack time shortened by any melee-haste aura on the attacker.
    let attack_time_ms = crate::combat::effective_swing_time(ctx, &attacker);
    // Outgoing-damage modifier on the ATTACKER (stance + A_MOD_COMBAT(COMBAT_DMG_DONE) — e.g. Curse of
    // Weakness) folded into the armor-mitigated range FIRST, exactly as resolve_swing/apply_target_damage
    // apply it, so a cursed attacker's swing readout shows the reduced hit — server-verifiable with no fight.
    let damage_done_pct = crate::spell::stance_damage_done_pct(attacker.stance)
        + crate::spell::combat_field_bonus(ctx, attacker_guid, crate::spell::COMBAT_DMG_DONE);
    let outgoing_min = crate::spell::apply_damage_taken(p.final_min, damage_done_pct);
    let outgoing_max = crate::spell::apply_damage_taken(p.final_max, damage_done_pct);
    // Incoming-damage modifier on the TARGET (A_MOD_DAMAGE_TAKEN — e.g. Shield Wall −75%) folded in NEXT,
    // exactly as live combat applies it after armor (combat tick_melee). So a shielded target's swing
    // readout shows the reduced hit — server-verifiable with no fight.
    let damage_taken_pct = crate::spell::damage_taken_bonus(ctx, target_guid);
    let final_min = crate::spell::apply_damage_taken(outgoing_min, damage_taken_pct);
    let final_max = crate::spell::apply_damage_taken(outgoing_max, damage_taken_pct);
    let readouts = ctx.db.game_debug_readout();
    let key = "swing".to_string();
    readouts.key().delete(&key); // upsert
    readouts.insert(DebugReadout {
        key,
        base_min: p.base_min,
        base_max: p.base_max,
        mitigation_pct: p.mitigation_pct,
        final_min,
        final_max,
        note: format!(
            "atk={attacker_guid}(lvl {}) tgt={target_guid}(armor {eff_armor}, dmg_taken {damage_taken_pct}%)",
            attacker.level
        ),
        dodge_bp,
        crit_bp,
        hit_miss_bp,
        parry_bp,
        attack_time_ms,
        damage_taken_pct,
        spell_crit_bp: 0, // melee row — spell fields unused
        spell_hit_normal: 0,
        spell_hit_crit: 0,
        block_bp,
        block_value,
        attacker_weapon_skill,
        defender_defense_skill,
    });
}

/// Compute the SERVER's SPELL hit profile for `caster` casting `spell_id` at `target` and write it to
/// `game_debug_readout` (key "spell") — so spell crit is verifiable via SQL with NO live cast/RNG. Reports
/// the caster's spell crit chance + the NORMAL and CRIT (×1.5) damage, both through the exact
/// spell-power-scale + magic-resistance path `apply_effect`'s E_DAMAGE arm uses (die variance excluded —
/// the readout uses the effect's authored `base_points` so the crit ratio is exact). Uses the spell's
/// FIRST E_DAMAGE effect. No-op if a guid / the spell / a damage effect is absent. Each call overwrites.
#[reducer]
pub fn debug_compute_spell(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    spell_id: u32,
) {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    let Some(hdr) = ctx.db.game_spell().spell_id().find(spell_id) else {
        return;
    };
    // Binary spell-miss chance for this caster→target level pair — surfaced in the note so it's
    // server-verifiable via SQL with no live cast (no readout column needed).
    let spell_miss_bp = crate::spell::spell_miss_chance_bp(caster.level, target.level);
    // The spell's first direct-damage (E_DAMAGE) effect — the one the crit fold applies to.
    let Some(eff) = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .find(|e| e.kind == crate::spell::E_DAMAGE)
    else {
        return;
    };
    // The SAME scale → crit → resist pipeline as the live E_DAMAGE arm, minus the random die + crit roll:
    // use the authored base_points so NORMAL vs CRIT is an exact ×1.5 readback.
    let sp = crate::spell::spell_power(ctx, caster_guid, hdr.school_mask);
    let scaled =
        crate::spell::compose_magnitude(eff.base_points, sp, crate::spell::SPELL_POWER_COEFF_PCT);
    let normal =
        crate::spell::apply_resistance(ctx, target_guid, caster_guid, hdr.school_mask, scaled);
    let crit_raw = crate::spell::apply_spell_crit(scaled, true);
    let crit =
        crate::spell::apply_resistance(ctx, target_guid, caster_guid, hdr.school_mask, crit_raw);
    let spell_crit_bp = crate::spell::spell_crit_bp(ctx, caster_guid);
    let readouts = ctx.db.game_debug_readout();
    let key = "spell".to_string();
    readouts.key().delete(&key); // upsert
    readouts.insert(DebugReadout {
        key,
        base_min: 0,
        base_max: 0,
        mitigation_pct: 0,
        final_min: 0,
        final_max: 0,
        note: format!(
            "caster={caster_guid}(lvl {}) spell={spell_id} '{}' tgt={target_guid}(lvl {}) miss={spell_miss_bp}bp",
            caster.level, hdr.name, target.level
        ),
        dodge_bp: 0,
        crit_bp: 0,
        hit_miss_bp: 0,
        parry_bp: 0,
        attack_time_ms: 0,
        damage_taken_pct: 0, // spell row — the swing-row field
        spell_crit_bp,
        spell_hit_normal: normal.max(0) as u32,
        spell_hit_crit: crit.max(0) as u32,
        block_bp: 0, // spell row — the swing-row fields
        block_value: 0,
        attacker_weapon_skill: 0,
        defender_defense_skill: 0,
    });
}

/// Use (consume) the item in `character_guid`'s inventory `slot` — drives the player `use_item` path
/// by explicit guid (the CLI identity owns no entity), so consumable effects are machine-verifiable
/// without the mouse-only inventory UI. Delegates to the SAME `apply_item_use` the player reducer uses.
#[reducer]
pub fn debug_use_item(ctx: &ReducerContext, character_guid: u64, slot: u8) -> Result<(), String> {
    crate::actor::use_item(ctx, character_guid, slot)
}

/// Grant `count`x item `entry` into `character_guid`'s bags — the test lever for item-dependent flows
/// (cooking reagents etc.) that the mouse-only loot/vendor UI and a SQL insert (game_item_instance has a
/// `created_at` Timestamp) can't drive. Delegates to the SAME `items::grant_item` the loot/quest paths use.
#[reducer]
pub fn debug_grant_item(
    ctx: &ReducerContext,
    character_guid: u64,
    entry: u32,
    count: u32,
) -> Result<(), String> {
    crate::items::grant_item(ctx, character_guid, entry, count)
}

/// Bind `character_guid`'s hearthstone home to its current position — drives the player `bind_home`
/// (innkeeper gossip) by explicit guid for the harness, via the shared `world::set_home`.
#[reducer]
pub fn debug_bind_home(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::world::set_home(ctx, character_guid);
    Ok(())
}

/// Recall `character_guid` to its hearthstone home — drives the hearthstone-use path by explicit guid
/// for the harness (emits a game_teleport_event to home), via the shared `world::recall_to_home`.
#[reducer]
pub fn debug_use_hearthstone(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::world::recall_to_home(ctx, character_guid);
    Ok(())
}

/// Move/swap `character_guid`'s item from `from_slot` to `to_slot` — drives the player `move_item`
/// path by explicit guid (the mouse-only drag-drop is undrivable), via the shared `apply_item_move`.
#[reducer]
pub fn debug_move_item(
    ctx: &ReducerContext,
    character_guid: u64,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), String> {
    crate::items::apply_item_move(ctx, character_guid, from_slot, to_slot)
}

/// Equip `character_guid`'s item in inventory `from_slot` into its matching `EQUIPMENT_SLOT_*` (auto-
/// resolved from the item's `inventory_type`, including the first-free of a finger/trinket pair) —
/// drives the player `equip_item` path by explicit guid (the mouse-only right-click-equip is
/// undrivable), via the shared `apply_equip_item`. Rejects a non-equippable item (e.g. food/junk).
#[reducer]
pub fn debug_equip_item(
    ctx: &ReducerContext,
    character_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    crate::actor::equip_item(ctx, character_guid, from_slot)
}

/// Unequip `character_guid`'s item in equipment `from_slot` (0..=18) into the first free backpack slot —
/// drives the player `unequip_item` path by explicit guid, via the shared `apply_unequip_item`.
#[reducer]
pub fn debug_unequip_item(
    ctx: &ReducerContext,
    character_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    crate::items::apply_unequip_item(ctx, character_guid, from_slot)
}

/// Repair `character_guid`'s item in `slot` to full durability — drives the shared `apply_repair_item`
/// (the future vendor/player repair) by explicit guid, so durability wear/break/repair is verifiable.
#[reducer]
pub fn debug_repair_item(
    ctx: &ReducerContext,
    character_guid: u64,
    slot: u8,
) -> Result<(), String> {
    crate::items::apply_repair_item(ctx, character_guid, slot)
}

/// Sell `character_guid`'s item in `slot` (whole stack) to `vendor_guid` — drives the player `sell_item`
/// path by explicit guid, via the shared `apply_item_sell` (now vendor-gated like buy: credits money,
/// removes the item, requires standing at a real vendor).
#[reducer]
pub fn debug_sell_item(
    ctx: &ReducerContext,
    character_guid: u64,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), String> {
    crate::actor::sell_item(ctx, character_guid, vendor_guid, slot)
}

/// Enter AreaTrigger `trigger_id` as `character_guid` — drives `enter_areatrigger`'s shared core
/// (`apply_enter_areatrigger`) by explicit guid for the harness: credits any "explore" quest tied to
/// the trigger, AND (work-item 225) routes through a cross-map teleport if `trigger_id` is an imported
/// `game_areatrigger_teleport` row — the same live runbook this debug twin drives for 224 exercises the
/// Deadmines portal end-to-end without a physical CMSG_AREATRIGGER from the client.
#[reducer]
pub fn debug_enter_areatrigger(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
) -> Result<(), String> {
    crate::quest::apply_enter_areatrigger(ctx, character_guid, trigger_id);
    Ok(())
}

/// Use the gameobject `go_guid` as `character_guid` — drives `use_gameobject` by explicit guid for the
/// harness (CHEST rolls its loot, GOOBER grants quest credit), via the shared `apply_use_gameobject`.
#[reducer]
pub fn debug_use_gameobject(
    ctx: &ReducerContext,
    character_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    crate::actor::use_gameobject(ctx, character_guid, go_guid)
}

/// Use the FIRST spawned gameobject of template `go_entry` as `character_guid` — same as
/// `debug_use_gameobject` but resolves by template entry, so `spacetime call` can drive it with SMALL
/// args (it mangles guids > 2^53). The parent uses this to server-verify the seeded chest/goober.
#[reducer]
pub fn debug_use_gameobject_entry(
    ctx: &ReducerContext,
    character_guid: u64,
    go_entry: u32,
) -> Result<(), String> {
    let go = ctx
        .db
        .game_gameobject()
        .iter()
        .find(|g| g.template_entry == go_entry)
        .ok_or_else(|| format!("no spawned gameobject of entry {go_entry}"))?;
    crate::actor::use_gameobject(ctx, character_guid, go.guid)
}

// `#[reducer]`: SpacetimeDB reducers take their arguments FLAT off the wire (`spacetime call`), so a parameter struct is not available.
#[allow(clippy::too_many_arguments)]
/// Spawn a gameobject (creating its template if absent) for harness verification — the seed only runs
/// on `init`, so a re-published DB has no seed GOs, and `game_gameobject`'s Timestamp column blocks a
/// SQL insert. Idempotent (deletes any prior GO at the derived HIGHGUID_GAMEOBJECT guid first).
///
/// `data1` (required skill level) + `gather_skill_line` (MINING 186 / HERBALISM 182) + `respawn_secs`
/// (per-node respawn window in seconds, 0 ⇒ 3-min fallback) + `gather_gray` (skill-up gray ceiling, 0 ⇒
/// the always-skill sentinel) are END-APPENDED args (the gateway tolerates extra END-appended reducer
/// args, per the marathon note) so the ONE spawn lever can mint a GATHER node (`type_id` 25) as well as a
/// CHEST/GOOBER — no dup lever. For a non-gather GO pass `... data1 0 gather_skill_line 0 respawn_secs 0
/// gather_gray 0`; for a 30s-respawn Copper Vein pass `... data0 2770 data1 1 skill 186 respawn_secs 30
/// gather_gray 0` (2770 = the real imported Copper Ore the vein grants).
#[reducer]
pub fn debug_spawn_gameobject(
    ctx: &ReducerContext,
    template_entry: u32,
    type_id: u8,
    display_id: u32,
    data0: u32,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    data1: u32,
    gather_skill_line: u32,
    respawn_secs: u32, // END-appended (respawn-variance slice): 0 ⇒ the 3-min fallback; else the node's own window
    gather_gray: u32,  // END-appended (skill-up-difficulty slice): 0 ⇒ the always-skill sentinel
) -> Result<(), String> {
    if ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(template_entry)
        .is_none()
    {
        ctx.db
            .game_gameobject_template()
            .insert(crate::gameobject::GameObjectTemplate {
                entry: template_entry,
                type_id,
                display_id,
                name: format!("Debug GO {template_entry}"),
                data0,
                data1,
                gather_skill_line,
                respawn_secs,
                gather_gray,
                lock_id: 0, // work-item 211: the debug spawn lever doesn't carry a lockId this slice
            });
    }
    let guid = (0xF110u64 << 48) | template_entry as u64;
    ctx.db.game_gameobject().guid().delete(guid);
    // Clear any prior Pick-Lock unlock (work-item 119) for this DERIVED guid so a re-spawn of a locked
    // template is deterministically LOCKED again (the verify's negative case relies on this).
    ctx.db.game_gameobject_unlocked().go_guid().delete(guid);
    ctx.db
        .game_gameobject()
        .insert(crate::gameobject::GameObject {
            guid,
            template_entry,
            map_id,
            x,
            y,
            z,
            orientation: 0.0,
            state: 0,
            created_at: ctx.timestamp,
            respawn_at_micros: 0, // a freshly-spawned node is ready (no pending respawn)
            instance_id: 0,       // debug spawns land in the open world (190 slice 2)
            grid_x: lyracore_shared::spatial::grid_cell(x, y).0,
            grid_y: lyracore_shared::spatial::grid_cell(x, y).1,
        });
    Ok(())
}

/// Force the FIRST depleted gameobject of template `go_entry` to respawn NOW — flips it state 0 +
/// clears its `respawn_at_micros` INLINE so the verify is deterministic with no wait for the ~4s sense
/// tick (`pass_gameobject_respawn` would do the same flip once the armed time elapses). Resolves by
/// template entry so `spacetime call` can drive it with SMALL args (it mangles guids > 2^53), mirroring
/// `debug_use_gameobject_entry`. The professions verify lever.
#[reducer]
pub fn debug_force_gameobject_respawn(ctx: &ReducerContext, go_entry: u32) -> Result<(), String> {
    let gos = ctx.db.game_gameobject();
    let mut go = gos
        .iter()
        .find(|g| g.template_entry == go_entry && g.state == 1)
        .ok_or_else(|| format!("no depleted gameobject of entry {go_entry}"))?;
    go.state = 0;
    go.respawn_at_micros = 0;
    gos.guid().update(go);
    Ok(())
}

/// Stand up a TEST gathering POOL with no re-import (the POOL-model verify lever). Ensures each member's
/// GATHER template exists, (re)creates the pool header + member point rows, then ARMS the pool to exactly
/// `max_active` weighted-distinct active rows — so init already shows the rotation property and the
/// MAX_ACTIVE invariant. Idempotent on `pool_id` (clears that pool's prior members + their live rows
/// first). Args packed as a STRING (the `spacetime call` u64-mangling + the existing packed-import
/// convention): `members` is `;`-separated points, each
/// `template_entry,display_id,data0,data1,gather_skill_line,respawn_secs,map,x,y,z,o,weight` (12 fields).
/// A higher-tier member is just a row with a different `template_entry` + a small `weight`.
///
/// `in_place` sets the pool's KIND (the tier-variety column): false = a ROAMING pool (gather → instant
/// reroll, the node WANDERS — the importer's single-tier spatial pools); true = an IN-PLACE TIER point
/// (`max_active` 1, members CO-LOCATED at one coord but of differing tiers — gather flips+arms the timer
/// and the respawn pass weighted-RE-ROLLS the tier in place, so the point mostly re-presents the common
/// tier and occasionally a rarer one, NO wander). Existing callers pass false (unchanged behavior).
///
/// Example A — a ROAMING pool (in_place false; pool 1, max 2, five Copper at weight 20 + one Tin at weight
/// 2, 5s respawn, by the spawn; data0 2770 = the real imported Copper Ore the vein grants):
///   spacetime call <db> debug_setup_gather_pool 1 2 false \
///     '9999,259,2770,1,186,5,0,-100,-100,0,0,20;9999,259,2770,1,186,5,0,-110,-100,0,0,20;\
///      9999,259,2770,1,186,5,0,-120,-100,0,0,20;9999,259,2770,1,186,5,0,-130,-100,0,0,20;\
///      9999,259,2770,1,186,5,0,-140,-100,0,0,20;9998,259,2770,1,186,5,0,-150,-100,0,0,2'
/// Example B — the IN-PLACE TIER demonstrator (in_place true; pool 2, max 1, Copper 1731 w85 + Tin 1732
/// w15 CO-LOCATED at the real Goldshire coord — both members share x,y,z so the rolled tier never wanders):
///   spacetime call <db> debug_setup_gather_pool 2 1 true \
///     '1731,259,2770,1,186,300,0,-9620.11,-46.3336,47.3641,2.04204,85;\
///      1732,259,2771,65,186,300,0,-9620.11,-46.3336,47.3641,2.04204,15'
#[reducer]
pub fn debug_setup_gather_pool(
    ctx: &ReducerContext,
    pool_id: u32,
    max_active: u32,
    in_place: bool,
    members: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let pool_tbl = ctx.db.game_gameobject_pool();
    let member_tbl = ctx.db.game_gameobject_pool_member();
    let tmpl_tbl = ctx.db.game_gameobject_template();
    let gos = ctx.db.game_gameobject();

    // CLEAR this pool's prior member rows + any live row at their derived guids (idempotent re-setup —
    // mirrors the import's clear-then-load, but scoped to THIS pool so it never touches others).
    let prior: Vec<crate::gameobject::GameObjectPoolMember> =
        member_tbl.iter().filter(|m| m.pool_id == pool_id).collect();
    for m in &prior {
        gos.guid()
            .delete(crate::gameobject::pool_point_guid(m.point_id));
        member_tbl.point_id().delete(m.point_id);
    }

    // POOL header (upsert). `in_place` carries the pool's KIND through to the deplete/respawn branch.
    pool_tbl.pool_id().delete(pool_id);
    pool_tbl.insert(crate::gameobject::GameObjectPool {
        pool_id,
        max_active,
        in_place,
    });

    // MEMBER points: parse + ensure each GATHER template exists + insert the point row (auto_inc point_id).
    let pu32 = |s: &str| s.trim().parse::<u32>().map_err(|_| format!("bad u32: {s}"));
    let pf32 = |s: &str| s.trim().parse::<f32>().map_err(|_| format!("bad f32: {s}"));
    let mut count = 0u32;
    for row in members.split(';').filter(|r| !r.trim().is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 12 {
            return Err(format!(
                "pool member row needs 12 fields, got {}: {row}",
                f.len()
            ));
        }
        let template_entry = pu32(f[0])?;
        let display_id = pu32(f[1])?;
        let data0 = pu32(f[2])?;
        let data1 = pu32(f[3])?;
        let gather_skill_line = pu32(f[4])?;
        let respawn_secs = pu32(f[5])?;
        let map_id = pu32(f[6])?;
        let (x, y, z, o) = (pf32(f[7])?, pf32(f[8])?, pf32(f[9])?, pf32(f[10])?);
        let weight = pu32(f[11])?;
        // Ensure the GATHER template (type 25) — so a brand-new test entry needs no separate spawn call.
        if tmpl_tbl.entry().find(template_entry).is_none() {
            tmpl_tbl.insert(crate::gameobject::GameObjectTemplate {
                entry: template_entry,
                type_id: crate::gameobject::go_type::GATHER,
                display_id,
                name: format!("Pool GO {template_entry}"),
                data0,
                data1,
                gather_skill_line,
                respawn_secs,
                gather_gray: 0,
                lock_id: 0, // work-item 211: pool test templates don't carry a lockId this slice
            });
        }
        member_tbl.insert(crate::gameobject::GameObjectPoolMember {
            point_id: 0, // auto_inc
            pool_id,
            template_entry,
            map_id,
            x,
            y,
            z,
            orientation: o,
            weight,
        });
        count += 1;
    }
    if count == 0 {
        return Err("pool setup had no members".to_string());
    }
    // ARM: insert exactly max_active weighted-distinct live rows (the only place the active count is SET).
    crate::gameobject::arm_pool(ctx, pool_id);
    Ok(())
}

/// Force a pool RE-ROLL NOW for a deterministic no-wait verify (the ~4s sense tick would do the same on
/// the armed timer). Deactivates `gathered_guid` and activates a different inactive weighted-chosen
/// point — the active count stays `max_active`. `gathered_guid` is the live guid of the point to roll;
/// resolve it from the SQL `SELECT guid FROM game_gameobject WHERE ...` (it's < 2^53 only for small
/// point_ids — pass it after a `--` if negative-looking, though GO guids are large/unsigned).
#[reducer]
pub fn debug_force_pool_reroll(
    ctx: &ReducerContext,
    pool_id: u32,
    gathered_guid: u64,
) -> Result<(), String> {
    if ctx
        .db
        .game_gameobject_pool()
        .pool_id()
        .find(pool_id)
        .is_none()
    {
        return Err(format!("no pool {pool_id}"));
    }
    crate::gameobject::reroll_pool(ctx, pool_id, gathered_guid);
    Ok(())
}

/// Buy `count`× `item_entry` for `character_guid` — drives the player `buy_item` path by explicit
/// guid, via the shared `apply_buy_item` (debits money, grants the item in a free backpack slot).
#[reducer]
pub fn debug_buy_item(
    ctx: &ReducerContext,
    character_guid: u64,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    crate::actor::buy_item(ctx, character_guid, vendor_guid, item_entry, count)
}

/// Split `count` units off `character_guid`'s stack in `slot` into the empty `to_slot` — drives the
/// player `split_item` path by explicit guid, via the shared `apply_item_split`.
#[reducer]
pub fn debug_split_item(
    ctx: &ReducerContext,
    character_guid: u64,
    slot: u8,
    count: u32,
    to_slot: u8,
) -> Result<(), String> {
    crate::items::apply_item_split(ctx, character_guid, slot, count, to_slot)
}

/// Arm a melee engagement `attacker_guid` → `target_guid` directly (insert/retarget a `game_melee_attack`
/// row) — the machine-test counterpart to the player `start_attack` (which is `ctx.sender`-bound and so
/// undrivable from the CLI identity). Lets a test put a CREATURE into combat so the threat-retarget pass
/// considers it, without a live client swing. No validation (dev only): the caller picks valid guids.
#[reducer]
pub fn debug_engage(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    let melee = ctx.db.game_melee_attack();
    let row = crate::MeleeAttack {
        attacker_guid,
        target_guid,
        last_swing_ms: 0,
        ranged_spell_id: 0,
        last_offhand_swing_ms: 0,
    };
    if melee.attacker_guid().find(attacker_guid).is_some() {
        melee.attacker_guid().update(row);
    } else {
        melee.insert(row);
    }
    // Point the entity at its target too (real aggro arming sets `target_guid`; mirror it so the live
    // row and the melee row agree before any retarget pass runs).
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(attacker_guid) {
        e.target_guid = target_guid;
        entities.guid().update(e);
    }
    Ok(())
}

/// Cast `spell_id` from `caster_guid` AT `target_guid` — drives the target-aware `resolve_cast_at`
/// (the player `cast_spell` resolves the target from the client; here it's explicit) so a direct-damage
/// spell is machine-verifiable. Sources the caster level from its entity, like `debug_force_cast`.
#[reducer]
pub fn debug_cast_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    // Debug direct cast resolves synchronously — not a timed-cast completion (instant packet
    // sequence); the actor verb sources the caster level from the live entity.
    crate::actor::cast_at(ctx, caster_guid, spell_id, target_guid)
}

/// 1-20 leveling audit (2026-07-17): for each of the 6 human-class trainers, resolve EVERY offering
/// through the server's own `resolve_learn_target` (wrapper → rank) and classify the rank spell's
/// effects — LIVE (has a real handler), HOLLOW (all effects are unmapped E_SCRIPTED no-ops, script_id
/// 0 — the spell "exists" but does nothing), or NO-EFFECT (learn-only/passive, no effect rows). Logs a
/// per-class band summary (L1-9 / L10-19 / L20) + the concrete HOLLOW list (class · req_level · id ·
/// name) — the actionable "which class abilities don't work through 1-20" wall list. Read the module
/// log after calling. Pure read, no state change.
#[reducer]
pub fn debug_audit_class_kits(ctx: &ReducerContext) {
    use crate::game_trainer_spell;
    use crate::spell::{A_FLAG, E_SCRIPTED};
    // (class label, trainer creature entry) — the stable CLASS_TRAINERS map (playerbots mod.rs).
    const TRAINERS: &[(&str, u32)] = &[
        ("Warrior", 913),
        ("Paladin", 927),
        ("Rogue", 917),
        ("Priest", 377),
        ("Mage", 328),
        ("Warlock", 906),
    ];
    // PASSIVE combat skills whose mechanic lives in the COMBAT ENGINE, not as a spell effect
    // (verified 2026-07-17: combat/tables.rs parry/block in the attack table, combat/mod.rs off-hand
    // swings). Their spell has no live effect but the ability WORKS — so they are NOT hollow. Matched
    // by name (rank-suffix stripped by the resolver). Keeps the HOLLOW list actionable.
    const ENGINE_PASSIVES: &[&str] = &[
        "Parry",
        "Dodge",
        "Block",
        "Shield Block",
        "Dual Wield",
        "Defense",
        "Two-Handed Swords",
        "Detect Magic",
        "Slow Fall",
        "Unending Breath",
    ];
    // Classify a resolved rank spell: Some(true)=LIVE, Some(false)=HOLLOW (unmapped no-op), None=no
    // effects, or the passive-engine bucket via the returned name check at the call site.
    let classify = |rank: u32| -> Option<bool> {
        let effects: Vec<_> = ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(&rank)
            .collect();
        if effects.is_empty() {
            return None;
        }
        // A_FLAG passive markers don't count as "does something"; an E_SCRIPTED with script_id 0 is
        // the unmapped no-op. Any other effect kind (or a scripted handler with a real id) is live.
        let live = effects
            .iter()
            .any(|e| e.kind != A_FLAG && !(e.kind == E_SCRIPTED && e.script_id == 0));
        Some(live)
    };
    let band = |lvl: u8| -> usize {
        if lvl >= 20 {
            2
        } else if lvl >= 10 {
            1
        } else {
            0
        }
    };
    log::info!("=== CLASS-KIT AUDIT (1-20 leveling) — LIVE / HOLLOW / NO-EFFECT per band ===");
    for (label, trainer) in TRAINERS {
        // [band][0]=live [1]=hollow [2]=no-effect
        let mut counts = [[0u32; 3]; 3];
        let mut hollow: Vec<(u8, u32, String)> = Vec::new();
        for off in ctx
            .db
            .game_trainer_spell()
            .iter()
            .filter(|t| t.trainer_entry == *trainer)
        {
            if off.required_level > 20 {
                continue;
            }
            let rank = crate::trainer::resolve_learn_target(ctx, off.spell_id);
            let b = band(off.required_level);
            let name = ctx
                .db
                .game_spell()
                .spell_id()
                .find(rank)
                .map(|s| s.name)
                .unwrap_or_default();
            match classify(rank) {
                Some(true) => counts[b][0] += 1,
                // An engine-implemented passive counts as LIVE (the mechanic works), not hollow.
                _ if ENGINE_PASSIVES.contains(&name.as_str()) => counts[b][0] += 1,
                Some(false) => {
                    counts[b][1] += 1;
                    hollow.push((off.required_level, rank, name));
                }
                None => counts[b][2] += 1,
            }
        }
        log::info!(
            "{label:8}  L1-9: {}L/{}H/{}N   L10-19: {}L/{}H/{}N   L20: {}L/{}H/{}N",
            counts[0][0],
            counts[0][1],
            counts[0][2],
            counts[1][0],
            counts[1][1],
            counts[1][2],
            counts[2][0],
            counts[2][1],
            counts[2][2],
        );
        hollow.sort_by_key(|(lvl, _, _)| *lvl);
        for (lvl, id, name) in hollow {
            log::info!("    HOLLOW {label} L{lvl:<2} {id:>6} {name}");
        }
    }
    log::info!("=== END CLASS-KIT AUDIT ===");
}

/// 1-20 quest-chain integrity audit (2026-07-17): for every quest with `quest_level` in 1..=20,
/// check the four things a leveling player needs to actually COMPLETE it, and log the broken ones —
/// the "which quests would stall a 1-20 playthrough" wall list. Checks: (1) a START giver exists (a
/// creature or GO offers it — else the quest is unreachable), (2) an END giver exists (else it can't
/// be turned in), (3) `prev_quest_id`, if set, resolves to a real quest (else a dangling chain), (4)
/// every KILL/COLLECT objective target + every reward item references an existing template. Pure read.
#[reducer]
pub fn debug_audit_quest_chains(ctx: &ReducerContext) {
    use crate::quest::quest_role;
    use crate::{
        game_creature_quest, game_gameobject_quest, game_quest_objective, game_quest_reward_item,
    };
    let cq = ctx.db.game_creature_quest();
    let goq = ctx.db.game_gameobject_quest();
    let has_giver = |quest: u32, role: u8| -> bool {
        cq.iter().any(|r| r.quest_entry == quest && r.role == role)
            || goq.iter().any(|r| r.quest_entry == quest && r.role == role)
    };
    let band = |lvl: u32| -> usize {
        if lvl >= 20 {
            2
        } else if lvl >= 10 {
            1
        } else {
            0
        }
    };
    let mut ok = [0u32; 3];
    let mut broken: Vec<(u32, u32, String, String)> = Vec::new(); // (level, entry, title, reasons)
    log::info!("=== QUEST-CHAIN AUDIT (1-20) — completability per band ===");
    for q in ctx.db.game_quest_template().iter() {
        if q.quest_level == 0 || q.quest_level > 20 {
            continue;
        }
        let mut reasons: Vec<&str> = Vec::new();
        if !has_giver(q.entry, quest_role::START) {
            reasons.push("no START giver");
        }
        if !has_giver(q.entry, quest_role::END) {
            reasons.push("no END giver");
        }
        if q.prev_quest_id != 0
            && ctx
                .db
                .game_quest_template()
                .entry()
                .find(q.prev_quest_id)
                .is_none()
        {
            reasons.push("dangling prev_quest");
        }
        for o in ctx.db.game_quest_objective().by_quest().filter(&q.entry) {
            use crate::quest::objective_kind;
            let missing = match o.kind {
                objective_kind::KILL_CREATURE => ctx
                    .db
                    .game_creature_template()
                    .entry()
                    .find(o.target_entry)
                    .is_none(),
                objective_kind::COLLECT_ITEM => ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(o.target_entry)
                    .is_none(),
                _ => false, // USE_GAMEOBJECT / EXPLORE: target validated elsewhere
            };
            if missing {
                reasons.push("bad objective target");
                break;
            }
        }
        for r in ctx.db.game_quest_reward_item().by_quest().filter(&q.entry) {
            if ctx
                .db
                .game_item_template()
                .entry()
                .find(r.item_entry)
                .is_none()
            {
                reasons.push("bad reward item");
                break;
            }
        }
        if reasons.is_empty() {
            ok[band(q.quest_level)] += 1;
        } else {
            broken.push((q.quest_level, q.entry, q.title.clone(), reasons.join(", ")));
        }
    }
    log::info!(
        "OK quests — L1-9: {}   L10-19: {}   L20: {}   (broken: {})",
        ok[0],
        ok[1],
        ok[2],
        broken.len()
    );
    broken.sort_by_key(|(lvl, _, _, _)| *lvl);
    for (lvl, entry, title, reasons) in broken {
        log::info!("    BROKEN L{lvl:<2} q{entry:<5} [{reasons}] {title}");
    }
    log::info!("=== END QUEST-CHAIN AUDIT ===");
}

/// Begin a (possibly cast-timed) cast from `caster_guid` AT `target_guid` — drives `begin_cast` so a
/// spell with `cast_time_ms > 0` schedules a `PendingCast` that resolves when the cast finishes (the
/// effect lands AFTER the bar, not instantly). Sources the caster level from its entity.
#[reducer]
pub fn debug_begin_cast(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .ok_or_else(|| format!("no live entity for guid {caster_guid}"))?;
    crate::spell::begin_cast(
        ctx,
        caster_guid,
        spell_id,
        e.level as u8,
        target_guid,
        false,
        None,
    )
}

/// Cast a GROUND-TARGETED spell at an explicit world point by caster guid — server-side proof of the 118
/// phase-2 dest plumbing (the CLI owns no entity, so it can't drive the real `cast_spell_at`). Drives the
/// full `begin_cast(Some(dest))` path, so a TIMED ground spell also exercises the PendingCast dest carry.
#[reducer]
pub fn debug_cast_spell_at(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .ok_or_else(|| format!("no live entity for guid {caster_guid}"))?;
    let target = if target_guid == 0 {
        caster_guid
    } else {
        target_guid
    };
    crate::spell::begin_cast(
        ctx,
        caster_guid,
        spell_id,
        e.level as u8,
        target,
        false,
        Some((x, y, z)),
    )
}

/// Log the reputation vendor discount (195) `player_guid` gets at a creature whose FactionTemplate is
/// `faction_template_id` — runs the real `vendor_discount_pct` resolution against live data (server-side
/// proof without needing to drive a full buy, whose vendor guid >2^53 mangles through `spacetime call`).
#[reducer]
pub fn debug_log_vendor_discount(
    ctx: &ReducerContext,
    player_guid: u64,
    faction_template_id: u32,
) -> Result<(), String> {
    let pct = crate::reputation::vendor_discount_pct(ctx, player_guid, faction_template_id);
    log::info!("debug_log_vendor_discount: player {player_guid} faction_template {faction_template_id} → {pct}% off");
    Ok(())
}

// `#[reducer]`: SpacetimeDB reducers take their arguments FLAT off the wire (`spacetime call`), so a parameter struct is not available.
#[allow(clippy::too_many_arguments)]
/// Drive the anti-cheat movement scorer (255) by explicit guid + two positions + move times — the CLI
/// owns no entity so it can't send a real heartbeat, this proves the live `score_and_log_movement` path
/// inserts a `game_movement_violation` row on an anomalous delta (and none on a legit one). Server-side
/// verification only (never in the live movement flow).
#[reducer]
pub fn debug_score_movement(
    ctx: &ReducerContext,
    guid: u64,
    old_x: f32,
    old_y: f32,
    x: f32,
    y: f32,
    z: f32,
    old_move_ms: u32,
    move_time_ms: u32,
) -> Result<(), String> {
    crate::world::score_and_log_movement(
        ctx,
        guid,
        old_x,
        old_y,
        x,
        y,
        z,
        old_move_ms,
        move_time_ms,
    );
    Ok(())
}

/// Take the item in `corpse_guid`'s loot slot `loot_slot` into `character_guid`'s bags — drives the
/// player `take_loot` path by explicit guid (the mouse-only loot window is undrivable), via the shared
/// `apply_take_loot`.
#[reducer]
pub fn debug_take_loot(
    ctx: &ReducerContext,
    character_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), String> {
    crate::actor::take_loot(ctx, character_guid, corpse_guid, loot_slot)
}

/// Equip (or swap) `character_guid`'s main-hand weapon to `item_entry` — drives the weapon-damage-in
/// -swing test without the mouse-only equip UI (the same DirectInput wall as loot/equip clicks).
/// Upserts the owned item instance in the main-hand slot (15) to the given template entry, stamping
/// the live entity's owner identity so the per-owner RLS filter still shows it. The swing-damage
/// lookup joins `game_item_template`, so a stronger seeded weapon immediately feeds `roll_swing` /
/// `debug_compute_swing`. Durability seeds from the template (0 if the template isn't loaded). [entity]
#[reducer]
pub fn debug_equip_weapon(
    ctx: &ReducerContext,
    character_guid: u64,
    item_entry: u32,
) -> Result<(), String> {
    equip_into(
        ctx,
        character_guid,
        item_entry,
        constants::starter_item::MAINHAND_SLOT,
    )
}

/// Equip (or swap) `character_guid`'s OFF-HAND to `item_entry` — the off-hand twin of
/// [`debug_equip_weapon`], for shield-block testing without the mouse-only equip UI. Equip a shield
/// (e.g. seed entry 53 "Battered Buckler") here and `effective_block_value`/`effective_block_bp` arm the
/// block band, verifiable via `debug_compute_swing` or a live fight with no client interaction. [entity]
#[reducer]
pub fn debug_equip_offhand(
    ctx: &ReducerContext,
    character_guid: u64,
    item_entry: u32,
) -> Result<(), String> {
    equip_into(
        ctx,
        character_guid,
        item_entry,
        crate::items::equip_slot::OFFHAND,
    )
}

/// Shared core of the `debug_equip_*` reducers: upsert `character_guid`'s owned item instance in `slot`
/// to `item_entry`, stamping the live entity's owner identity (so the per-owner RLS filter still shows
/// it) and seeding durability from the template (0 if the template isn't loaded). The slot-derived guid
/// is unique per slot, so equipping different slots never collides.
fn equip_into(
    ctx: &ReducerContext,
    character_guid: u64,
    item_entry: u32,
    slot: u8,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let guid = crate::items::item_guid_for(character_guid, slot);
    let tmpl = ctx.db.game_item_template().entry().find(item_entry);
    let durability = tmpl.as_ref().map(|t| t.max_durability).unwrap_or(0);
    // This helper places the item DIRECTLY on the body (bypassing the pickup->equip two-step apply_item_move
    // drives), so either a BoP or a BoE template binds immediately here — mirroring "the item is now worn".
    let soulbound = tmpl
        .as_ref()
        .map(|t| crate::items::binds_on_grant(t.bonding) || crate::items::binds_on_equip(t.bonding))
        .unwrap_or(false);
    let instances = ctx.db.game_item_instance();
    if let Some(mut inst) = instances.guid().find(guid) {
        inst.entry = item_entry;
        inst.durability = durability;
        inst.soulbound = inst.soulbound || soulbound;
        instances.guid().update(inst);
    } else {
        instances.insert(ItemInstance {
            guid,
            entry: item_entry,
            owner_identity: e.owner_identity,
            owner_guid: character_guid,
            slot,
            stack_count: 1,
            durability,
            created_at: ctx.timestamp,
            enchant_id: 0, // debug-granted item — unenchanted
            soulbound,
        });
    }
    Ok(())
}

/// Accept quest `quest_entry` from giver `giver_guid` for `character_guid` — drives the player
/// `accept_quest` path by explicit guid (the gossip/quest UI is mouse-only / not yet wired), via the
/// shared `apply_accept_quest`. The harness positions the player in range of the giver first.
#[reducer]
pub fn debug_accept_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    crate::actor::accept_quest(ctx, character_guid, giver_guid, quest_entry)
}

/// 279 relay-stress: ONE transaction shaped like the 277 killer — `junk_rows` rows the target's
/// subscriptions don't match (creature-move legs for nonexistent movers, reaped by the 1s event
/// TTL) PLUS the relay-carried rows (a quest kill credit for `victim_entry` and an item grant).
/// The wire test asserts the client still receives SMSG_QUESTUPDATE_ADD_KILL and
/// SMSG_ITEM_PUSH_RESULT out of this fat transaction — the delivery guarantee the coordinator
/// relay migration exists to provide.
#[reducer]
pub fn debug_stress_relay(
    ctx: &ReducerContext,
    character_guid: u64,
    victim_entry: u32,
    item_entry: u32,
    junk_rows: u32,
) -> Result<(), String> {
    let moves = ctx.db.game_creature_move_event();
    for i in 0..junk_rows {
        moves.insert(crate::CreatureMoveEvent {
            id: 0,
            mover_guid: u64::MAX - 100_000 - i as u64, // no such entities — pure txn ballast
            start_x: 20_000.0,
            start_y: 20_000.0,
            start_z: 0.0,
            dest_x: 20_001.0,
            dest_y: 20_001.0,
            dest_z: 0.0,
            duration_ms: 100,
            spline_id: i,
            created_at: ctx.timestamp,
            run: false,
        });
    }
    crate::quest::on_creature_killed(ctx, character_guid, victim_entry);
    crate::items::grant_item(ctx, character_guid, item_entry, 1)?;
    Ok(())
}

/// Grant quest `quest_entry` to `character_guid` with NO giver — stages a quest in the log for live
/// testing (verifying the quest-log and kill-feedback paths). Bypasses the giver validation that the
/// player/`debug_accept_quest` path needs, because the giver's creature guid is a u64 > 2^53 that the CLI
/// `spacetime call` mangles via JSON f64. Same insert as a real accept, minus the gates.
#[reducer]
pub fn debug_grant_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    crate::actor::stage_quest(ctx, character_guid, quest_entry)
}

/// Force-expire `character_guid`'s `quest_entry` (work-item 194) — the harness twin of the timed-quest
/// tick (`quest::quest_timer_pass`), for the runbook: verify `SMSG_QUESTUPDATE_FAILEDTIMER` fires on
/// the wire and the quest is re-acceptable, without waiting out `limit_time` seconds live.
#[reducer]
pub fn debug_expire_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    // REFUSE verdict (issue #30): the write lands in `game_character_quest`, a MANIFEST table — the
    // export blob's own enumeration — so post-begin it is a lost write cross-database.
    crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or_else(|| format!("no character {character_guid}"))?;
    crate::quest::debug_force_expire(ctx, character_guid, quest_entry)
}

/// Share `quest_entry` from `character_guid` to its party (work-item 194) — drives
/// `push_quest_to_party` by explicit guid (the CLI identity owns no entity to resolve via
/// `ctx.sender()`).
#[reducer]
pub fn debug_push_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    crate::quest::apply_push_quest_to_party(ctx, character_guid, quest_entry)
}

/// Fire the `on_gossip_select` notify-hook by explicit guid — the harness twin of
/// the player `gossip_select` reducer (the CLI identity owns no entity, and a real gossip click is
/// mouse-only). Pure notification, no gossip behavior.
#[reducer]
pub fn debug_gossip_select(
    ctx: &ReducerContext,
    character_guid: u64,
    npc_guid: u64,
    option_id: u32,
    option_row_id: u32,
) -> Result<(), String> {
    crate::hooks::fire_on_gossip_select(
        ctx,
        &crate::hooks::GossipSelectPayload {
            character_guid,
            npc_guid,
            option_id,
            option_row_id,
        },
    );
    Ok(())
}

/// Turn quest `quest_entry` in to giver `giver_guid` for `character_guid` — drives the player
/// `turn_in_quest` path by explicit guid, via the shared `apply_turn_in_quest` (grants money/XP/items).
/// `reward_index` picks the choice reward (pick-1-of-N); pass 0 for a quest with no choice rewards.
#[reducer]
pub fn debug_turn_in_quest(
    ctx: &ReducerContext,
    character_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), String> {
    crate::actor::turn_in_quest(ctx, character_guid, giver_guid, quest_entry, reward_index)
}

/// Kill creature `target_guid` crediting player `killer_guid` — drives the SHARED killing-blow path
/// (`combat::kill_creature` with a killer), so it produces the exact same corpse + loot AND the XP +
/// quest-objective credit a real player kill does. The machine-test counterpart to fighting a mob to
/// death: lets a test advance a kill quest deterministically without the combat tick. Errors only if
/// nothing was killed (missing guid / a player / an already-dead unit).
#[reducer]
pub fn debug_kill_creature(
    ctx: &ReducerContext,
    killer_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    if crate::combat::kill_creature(ctx, target_guid, Some(killer_guid)) {
        Ok(())
    } else {
        Err(format!("nothing killed for target {target_guid}"))
    }
}

/// Kill the nearest LIVE creature of `creature_entry` to `killer_guid`, crediting the killer — same shared
/// killing-blow path as [`debug_kill_creature`] but selects the target SERVER-SIDE by entry, so a test can
/// advance a kill quest with only small args (the creature's own guid is a u64 > 2^53 the CLI `spacetime
/// call` mangles). Used to verify kill-feedback and the live quest-log update end to end.
#[reducer]
pub fn debug_kill_nearest(
    ctx: &ReducerContext,
    killer_guid: u64,
    creature_entry: u32,
) -> Result<(), String> {
    let killer = ctx
        .db
        .game_world_entity()
        .guid()
        .find(killer_guid)
        .ok_or_else(|| format!("killer {killer_guid} not in world"))?;
    let mut best: Option<(u64, f32)> = None;
    for e in ctx.db.game_world_entity().iter() {
        if e.entry != creature_entry || e.guid == killer_guid || e.dead {
            continue;
        }
        let d = (e.x - killer.x).powi(2) + (e.y - killer.y).powi(2);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((e.guid, d));
        }
    }
    let target_guid = best
        .ok_or_else(|| format!("no live creature entry {creature_entry} near killer"))?
        .0;
    if crate::combat::kill_creature(ctx, target_guid, Some(killer_guid)) {
        Ok(())
    } else {
        Err(format!("nothing killed for entry {creature_entry}"))
    }
}

/// SKIN the NEAREST skinnable BEAST CORPSE to `character_guid` (Skinning, professions) — the
/// small-arg test lever (a creature corpse's own guid is a u64 > 2^53 the CLI `spacetime call` mangles, so
/// the big corpse guid is resolved SERVER-SIDE here, mirroring `debug_kill_nearest`'s nearest-search). It
/// finds the closest dead non-player creature whose template `creature_type == BEAST_TYPE` and isn't yet
/// `skinned`, then funnels through the shared `loot::skin_corpse` core (auto-learn → grant leather →
/// climb Skinning → mark skinned). A future `CMSG`-routed skin over the open corpse will share that core.
#[reducer]
pub fn debug_skin_nearest(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let looter = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("skinner {character_guid} not in world"))?;
    let mut best: Option<(u64, f32)> = None;
    for e in ctx.db.game_world_entity().iter() {
        // A skinnable target: a dead non-player BEAST corpse not yet skinned, on the looter's map
        // AND in the looter's instance (190 slice 2 — mirrors `loot::can_skin`'s live gate, so the
        // debug lever can never find a corpse the real skin path would refuse).
        if !e.dead
            || e.is_player()
            || e.skinned
            || e.map_id != looter.map_id
            || e.instance_id != looter.instance_id
            || !crate::loot::entry_is_beast(ctx, e.entry)
        {
            continue;
        }
        let d = (e.x - looter.x).powi(2) + (e.y - looter.y).powi(2);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((e.guid, d));
        }
    }
    let corpse_guid = best
        .ok_or_else(|| "no skinnable beast corpse near".to_string())?
        .0;
    crate::loot::skin_corpse(ctx, character_guid, corpse_guid)
}

/// FISH at `character_guid`'s spot (FISHING, completing the 13) — the small-arg test twin of the `fish`
/// player reducer (the CLI identity owns no entity, so the guid is explicit). Drives the shared
/// `loot::apply_fish` core (auto-learn → grant a random fish from the pool → climb Fishing). Verify: a fish
/// item (6291/6303/6289) appears in the backpack + the Fishing skill row climbs.
#[reducer]
pub fn debug_fish(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::loot::apply_fish(ctx, character_guid)
}

/// DISENCHANT the item in equipment `slot` for `character_guid` (ENCHANTING, completing the 13) — the
/// small-arg test twin of the `disenchant` player reducer. Drives `loot::apply_disenchant` (consume the
/// item → grant Strange Dust 10940 → climb Enchanting). Verify: the item row is DELETED, mats granted,
/// Enchanting +1.
#[reducer]
pub fn debug_disenchant(ctx: &ReducerContext, character_guid: u64, slot: u8) -> Result<(), String> {
    crate::loot::apply_disenchant(ctx, character_guid, slot)
}

/// ENCHANT the item in `target_slot` for `character_guid` with `enchant_id` (ENCHANTING, completing the
/// 13) — the small-arg test twin of the `enchant_item` player reducer. Drives `loot::apply_enchant_item`
/// (validate id → consume mats → stamp the per-instance overlay → climb Enchanting). Verify:
/// `item_instance.enchant_id == enchant_id`, then `debug_compute_swing` shows the swing/strength readout
/// rose by the enchant's +stat (server-real); a Stamina enchant moves max-HP via recompute_vitals.
#[reducer]
pub fn debug_enchant_item(
    ctx: &ReducerContext,
    character_guid: u64,
    target_slot: u8,
    enchant_id: u32,
) -> Result<(), String> {
    crate::loot::apply_enchant_item(ctx, character_guid, target_slot, enchant_id)
}

/// Start `attacker_guid`'s RANGED auto-attack (#10) on the NEAREST live creature, with `spell_id`
/// (75 Auto Shot / 5019 wand Shoot) — the small-arg test hook (a creature's own guid is a u64 > 2^53 the
/// CLI `spacetime call` mangles). Requires a ranged weapon equipped (the shared gate enforces it). Mirrors
/// `debug_kill_nearest`'s nearest-search, then arms the engagement via `apply_start_ranged_attack`.
#[reducer]
pub fn debug_ranged_attack_nearest(
    ctx: &ReducerContext,
    attacker_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let attacker = ctx
        .db
        .game_world_entity()
        .guid()
        .find(attacker_guid)
        .ok_or_else(|| format!("attacker {attacker_guid} not in world"))?;
    let mut best: Option<(u64, f32)> = None;
    for e in ctx.db.game_world_entity().iter() {
        if e.guid == attacker_guid || e.dead || e.is_player() {
            continue;
        }
        let d = (e.x - attacker.x).powi(2) + (e.y - attacker.y).powi(2);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((e.guid, d));
        }
    }
    let target_guid = best
        .ok_or_else(|| "no live creature near attacker".to_string())?
        .0;
    crate::actor::ranged_attack(ctx, attacker_guid, target_guid, spell_id)
}

/// Teach `character_guid` the spell `spell_id` (#10 test hook — e.g. Auto Shot 75 on a non-Hunter so the
/// client's spellbook gets it and `CastSpellByName("Auto Shot")` works). Drives the shared
/// `spell::learn_spell` by explicit guid (the player path is `ctx.sender`-bound). Needs a relog for the
/// learned spell to reach the client spellbook (sent in SMSG_INITIAL_SPELLS at login).
#[reducer]
pub fn debug_learn_spell(
    ctx: &ReducerContext,
    character_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    crate::spell::learn_spell(ctx, character_guid, e.owner_identity, spell_id);
    Ok(())
}

/// Set the realm XP-rate (`game_config` singleton) for testing — upserts row 0 (the seed default may be
/// absent on a re-published DB). Negative clamps to 0. Production realms can also `UPDATE game_config`.
#[reducer]
pub fn debug_set_xp_rate(ctx: &ReducerContext, rate: f32) -> Result<(), String> {
    let rate = rate.max(0.0);
    let cfg = ctx.db.game_config();
    match cfg.id().find(0) {
        Some(mut c) => {
            c.xp_rate = rate;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: rate,
                nav_enabled: false,
                hosts_instances: true,
                bots_idle: false,
            });
        }
    }
    Ok(())
}

/// Re-stamp `grid_x`/`grid_y` on every `game_world_entity` from its live `(x, y)` with the CURRENT
/// `GRID_CELL_SIZE`. Run after changing the AOI cell size: the columns are baked, so a constant change
/// alone leaves stationary entities at stale cells (moving ones self-correct on their next move).
/// Collect-then-update so we don't mutate the table mid-iteration. Idempotent full sweep.
#[reducer]
pub fn debug_regrid(ctx: &ReducerContext) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let updates: Vec<(u64, i32, i32)> = entities
        .iter()
        .filter_map(|e| {
            let (gx, gy) = spatial::grid_cell(e.x, e.y);
            (e.grid_x != gx || e.grid_y != gy).then_some((e.guid, gx, gy))
        })
        .collect();
    let n = updates.len();
    for (guid, gx, gy) in updates {
        if let Some(mut e) = entities.guid().find(guid) {
            e.grid_x = gx;
            e.grid_y = gy;
            entities.guid().update(e);
        }
    }
    log::info!("debug_regrid: re-stamped {n} entities at the current GRID_CELL_SIZE");
    Ok(())
}

/// Grant `amount` reputation for `character_guid` with `faction_id` (#13 server-verify hook — quest-rep is
/// the only live caller, and the player turn-in path is `ctx.sender`-bound). Drives `reputation::grant_reputation`
/// directly; no-ops if the faction has no rep bar / is unknown. Verify via `SELECT * FROM game_player_reputation`
/// and the relayed `SMSG_SET_FACTION_STANDING`. Faction ids are small (< 2^53) so `spacetime call` carries them fine.
#[reducer]
pub fn debug_grant_reputation(
    ctx: &ReducerContext,
    character_guid: u64,
    faction_id: u32,
    amount: i32,
) -> Result<(), String> {
    // REFUSE verdict (issue #30): `game_player_reputation` is a MANIFEST table — see
    // `debug_expire_quest` for the same reasoning.
    crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or_else(|| format!("no character {character_guid}"))?;
    crate::reputation::grant_reputation(ctx, character_guid, faction_id, amount);
    Ok(())
}

/// Buy spell `spell_id` from the trainer whose creature-template `trainer_entry` is spawned in-world, for
/// `character_guid` — drives the validated `trainer::buy_trainer_spell` by explicit small args (the player
/// path is `ctx.sender`-bound, and a trainer GUID is > 2^53 so `spacetime call` would mangle it; the
/// template entry is small). Resolves the FIRST spawned non-player entity of that entry, then runs the
/// SAME buy logic (range/flag/level/cost/known gates → charge + learn_spell). Verify via
/// `SELECT * FROM game_player_spell` (row appears) + the caster's `money` dropping.
#[reducer]
pub fn debug_buy_trainer_spell(
    ctx: &ReducerContext,
    character_guid: u64,
    trainer_entry: u32,
    spell_id: u32,
) -> Result<(), String> {
    let trainer = ctx
        .db
        .game_world_entity()
        .iter()
        .find(|e| e.entry == trainer_entry && !e.is_player())
        .ok_or_else(|| format!("no spawned trainer with entry {trainer_entry}"))?;
    crate::actor::trainer_buy(ctx, character_guid, trainer.guid, spell_id)
}

/// Learn a talent for `character_guid` (drives the validated `talent::do_learn_talent` by explicit guid —
/// the player `learn_talent` is `ctx.sender`-bound and undrivable from the CLI identity). Applies the
/// talent's passive aura at the new rank immediately (no relog) so the combat effect is verifiable via
/// `debug_compute_swing`. Errors on the same gates as the player path (no points / max rank / tier / prereq).
#[reducer]
pub fn debug_learn_talent(
    ctx: &ReducerContext,
    character_guid: u64,
    talent_id: u32,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    crate::talent::do_learn_talent(ctx, character_guid, e.owner_identity, talent_id).map(|_| ())
}

/// Reset every learned talent for `character_guid` at `trainer_guid` (drives the validated
/// `talent::do_reset_talents` by explicit guid — the player `reset_talents` is `ctx.sender`-bound and
/// undrivable from the CLI identity). Same trainer/range gates + escalating gold cost as the player
/// path. Verify via `SELECT * FROM game_character_talent` (rows gone), `game_aura` (passive auras gone),
/// `game_player_spell` (taught abilities gone), and `game_character.money`/`respec_count`.
#[reducer]
pub fn debug_reset_talents(
    ctx: &ReducerContext,
    character_guid: u64,
    trainer_guid: u64,
) -> Result<(), String> {
    crate::talent::do_reset_talents(ctx, character_guid, trainer_guid).map(|_| ())
}

/// (Re)seed the talent metadata + the 51xxx passive talent spells — needed on an already-migrated dev DB
/// where `seed::init` did NOT re-run after an auto-migrate publish (the two talent tables are created empty,
/// but the rows aren't). Idempotent (`talent::seed_talents` inserts only absent rows).
#[reducer]
pub fn debug_seed_talents(ctx: &ReducerContext) {
    crate::talent::seed_talents(ctx);
    // row_count: total seeded talent rows present after this call (work-item 216 provenance stamp).
    crate::import_meta::stamp(
        ctx,
        "debug_seed_talents",
        "",
        "",
        ctx.db.game_talent().count(),
    );
}

/// (Re)grant the seeded Tester (guid 1) `gm_level` 3 — needed on an already-migrated dev DB where
/// `seed::init` did NOT re-run after an auto-migrate publish (work-item 223: the `gm_level` column
/// auto-migrates existing rows to 0, so a pre-223 Tester needs this to regain GM playtest access).
/// Idempotent (always re-sets to 3); no-op if the Tester character row is gone.
#[reducer]
pub fn debug_seed_gm_tester(ctx: &ReducerContext) {
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(1) {
        c.gm_level = 3;
        chars.guid().update(c);
        log::info!("debug_seed_gm_tester: Tester (guid 1) -> gm_level 3");
    }
}

/// (Re)seed the createinfo starting-kit rows (`game_createinfo_spell`) — needed on an
/// already-migrated dev DB where `seed::init` did NOT re-run after an auto-migrate publish (the
/// table is created EMPTY there, so character creation would grant no kit and racial passives
/// would skip). Idempotent only-if-empty; run once after publishing onto a pre-178 DB, BEFORE
/// creating characters. Characters created while it was empty have no kit rows (alpha: re-create
/// them). Same precedent as `debug_seed_talents`.
#[reducer]
pub fn debug_seed_createinfo_spells(ctx: &ReducerContext) {
    crate::seed::seed_createinfo_spells(ctx);
    // row_count: total createinfo-kit rows present after this call (work-item 216 provenance stamp).
    crate::import_meta::stamp(
        ctx,
        "debug_seed_createinfo_spells",
        "",
        "",
        ctx.db.game_createinfo_spell().count(),
    );
}

/// (Re)seed the stacking-group starter set (`game_spell_group` + `game_spell_group_rule`, work-item 192)
/// — needed on an already-migrated dev DB where `seed::init` did NOT re-run after an auto-migrate publish
/// (both tables are created EMPTY there, so every stacking-group check is a silent no-op until this runs).
/// Idempotent (`seed::seed_spell_groups` only-if-empty); same precedent as `debug_seed_createinfo_spells`.
#[reducer]
pub fn debug_seed_spell_groups(ctx: &ReducerContext) {
    use crate::spell::stacking::game_spell_group;
    crate::seed::seed_spell_groups(ctx);
    // row_count: total `game_spell_group` membership rows present after this call (work-item 216
    // provenance stamp).
    crate::import_meta::stamp(
        ctx,
        "debug_seed_spell_groups",
        "",
        "",
        ctx.db.game_spell_group().count(),
    );
}

/// (Re)seed Weakened Soul (6788) + the Test PW:Shield fixture (50072) — needed on an
/// already-migrated dev DB where `seed::init` did NOT re-run after an auto-migrate publish. Idempotent
/// (`seed::seed_pw_shield_fixture` inserts only absent rows); same precedent as `debug_seed_talents`.
#[reducer]
pub fn debug_seed_pw_shield_fixture(ctx: &ReducerContext) {
    crate::seed::seed_pw_shield_fixture(ctx);
    // row_count: how many of this fixture's 2 spell ids (Weakened Soul 6788, Test PW:Shield 50072)
    // are present after this call (work-item 216 provenance stamp).
    let n = [6788u32, 50072u32]
        .iter()
        .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
        .count() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_pw_shield_fixture", "", "", n);
}

/// (Re)seed the scenario-fixture CATALOGUE rows (Tempered Blade 5090050 + Tough Jerky 5090052 +
/// fixture faction 50900) — needed on an already-migrated dev DB where `seed::init` did NOT re-run
/// after an auto-migrate publish (issue #85: without this, such a shard's `items`/`dbc_reference`
/// catalogue fingerprints permanently disagree with a freshly-published sibling). Idempotent
/// (`seed::seed_fixture_catalogue` inserts only absent rows); same precedent as
/// `debug_seed_pw_shield_fixture`.
#[reducer]
pub fn debug_seed_fixture_catalogue(ctx: &ReducerContext) {
    crate::seed::seed_fixture_catalogue(ctx);
    // row_count: how many of the 3 fixture rows (2 items + 1 faction) are present after this call
    // (work-item 216 provenance stamp).
    let items = [crate::seed::FIXTURE_BLADE, crate::seed::FIXTURE_JERKY]
        .iter()
        .filter(|e| ctx.db.game_item_template().entry().find(**e).is_some())
        .count() as u64;
    let faction = ctx
        .db
        .game_faction()
        .faction_id()
        .find(crate::seed::FIXTURE_FACTION)
        .is_some() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_fixture_catalogue", "", "", items + faction);
}

/// (Re)seed the Soul Shard item template (6265) — needed on an already-migrated dev DB
/// where `seed::init` did NOT re-run after an auto-migrate publish. Idempotent
/// (`seed::seed_soul_shard_item` inserts only if absent); same precedent as `debug_seed_pw_shield_fixture`.
#[reducer]
pub fn debug_seed_soul_shard_item(ctx: &ReducerContext) {
    crate::seed::seed_soul_shard_item(ctx);
    // row_count: 1 if the Soul Shard item template is present after this call, else 0
    // (work-item 216 provenance stamp).
    let n = ctx
        .db
        .game_item_template()
        .entry()
        .find(crate::combat::SOUL_SHARD_ENTRY)
        .is_some() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_soul_shard_item", "", "", n);
}

/// (Re)seed the Drain Soul (1120) channel fixture — needed on an already-migrated dev DB
/// where `seed::init` did NOT re-run after an auto-migrate publish. Idempotent
/// (`seed::seed_drain_soul_fixture` inserts only if absent); same precedent as `debug_seed_pw_shield_fixture`.
#[reducer]
pub fn debug_seed_drain_soul_fixture(ctx: &ReducerContext) {
    crate::seed::seed_drain_soul_fixture(ctx);
    // row_count: 1 if the Drain Soul spell header is present after this call, else 0
    // (work-item 216 provenance stamp).
    let n = ctx
        .db
        .game_spell()
        .spell_id()
        .find(crate::combat::DRAIN_SOUL_SPELL_ID)
        .is_some() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_drain_soul_fixture", "", "", n);
}

/// (Re)seed the Mana Burn (real 8129) E_POWER_BURN fixture — needed on an
/// already-migrated dev DB where `seed::init` did NOT re-run after an auto-migrate publish. Idempotent
/// (`seed::seed_mana_burn_fixture` inserts only if absent); same precedent as `debug_seed_drain_soul_fixture`.
#[reducer]
pub fn debug_seed_mana_burn_fixture(ctx: &ReducerContext) {
    crate::seed::seed_mana_burn_fixture(ctx);
    // row_count: 1 if the Mana Burn (8129) spell header is present after this call, else 0
    // (work-item 216 provenance stamp).
    let n = ctx.db.game_spell().spell_id().find(8129u32).is_some() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_mana_burn_fixture", "", "", n);
}

/// (Re)seed the Stealth (real 1784) presence fixture — needed on an already-migrated
/// dev DB where `seed::init` did NOT re-run after an auto-migrate publish. Idempotent
/// (`seed::seed_stealth_fixture` inserts only if absent); same precedent as `debug_seed_pw_shield_fixture`.
#[reducer]
pub fn debug_seed_stealth_fixture(ctx: &ReducerContext) {
    crate::seed::seed_stealth_fixture(ctx);
    // row_count: 1 if the Stealth (1784) spell header is present after this call, else 0
    // (work-item 216 provenance stamp).
    let n = ctx.db.game_spell().spell_id().find(1784u32).is_some() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_stealth_fixture", "", "", n);
}

/// (Re)seed Chilled (6136) + Frost Armor (168) — needed on an already-migrated dev DB
/// where `seed::init` did NOT re-run after an auto-migrate publish. Idempotent
/// (`seed::seed_frost_armor_fixture` inserts only if absent); same precedent as `debug_seed_pw_shield_fixture`.
#[reducer]
pub fn debug_seed_frost_armor_fixture(ctx: &ReducerContext) {
    crate::seed::seed_frost_armor_fixture(ctx);
    // row_count: how many of this fixture's 2 spell ids (Chilled 6136, Frost Armor 168) are present
    // after this call (work-item 216 provenance stamp).
    let n = [6136u32, 168u32]
        .iter()
        .filter(|id| ctx.db.game_spell().spell_id().find(**id).is_some())
        .count() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_frost_armor_fixture", "", "", n);
}

/// (Re)seed Demon Skin (696) — needed on an already-migrated dev DB where `seed::init`
/// did NOT re-run after an auto-migrate publish. Idempotent (`seed::seed_demon_skin_fixture` inserts only
/// if absent); same precedent as `debug_seed_frost_armor_fixture`.
#[reducer]
pub fn debug_seed_demon_skin_fixture(ctx: &ReducerContext) {
    crate::seed::seed_demon_skin_fixture(ctx);
    // row_count: 1 if the Demon Skin (696) spell header is present after this call, else 0
    // (work-item 216 provenance stamp).
    let n = ctx.db.game_spell().spell_id().find(696u32).is_some() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_demon_skin_fixture", "", "", n);
}

/// (Re)seed Test Regeneration (50137, the combat-regen probe's kind-169
/// `A_COMBAT_HEALTH_REGEN_PCT` source) — needed on an already-migrated dev DB where `seed::init`
/// did NOT re-run after an auto-migrate publish (work-item 152: without this twin, an
/// already-migrated node never gains the fixture and `wire-suite.sh`'s `HAS_COMBAT_REGEN_EFFECT`
/// gate stays closed forever, exactly like the other seed kits before their twins existed).
/// Idempotent (`seed::seed_regen_fixture` inserts only if absent); same precedent as
/// `debug_seed_demon_skin_fixture`.
#[reducer]
pub fn debug_seed_regen_fixture(ctx: &ReducerContext) {
    crate::seed::seed_regen_fixture(ctx);
    // row_count: 1 if the Test Regeneration (50137) spell header is present after this call, else 0
    // (work-item 216 provenance stamp).
    let n = ctx.db.game_spell().spell_id().find(50137u32).is_some() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_regen_fixture", "", "", n);
}

/// Re-arm the GLOBAL creature movement tick to 0.5s on a LIVE database. `seed::init` does NOT re-run
/// on an auto-migrate publish, so the existing `game_creature_move_schedule` row keeps its old 4s
/// interval — this deletes the CATCH-ALL row(s) and re-inserts one at 500ms. Idempotent; owner-called
/// once after a publish (same precedent as `debug_seed_talents`). Work-item 229: DEDICATED
/// per-instance rows are deliberately left alone (they're someone's live tuning; use
/// `debug_disarm_instance_tick` to remove one) — and this reducer doubles as the recovery path if the
/// load-bearing catch-all row was ever deleted (without it, every instance lacking a dedicated row
/// stops ticking entirely, including the open world).
#[reducer]
pub fn debug_rearm_creature_tick(ctx: &ReducerContext) {
    let sched = ctx.db.game_creature_move_schedule();
    let ids: Vec<u64> = sched
        .iter()
        .filter(|r| r.instance_id == crate::creatures::GLOBAL_TICK_INSTANCE)
        .map(|r| r.scheduled_id)
        .collect();
    for id in ids {
        sched.scheduled_id().delete(id);
    }
    sched.insert(CreatureMoveSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(500_000)),
        instance_id: crate::creatures::GLOBAL_TICK_INSTANCE,
    });
}

/// Floor for a dedicated per-instance tick interval (work-item 229's honesty addendum: every firing
/// is a transaction on the ONE serialized commit stream — 10 instances at 100ms is already 100
/// extra transactions/sec preempting player actions; anything tighter than 50ms is a foot-gun with
/// no gameplay payoff at 1.12 animation cadences).
const INSTANCE_TICK_MS_FLOOR: u64 = 50;
/// Ceiling for `debug_arm_instance_tick` (229 review): 10 minutes — far beyond any sane cadence
/// (the pause substitute the runbook suggests is seconds, not minutes), and small enough that
/// `tick_ms * 1000` can never overflow `i64` into a NEGATIVE interval (a negative scheduled-table
/// interval risks continuous refiring — the exact commit-stream flood the floor exists to prevent).
const INSTANCE_TICK_MS_CEIL: u64 = 600_000;

/// Arm (or re-arm) a DEDICATED creature-tick row for `instance_id` at `tick_ms` (work-item 229): that
/// instance's creature passes then fire on THIS row at its own cadence, and the catch-all row skips
/// the instance (coverage is a partition — see `TickScope` in creatures/ai.rs). The global due-time
/// passes (decay/respawn/regen/combat-drop) STAY on the catch-all row for all instances, so this knob
/// only smooths movement/AI latency — per the item's honest bound it buys NO parallel throughput and
/// each firing taxes the shared serialized commit stream; use tight cadences sparingly and read the
/// once-a-minute "pass rows-visited" log line to see what each row actually scans.
///
/// 190 slice 2 LANDED: `instance::create_instance` inserts exactly this row shape (500ms default)
/// and the instance reap (`instance::teardown_instance`) deletes it — `debug_disarm_instance_tick`'s
/// body. This reducer remains the operator RETUNE lever for a live instance's cadence.
/// Idempotent per instance (replaces any existing dedicated row).
#[reducer]
pub fn debug_arm_instance_tick(
    ctx: &ReducerContext,
    instance_id: u64,
    tick_ms: u64,
) -> Result<(), String> {
    if instance_id == crate::creatures::GLOBAL_TICK_INSTANCE {
        return Err(
            "instance_id u64::MAX is the catch-all sentinel — use debug_rearm_creature_tick".into(),
        );
    }
    if tick_ms < INSTANCE_TICK_MS_FLOOR {
        return Err(format!("tick_ms {tick_ms} below the {INSTANCE_TICK_MS_FLOOR}ms floor (serialized commit-stream tax — see work-item 229)"));
    }
    if tick_ms > INSTANCE_TICK_MS_CEIL {
        return Err(format!("tick_ms {tick_ms} above the {INSTANCE_TICK_MS_CEIL}ms ceiling (overflow guard — see the const's doc)"));
    }
    let sched = ctx.db.game_creature_move_schedule();
    // Replace any existing dedicated row for this instance (re-arm = retune the cadence).
    let stale: Vec<u64> = sched
        .iter()
        .filter(|r| r.instance_id == instance_id)
        .map(|r| r.scheduled_id)
        .collect();
    for id in stale {
        sched.scheduled_id().delete(id);
    }
    sched.insert(CreatureMoveSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(tick_ms as i64 * 1000)),
        instance_id,
    });
    Ok(())
}

/// Remove the dedicated creature-tick row for `instance_id` (work-item 229) — coverage of that
/// instance returns to the catch-all row on its next firing (the `TickScope` rebuild is per firing),
/// so its creatures keep ticking at the global 0.5s cadence; nothing is ever stranded. This is also
/// the future instance-reap's tick-row cleanup (190 slice 2).
#[reducer]
pub fn debug_disarm_instance_tick(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    if instance_id == crate::creatures::GLOBAL_TICK_INSTANCE {
        return Err("refusing to delete the load-bearing catch-all row".into());
    }
    let sched = ctx.db.game_creature_move_schedule();
    let ids: Vec<u64> = sched
        .iter()
        .filter(|r| r.instance_id == instance_id)
        .map(|r| r.scheduled_id)
        .collect();
    if ids.is_empty() {
        return Err(format!("no dedicated tick row for instance {instance_id}"));
    }
    for id in ids {
        sched.scheduled_id().delete(id);
    }
    Ok(())
}

/// Server-side integration probe for the `A_COMBAT_HEALTH_REGEN_PCT` aura (combat health regen
/// in combat). Reads the LIVE `game_aura` table for `character_guid`, sums every active
/// `A_COMBAT_HEALTH_REGEN_PCT` aura effect (exactly as `pass_regen` does), then computes the
/// expected per-tick delta using the same `regen_health_in_combat` path. Logs the result at INFO.
///
/// Returns `Ok` when the pct sum AND the tick delta are both non-zero (the aura is present AND is
/// large enough to move health by at least 1 HP per tick). Returns `Err` with a diagnostic otherwise
/// so the caller can distinguish "no aura" from "aura too small for the entity's stats".
///
/// **Usage** (verify a Human has the regen passive and will tick in combat):
/// ```text
/// spacetime call debug_verify_combat_regen '[<guid>]'
/// spacetime logs | grep verify_combat_regen
/// ```
/// The logs show: `pct=10 full_tick=71 partial=7 health=950 max=1000 → would_heal=957`
#[reducer]
pub fn debug_verify_combat_regen(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let pct = crate::spell::combat_health_regen_pct(ctx, character_guid);
    let pct_u32 = pct.max(0) as u32;
    let would_heal =
        crate::combat::regen_health_in_combat(e.health, e.max_health, e.spirit, e.level, pct_u32);
    let delta = would_heal.saturating_sub(e.health);
    log::info!(
        "debug_verify_combat_regen: guid={character_guid} pct={pct} health={} max={} spirit={} level={} → would_heal={would_heal} delta={delta}",
        e.health, e.max_health, e.spirit, e.level
    );
    if pct <= 0 {
        return Err(format!(
            "no A_COMBAT_HEALTH_REGEN_PCT aura on guid {character_guid} (pct={pct}); grant the passive first"
        ));
    }
    if delta == 0 {
        return Err(format!(
            "aura present (pct={pct}) but tick delta is 0 — entity spirit/level too low for integer math to produce ≥1 HP (spirit={} level={}); expected at higher level or spirit",
            e.spirit, e.level
        ));
    }
    Ok(())
}

/// Cast `spell_id` as `character_guid` THROUGH the spellbook gate (`knows_spell`) — the by-guid mirror of
/// the sender-bound `cast_spell`, so a test can verify the learnable-spell gate (rank 27 #1): the cast is
/// rejected until the spell is in the baseline kit OR learned (e.g. via an ability talent), then accepted.
/// Unlike `debug_cast_at` (which drives `resolve_cast_at` and BYPASSES the gate), this exercises the gate.
#[reducer]
pub fn debug_cast_spell(
    ctx: &ReducerContext,
    character_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    let e = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    if !crate::spell::knows_spell(ctx, character_guid, spell_id) {
        return Err(format!("spell {spell_id} is not in the caster's spellbook"));
    }
    let target = if target_guid == 0 {
        character_guid
    } else {
        target_guid
    };
    crate::spell::begin_cast(
        ctx,
        character_guid,
        spell_id,
        e.level as u8,
        target,
        false,
        None,
    )
}

/// Read back the two `build.rs`-generated character-owned-sweep registries
/// LIVE, inside the actual wasm runtime (not just `cargo test`, which never proved the build.rs codegen
/// survives the `wasm32-unknown-unknown` compile + the SpacetimeDB module-init path). Logs both slice
/// lengths and errors out if either is unexpectedly empty — a silently-empty registry (the wasm-ld/
/// codegen regression this whole mechanism has to guard against) must be LOUD, not a no-op sweep.
#[reducer]
pub fn debug_sweep_slice_lens(_ctx: &ReducerContext) -> Result<(), String> {
    let delete_len = crate::CHARACTER_OWNED_DELETE_SWEEPS.len();
    let restamp_len = crate::CHARACTER_OWNED_RESTAMP_SWEEPS.len();
    log::info!(
        "debug_sweep_slice_lens: CHARACTER_OWNED_DELETE_SWEEPS={delete_len} CHARACTER_OWNED_RESTAMP_SWEEPS={restamp_len}"
    );
    if delete_len == 0 {
        return Err("CHARACTER_OWNED_DELETE_SWEEPS is empty — the build.rs marker scan found nothing (registry mechanism broken)".to_string());
    }
    if restamp_len == 0 {
        return Err("CHARACTER_OWNED_RESTAMP_SWEEPS is empty — the build.rs marker scan found nothing (registry mechanism broken)".to_string());
    }
    Ok(())
}

/// (Re)seed the scenario-runner fixtures: quest 50900 + questgiver 51003 + vendor
/// 51004 + faction 79 rep bar + a Lesser Heal trainer offering on 51001. Idempotent
/// (`seed::seed_scenario_fixtures` inserts only if absent); same precedent as
/// `debug_seed_pw_shield_fixture`. The scenario orchestrators call this before every run.
#[reducer]
pub fn debug_seed_scenario_fixtures(ctx: &ReducerContext) {
    crate::seed::seed_scenario_fixtures(ctx);
    // Post-ETL one-call restore (2026-07-16): the spell-fixture seeders ride along so a single
    // reducer re-arms EVERY suite fixture after an import pass (Test Regeneration 50137 and
    // Test PW:Shield 50072 were found missing live — only the scenario NPC/quest slice was being
    // reseeded, and the suite's combat-regen probe hard-failed on "unknown spell 50137").
    crate::seed::seed_regen_fixture(ctx);
    crate::seed::seed_pw_shield_fixture(ctx);
    // row_count: quest 50900 (1) + how many of the 3 scenario NPC templates (questgiver 51003,
    // vendor 51004, weapon master 51005) are present after this call — an anchor-row proxy for the
    // fixture's full spread across quest/text/objective/reward/vendor/trainer tables, not an exact
    // total (work-item 216 provenance stamp).
    let quest = ctx
        .db
        .game_quest_template()
        .entry()
        .find(50900u32)
        .is_some() as u64;
    let npcs = [51003u32, 51004, 51005]
        .iter()
        .filter(|e| ctx.db.game_creature_template().entry().find(**e).is_some())
        .count() as u64;
    crate::import_meta::stamp(ctx, "debug_seed_scenario_fixtures", "", "", quest + npcs);
}

/// Set a character's purse to an EXACT copper amount (live entity + persisted row) so scenario money
/// assertions are deterministic run-over-run — idempotent by design.
#[reducer]
pub fn debug_set_money(
    ctx: &ReducerContext,
    character_guid: u64,
    copper: u32,
) -> Result<(), String> {
    let chars = ctx.db.game_character();
    // REFUSE verdict (issue #30) — harness writers get the same fence as production ones.
    let mut c = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or_else(|| format!("no character {character_guid}"))?;
    c.money = copper;
    chars.guid().update(c);
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(character_guid) {
        e.money = copper;
        entities.guid().update(e);
    }
    Ok(())
}

/// Set a live entity's power (mana/rage/energy) for scenario staging, lifting `max_power` when
/// needed — a no-import sandbox has no mana curve (`game_level_stats` empty), so a wire-created
/// caster logs in with 0/0 mana and every costed cast would fail its power gate. Mirrors
/// `debug_set_health`'s live-entity contract.
#[reducer]
pub fn debug_set_power(ctx: &ReducerContext, guid: u64, power: u32) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut e = entities
        .guid()
        .find(guid)
        .ok_or_else(|| format!("no live entity for guid {guid}"))?;
    if e.max_power < power {
        e.max_power = power;
    }
    e.power = power;
    entities.guid().update(e);
    Ok(())
}

// ===========================================================================================
//  Encounter kernel levers (work-item 228) — operator stand-ins until 227's Deadmines package
//  consumes the primitives for real. Each is a thin `?`-wrapper over the `crate::encounter` fn it
//  names, so the runbook can exercise every primitive on a live node without an encounter package.
// ===========================================================================================

/// Register an HP-threshold watch (`encounter::watch_hp_threshold`). NOTE: a watch alone fires
/// nothing — `on_hp_threshold` handlers are compile-time registrations (`game_hook!` markers), so
/// without one anywhere in the build the kernel's damage probe stays on its zero-cost early-out.
#[reducer]
pub fn debug_encounter_watch_hp(ctx: &ReducerContext, entry: u32, pct: u8) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::watch_hp_threshold(ctx, entry, pct)
}

/// Set one encounter's state + payload (`encounter::set_encounter_state`/`set_encounter_data`) —
/// readable back via `spacetime sql "select * from game_encounter_state"`.
#[reducer]
pub fn debug_encounter_set_state(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    state: u8,
    data: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::set_encounter_state(ctx, instance_id, encounter_id, state)?;
    crate::encounter::set_encounter_data(ctx, instance_id, encounter_id, data)?;
    Ok(())
}

/// Flip every DOOR/BUTTON of `go_entry` open (`encounter::open_door`); logs the flip count.
#[reducer]
pub fn debug_encounter_open_door(
    ctx: &ReducerContext,
    go_entry: u32,
    instance_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let opened = crate::encounter::open_door(ctx, go_entry, instance_id)?;
    log::info!("debug_encounter_open_door: opened {opened} gameobject(s) of entry {go_entry}");
    Ok(())
}

/// Spawn `count` adds of `entry` as one tracked wave (`encounter::spawn_wave`); logs the guids.
#[allow(clippy::too_many_arguments)] // a debug lever mirrors the primitive's full signature
#[reducer]
pub fn debug_encounter_spawn_wave(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    map_id: u32,
    entry: u32,
    count: u32,
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let entries = vec![entry; count as usize];
    let guids = crate::encounter::spawn_wave(
        ctx,
        instance_id,
        encounter_id,
        map_id,
        &entries,
        x,
        y,
        z,
        orientation,
    );
    if guids.is_empty() {
        return Err(format!(
            "spawn_wave spawned nothing (no template for entry {entry}?)"
        ));
    }
    log::info!("debug_encounter_spawn_wave: spawned {guids:?} for encounter {encounter_id} in instance {instance_id}");
    Ok(())
}

/// Write a creature's virtual-item slots (`encounter::equip_swap`) — module-side row only; the
/// client-visible UNIT_VIRTUAL_ITEM_SLOT_DISPLAY relay is [V]-deferred (see game_encounter_equip).
#[reducer]
pub fn debug_encounter_equip(
    ctx: &ReducerContext,
    creature_guid: u64,
    main_hand: u32,
    off_hand: u32,
    ranged: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::equip_swap(ctx, creature_guid, main_hand, off_hand, ranged)
}

/// Send a creature on one move leg (`encounter::move_to_point`) — the Smite rack-run motion.
#[reducer]
pub fn debug_encounter_move(
    ctx: &ReducerContext,
    creature_guid: u64,
    x: f32,
    y: f32,
    z: f32,
    run: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::move_to_point(ctx, creature_guid, x, y, z, run)
}

/// Reset one encounter (`encounter::encounter_reset`): state → NotStarted, tracked wave despawned.
/// HP fired-marks are entry-keyed, so clear them separately: `debug_encounter_reset_hp_fired`.
#[reducer]
pub fn debug_encounter_reset(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::encounter_reset(ctx, instance_id, encounter_id);
    Ok(())
}

/// Clear the per-instance HP fired-marks for `entry` (`encounter::reset_hp_fired`).
#[reducer]
pub fn debug_encounter_reset_hp_fired(
    ctx: &ReducerContext,
    instance_id: u64,
    entry: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::reset_hp_fired(ctx, instance_id, entry);
    Ok(())
}

/// Sweep EVERY kernel row for an instance (`encounter::sweep_encounter_state`) — kept as the
/// narrow kernel-only lever; the FULL instance reap (which calls this sweep as its 228 splice)
/// is `debug_reap_instance` below (190 slice 3 landed).
#[reducer]
pub fn debug_sweep_encounter_state(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::encounter::sweep_encounter_state(ctx, instance_id);
    Ok(())
}

// ===========================================================================================
//  Dungeon-instancing fixture + operator levers (work-item 190 slices 2+3)
// ===========================================================================================

/// THE FIXTURE DUNGEON (190's done-when, headless, zero imports): resolve-or-create an instance of
/// the character's CURRENT map through the REAL production path (`resolve_or_create_instance`:
/// own binding → party's live instance → create, 5-cap enforced) and same-map-teleport them into
/// it in place. On the dev map this instances the SEEDED world — the chicken/wolf/trainer roster
/// copies in as the per-instance population, so two ungrouped characters get DISJOINT populations
/// (distinct instance ids, distinct creature guids) and two GROUPED characters land in the SAME
/// one. Call it twice for one character → the second call RESOLVES the binding (same id back).
/// Verify: `spacetime sql "SELECT * FROM game_instance"` / `"... FROM game_instance_binding"`,
/// and the log line below carries the id.
///
/// CLIENT-VIEW CAVEAT (dev-only, inherent to the same-map hop): a same-map instance switch is a
/// MSG_MOVE_TELEPORT_ACK teleport — the gateway's per-viewer `created` dedup set only resets on a
/// CROSS-map transfer (the enter_world resubscribe), so a live 1.12 client keeps rendering the
/// instance-0 objects it already saw (frozen — their relays are now instance-gated) until relog.
/// SERVER-side isolation is full either way (module gates + relay gates); wire assertions about
/// "sees only its own population" should therefore be made on a FRESH login after the switch
/// (login rebuilds into `pending_instance_id` with a fresh created set + initial-apply sweep).
/// The real dungeon flow (areatrigger portal) is cross-map and has no such caveat.
///
/// COST NOTE: the population copy covers EVERY spawn template on the character's map — on the
/// bare seeded dev node that's a handful of creatures; on a node with the full Elwynn/Westfall
/// import it's the whole ~2k-spawn map-0 roster per instance. Fixture-scale testing belongs on
/// the seeded node (or accept the copy cost knowingly).
#[reducer]
pub fn debug_create_fixture_instance(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    let map_id = player.map_id;
    let instance_id = crate::instance::resolve_or_create_instance(ctx, character_guid, map_id)?;
    // Same-map teleport in place: the in-place branch stamps entity + Character with the instance.
    crate::world::teleport_player(
        ctx,
        character_guid,
        map_id,
        instance_id,
        player.x,
        player.y,
        player.z,
        player.orientation,
    );
    log::info!("debug_create_fixture_instance: character {character_guid} → instance {instance_id} on map {map_id}");
    Ok(())
}

/// Place a character into an EXISTING instance on their current map (bypasses binding/resolve —
/// the raw placement lever for cap/isolation experiments). Same-map only: a cross-map placement
/// would land them at their current coords on a foreign map (garbage) — use the areatrigger path
/// or `debug_teleport` + `debug_enter_instance` in two steps for cross-map trips.
#[reducer]
pub fn debug_enter_instance(
    ctx: &ReducerContext,
    character_guid: u64,
    instance_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    if instance_id != 0 {
        let inst = ctx
            .db
            .game_instance()
            .instance_id()
            .find(instance_id)
            .ok_or_else(|| format!("no such instance {instance_id}"))?;
        if inst.map_id != player.map_id {
            return Err(format!(
                "instance {instance_id} is on map {} but the character is on map {} (same-map only)",
                inst.map_id, player.map_id
            ));
        }
    }
    crate::world::teleport_player(
        ctx,
        character_guid,
        player.map_id,
        instance_id,
        player.x,
        player.y,
        player.z,
        player.orientation,
    );
    Ok(())
}

/// Force-reap an instance NOW (the full slice-3 teardown: population → encounter sweep → tick row
/// → bindings → row) without waiting out the 30min empty window — refuses if players are inside
/// (the teardown's own belt re-checks). The headless reap-verification lever.
#[reducer]
pub fn debug_reap_instance(ctx: &ReducerContext, instance_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if instance_id == 0 {
        return Err("instance 0 is the open world".to_string());
    }
    if ctx
        .db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_none()
    {
        return Err(format!("no such instance {instance_id}"));
    }
    if ctx
        .db
        .game_world_entity()
        .iter()
        .any(|e| e.instance_id == instance_id && e.is_player())
    {
        return Err(format!(
            "instance {instance_id} has live players — reap refused"
        ));
    }
    crate::instance::teardown_instance(ctx, instance_id);
    Ok(())
}

/// Drive the player-facing `reset_instance` core by explicit guid (the CLI identity owns no
/// entity): flags every UNOCCUPIED instance the character may reset; the reaper (60s) tears each
/// down on its next firing. Returns Err when nothing was eligible, mirroring the player reducer.
#[reducer]
pub fn debug_reset_instance(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::instance::apply_reset_instances(ctx, character_guid).map(|n| {
        log::info!("debug_reset_instance: {n} instance(s) flagged for reset by {character_guid}");
    })
}

/// Re-seed the instance-reaper schedule row (60s) — the auto-migrate publish path: `init` only
/// runs on a FRESH publish, so a live node that migrated the new `game_instance_reaper_schedule`
/// table in has no row and the reap never fires until this is called once
/// (the `debug_rearm_creature_tick` precedent). Idempotent (replaces any existing rows).
#[reducer]
pub fn debug_rearm_instance_reaper(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let sched = ctx.db.game_instance_reaper_schedule();
    let stale: Vec<u64> = sched.iter().map(|r| r.scheduled_id).collect();
    for id in stale {
        sched.scheduled_id().delete(id);
    }
    sched.insert(crate::instance::InstanceReaperSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
            crate::instance::INSTANCE_REAPER_INTERVAL_MICROS,
        )),
    });
    Ok(())
}

/// 241 spot-probe: log walkability + obstruction at (x, y) on `map`. The rasterizer's
/// server-side verification hook (nav has no client-visible readback until 243 wires it).
#[reducer]
pub fn debug_nav_probe(ctx: &ReducerContext, map: u32, x: f32, y: f32) {
    let walk = crate::nav::walkable(ctx, map, x, y);
    let obs = crate::nav::obstruction_top(ctx, map, x, y);
    let ground = crate::terrain::ground_z(ctx, map, x, y);
    log::info!(
        "nav probe ({x:.1},{y:.1}) map {map}: walkable={walk:?} obstruction_top={obs:?} ground_z={ground:?}"
    );
}

/// Toggle nav-grid consumption (`game_config.nav_enabled`, work-item 243) — upserts row 0 like
/// `debug_set_xp_rate`. OFF = pre-243 straight-line movement + wall-blind aggro/casts.
#[reducer]
pub fn debug_set_nav_enabled(ctx: &ReducerContext, enabled: bool) -> Result<(), String> {
    let cfg = ctx.db.game_config();
    match cfg.id().find(0) {
        Some(mut c) => {
            c.nav_enabled = enabled;
            cfg.id().update(c);
        }
        None => {
            cfg.insert(ServerConfig {
                id: 0,
                xp_rate: 1.0,
                nav_enabled: enabled,
                hosts_instances: true,
                bots_idle: false,
            });
        }
    }
    Ok(())
}

/// 243 spot-probe: log the nav-aware leg from (x0,y0) to (x1,y1) — the exact query
/// `nav_step` runs. Shows the string-pulled waypoints (detour) or the fast/fallback shape.
#[reducer]
pub fn debug_nav_leg(ctx: &ReducerContext, map: u32, x0: f32, y0: f32, x1: f32, y1: f32) {
    let enabled = crate::nav::nav_enabled(ctx);
    let step = crate::nav::nav_step(ctx, map, (x0, y0), (x1, y1), 4.0, 0.0);
    // LoS endpoints stand on the ground like real units (falls back to z=0 off-slice).
    let z0 = crate::terrain::ground_z(ctx, map, x0, y0).unwrap_or(0.0);
    let z1 = crate::terrain::ground_z(ctx, map, x1, y1).unwrap_or(0.0);
    let los = crate::nav::has_los(ctx, map, (x0, y0, z0), (x1, y1, z1));
    let raw = crate::nav::debug_find_leg(ctx, map, (x0, y0), (x1, y1));
    log::info!(
        "nav leg ({x0:.1},{y0:.1})->({x1:.1},{y1:.1}) map {map}: nav_enabled={enabled} first_step=({:.2},{:.2}) has_los={los} raw={raw}",
        step.0, step.1
    );
}

/// Backfill the imported default action bar (work-items 110/212) onto an EXISTING character —
/// chars created before the `game_createinfo_action` import have no `game_player_action` rows and
/// fall back to the gateway's known-spells synth (which slots passives and misses the stance
/// pages). Idempotent per button (the grant skips occupied ones). Takes effect next login.
#[reducer]
pub fn debug_grant_default_actions(
    ctx: &ReducerContext,
    character_name: String,
) -> Result<(), String> {
    // REFUSE verdict (issue #30) — harness writers get the same fence as production ones.
    let c = crate::helpers::character_by_name(ctx, &character_name)
        .ok_or_else(|| format!("no character named {character_name}"))?;
    crate::action_bar::grant_createinfo_actions(ctx, c.guid, c.owner_identity, c.race, c.class);
    log::info!("debug_grant_default_actions: {} (guid {})", c.name, c.guid);
    Ok(())
}

/// One-time backfill (246): stamp grid_x/grid_y on every existing gameobject row from its (x, y)
/// — imported/seeded rows predate the columns. Idempotent.
#[reducer]
pub fn debug_backfill_go_grid(ctx: &ReducerContext) {
    let gos = ctx.db.game_gameobject();
    let all: Vec<u64> = gos.iter().map(|g| g.guid).collect();
    let mut n = 0u32;
    for guid in all {
        if let Some(mut g) = gos.guid().find(guid) {
            let (gx, gy) = lyracore_shared::spatial::grid_cell(g.x, g.y);
            if g.grid_x != gx || g.grid_y != gy {
                g.grid_x = gx;
                g.grid_y = gy;
                gos.guid().update(g);
                n += 1;
            }
        }
    }
    log::info!("debug_backfill_go_grid: {n} rows stamped");
}

// ===========================================================================================
//  Catalogue parity fingerprint (issue #82) — content-hash the tables that are supposed to be
//  IDENTICAL on every shard (spells, items, DBC-derived reference tables), so a partial or stale
//  re-import shows up as a loud mismatch instead of "this spell behaves differently in Durotar".
//  Row counts alone are not enough (a count matches trivially while contents differ): every row is
//  BSATN-encoded — the same canonical byte encoding SpacetimeDB already uses to store/diff rows
//  (`spacetimedb::sats::bsatn`), so this needs no hand-written per-table serializer — and the SORTED
//  set of encodings for a table is folded into one hash with `DefaultHasher` (stdlib, no new
//  dependency; deterministic for identical input on the SAME compiled wasm, which is exactly the
//  "did every shard run the same import?" question that the internal cross-database parity check
//  uses this table to answer).
// ===========================================================================================

/// A family's content fingerprint, recomputed wholesale by [`debug_catalogue_fingerprint`]. Public so
/// `spacetime sql` (or an equivalent cross-database parity check) can read it per database. [reference]
#[table(accessor = game_catalogue_fingerprint, public)]
pub struct CatalogueFingerprint {
    #[primary_key]
    pub family: String,
    pub row_count: u64,
    pub fingerprint: u64,
    /// Comma-separated table list actually hashed — so a family definition drifting between two
    /// module versions (a table added/renamed here but not there) shows up as a visible diff
    /// instead of a silent apples-to-oranges compare.
    pub tables: String,
}

/// Fold one table's rows into `hasher`/`count`: BSATN-encode every row, SORT the encodings (content
/// order, not iteration order — SpacetimeDB does not guarantee `.iter()` order is stable across
/// deployments/inserts), then hash the table name followed by the sorted encodings. The table name is
/// hashed first so two families with the same total content but a table added/renamed still differ.
fn fold_table<Row: spacetimedb::Serialize>(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    count: &mut u64,
    table_name: &str,
    rows: impl Iterator<Item = Row>,
) {
    use std::hash::Hash;
    table_name.hash(hasher);
    let mut encoded: Vec<Vec<u8>> = rows
        .map(|r| {
            spacetimedb::sats::bsatn::to_vec(&r)
                .expect("row must BSATN-encode (already required for SpacetimeDB row storage)")
        })
        .collect();
    encoded.sort();
    *count += encoded.len() as u64;
    encoded.hash(hasher);
}

/// Recompute all catalogue fingerprints (clear+reinsert — idempotent, call any time). Driven once per
/// connected shard as part of the deploy/verify path by an internal cross-database parity check. The
/// family list below is deliberately curated — see the per-family comments in the body for why each
/// table is (or, for the excluded ones, is not) included.
#[reducer]
pub fn debug_catalogue_fingerprint(ctx: &ReducerContext) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;

    let out = ctx.db.game_catalogue_fingerprint();
    for row in out.iter().collect::<Vec<_>>() {
        out.family().delete(&row.family);
    }

    // SPELLS — Spell.dbc (`importer/src/spell.rs::run_spells`) plus its box-INDEPENDENT dump
    // companions (spell_chain/spell_learn_spell/createinfo_spell — none of their `build_*_sql`
    // functions in `importer/src/main.rs` take a box/local-entry argument), plus the hand-seeded
    // stacking-group starter set (`module/src/seed.rs`), which can independently skew if
    // `debug_reseed_stacking_groups` was run on one shard's auto-migration and not another's.
    {
        let mut h = DefaultHasher::new();
        let mut n = 0u64;
        fold_table(&mut h, &mut n, "game_spell", ctx.db.game_spell().iter());
        fold_table(
            &mut h,
            &mut n,
            "game_spell_effect",
            ctx.db.game_spell_effect().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_spell_reagent",
            ctx.db.game_spell_reagent().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_spell_chain",
            ctx.db.game_spell_chain().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_spell_learn",
            ctx.db.game_spell_learn().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_spell_group",
            ctx.db.game_spell_group().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_spell_group_rule",
            ctx.db.game_spell_group_rule().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_createinfo_spell",
            ctx.db.game_createinfo_spell().iter(),
        );
        out.insert(CatalogueFingerprint {
            family: "spells".into(),
            row_count: n,
            fingerprint: h.finish(),
            tables:
                "game_spell,game_spell_effect,game_spell_reagent,game_spell_chain,game_spell_learn,\
                     game_spell_group,game_spell_group_rule,game_createinfo_spell"
                    .into(),
        });
    }

    // ITEMS — item_template is imported WHOLE regardless of box (`importer/src/main.rs`: "3)
    // item_template rows — the FULL vanilla item set"); every shard should carry the identical set.
    {
        let mut h = DefaultHasher::new();
        let mut n = 0u64;
        fold_table(
            &mut h,
            &mut n,
            "game_item_template",
            ctx.db.game_item_template().iter(),
        );
        out.insert(CatalogueFingerprint {
            family: "items".into(),
            row_count: n,
            fingerprint: h.finish(),
            tables: "game_item_template".into(),
        });
    }

    // DBC_REFERENCE — every table the client-DBC importer (`importer/src/dbc.rs`) produces, plus the
    // handful of cmangos-dump tables that are ALSO box-independent (`build_start_position_sql` /
    // `build_graveyard_zone_sql` / `build_areatrigger_teleport_sql` / `build_createinfo_action_sql`,
    // none of which take a box/local-entry argument) — world-wide reference data with no geographic
    // scoping, so every shard should carry the identical set.
    {
        let mut h = DefaultHasher::new();
        let mut n = 0u64;
        fold_table(&mut h, &mut n, "game_area", ctx.db.game_area().iter());
        fold_table(
            &mut h,
            &mut n,
            "game_area_trigger",
            ctx.db.game_area_trigger().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_areatrigger_teleport",
            ctx.db.game_areatrigger_teleport().iter(),
        );
        fold_table(&mut h, &mut n, "game_faction", ctx.db.game_faction().iter());
        fold_table(
            &mut h,
            &mut n,
            "game_faction_template",
            ctx.db.game_faction_template().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_graveyard",
            ctx.db.game_graveyard().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_graveyard_zone",
            ctx.db.game_graveyard_zone().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_creature_family",
            ctx.db.game_creature_family().iter(),
        );
        fold_table(&mut h, &mut n, "game_lock", ctx.db.game_lock().iter());
        fold_table(
            &mut h,
            &mut n,
            "game_skill_line",
            ctx.db.game_skill_line().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_skill_ability",
            ctx.db.game_skill_ability().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_skill_availability",
            ctx.db.game_skill_availability().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_race_info",
            ctx.db.game_race_info().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_char_base_info",
            ctx.db.game_char_base_info().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_start_item",
            ctx.db.game_start_item().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_start_position",
            ctx.db.game_start_position().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_createinfo_action",
            ctx.db.game_createinfo_action().iter(),
        );
        fold_table(&mut h, &mut n, "game_talent", ctx.db.game_talent().iter());
        fold_table(
            &mut h,
            &mut n,
            "game_talent_tab",
            ctx.db.game_talent_tab().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_class_level_stats",
            ctx.db.game_class_level_stats().iter(),
        );
        fold_table(
            &mut h,
            &mut n,
            "game_level_stats",
            ctx.db.game_level_stats().iter(),
        );
        out.insert(CatalogueFingerprint {
            family: "dbc_reference".into(),
            row_count: n,
            fingerprint: h.finish(),
            tables: "game_area,game_area_trigger,game_areatrigger_teleport,game_faction,\
                     game_faction_template,game_graveyard,game_graveyard_zone,game_creature_family,\
                     game_lock,game_skill_line,game_skill_ability,game_skill_availability,\
                     game_race_info,game_char_base_info,game_start_item,game_start_position,\
                     game_createinfo_action,game_talent,game_talent_tab,game_class_level_stats,\
                     game_level_stats"
                .into(),
        });
    }

    log::info!("debug_catalogue_fingerprint: recomputed 3 families (spells, items, dbc_reference)");
}
