//! Creature data tables (template / cast / waypoint / spawn), the live-entity builders for both
//! creatures and players, the level/rank stat rolls, and the dev content importer. [static]

use lyracore_shared::{constants, packing, spatial};
use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

#[cfg(feature = "debug_reducers")]
use crate::creatures::tick::game_creature_spline;
use crate::{game_active_taxi_flight, game_race_info, game_world_entity, Character, WorldEntity};

// ===========================================================================================
//  Creature data [static]
// ===========================================================================================

/// Static creature definition, keyed by creature entry. Feeds the Unit CREATE_OBJECT fields and
/// the `CMSG_CREATURE_QUERY` reply. The live creature is a `game_world_entity` row (type Unit). [static]
#[table(
    accessor = game_creature_template,
    public,
    // Perf catalog 1.19: `tick::active_cell_radius` used to fold `max(aggro_range)` over EVERY template
    // on every 500ms firing. With this btree it asks the only question that can change the answer —
    // "does any template override exceed the visibility floor?" — as one index probe.
    index(accessor = by_aggro_range, btree(columns = [aggro_range]))
)]
pub struct CreatureTemplate {
    #[primary_key]
    pub entry: u32,
    pub name: String,
    pub subname: String,
    pub display_id: u32,
    pub level: u32,
    pub health: u32,
    pub faction_template: u32,
    pub npc_flags: u32,
    pub unit_flags: u32,
    pub creature_type: u8, // e.g. 8 = Critter
    pub creature_family: u8,
    pub type_flags: u32,
    pub rank: u8,
    pub scale: f32,
    pub base_attack_time_ms: u32, // melee swing interval, mirrored to UNIT_FIELD_BASEATTACKTIME
    // Copper loot range rolled onto the corpse on the killing blow. `#[default(0)]` so
    // adding these columns auto-migrates existing rows (no `-c` wipe).
    #[default(0)]
    pub money_min: u32,
    #[default(0)]
    pub money_max: u32,

    // Level variance: cmangos MaxLevel / MaxLevelHealth. `level`/`health` above are the MIN; a spawn
    // rolls a level in `[level, max_level]` and the matching health (so a pack isn't uniformly L1).
    // 0 = "no range" (use the min) — the default, so existing rows auto-migrate and un-imported
    // creatures keep a fixed level. `#[default(0)]` + end-appended (migration rule).
    #[default(0)]
    pub max_level: u32,
    #[default(0)]
    pub max_level_health: u32,

    // Proximity aggro (vanilla creature AI): a hostile creature whose `aggro_range` (yards) covers a
    // player self-engages on sight — the aggro pass in `tick_creatures` arms a creature→player melee
    // row so the swing tick makes it attack. 0 = passive (engages only when attacked). Template-only:
    // read at tick time, never mirrored onto the live entity.
    // `#[default(0)]` + end-appended so adding it auto-migrates existing rows (no `-c` wipe) and every
    // un-imported / un-updated creature stays passive — baseline-safe.
    #[default(0)]
    pub aggro_range: u32,

    // Per-creature melee swing damage (parity #7): cmangos MinMeleeDmg/MaxMeleeDmg, rounded to int. Lets
    // an L5 Garrick (6-7) hit harder than an L1 wolf (2) instead of every mob sharing the flat 1-3.
    // `damage_max == 0` ⇒ "not imported" → `swing_range_ctx` falls back to CREATURE_MELEE_MIN/MAX, so
    // existing rows + the seed chicken stay byte-identical. `#[default(0)]` + end-appended (migration rule).
    #[default(0)]
    pub damage_min: u32,
    #[default(0)]
    pub damage_max: u32,

    // Creature armor (parity: `armor_mitigation_pct` already reads the live entity's armor for BOTH
    // attack directions, but every spawn hardcoded 0 — so player physical damage against creatures was
    // never mitigated). cmangos `creature_template.armor`. 0 ⇒ "not imported" → unmitigated, matching the
    // old hardcoded behavior — baseline-safe. `#[default(0)]` + end-appended (migration rule).
    #[default(0)]
    pub armor: u32,

    // Loot-family completeness (work-item 210): the creature's PICKPOCKET and SKIN loot-table ids —
    // cmangos `creature_template.PickpocketLootId` / `SkinLootId`, sitting immediately after `LootId`
    // in the real schema (the importer's `ct::PICKPOCKET_LOOT_ID`/`ct::SKIN_LOOT_ID`, `[V]` — confirm
    // against your own dump). `pickpocket_loot_id` keys `game_pickpocket_loot` directly by CREATURE
    // entry (collapsed like `LootId`); `skin_loot_id` keys `game_skinning_loot` (NOT collapsed — many
    // creatures of the same level band share one skin table). 0 on either ⇒ "not imported" — E_PICKPOCKET
    // grants copper only (unchanged) and `skin_corpse` falls back to the flat Light Leather — so every
    // existing row (imported pre-210, or seeded) auto-migrates byte-identical. `#[default(0u32)]` +
    // end-appended (migration rule).
    #[default(0u32)]
    pub pickpocket_loot_id: u32,
    #[default(0u32)]
    pub skin_loot_id: u32,

    // Which class a trainer serves. `creature_template.TrainerType`/`TrainerClass`, columns 71/73 —
    // verified against the DDL of the dump `importer/scripts/classic-db.lock` pins, along with every
    // other `ct::` anchor.
    //
    // `trainer_type` is CLASS 0 · MOUNTS 1 · TRADESKILLS 2 · PETS 3. 0 is a real value AND this
    // column's default, so most templates read CLASS without being trainers at all — never gate on
    // it alone (danger-zones §1.2: a default that is a valid value).
    //
    // `trainer_class` is a class ID, not a mask; 0 means "serves everyone", which is what keeps the
    // gate fail-open on a world that has not been re-imported.
    #[default(0u8)]
    pub trainer_type: u8,
    #[default(0u8)]
    pub trainer_class: u8,
}

/// Beast-family reference data, keyed by the family id that `CreatureTemplate.creature_family`
/// points at (cmangos `creature_template.family`, already imported — see that column's doc
/// comment). Source: `CreatureFamily.dbc` (work-item 214, the 188 pet system's data half).
/// `pet_food_mask` is the cmangos `PetDiet` bitmask (`MEAT 0x1 · FISH 0x2 · CHEESE 0x4 ·
/// BREAD 0x8 · MUSHROOM 0x10 · FRUIT 0x20 · RAW_MEAT 0x40 · RAW_FISH 0x80`) — which food item
/// classes satisfy this family's hunger. `pet_talent_type` is `-1` for a non-pet family (most
/// beasts — wolves, boars, bears that are never tameable) and `>= 0` for a tameable Hunter-pet
/// family (the talent tree the tamed pet gets). `category` is the DBC's own grouping id, kept
/// as the raw foreign key (same "store the raw key" convention as `race_info_sql`/`faction_sql`
/// in the importer). NOTE: vanilla `CreatureFamily.dbc` carries NO skill-line column — a pet
/// family's actual spells ride `SkillLineAbility` (work-item 208's `game_skill_ability`), not
/// this table. Consumer: work-item 188 (the pet feeding gate reads `pet_food_mask`, the
/// tameable gate reads `pet_talent_type != -1`) — not wired up here, data-only. No Timestamp →
/// SQL-seedable, importer-owned (clear+reload). [static]
#[table(accessor = game_creature_family, public)]
pub struct CreatureFamily {
    #[primary_key]
    pub family_id: u32,
    pub name: String,
    pub pet_food_mask: i32,
    pub pet_talent_type: i32,
    pub category: i32,
}

