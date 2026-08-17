//! Notify-hook event catalog + package tick-pass dispatch.
//!
//! This is the runtime half of the package substrate's extension points; the compile-time half is
//! `build.rs`'s marker scan (see the `game_hook` / `game_tick_pass` macro docs in `lib.rs`). Each
//! core chokepoint calls exactly ONE `fire_*` fn here, which iterates the generated per-event
//! registry. Handlers are NOTIFY-ONLY: they observe the payload and may act through the same
//! reducer-internal fns core code uses (grant, damage, spawn, ...), but there is no veto/fold —
//! mutating/decorator hooks are a separately-green-lit Phase 2.
//!
//! The catalog:
//!
//! | event               | fires from                                             | payload |
//! |---------------------|--------------------------------------------------------|---------|
//! | `on_damage_taken`   | `spell::break_auras_on_damage` — the single helper every real-damage path (melee swing, direct spell, DoT tick) already calls | [`DamageTakenPayload`] |
//! | `on_creature_spawn` | `creatures::insert_creature_entity` — the single wrapper every creature-entity insert routes through (seed, respawn, debug spawn, pet summon, tame) | [`CreatureSpawnPayload`] |
//! | `on_levelup`        | `xp::grant_xp`'s ding loop, once per level gained      | [`LevelupPayload`] |
//! | `on_group_invite`   | `group::group_invite`, after the invite row + event are written | [`GroupInvitePayload`] |
//! | `on_death`          | `combat::kill_creature` (creature corpse AND pet despawn), plus each player-death site (melee tick, spell damage, `debug_set_health(0)`) | [`DeathPayload`] |
//! | `on_kill`           | `combat::kill_creature` when a player `killer` gets credit | [`KillPayload`] |
//! | `on_aggro`          | the behavior cycle's aggro phase — direct proximity aggro AND the pack-assist arm | [`AggroPayload`] |
//! | `on_cast_resolved`  | `spell::resolve_cast_at` success exit — every cast path funnels through it | [`CastResolvedPayload`] |
//! | `on_loot`           | `items::apply_take_loot` success — player + debug loot both route here | [`LootPayload`] |
//! | `on_quest_accept`   | `quest::apply_accept_quest` AND `quest::grant_quest_unchecked` (the debug/harness grant) | [`QuestAcceptPayload`] |
//! | `on_quest_turnin`   | `quest::apply_turn_in_quest` success (the rep/XP site)  | [`QuestTurninPayload`] |
//! | `on_login`          | `world::player_login` success exit                      | [`LoginPayload`] |
//! | `on_logout`         | `world::remove_from_world` — covers explicit logout AND abrupt disconnect | [`LogoutPayload`] |
//! | `on_gossip_select`  | `world::gossip_select` — the notify reducer the gateway calls on CMSG_GOSSIP_SELECT_OPTION | [`GossipSelectPayload`] |
//! | `on_creature_death` | `combat::kill_creature`, non-pet branch — the encounter-grade twin of `on_death` (entry + instance snapshot; work-item 228) | [`CreatureDeathPayload`] |
//! | `on_hp_threshold`   | `encounter::encounter_hp_probe` — the kernel's own `on_damage_taken` handler; fires once per registered `(entry, pct)` crossing per instance (work-item 228) | [`HpThresholdPayload`] |
//! | `on_go_used`        | `gameobject::apply_use_gameobject` success exit — player use AND the debug drivers (work-item 228) | [`GoUsedPayload`] |
//!
//! Extending the catalog = the payload struct here + the `HOOK_EVENTS` row in `module/build.rs`
//! (which generates the `payload_for` alias, the `fire_*` fn, and the registry array from that
//! row), and the one dispatch line at the new chokepoint. Two files, one row each — the catalog
//! cannot drift between alias, dispatch, and array.

use spacetimedb::ReducerContext;

/// A unit took real damage (`dmg > 0`, target survived). Fired from `break_auras_on_damage`, so it
/// inherits that helper's exact coverage: every melee-swing, direct-spell, and DoT-tick damage path.
/// `attacker_guid` is the genuine MELEE assailant or `0` when the damage path has no such concept
/// (direct spell / DoT tick — same convention as the helper itself). `periodic` marks a DoT tick.
pub struct DamageTakenPayload {
    pub target_guid: u64,
    pub attacker_guid: u64,
    pub periodic: bool,
}