/// Per-creature gossip menu: maps a creature template entry to the `npc_text` id shown in
/// `SMSG_GOSSIP_MESSAGE`. The lookup is: `game_world_entity.entry` → `game_gossip_menu.entry`
/// → `game_gossip_menu.text_id` → `game_npc_text.text`. Keyed by `entry` (the creature template
/// entry, NOT the cmangos `gossip_menu.entry`); this collapses the indirection. A creature with
/// no row falls back to `GOSSIP_GREETING_TEXT_ID = 1` and the generic greeting. No Timestamp →
/// SQL-seedable. Importer-owned (clear+reload). [static]
#[table(accessor = game_gossip_menu, public)]
pub struct GossipMenu {
    #[primary_key]
    pub entry: u32, // creature_template.entry (the live creature's entry field)
    pub text_id: u32, // → game_npc_text.text_id (the npc_text row to show)
}

/// Per-id NPC greeting text: the body of the gossip window's title panel, resolved by the client
/// via `CMSG_NPC_TEXT_QUERY` → `SMSG_NPC_TEXT_UPDATE`. `text_id` matches `game_gossip_menu.text_id`
/// and the `title_text_id` in `SMSG_GOSSIP_MESSAGE`. `text` is the first non-empty slot from the
/// cmangos `npc_text` row (or its `broadcast_text` reference). Importer-owned (clear+reload).
/// Answers ANY queried id — rows not imported here fall back to the generic greeting at the gateway.
/// No Timestamp → SQL-seedable. [static]
///
#[table(accessor = game_npc_text, public)]
pub struct NpcText {
    #[primary_key]
    pub text_id: u32,
    pub text: String, // slot 0, male (back-compat)
}

/// The remaining weighted npc_text slots (work-item 217 — vanilla ships 8 greeting variants; the
/// CLIENT does the random weighted pick from `SMSG_NPC_TEXT_UPDATE`'s 8-slot array, no server RNG,
/// see `gateway::codec::build_npc_text_update`). A SEPARATE table from `NpcText`, NOT an end-append
/// of it: SpacetimeDB 2.5's `#[table]` macro cannot default a `String` column — `#[default(String::new())]`
/// fails to compile (`error[E0493]`: `String`'s `Drop` can't run inside the macro's compile-time
/// type-check, which is a plain `const { .. }` block — this is a hard Rust limitation, not a repo
/// convention, so it applies to ANY end-appended `String` column, verified in this pass). A brand
/// NEW table sidesteps it entirely (zero existing rows ⇒ nothing to backfill ⇒ no column needs a
/// default at all), and this is the SAME one-row-plus-child-rows shape already used everywhere else
/// in this file for a one-to-many relation (`CreatureWaypoint` by `by_creature`, `CreatureSpell` by
/// `by_entry`) — so despite the plan's original "child-table alt rejected" note (written before this
/// macro constraint was hit), this is not a departure from house style.
///
/// One row per `(text_id, slot_index)`, `slot_index` 0..=7. When the dump's slot 0 is non-empty,
/// its male line DUPLICATES `NpcText.text` (a reader who only touches this table still gets a
/// self-contained slot 0; when slot 0 is empty, `NpcText.text` instead carries the first NON-empty
/// slot for back-compat and no slot-0 row exists here — empty slots are never emitted). A
/// `text_id` with NO rows here at all (an existing pre-217 row, or any id nobody has re-imported
/// since) is read by the gateway as "legacy single-slot": `NpcText.text` in both genders at
/// probability 1.0, every other slot silent — byte-identical to pre-217 behavior. Importer-owned
/// (clear+reload). No Timestamp → SQL-seedable. [static]
#[table(accessor = game_npc_text_slot, public, index(accessor = by_text_id, btree(columns = [text_id])))]
pub struct NpcTextSlot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub text_id: u32,   // -> NpcText.text_id
    pub slot_index: u8, // 0..=7 — the client's weighted-pick slot
    pub text_male: String,
    pub text_female: String,
    pub probability: f32,
}

/// Per-creature gossip MENU OPTION (work-item 217): a clickable line in `SMSG_GOSSIP_MESSAGE`
/// (browse-goods / make-home / "Train me" / plain gossip text / submenu link), imported from cmangos
/// `gossip_menu_option`. Keyed by creature template `entry` (NOT the cmangos `gossip_menu_option.menu_id`
/// — the importer collapses the same menu→entry indirection `GossipMenu` already collapses for the
/// title text, since a spawned creature only ever shows ITS OWN entry's menu). `option_index` is the
/// importer's DENSE 0-based position among a creature's options (matches cmangos `gossip_menu_option.id`
/// order) — the gateway sorts by it before assigning wire `gossip_list_id`s, and the SAME sort must be
/// used on both `CMSG_GOSSIP_HELLO` (render) and `CMSG_GOSSIP_SELECT_OPTION` (re-derive the click) or the
/// two fall out of alignment (see `gateway::world`'s HELLO/SELECT_OPTION dispatch note).
///
/// `action` is the cmangos `OptionType`/`option_id` verbatim — see `lyracore_shared::constants::gossip_option`
/// for what each value means; only VENDOR(3)/INNKEEPER(8)/TRAINER(5) route to a real system today, the
/// rest render but stay inert (submenu navigation deferred — `action_menu_id` is stored but never
/// followed). `cond_type`/`cond_value1`/`cond_value2` gate visibility — see
/// `lyracore_shared::constants::gossip_condition`; an option the importer can't classify gets `cond_type = 0`
/// (fail-open, always shown) rather than silently hidden, and is logged at import time.
///
/// A NEW table (work-item 217) → auto-migrates with no `-c`. No Timestamp → SQL-seedable,
/// importer-owned (clear+reload each ETL run, like its sibling `GossipMenu`). [static]
#[table(accessor = game_gossip_option, public, index(accessor = by_entry, btree(columns = [entry])))]
pub struct GossipOption {
    #[primary_key]
    pub row_id: u32, // dense importer-assigned id (unique across every creature's options)
    pub entry: u32, // creature_template.entry (collapsed from cmangos menu_id, like GossipMenu.entry)
    pub option_index: u32, // dense 0-based position among THIS creature's options (render + select order)
    pub icon: u32,         // cmangos OptionIcon (gossip icon glyph); truncated to u8 on the wire
    pub text: String, // resolved option label (direct npc_text-style string, or via broadcast_text)
    pub action: u32, // cmangos OptionType/option_id — see `lyracore_shared::constants::gossip_option`
    pub action_menu_id: u32, // cmangos ActionMenuId (submenu target) — stored INERT, never navigated (217 scope)
    pub cond_type: u32,      // see `lyracore_shared::constants::gossip_condition` (0 = always show)
    pub cond_value1: u32,    // cond_type's primary operand (a quest id for the QUEST_* conditions)
    pub cond_value2: u32,    // reserved (unused by the current minimal condition set)
}

/// Which spell a caster-type creature nukes/debuffs with while engaged, keyed by creature entry (one
/// cast spell per entry for this slice). The CAST pass in `tick_creatures` fires it at the creature's
/// current melee target. The cast cadence is the SPELL'S OWN GCD — `resolve_cast_at` rejects (returns
/// `Err`, ignored) while on cooldown — so no separate cooldown column is needed. Hand-authored
/// reference data, public + read-only, no Timestamp → SQL-seedable. A creature with NO row never casts
/// (baseline-safe — it just melees/chases as before). [static]
#[table(accessor = game_creature_cast, public)]
pub struct CreatureCast {
    #[primary_key]
    pub creature_entry: u32,
    pub spell_id: u32,
}

/// A caster creature's spell ROTATION (rank 20): multiple conditional spells per entry, tried by
/// `priority` (high → low) each cast tick, the first whose `condition` holds and that is off cooldown
/// firing. Generalises the single-spell `game_creature_cast` into state-driven AI (heal-when-low /
/// buff-self / debuff / nuke). `by_entry` (not a unique PK) so an entry carries several rows — a NEW
/// table, so it auto-migrates with no `-c`. A creature with rotation rows uses them; one with NONE falls
/// back to `game_creature_cast` (baseline-safe — existing single-spell casters are unchanged). The cast
/// TARGET is derived from the condition (self for heal/buff, the melee target for nuke/debuff). No
/// Timestamp → SQL-seedable. [static]
#[table(accessor = game_creature_spell, public, index(accessor = by_entry, btree(columns = [creature_entry])))]
pub struct CreatureSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub spell_id: u32,
    pub priority: u8, // higher is tried first; ties broken by id (deterministic)
    /// 0 ALWAYS (nuke, → melee target) · 1 SELF_HP_BELOW_PCT (heal, → self, gated by `condition_value`%) ·
    /// 2 TARGET_MISSING_AURA (debuff, → melee target, only if it lacks this spell's aura) ·
    /// 3 SELF_MISSING_AURA (buff, → self, only if self lacks this spell's aura).
    pub condition: u8,
    pub condition_value: u8, // the HP% threshold for SELF_HP_BELOW_PCT; ignored by the other conditions
}

/// `game_creature_spell.condition` discriminants — the creature-AI rotation gates. [static]
pub mod cast_condition {
    pub const ALWAYS: u8 = 0; // unconditional (the nuke / default), cast at the melee target
    pub const SELF_HP_BELOW_PCT: u8 = 1; // self HP% < condition_value → cast at self (heal)
    pub const TARGET_MISSING_AURA: u8 = 2; // melee target lacks this spell's aura → cast at it (debuff)
    pub const SELF_MISSING_AURA: u8 = 3; // self lacks this spell's aura → cast at self (buff)
}

/// A patrol waypoint for a creature. The creature walks between its waypoints. [static]
#[table(accessor = game_creature_waypoint, public, index(accessor = by_creature, btree(columns = [creature_guid])))]
pub struct CreatureWaypoint {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// cmangos `creature.MovementType`, deciding which idle (non-engaged) movement pass a creature runs:
/// IDLE holds its spawn post + orientation, RANDOM free-wanders near home, WAYPOINT follows a
/// `game_creature_waypoint` patrol. Most world NPCs (quest givers, vendors, guards) are IDLE.
pub const MOVEMENT_IDLE: u8 = 0;
pub const MOVEMENT_RANDOM: u8 = 1;
pub const MOVEMENT_WAYPOINT: u8 = 2;

/// The persistent spawn record for a creature — the source of truth that survives the creature's
/// death. The live `game_world_entity` row is deleted on death; this row holds the data to
/// re-create it once `respawn_at` elapses. While the creature is alive, `respawn_at` is
/// ignored. [static]
/// The **not-armed sentinel** for `respawn_at` / `despawn_at`.
///
/// Both columns are "ignored" in most states — `respawn_at` while the creature is alive,
/// `despawn_at` while it is not a corpse — and they used to be stamped with `ctx.timestamp` at
/// creation, i.e. a value permanently in the PAST. That made the due predicate
/// (`respawn_at <= now`) true for every row in the table, so `pass_respawn` and `pass_decay` could
/// only ever find their handful of genuinely-due rows by scanning all of them and re-testing the
/// state that actually mattered (is the entity gone / is it dead).
///
/// Measured: three such passes visited 5,735 rows each, every sense tick, with **nobody online** —
/// 2.2% of the writer at idle, and it scales with SPAWN COUNT, not players, so a full-world import
/// (~10× the spawns) multiplies it.
///
/// Parking the columns far in the future instead makes the due predicate mean what it says, so a
/// btree range scan (`..=now`) visits only rows that are actually armed — normally none. Same trick
/// as `game_aura`'s `by_next_tick` (`0` sentinel, `1..=now` range). `encounter.rs` already used a
/// far-future `respawn_at` for exactly this reason; this promotes that one-off to the rule.
///
/// Relative rather than absolute (`now + u32::MAX seconds`, ~136 years) to match that precedent and
/// to stay clear of `Timestamp` overflow — anything past `now` is equally "never" to a `..=now` scan.
/// **One-time migration for `timer_never`.** Every spawn row created before the sentinel existed
/// carries `respawn_at`/`despawn_at` stamped at creation time — permanently in the past — so the
/// range scans in `pass_respawn`/`pass_decay` would still visit all of them and the index would
/// narrow nothing. This disarms the timers on rows that are not actually pending anything:
///
///   * `respawn_at` is disarmed when the creature is ALIVE (a live entity exists for the guid), and
///   * `despawn_at` is disarmed when it is NOT a corpse (no entity, or an entity that is not dead).
///
/// Deliberately conservative: a genuinely pending respawn (dead/absent creature with a timer) and a
/// genuinely rotting corpse are both left alone, so running this can never resurrect or vanish
/// anything. Idempotent, and safe to run repeatedly.
///
/// `limit` caps rows touched per call so a big world can be migrated in chunks rather than one
/// enormous transaction; it returns how many it changed, so an operator can loop until it reports 0.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_normalize_spawn_timers(ctx: &ReducerContext, limit: u32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let spawns = ctx.db.game_creature_spawn();
    let entities = ctx.db.game_world_entity();
    let never = timer_never(ctx);
    let cap = if limit == 0 { u32::MAX } else { limit } as usize;