/// A creature `game_world_entity` row was just inserted (world seed, respawn pass, debug spawn, or
/// pet summon). The payload carries the row's identity/position snapshot; the row IS already
/// committed to the table when handlers run, so `ctx.db.game_world_entity()` lookups by `guid` see it.
pub struct CreatureSpawnPayload {
    pub guid: u64,
    pub entry: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A group invite was just recorded for `target_guid`. Fired AFTER the invite row + notification
/// event are written — a handler may accept immediately via `group::accept_invite_for` (the
/// playerbots auto-accept path).
pub struct GroupInvitePayload {
    pub target_guid: u64,
    pub inviter_guid: u64,
}

/// A player entity crossed a level threshold (once per level in a multi-level award). NOTE: fired
/// from inside `grant_xp`'s ding loop, where the caller persists the mutated entity row AFTER the
/// loop — so within the firing transaction the `game_world_entity` row may still show the pre-ding
/// state. Read the payload, not the table, for the new level.
pub struct LevelupPayload {
    pub character_guid: u64,
    pub new_level: u32,
}

/// A unit DIED. Fires for every death path: a creature killed via `combat::kill_creature` (melee
/// killing blow, lethal spell/DoT, `debug_kill_creature`, `debug_set_health(0)`) INCLUDING the pet
/// clean-despawn branch, and a PLAYER death at each of its sites (melee tick, spell-damage path,
/// `debug_set_health(0)`). `killer_guid` is the credited killer or `0` when the path has none (a DoT
/// with no attacker, a debug kill). Fired AFTER the death is committed (row updated/deleted), so
/// handlers observe the post-death world.
pub struct DeathPayload {
    pub victim_guid: u64,
    pub killer_guid: u64,
    pub victim_is_player: bool,
}

/// A PLAYER got kill CREDIT for a creature (inside `combat::kill_creature`,
/// `killer = Some`). Complements `on_death`: this is the killer-centric event (one per credited
/// kill), `on_death` is the victim-centric one (one per death, credited or not). `victim_entry` /
/// `victim_level` are snapshotted before the corpse's fields are cleared.
pub struct KillPayload {
    pub killer_guid: u64,
    pub victim_guid: u64,
    pub victim_entry: u32,
    pub victim_level: u32,
}

/// A creature ENGAGED a player on its own initiative — proximity aggro from the aggro pass, or a
/// same-faction neighbor answering a pack-mate's call (`assist = true`). Fired once per new
/// engagement, right after the melee row is armed. Retaliation (creature hit first) and
/// `start_attack` do NOT fire this — it's specifically "the world noticed you".
pub struct AggroPayload {
    pub creature_guid: u64,
    pub target_guid: u64,
    pub assist: bool,
}

/// A cast fully RESOLVED (all effects dispatched, visual emitted, GCD/cooldown started) — the
/// success exit of `spell::resolve_cast_at`, the single core every cast path funnels through
/// (player cast, timed completion, channel tick start, creature cast, triggered cast, debug cast).
/// Rejected/failed casts never fire this.
pub struct CastResolvedPayload {
    pub caster_guid: u64,
    pub spell_id: u32,
    pub target_guid: u64,
}

/// A unit LOOTED one item stack from a corpse/gameobject loot window (`items::apply_take_loot`
/// success — the player reducer and `debug_take_loot` both route here). Money pickup is a separate
/// path and does not fire this.
pub struct LootPayload {
    pub looter_guid: u64,
    pub corpse_guid: u64,
    pub item_entry: u32,
    pub count: u32,
}

/// A quest landed in a character's log: the real accept (`quest::apply_accept_quest`, all gates
/// passed) or the harness grant (`quest::grant_quest_unchecked` behind `debug_grant_quest`, gates
/// skipped). Both write the same CharacterQuest row shape, so consumers see one event either way.
pub struct QuestAcceptPayload {
    pub character_guid: u64,
    pub quest_entry: u32,
}

/// A quest was TURNED IN (`quest::apply_turn_in_quest` success — rewards granted, row marked
/// rewarded). This is the rep/XP site from the research catalog.
pub struct QuestTurninPayload {
    pub character_guid: u64,
    pub quest_entry: u32,
}

/// A player finished `world::player_login`: the live entity exists, owned rows are restamped, and
/// the character row is updated (online, rested XP accrued). Server-side bots do NOT fire this —
/// they enter the world via their own spawn path, not `player_login`.
pub struct LoginPayload {
    pub character_guid: u64,
}

/// A player LEFT the world (`world::remove_from_world` — explicit CMSG_LOGOUT_REQUEST and abrupt
/// socket disconnect both land here). Fired BEFORE the live entity row is deleted, so handlers can
/// still read it; progression is already persisted back to the character row.
pub struct LogoutPayload {
    pub character_guid: u64,
}

/// A player clicked a gossip option (`world::gossip_select`, the notify reducer the gateway calls
/// on CMSG_GOSSIP_SELECT_OPTION before running its own vendor/innkeeper handling). Pure
/// notification — core gossip behavior (vendor window, inn bind, window close) is unaffected.
pub struct GossipSelectPayload {
    pub character_guid: u64,
    pub npc_guid: u64,
    /// The clicked option's POSITION in the per-player filtered menu — NOT stable across viewers
    /// (a cond-gated row or a second option renumbers it). Kept for the vendor/inn routing; a hook
    /// that needs to identify a SPECIFIC option must use `option_row_id` instead (283).
    pub option_id: u32,
    /// The clicked option's `game_gossip_option.row_id` — the STABLE identifier (283). A package
    /// that mints its own gossip option at a known reserved row_id (≥50000) keys on this, immune to
    /// menu position. `u32::MAX` for the trailing Farewell line and the flag-synthesized
    /// vendor/inn/browse options (which have no imported row).
    pub option_row_id: u32,
}

/// A CREATURE died for real (work-item 228, the encounter kernel's death event) — fired from
/// `combat::kill_creature`'s non-pet branch right after `on_death`, with the encounter-grade
/// identity `on_death` lacks: the victim's `entry` and `instance_id`, SNAPSHOTTED before the corpse
/// row was mutated. Pet clean-despawns and player deaths do NOT fire this (encounters key on wild
/// creatures); `killer_guid` is the credited killer or 0 (same convention as `on_death`).
pub struct CreatureDeathPayload {
    pub creature_guid: u64,
    pub entry: u32,
    pub instance_id: u64,
    pub killer_guid: u64,
    /// The victim's threat table `(source_guid, threat)` as it stood at the death — snapshotted
    /// BEFORE `disengage` wipes it (hooks fire after the wipe, so handlers can't read the table
    /// themselves). Threat is the engine's only per-player damage/heal ledger (every damage path
    /// adds `threat_from_damage`, heals add `threat_from_heal_each`), so this is the engine's
    /// ready-made contribution-share input for any "who earned this kill" split (its original
    /// consumer, the dynamic-events package, was removed). Only player sources exist by
    /// construction (creatures keep no threat on creatures). Empty for uncontested deaths
    /// (debug kills, DoT deaths after disengage).
    pub threat_snapshot: Vec<(u64, i64)>,
}

/// A watched creature crossed a registered HP threshold (work-item 228): its post-damage health
/// reached `pct`% or below for the FIRST time in this instance. Fired by the encounter kernel's
/// damage probe (`encounter::encounter_hp_probe`, itself an `on_damage_taken` handler), once per
/// `(entry, pct)` watch per instance — a heal back above and a re-drop never re-fires until
/// `encounter::reset_hp_fired`/the instance sweep clears the fired-mark. Register watches via
/// `encounter::watch_hp_threshold(ctx, entry, pct)`; with no registered `on_hp_threshold` handler
/// anywhere in the build the probe is a single constant branch per damage event (zero-cost path).
/// A LETHAL blow never reaches the probe (every kill path returns before the damage hook), so
/// thresholds skipped by the killing hit do NOT fire — key final-phase/execute logic on
/// `on_creature_death`, never on a low-pct watch (review note; a 1% watch is unreliable by design).
pub struct HpThresholdPayload {
    pub creature_guid: u64,
    pub entry: u32,
    pub instance_id: u64,
    pub pct: u8,
}

/// A unit successfully USED a gameobject (work-item 228) — fired at `apply_use_gameobject`'s
/// success exit (player `use_gameobject` and the debug drivers share that core), AFTER the
/// type-dispatch committed its effect (chest looted, door toggled, goober credited, inert no-op).
/// Rejected uses (out of range, already-looted chest...) never fire. `instance_id` is the GO
/// ROW's own (190 slice 2: rows are instance-tagged; per-instance copies carry their instance,
/// static rows carry 0) — handlers key state checks AND primitives off it directly.
pub struct GoUsedPayload {
    pub go_guid: u64,
    pub go_entry: u32,
    pub user_guid: u64,
    pub instance_id: u64,
}

// The `payload_for` alias mod + the per-event `fire_*` dispatch fns are GENERATED by build.rs from
// its `HOOK_EVENTS` catalog. Included HERE so every existing path
// (`hooks::fire_*`, `hooks::payload_for::*`) is unchanged. Adding an event = the payload struct
// above + the `HOOK_EVENTS` row in module/build.rs + the dispatch line at the new chokepoint.
include!(concat!(env!("OUT_DIR"), "/hook_dispatch.rs"));

/// Run every registered package tick pass, in the registry's deterministic (sorted-by-path) order.
/// Called at the END of `tick_creatures` every tick (0.5s), after all core passes — see the cadence
/// note in the `game_tick_pass` macro doc.
pub(crate) fn run_package_tick_passes(ctx: &ReducerContext) {
    for (_name, f) in crate::GAME_TICK_PASSES {
        f(ctx);
    }
}