    let mut changed = 0usize;
    let pending: Vec<CreatureSpawn> = spawns
        .iter()
        .filter(|s| s.respawn_at <= ctx.timestamp || s.despawn_at <= ctx.timestamp)
        .take(cap)
        .collect();
    for mut s in pending {
        let entity = entities.guid().find(s.guid);
        let alive = entity.is_some();
        let is_corpse = entity.map(|e| e.dead).unwrap_or(false);
        let mut touched = false;
        if alive && s.respawn_at <= ctx.timestamp {
            s.respawn_at = never; // alive: nothing to respawn
            touched = true;
        }
        if !is_corpse && s.despawn_at <= ctx.timestamp {
            s.despawn_at = never; // not a corpse: nothing to decay
            touched = true;
        }
        if touched {
            spawns.guid().update(s);
            changed += 1;
        }
    }
    spacetimedb::log::info!(
        "debug_normalize_spawn_timers: disarmed {changed} spawn row(s) (limit {limit}); \
         re-run until it reports 0"
    );
    Ok(())
}

/// #194: retire every creature whose SPAWN point sits inside the given cell box on THIS database —
/// spawn row, live entity, spline leg, and (globally, like `import_creature_spawns`) the threat
/// table. Regions are single-owner (creatures are region-static, spec #12): when a region is
/// assigned to another shard, the non-owner runs this so exactly one database holds the
/// population. Keyed by SPAWN position, not the entity's current one — a wanderer belongs to the
/// region of its home. Recoverable only by re-import; operator- and debug-feature-gated on purpose.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_retire_region_creatures(
    ctx: &ReducerContext,
    map_id: u32,
    gx_min: i32,
    gx_max: i32,
    gy_min: i32,
    gy_max: i32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let spawns = ctx.db.game_creature_spawn();
    let entities = ctx.db.game_world_entity();
    let splines = ctx.db.game_creature_spline();
    let doomed: Vec<u64> = spawns
        .iter()
        .filter(|s| {
            let (gx, gy) = lyracore_shared::spatial::grid_cell(s.x, s.y);
            s.map_id == map_id && gx >= gx_min && gx <= gx_max && gy >= gy_min && gy <= gy_max
        })
        .map(|s| s.guid)
        .collect();
    for g in &doomed {
        entities.guid().delete(g);
        splines.guid().delete(g);
        spawns.guid().delete(g);
    }
    // Same posture as `import_creature_spawns`' full wipe: threat rows are keyed by creature guid
    // and ephemeral; a scoped sweep is not worth the code when the retire is an ops-time action.
    if !doomed.is_empty() {
        crate::threat::clear_all(ctx);
    }
    spacetimedb::log::info!(
        "debug_retire_region_creatures: retired {} creature(s) in map {map_id} cells \
         ({gx_min}..{gx_max}, {gy_min}..{gy_max})",
        doomed.len()
    );
    Ok(())
}

pub(crate) fn timer_never(ctx: &spacetimedb::ReducerContext) -> spacetimedb::Timestamp {
    ctx.timestamp
        .checked_add(spacetimedb::TimeDuration::from_micros(
            u32::MAX as i64 * 1_000_000,
        ))
        .unwrap_or(ctx.timestamp)
}

#[table(
    accessor = game_creature_spawn,
    public,
    // Perf: the due-time range scans behind `pass_respawn` / `pass_decay`. Plain INDEX ADDs over
    // already-existing columns — no new column, no default, no data migration (same shape as
    // `game_aura`'s `by_expiry`, and `gateway/tests/schema_parity.rs` checks columns/bindings, not
    // indexes, so the gateway is unaffected).
    index(accessor = by_respawn_at, btree(columns = [respawn_at])),
    index(accessor = by_despawn_at, btree(columns = [despawn_at]))
)]
pub struct CreatureSpawn {
    #[primary_key]
    pub guid: u64,
    pub entry: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub respawn_at: Timestamp, // when a despawned creature should re-spawn; ignored until the corpse despawns
    pub despawn_at: Timestamp, // when a corpse should despawn (DESTROY); ignored while alive
    /// cmangos MovementType (0 idle / 1 random-wander / 2 waypoint — see the `MOVEMENT_*` consts). Gates
    /// the idle-movement passes: the wander pass runs ONLY for RANDOM; IDLE creatures hold their post.
    /// END-appended + defaulted so the column auto-migrates existing rows to IDLE (stationary) until a
    /// re-import populates the real per-creature values.
    #[default(0)]
    pub movement_type: u8,

    /// cmangos `spawntimesecsmin`/`spawntimesecsmax` (this importer takes the min leg — see
    /// `importer/src/main.rs` `cr::SPAWN_TIME_SECS`): the real per-spawn respawn delay, in seconds,
    /// counted from the creature's DEATH (not from corpse-decay). `0` = "not imported" → the decay
    /// pass (`creatures::tick::pass_decay`) falls back to the flat `RESPAWN_MICROS` timer (15s after
    /// decay, i.e. ~75s after death) so every existing/un-imported row stays byte-identical.
    /// END-appended + defaulted (migration rule) — no `-c` wipe needed.
    #[default(0)]
    pub respawn_secs: u32,
}

/// Roll a creature's `(level, health)` within its template's `[min, max]` range from a random `u32`.
/// `max_level <= min_level` (or 0, the default) means "no range" → the min level/health verbatim.
/// Otherwise the level is uniform in `[min_level, max_level]` and the health is interpolated linearly
/// between `min_health` and `max_health` by the level's position in the span (so a higher-level spawn
/// is correspondingly beefier — no L3-mob-with-L1-HP inconsistency). Pure — unit-tested.
pub fn rolled_creature_stats(
    min_level: u32,
    max_level: u32,
    min_health: u32,
    max_health: u32,
    rand: u32,
) -> (u32, u32) {
    if max_level <= min_level {
        return (min_level, min_health);
    }
    let span = max_level - min_level;
    let level = min_level + rand % (span + 1);
    // Linear interpolation of health by how far `level` sits into the span (integer math).
    let health = if max_health > min_health {
        min_health + (max_health - min_health) * (level - min_level) / span
    } else {
        min_health
    };
    (level, health)
}

/// Per-1000 HP multiplier for a creature's classification `rank`, applied to the rolled base health
/// when the live entity is built (`build_creature_entity`). cmangos `creature_template.rank` values:
/// `0`=normal, `1`=elite, `2`=rare-elite, `3`=boss/world-boss, `4`=rare. The multipliers approximate
/// vanilla's elite HP scaling (an elite of a given level carries several times a normal mob's HP):
/// normal 1.0×, elite 1.5×, rare-elite 2.8×, boss 4.0×, rare 1.8×. Stored as PERMILLE (×1000) so the
/// scaling is exact integer math (`hp * permille / 1000`) — no float drift, no platform-dependent
/// rounding, and rank 0 (`1000`) is the identity (`hp * 1000 / 1000 == hp`), so every current rank-0
/// spawn stays BYTE-IDENTICAL. An unknown/out-of-range rank falls back to the normal multiplier (safe
/// default — never amplifies an unrecognized value). Pure — unit-tested.
pub fn rank_hp_multiplier_permille(rank: u8) -> u32 {
    match rank {
        0 => 1000, // normal — identity (baseline)
        1 => 1500, // elite
        2 => 2800, // rare-elite
        3 => 4000, // boss / world boss
        4 => 1800, // rare
        _ => 1000, // unknown rank → treat as normal (never amplify an unrecognized value)
    }
}

/// Apply the rank HP multiplier to a base health value (integer permille math, saturating so a huge
/// base × a high multiplier can't wrap a `u32`). Shared by `build_creature_entity` and its unit test
/// so the live scaling and the asserted numbers never drift. Pure.
pub fn scale_health_for_rank(base_health: u32, rank: u8) -> u32 {
    let permille = rank_hp_multiplier_permille(rank);
    // Clamp the u64 product to the u32 ceiling BEFORE the cast — a bare `as u32` would silently
    // truncate (wrap) a huge base × >1000‰ product instead of saturating as documented.
    ((base_health as u64 * permille as u64) / 1000).min(u32::MAX as u64) as u32
}

/// Scale a template's minimum-level damage range to a live creature level. The level-matching case
/// is the identity, so ordinary spawns are unchanged; the swing fold uses this to give an advancing
/// Hunter pet a monotonic, overflow-safe damage refresh.
pub fn scale_creature_damage_for_level(
    min: u32,
    max: u32,
    template_level: u32,
    live_level: u32,
) -> (u32, u32) {
    if template_level == 0 || live_level == template_level {
        return (min, max);
    }
    let scale = |value: u32| {
        (u64::from(value) * u64::from(live_level) / u64::from(template_level))
            .min(u64::from(u32::MAX)) as u32
    };
    (scale(min).max(1), scale(max).max(1))
}

/// Insert a freshly-built creature `game_world_entity` row and fire the `on_creature_spawn` notify
/// hook. The SINGLE chokepoint every creature-entity insert routes through — world
/// seed, respawn pass, debug spawn, pet summon, tame — so a package hook sees every creature that
/// enters the world without any core dispatch-site edits. Player-entity inserts (login,
/// debug_spawn_player_entity) deliberately do NOT come through here.
pub(crate) fn insert_creature_entity(ctx: &spacetimedb::ReducerContext, mut entity: WorldEntity) {
    // #526: stand on a model floor (bridge, WMO interior deck) instead of the imported spawn.z
    // when one's imported at/below this spawn point — `floor_z` is `None` off vmap-slice/gate, so
    // an unimported map spawns byte-identical to before this line existed.
    if let Some(floor) = crate::vmap::floor_z(ctx, entity.map_id, entity.x, entity.y, entity.z) {
        entity.z = entity.z.max(floor);
    }
    let payload = crate::hooks::CreatureSpawnPayload {
        guid: entity.guid,
        entry: entity.entry,
        x: entity.x,
        y: entity.y,
        z: entity.z,
    };
    ctx.db.game_world_entity().insert(entity);
    // AFTER the insert: handlers may look the row up by guid (documented on the payload).
    crate::hooks::fire_on_creature_spawn(ctx, &payload);
}

/// Build the live `game_world_entity` row for a creature from its spawn record + template. Used by
/// both the initial seed and the respawn pass. `rand` (the caller's `ctx.random()`) drives the
/// level/health roll within the template range (`rolled_creature_stats`); pass a fixed value for a
/// deterministic spawn. `owner_identity` is the server sentinel (ZERO); `unit_bytes_0` is a
/// dummy-but-valid Human/Warrior/Male/Mana (non-rendering for a Unit; the codec rejects race 0).
/// `instance_id` is a REQUIRED param (work-item 190 slice 1) — every slice-1 caller passes 0
/// (open world); it is NOT read from `CreatureSpawn` this slice (spawn templates stay
/// instance-agnostic — slice 2's dungeon population creates per-instance copies at a call site,
/// not by adding a column here).
pub fn build_creature_entity(
    spawn: &CreatureSpawn,
    tmpl: &CreatureTemplate,
    rand: u32,
    instance_id: u64,
) -> WorldEntity {
    let (grid_x, grid_y) = spatial::grid_cell(spawn.x, spawn.y);
    let (level, rolled_health) = rolled_creature_stats(
        tmpl.level,
        tmpl.max_level,
        tmpl.health,
        tmpl.max_level_health,
        rand,
    );
    // ELITE/RARE/BOSS scaling: the level-rolled base HP is multiplied by the template's classification
    // rank. Rank 0 (every current spawn) → ×1.0, so this spawn is byte-identical to before; only a
    // template explicitly marked elite/rare/boss gets the beefier HP pool.
    let health = scale_health_for_rank(rolled_health, tmpl.rank);
    WorldEntity {
        guid: spawn.guid,
        owner_identity: Identity::ZERO,
        account_id: 0,
        map_id: spawn.map_id,
        x: spawn.x,
        y: spawn.y,
        z: spawn.z,
        orientation: spawn.orientation,
        grid_x,
        grid_y,
        cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
        last_move_ms: 0,
        type_mask: constants::type_mask::CREATURE, // 0x9 (no PLAYER bit)
        entry: spawn.entry,
        scale_x: tmpl.scale,
        health,
        max_health: health,
        power: 0,
        max_power: 0,
        level,
        faction_template: tmpl.faction_template,
        unit_bytes_0: 0x0101,
        display_id: tmpl.display_id,
        native_display_id: tmpl.display_id,
        unit_flags: tmpl.unit_flags,
        base_attack_time_ms: tmpl.base_attack_time_ms,
        dynamic_flags: 0,
        dead: false,
        player_bytes: 0,
        player_bytes_2: 0,
        player_bytes_3: 0,
        player_flags: 0,
        xp: 0,
        next_level_xp: 0,
        target_guid: 0,
        money: 0,        // no loot until the killing blow rolls it onto the corpse
        unit_bytes_1: 0, // creatures aren't ghosts
        unit_bytes_2: 0, // sheath state UNARMED — a creature spawns with its weapon stowed
        // A creature is a Unit — no character sheet — so the five player attributes are 0.
        strength: 0,
        agility: 0,
        stamina: 0,
        intellect: 0,
        spirit: 0,
        // UNIT_NPC_FLAGS straight from the cmangos template (gossip/vendor/questgiver/trainer icons).
        npc_flags: tmpl.npc_flags,
        armor: tmpl.armor, // cmangos creature_template.armor — feeds armor_mitigation_pct for both directions
        leg_ends_ms: 0, // fresh spawn: no in-flight leg, no waypoint cursor (re-acquire on first patrol tick)
        wp_target: 0,
        movement_flags: 0,             // freshly spawned, standing still
        combat_until_ms: 0,            // not in combat until it aggros / is attacked
        pickpocketed: false, // fresh spawn: pockets intact — re-set to false here gives the free per-life reset
        next_swing_spell: 0, // creatures don't queue on-next-swing abilities
        overpower_until_ms: 0, // no react window (creatures don't cast Overpower)
        revenge_until_ms: 0, // no react window (creatures don't cast Revenge)
        stance: 0, // creatures have no Warrior stance (0 = Battle default, never switched)
        owner_guid: 0, // a normal (wild) creature is NOT a pet — the pet builder stamps this
        skinned: false, // fresh spawn: corpse not yet skinned — re-set here gives the free per-life reset
        mana_regen_paused_until_ms: 0, // creatures have no mana pool; FSR never fires (max_power == 0)
        death_expire_micros: 0, // creatures have no corpse-reclaim escalation (player-only field)
        instance_id,            // slice 1: every caller passes 0 (open world)
        run_speed_mult_bp: 10_000, // 1× — GM `.speed` targets players only
        godmode: false,         // GM `.god` targets players only
        resting: false,         // creatures never rest (196)
        // A creature never calls `recompute_sheet`, so its sheet fields stay 0.
        sheet_str_bonus: 0,
        sheet_agi_bonus: 0,
        sheet_sta_bonus: 0,
        sheet_int_bonus: 0,
        sheet_spi_bonus: 0,
        sheet_ap_base: 0,
        sheet_ap_mods: 0,
        sheet_dmg_min: 0,
        sheet_dmg_max: 0,
        sheet_crit_bp: 0,
        bank_bag_slots: 0,   // a creature owns no bank slots
        mount_display_id: 0, // creatures do not use the player taxi presentation field
        zone_id: 0,          // unresolved: nothing routes zone-scoped delivery to a creature
        sheet_ranged_ap: 0,
        sheet_ranged_dmg_min: 0,
        sheet_ranged_dmg_max: 0,
    }
}

/// Build the live `game_world_entity` row for a PLAYER from its durable `game_character` row — the
/// single source of truth shared by `world::player_login` and `debug::debug_spawn_player_entity`
/// (the player counterpart to `build_creature_entity`). Resolves the power type, the gender-correct
/// per-race display + faction from the imported ChrRaces data (falling back to the Human-Male values
/// — display 49 / faction 1 — when the table isn't loaded), and the full stat curve via
/// `stats::apply_level_stats` (the same writer the ding loop and a GM level-set use, #362); health ==
/// max_health on a fresh build, mana classes start full while rage/energy start empty
/// (`stats::starting_power`).
///
/// `owner_identity` is the ONLY genuine difference between the two call sites: login passes
/// `ctx.sender()` (the connection's bound identity), while the debug materialize passes the
/// character's persisted `owner_identity` (a `spacetime call` runs as the CLI identity, which owns no
/// player binding). `account_id` is taken from the character row — login gates on
/// `character.account_id == account.id` before building, so the two are identical there.
pub fn build_player_entity(
    ctx: &ReducerContext,
    character: &Character,
    owner_identity: Identity,
) -> WorldEntity {
    let level = character.level as u32;
    let power_type = packing::power_type::for_class(character.class);
    // Per-race display + faction from the imported ChrRaces data (importer P1), gender-correct.
    // Falls back to the Human-Male values (display 49 / faction 1) when the table isn't loaded, so
    // login never breaks and a Human Male is identical either way.
    let race_info = ctx.db.game_race_info().race().find(character.race);
    let display = match &race_info {
        Some(ri) if character.gender != 0 => ri.female_display,
        Some(ri) => ri.male_display,
        None => 49,
    };
    let faction = race_info
        .as_ref()
        .map(|ri| ri.faction_template)
        .unwrap_or(1);
    let (grid_x, grid_y) = spatial::grid_cell(character.x, character.y);
    // Stat-block fields (strength..spirit, armor, max_health, max_power) are placeholders here —
    // `apply_level_stats` fills them right after construction (below). health/power are then
    // resolved from the persisted value against the freshly-written max.
    let mut entity = WorldEntity {
        guid: character.guid,
        owner_identity,
        account_id: character.account_id,
        map_id: character.map_id,
        x: character.x,
        y: character.y,
        z: character.z,
        orientation: character.orientation,
        grid_x,
        grid_y,
        cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
        last_move_ms: 0,
        type_mask: constants::type_mask::PLAYER,
        entry: 0,
        scale_x: 1.0,
        health: 0,
        max_health: 0,
        power: 0,
        max_power: 0,
        level,
        faction_template: faction, // per-race from ChrRaces (importer P1); fallback 1 (Human)
        unit_bytes_0: packing::unit_bytes_0(
            character.race,
            character.class,
            character.gender,
            power_type,
        ),
        display_id: display,
        native_display_id: display,
        unit_flags: constants::unit_flags::PLAYER_CONTROLLED,
        base_attack_time_ms: crate::DEFAULT_ATTACK_TIME_MS, // unarmed 2.0s
        dynamic_flags: 0,
        dead: false,
        player_bytes: packing::player_bytes(
            character.skin,
            character.face,
            character.hair_style,
            character.hair_color,
        ),
        // Rest state (196): bake the RESTED byte if this character logged out in an inn, so it logs
        // back in already showing the zzz icon + blue XP bar (no post-login relay needed). Byte 2
        // carries the persisted bank bag slot count so a purchase survives logout without a relog.
        player_bytes_2: packing::player_bytes_2_with_rest(
            character.facial_hair,
            character.bank_bag_slots,
            character.resting,
        ),
        player_bytes_3: character.gender as u32,
        player_flags: 0,
        xp: character.xp,
        next_level_xp: character.next_level_xp,
        target_guid: 0,
        money: character.money,                   // load the persisted purse
        bank_bag_slots: character.bank_bag_slots, // and the slots bought with it
        // Warriors start in Battle Stance (form 17 in UNIT_FIELD_BYTES_1[2]) so the action bar
        // shows the stance bar from login without requiring a manual stance cast. RAGE = Warrior.
        unit_bytes_1: if power_type == packing::power_type::RAGE {
            17u32 << 16
        } else {
            0
        },
        // Sheath state UNARMED (#101). Deliberately NOT persisted across logout: vanilla rebuilds a
        // player with weapons stowed, and the client re-sends `CMSG_SETSHEATHED` when the player
        // draws again, so there is nothing to restore.
        unit_bytes_2: 0,
        strength: 0,
        agility: 0,
        stamina: 0,
        intellect: 0,
        spirit: 0,
        npc_flags: 0,   // a player is not an NPC (no gossip/vendor flags)
        armor: 0,       // item armor folds in later
        leg_ends_ms: 0, // players don't run the creature movement passes; the columns are inert for them
        wp_target: 0,
        movement_flags: 0, // set live by movement_update on the player's first move
        combat_until_ms: 0, // set when the player attacks / is attacked (enter_combat)
        pickpocketed: false, // never read on a player (pickpocket targets creatures only)
        next_swing_spell: 0, // no queued strike at login (set by an E_NEXT_SWING cast, cleared by the swing)
        overpower_until_ms: 0, // no react window at login (armed by a dodge in resolve_swing)
        revenge_until_ms: 0, // no react window at login (armed by a dodge/parry/block in resolve_swing)
        stance: 0, // every character logs in in Battle Stance (the L1 baseline); switched by a stance cast
        owner_guid: 0, // a player is never a pet (pets are owner-stamped creatures)
        skinned: false, // never read on a player (skinning targets beast corpses only)
        mana_regen_paused_until_ms: 0, // no FSR window at login (stamped when mana is first spent in a cast)
        // Threaded from the DURABLE column: the reclaim-escalation ladder must survive a
        // disconnect/relog, or die-relog-die resets every death to the 30s floor.
        death_expire_micros: character.death_expire_micros,
        // Threaded from the DURABLE column (work-item 190 slice 1, always 0 this slice) so a relog
        // rebuilds the entity into the instance it was in, not open world — the `death_expire_micros`
        // precedent.
        instance_id: character.pending_instance_id,
        // GM playtest fields (work-item 223) threaded from the DURABLE carry columns (work-item 289 —
        // the `death_expire_micros` precedent). They are NOT durable settings: `persist_entity` clears
        // them on a real logout/disconnect and carries them across a despawn/rebuild WITHIN a session
        // (`persisted_gm_playtest`), so a login still starts at 1× speed / not-godmode while a
        // CROSS-MAP `.tele` — which despawns the entity and rebuilds it right here — no longer drops
        // `.god`/`.speed` on the far side.
        run_speed_mult_bp: character.pending_run_speed_mult_bp,
        godmode: character.pending_godmode,
        resting: character.resting, // 196: restore the live rest flag (relog into an inn shows rested)
        // Placeholder — `spell::recompute_sheet` (called right after login, alongside `recompute_vitals`)
        // fills these in from the real base stats `apply_level_stats` writes below.
        sheet_str_bonus: 0,
        sheet_agi_bonus: 0,
        sheet_sta_bonus: 0,
        sheet_int_bonus: 0,
        sheet_spi_bonus: 0,
        sheet_ap_base: 0,
        sheet_ap_mods: 0,
        sheet_dmg_min: 0,
        sheet_dmg_max: 0,
        sheet_crit_bp: 0,
        mount_display_id: 0,
        // WORLD ENTRY resolves the zone from the position this row is built at — login, the
        // WORLDPORT_ACK rebuild after a cross-map teleport, and a Transfer arrival's first login all
        // come through here, so every world-entry path yields a fresh zone before the Gateway reads
        // the row. Off the imported terrain slice it falls back to the durable zone the character
        // logged out with, which is 0 for a character that has never resolved one.
        zone_id: crate::terrain::zone_id_at(ctx, character.map_id, character.x, character.y)
            .unwrap_or(character.zone_id),
        sheet_ranged_ap: 0,
        sheet_ranged_dmg_min: 0,
        sheet_ranged_dmg_max: 0,
    };
    // An interrupted connection does not cancel a paid flight. Restore its presentation while the
    // scheduler continues from its original authoritative timestamp.
    if let Some(flight) = ctx
        .db
        .game_active_taxi_flight()
        .character_guid()
        .find(character.guid)
    {
        entity.mount_display_id = flight.mount_display_id;
        entity.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
    }
    // The level-derived stat block — the five base attributes, armor, and max health/power — from the
    // real class/level curve (importer P3), via the ONE shared writer also used by the ding loop and a
    // GM level-set (#362). Falls back to the flat placeholder/zeros when the curve isn't loaded, so an
    // L1 character is 60 HP either way.
    crate::stats::apply_level_stats(ctx, &mut entity, character.race, character.class, level);
    // Resume at the persisted vitals rather than always healing to full.
    // `character.health == 0` is the sentinel for "nothing persisted yet" (a fresh character, or an
    // existing row that predates this column) — spawn at full health/starting power in that case.
    // A real persisted health is clamped into `1..=max_health` in case the level's
    // max_health curve changed (e.g. a stat/curve reimport) since the last logout. Power only persists
    // for MANA — ENERGY and RAGE are non-persisted in vanilla (energy always 100/100, rage always 0 at
    // login; see `starting_power`'s doc comment), so every relog still routes those through
    // `starting_power` regardless of the health sentinel.
    entity.health = if character.health == 0 {
        entity.max_health
    } else {
        character.health.clamp(1, entity.max_health)
    };
    entity.power = if character.health != 0 && power_type == packing::power_type::MANA {
        character.power.min(entity.max_power)
    } else {
        crate::stats::starting_power(power_type, entity.max_power)
    };
    entity
}

#[cfg(test)]
mod tests {
    use lyracore_shared::constants::unit_flags;

    #[test]
    fn character_materialization_sets_the_player_controlled_flag() {
        let materialize =
            crate::test_scan::code_of(include_str!("spawn.rs"), "pub fn build_player_entity(");

        assert_eq!(unit_flags::PLAYER_CONTROLLED, 0x0000_0008);
        assert!(
            materialize.contains("unit_flags: constants::unit_flags::PLAYER_CONTROLLED,"),
            "every world-entry path shares build_player_entity, which must mark its live Character as player-controlled. Body was:\n{materialize}"
        );
    }

    #[test]
    fn combat_and_taxi_flag_transitions_retain_player_controlled() {
        let combat = crate::test_scan::code_of(
            include_str!("../combat/engage.rs"),
            "pub(crate) fn enter_combat(",
        );
        let disengage = crate::test_scan::code_of(
            include_str!("../combat/engage.rs"),
            "pub(crate) fn disengage(",
        );
        let taxi = crate::test_scan::code_of(include_str!("../taxi.rs"), "fn activate(");
        let taxi_end =
            crate::test_scan::code_of(include_str!("../taxi.rs"), "fn cleared_presentation(");

        assert!(
            combat.contains("e.unit_flags |= lyracore_shared::constants::unit_flags::IN_COMBAT;"),
            "enter_combat must add its bit without replacing Character state. Body was:\n{combat}"
        );
        assert!(
            disengage
                .contains("e.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;"),
            "disengage must clear only its bit. Body was:\n{disengage}"
        );
        assert!(
            taxi.contains(
                "self.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;"
            ),
            "taxi activation must add its bit without replacing Character state. Body was:\n{taxi}"
        );
        assert!(
            taxi_end.contains(
                "state.unit_flags &= !lyracore_shared::constants::unit_flags::TAXI_FLIGHT;"
            ),
            "taxi completion must clear only its bit. Body was:\n{taxi_end}"
        );

        let entered_combat = unit_flags::PLAYER_CONTROLLED | unit_flags::IN_COMBAT;
        let in_taxi = entered_combat | unit_flags::TAXI_FLIGHT;
        let left_combat = in_taxi & !unit_flags::IN_COMBAT;
        let landed = left_combat & !unit_flags::TAXI_FLIGHT;
        assert_ne!(landed & unit_flags::PLAYER_CONTROLLED, 0);
    }
}

// ===========================================================================================
//  Content import (dev) — replace the live creature roster from a parsed cmangos slice
// ===========================================================================================

/// Replace the live creature roster with an imported content slice (see `importer/`). Resets live
/// state — deletes every CREATURE `game_world_entity` (never a player) and every `game_creature_spawn`
/// — then loads the supplied spawns. The slow `tick_creatures` respawn pass rebuilds the live
/// entities from these spawns + their templates (loaded separately, via SQL — they have no Timestamp
/// columns), so we never hand-encode the entity row here.
///
/// Spawns go through a reducer rather than SQL because `game_creature_spawn` has `Timestamp` columns
/// (`respawn_at`/`despawn_at`) and SpacetimeDB 2.5 SQL has no Timestamp literal; `ctx.timestamp` here
/// is a valid in-the-past time, so the very next tick builds the entities at once.
///
/// `packed` is the importer payload: rows separated by `;`, fields by `,`, in the order
/// `guid,entry,map_id,x,y,z,orientation,movement_type`. (u64 guids exceed JSON's safe-integer range, so the payload
/// is one delimited string — no per-field JSON number to lose precision.)
///
/// Privileged in production (restrict to the operator/gateway identity); permissive here for dev,
/// matching `provision_account`/`create_character`.
#[reducer]
pub fn import_creature_spawns(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();

    // 1) Reset: drop every creature entity (PLAYER bit absent → never a player) and every spawn.
    let creature_guids: Vec<u64> = entities
        .iter()
        .filter(|e| e.type_mask == constants::type_mask::CREATURE)
        .map(|e| e.guid)
        .collect();
    for g in creature_guids {
        entities.guid().delete(g);
    }
    let spawn_guids: Vec<u64> = spawns.iter().map(|s| s.guid).collect();
    for g in spawn_guids {
        spawns.guid().delete(g);
    }
    // The threat tables are keyed by the creature guids we just deleted — wipe them so a re-import
    // doesn't orphan stale aggro rows (the fresh roster starts with empty threat).
    crate::threat::clear_all(ctx);

    // 2) Load the (first/only) batch. respawn_at = now (in the past by the next tick) so the respawn
    //    pass re-creates each live entity from this spawn + its template immediately; it disarms the
    //    timer as it does. despawn_at starts NOT-ARMED — nothing has died yet.
    if load_spawn_batch(ctx, &packed)? == 0 {
        return Err("import payload was empty".to_string());
    }
    Ok(())
}

/// Append a batch of packed spawns WITHOUT the reset — paired with `import_creature_spawns` (which
/// clears the roster ONCE + loads the first batch), so a whole zone can load across several calls (a
/// single `spacetime call` string arg can't hold ~2k spawns). note: the importer sends batch 0 via
/// `import_creature_spawns` and the remaining batches via this; the respawn tick then builds the
/// entities for every loaded spawn. No reset here — re-calling this alone never wipes the roster.
#[reducer]
pub fn import_creature_spawns_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_spawn_batch(ctx, &packed)?;
    Ok(())
}

/// Parse `packed` (`;`-separated rows of `guid,entry,map,x,y,z,o,mt[,respawn_secs]`) into
/// `game_creature_spawn` rows stamped at `ctx.timestamp`, returning the count loaded. Shared by the
/// clear+load reducer above and the append reducer. The trailing `respawn_secs` field is OPTIONAL (9
/// fields with it, 8 without) so an older importer payload without it still parses — a row
/// missing it defaults to `0` (falls back to the flat `RESPAWN_MICROS` timer, see `CreatureSpawn`'s
/// doc comment).
fn load_spawn_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let spawns = ctx.db.game_creature_spawn();
    let now = ctx.timestamp;
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 8 && f.len() != 9 {
            return Err(format!(
                "import row needs 8 or 9 fields, got {}: {row}",
                f.len()
            ));
        }
        let pu64 = |s: &str| s.parse::<u64>().map_err(|_| format!("bad u64: {s}"));
        let pu32 = |s: &str| s.parse::<u32>().map_err(|_| format!("bad u32: {s}"));
        let pu8 = |s: &str| s.parse::<u8>().map_err(|_| format!("bad u8: {s}"));
        let pf32 = |s: &str| s.parse::<f32>().map_err(|_| format!("bad f32: {s}"));
        let spawn = CreatureSpawn {
            guid: pu64(f[0])?,
            entry: pu32(f[1])?,
            map_id: pu32(f[2])?,
            x: pf32(f[3])?,
            y: pf32(f[4])?,
            z: pf32(f[5])?,
            orientation: pf32(f[6])?,
            // respawn_at STAYS ARMED (`now`): this is how an imported creature first appears —
            // `pass_respawn` sees a due timer with no live entity and builds the entity from this
            // row. It disarms the timer once it has (see `timer_never`), so the row leaves the
            // range scan after one tick instead of sitting in it forever.
            respawn_at: now,
            despawn_at: timer_never(ctx), // not a corpse — nothing to decay yet
            movement_type: pu8(f[7])?,
            respawn_secs: if f.len() == 9 { pu32(f[8])? } else { 0 },
        };
        // try_insert: a duplicate guid within the payload (shouldn't happen — guids are unique per
        // spawn) fails the run cleanly rather than panicking the reducer.
        spawns
            .try_insert(spawn)
            .map_err(|e| format!("duplicate spawn guid {}: {e}", f[0]))?;
        loaded += 1;
    }
    Ok(loaded)
}
