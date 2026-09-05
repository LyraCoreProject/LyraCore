//! Gameobjects — the world's interactive props. A spawned `game_gameobject` is relayed to clients as a
//! GameObject CREATE_OBJECT (gateway); `use_gameobject` (CMSG_GAMEOBJ_USE) dispatches by template type:
//! a CHEST rolls its real `game_gameobject_loot` table (work-item 210) into the shared
//! `game_corpse_loot` table KEYED ON THE GO GUID (so the existing corpse loot window + take path serve
//! it unchanged) — falling back to the legacy single `data0` drop when the chest has no data-driven
//! table (the seed/demo chest) — a GOOBER grants quest credit for a USE_GAMEOBJECT objective, a
//! DOOR/BUTTON toggles open/closed. Every OTHER in-scope cmangos GO type imports
//! (template + spawn) but is INERT — `use` is a benign no-op, not an error (see `go_type::*` +
//! `apply_use_gameobject`'s catch-all arm). A lock with a real opener blocks CHEST, GOOBER,
//! DOOR and BUTTON use until `apply_pick_lock` records it as unlocked. New tables use additive
//! auto-migration. [entity]

use std::collections::HashSet;

use spacetimedb::{reducer, table, ReducerContext, Table, TimeDuration, Timestamp};

use crate::game_corpse_loot;
use crate::game_gameobject_loot; // CHEST data-driven loot table (work-item 210)
use crate::game_player_skill; // GATHER skill-gate reads the gather skill row (accessor trait)
use crate::game_world_entity;
use crate::loot::CorpseLoot;
use crate::nav::game_nav_chunk; // arm_pool's map-fence — neither is wildcard-exported at the crate root
use crate::terrain::game_terrain_chunk;

/// The gameobject types this slice handles (cmangos `GAMEOBJECT_TYPE_*`). Deliberate
/// simplification: only these two plus the synthetic GATHER marker.
pub mod go_type {
    /// Door prop. `use` toggles open/closed (state 0↔1 via [`super::toggle_state`]), byte-identical
    /// shape to the BUTTON toggle below. A lock with a real opener must be opened first.
    /// autoCloseTime (cmangos data2) is NOT modeled — a toggled-open door stays open
    /// until toggled again (no auto-close timer this slice). cmangos type 0 (GAMEOBJECT_TYPE_DOOR).
    pub const DOOR: u8 = 0;
    /// Button/lever prop — cmangos models a lever as a differently-DISPLAYED door; the toggle
    /// semantics are identical to DOOR (same match arm). cmangos type 1 (GAMEOBJECT_TYPE_BUTTON).
    pub const BUTTON: u8 = 1;
    /// Quest-giver prop (Wanted Poster, a corpse that offers/completes a quest, …). Its giver/role
    /// gate lives entirely in `game_gameobject_quest` (see `quest::validate_giver`); `use_gameobject`
    /// still does not dispatch on this type SERVER-SIDE (opening the quest window is presentation, not
    /// a state mutation — the gateway's `CMSG_GAMEOBJ_USE` handler does that dispatch itself,
    /// work-item 041). Re-exported from `lyracore_shared::constants::go_type` (not a local literal) so the
    /// module and gateway copies of this value can never drift. cmangos type 2
    /// (GAMEOBJECT_TYPE_QUESTGIVER).
    pub const QUESTGIVER: u8 = lyracore_shared::constants::go_type::QUESTGIVER;
    /// Loot container — `use` rolls its drop into the loot window. cmangos type 3.
    pub const CHEST: u8 = lyracore_shared::constants::go_type::CHEST;
    /// Spell trap. Relay activation uses its imported spell and cooldown metadata.
    pub const TRAP: u8 = 6;
    /// Quest-use object such as a lever or totem. Unlocked `use` grants USE_GAMEOBJECT quest credit.
    /// This is cmangos type 10.
    pub const GOOBER: u8 = 10;
    /// Gather node (mining vein / herb bush). `use` skill-gates then DIRECT-grants the ore/herb
    /// (skinning-style, NOT the loot window — the gtker loot-window codec is incomplete). cmangos models
    /// these as locked CHESTs; we give them a distinct synthetic type so the gather path is unambiguous
    /// and never collides with the CHEST loot-window path. `data0` = item entry granted; `data1` =
    /// required skill level; `gather_skill_line` = MINING/HERBALISM the use requires.
    pub const GATHER: u8 = 25; // synthetic; vanilla has no type 25 — our gather marker
                               // COLLISION NOTE (work-item 211): real cmangos type 25 IS assigned (GAMEOBJECT_TYPE_FISHINGHOLE) —
                               // it just happens to collide with this synthetic marker. The importer's TYPE-25 COLLISION GUARD
                               // (`importer/src/main.rs::classify_go_type`) is the ONLY place that decides what gets stored as
                               // module type 25: a real FISHINGHOLE row is dropped from import, never stored here as GATHER.
}

/// Pure skill-gate for a gather node (ctx-free, unit-tested): a character may gather iff it has LEARNED
/// the required gather skill (`learned > 0`) AND its level meets the node's `required`. `learned` is the
/// character's current skill in the node's `gather_skill_line` (0 = not learned — no skill row). Mirrors
/// `loot::can_skin`'s decision/test split. Returns the reject reason the use reducer surfaces, else `Ok`.
pub(crate) fn can_gather(learned: u32, required: u32) -> Result<(), String> {
    if learned == 0 {
        return Err("you have not learned this gathering skill".to_string());
    }
    if learned < required {
        return Err("your skill is too low to gather this node".to_string());
    }
    Ok(())
}

/// A gameobject definition (cmangos `gameobject_template` subset). Public + read-only, no Timestamp →
/// SQL-loadable like the other template tables. [static]
#[table(accessor = game_gameobject_template, public)]
pub struct GameObjectTemplate {
    #[primary_key]
    pub entry: u32,
    pub type_id: u8,     // go_type::*
    pub display_id: u32, // visual model (the client resolves it from its own DBC)
    pub name: String,
    pub data0: u32, // CHEST: item entry it drops; GATHER: item entry it grants (deliberately a single drop)
    pub data1: u32, // GATHER: required skill level to gather; CHEST/GOOBER: reserved (lock id / spell)
    // END-APPENDED defaulted column (additive auto-migration, same discipline as trainer.rs
    // `learn_skill_line`): which gather skill a GATHER `use` requires. 0 = not a gather node.
    #[default(0u32)]
    pub gather_skill_line: u32, // 0 = none; else MINING (186) / HERBALISM (182)
    // END-APPENDED (respawn variance). Per-node respawn window in SECONDS (NOT a
    // Timestamp, NOT micros — keeps the SQL seed human-readable; scaled to micros at runtime). 0 ⇒ fall
    // back to RESPAWN_WINDOW_MICROS (3 min), so every EXISTING node is byte-identical (default-migrated 0).
    #[default(0u32)]
    pub respawn_secs: u32,
    // END-APPENDED (skill-up difficulty). The GATHER node's "gray" difficulty ceiling:
    // at/above this skill a gather no longer skills up. 0 ⇒ the always-skill sentinel (orange=data1,
    // gray=0 → skillup_chance_bp returns 10000), so existing nodes deterministically +1 (byte-identical).
    // The "orange" floor is the node's existing `data1` (required skill). Real tables (gray>0) are DEFERRED.
    #[default(0u32)]
    pub gather_gray: u32,
    // END-APPENDED. Uses additive auto-migration, with the same discipline as the columns above.
    // The cmangos lockId is hand-synced positionally into the gateway's
    // `game_object_template_type.rs` binding (decode-only — the gateway never READS this field, only
    // decodes past it). CHEST/GOOBER source it from the dump's `data0`; DOOR/BUTTON from `data1` (see
    // `importer/src/dbc.rs`'s `go_template_row`). `game_lock` holds the Lock.dbc requirements for this
    // id. A real opener gates CHEST, GOOBER, DOOR and BUTTON use. 0 = unlocked or not carried for this
    // type. Existing rows default to 0.
    #[default(0u32)]
    pub lock_id: u32,
    // END-APPENDED. The cmangos `gameobject_template.size` the client renders this prop at
    // (OBJECT_FIELD_SCALE_X). 0 = no size stored, which the gateway renders as 1.0. Read by the
    // gateway, so it is hand-synced into `game_object_template_type.rs` (unlike `lock_id` above).
    #[default(0f32)]
    pub size: f32,
}

/// Source trap fields needed by the relay-use authority. Module only. [static]
#[table(accessor = game_gameobject_trap)]
pub struct GameObjectTrap {
    #[primary_key]
    pub entry: u32,
    pub spell_id: u32,
    /// Zero selects the source default of four seconds.
    pub cooldown_secs: u32,
}

/// Per-spawn trap cooldown after a successful relay activation. Module only. [entity]
#[table(
    accessor = game_gameobject_trap_cooldown,
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct GameObjectTrapCooldown {
    #[primary_key]
    pub go_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub ready_at: Timestamp,
}

/// Lock.dbc data in `game_lock`. `apply_use_gameobject` reads this table to gate supported locked
/// types. One Lock.dbc row packs up to 8 PARALLEL alternative ways to open it, such as the right key
/// or enough Lockpicking. Each non-`LockType::None` alternative un-packs into its own row here,
/// `index`-numbered 0..8 (see `importer/src/dbc.rs::lock_sql` — a lock with 1 real requirement emits
/// exactly 1 row). Look up by [`GameObjectTemplate::lock_id`] via `by_lock`. `kind`: 1 = ITEM (open
/// with the key item `property` names) — 2 = SKILL (need SkillLine `property` at >= `required_skill`;
/// Lockpicking, SkillLine 633, is itself just another skill line, so a lockpicking-only lock is a
/// plain `kind==2` row keyed on 633). Public + read-only, no Timestamp → SQL-loadable clear+reload
/// like `game_creature_family`/`game_skill_line` — no reducer needed. UNSUBSCRIBED (no gateway
/// binding files, no gateway subscription) — the gateway never reads this table, following the
/// `game_creature_family` precedent for a module-only reference table. [static]
#[table(accessor = game_lock, public, index(accessor = by_lock, btree(columns = [lock_id])))]
pub struct GameLock {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub lock_id: u32,        // == GameObjectTemplate.lock_id (the Lock.dbc row id)
    pub index: u8,           // which of the lock's up-to-8 parallel alternatives this is (0..8)
    pub kind: u8, // 1 = item (property is an item entry); 2 = skill (property is a SkillLine)
    pub property: u32, // kind 1: required key item entry; kind 2: required SkillLine id
    pub required_skill: u32, // kind 2: the skill VALUE needed; kind 1: unused (Lock.dbc's own 0/garbage)
}

/// `game_lock.kind` values (work-item 119): the two opener kinds [`GameLock`] models.
pub(crate) const LOCK_KIND_ITEM: u8 = 1; // property = the key item entry the opener must HOLD
pub(crate) const LOCK_KIND_SKILL: u8 = 2; // property = a SkillLine id (Lockpicking 633 / Herbalism 182 / Mining 186)

/// Is a `game_lock` row a REAL opener — a requirement a player can be gated on AND can satisfy
/// (ctx-free, unit-tested)? A SKILL row (kind 2) counts ONLY when it names a real SkillLine
/// (`property != 0`): the importer stores an UNMAPPED `LockType.dbc` id (LOCKTYPE_OPEN / blasting /
/// disarm-trap — "open by hand or a non-lockpick method") as `property 0` (see `importer/src/dbc.rs`'s
/// `LOCKTYPE_TO_SKILL_LINE` — an id outside {1,2,3} resolves to SkillLine 0). Such a slot is NOT
/// lockpicking (Pick Lock can't satisfy it) AND must NOT lock a hand-openable chest — those opened
/// freely pre-119 and still do. An ITEM row (kind 1) always names a key item entry. Any other kind →
/// not a lock this slice models (never gates the use path, never picked).
pub(crate) fn is_real_lock_opener(kind: u8, property: u32) -> bool {
    (kind == LOCK_KIND_SKILL && property != 0) || kind == LOCK_KIND_ITEM
}

/// A GameObject whose lock has been PICKED open. The CHEST/GOOBER/DOOR use-gate reads
/// this by GO guid to admit a use that a lock would refuse. Module-only + UNSUBSCRIBED (no gateway
/// binding files, no gateway subscription — the `game_lock` precedent above); `public` + no Timestamp so
/// a verify can inspect it via `spacetime sql`. One row per unlocked GO guid; never re-locked this slice
/// (matches vanilla — a picked chest stays open). A row for a despawned GO is a harmless orphan (a live
/// HIGHGUID_GAMEOBJECT guid is minted per spawn; `debug_spawn_gameobject` clears the row for its DERIVED
/// guid so a re-spawn is deterministically locked again). New table → additive auto-migration. [entity]
#[table(accessor = game_gameobject_unlocked, public)]
pub struct GameObjectUnlocked {
    #[primary_key]
    pub go_guid: u64,
}

/// A spawned gameobject in the world. Public (broadcast like creatures/corpses); relayed to clients as
/// a GameObject CREATE_OBJECT. `state`: 0 = ready (chest closed), 1 = used (chest looted). [entity]
#[table(
    accessor = game_gameobject,
    public,
    index(accessor = by_map, btree(columns = [map_id])),
    index(accessor = by_grid, btree(columns = [map_id, grid_x, grid_y])),
    // Perf catalog 1.21: the sense-tick respawn pass used to full-scan this table (~25-30k rows after a
    // full-world GO import) for a minutes-scale due check. `respawn_at_micros == 0` is the not-armed
    // sentinel on every live row, so a `1..=now` range visits only DUE nodes — the same trick
    // `game_aura.by_next_tick` uses.
    index(accessor = by_respawn_at, btree(columns = [respawn_at_micros])),
    // The AOI cell index — exactly 3 columns, all matched by equality terms, which is the
    // only shape SpacetimeDB 2.7.1's subscription planner can serve (see the `cell` column).
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct GameObject {
    #[primary_key]
    pub guid: u64,
    pub template_entry: u32, // -> GameObjectTemplate.entry
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub state: u8,
    pub created_at: Timestamp,
    /// When a DEPLETED (state==1) gather node becomes gatherable again, as micros since the unix epoch
    /// (NOT a Timestamp — adding one to this live, populated table would abort the publish; teleport-core
    /// lesson). 0 = no pending respawn (a ready node, or a non-gather GO). Set at gather-deplete from
    /// ctx.timestamp + RESPAWN_WINDOW_MICROS; cleared (back to 0) when the respawn pass flips state 0.
    #[default(0u64)]
    pub respawn_at_micros: u64,
    /// Which instance this GO belongs to (work-item 190 slice 2). 0 = open world — EVERY static
    /// imported/seeded/pool/debug row stays 0; only the per-instance COPIES `instance::create_instance`
    /// makes of a dungeon's DOOR/BUTTON/CHEST/GOOBER rows carry a live instance id (GO_COPY_BAND
    /// guids), and the instance reap deletes them. END-appended + `#[default(0u64)]` (danger-zones §2
    /// additive migration). GATEWAY-SUBSCRIBED table → the binding
    /// (`gateway/src/stdb/bindings/game_object_type.rs`) and the `schema_parity.rs` manifest are
    /// hand-synced in the SAME change (playbook failure-mode #1).
    #[default(0u64)]
    pub instance_id: u64,
    /// AOI grid cell (246): stamped from (x, y) at insert — GOs are static, no re-stamp. The
    /// per-player GO subscription scopes to the viewer's grid box exactly like game_world_entity
    /// (the un-scoped table made the client draw all ~4.6k zone GOs at once — the felt frame
    /// hitch). END-appended + defaulted → auto-migrates; backfill via debug_backfill_go_grid.
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
    /// `(grid_x, grid_y)` packed into ONE indexed value — the AOI subscription's cell key.
    ///
    /// SpacetimeDB 2.7.1's subscription planner can only serve a query from an index when EVERY
    /// column of that index is matched by an equality term, and it skips any index with more than 3
    /// columns outright (`MAX_EXACT_INDEX_COLS`); range predicates are never index-served at all
    /// (`IndexProbe::Range` — "we currently never construct this variant") and an `OR` is evaluated
    /// row-by-row. So a `grid_x BETWEEN .. AND grid_y BETWEEN ..` box degrades to a full partition
    /// scan — 1.1 BILLION rows examined on `game_gameobject` in a 445-player measurement. Folding the
    /// two grid columns into one makes `by_cell` a 3-column all-equality index, which the planner CAN
    /// serve, and the AOI box becomes 25 point probes instead of a scan.
    ///
    /// ALWAYS written from `spatial::grid_cell_id(grid_x, grid_y)` in the SAME statement that writes
    /// `grid_x`/`grid_y` — a stale value here does not merely slow a query down, it puts the row in
    /// the wrong cell and shows players the wrong world. `module/src/tripwires.rs::grid_cell_tripwire`
    /// is the enforcement.
    ///
    /// `#[default(0i64)]` (typed — an i64 column needs an explicitly-typed literal) + END-appended so
    /// `publish` auto-migrates. **The default is cell (0, 0), not "unset"**, so every pre-existing row
    /// is mis-addressed until `backfill_cell_ids` re-stamps it — and gameobjects are STATIC, so
    /// nothing ever re-stamps them on its own. That backfill is a REQUIRED post-publish step.
    #[default(0i64)]
    pub cell: i64,
    /// The cmangos spawn quaternion (`gameobject.rotation0..3`). The client renders a
    /// static prop's ORIENTATION from this 4-float quaternion (`GAMEOBJECT_ROTATION`, wire index 10),
    /// not from `orientation` alone: rot2/rot3 carry yaw, rot0/rot1 carry the terrain pitch/roll a
    /// bench flush against a sloped wall needs. `orientation` above still drives movement-facing math
    /// and the CREATE's `MovementBlock_UpdateFlag_Living` heading; these four are ADDITIONAL, wire-only.
    /// END-appended + `#[default(0.0f32)]` (additive auto-migration; f32 defaults fine per danger-zones
    /// §1.2, unlike a `String` end-append). All-zero (every pre-migration row, and any hand-seeded
    /// fixture that never set these) is the codec's signal to DERIVE a yaw-only quaternion from
    /// `orientation` instead of sending a degenerate zero rotation — see
    /// `gateway/src/codec/gameobject.rs::build_gameobject_rotation_values`.
    /// ⚠ Field spelling: SpacetimeDB normalizes column names to snake_case, so a Rust field
    /// `rotation0` is STORED as column `rotation_0`. The import shipped it as `rotation0..3` (stored
    /// `rotation_0..3`); a parallel session saw the normalized live columns, took them for orphans,
    /// and re-declared them as `rotation_0..3` — same four columns, two spellings, briefly published
    /// as a duplicate-name type that broke row decode. The merge collapsed everything onto the
    /// stored spelling. Keep field name == column name here; never re-introduce `rotation0`.
    #[default(0.0f32)]
    pub rotation_0: f32,
    #[default(0.0f32)]
    pub rotation_1: f32,
    #[default(0.0f32)]
    pub rotation_2: f32,
    #[default(0.0f32)]
    pub rotation_3: f32,
}

// ===========================================================================================
//  Gathering POOL model (cmangos pool_template + pool_gameobject) — MODEL A (delete/insert)
// ===========================================================================================
//
// A POOL has many spawn POINTS and a fixed MAX_ACTIVE count. On a pooled node being gathered + its
// respawn firing, the pool RE-ROLLS: the gathered point DEACTIVATES (its game_gameobject row is
// DELETEd → gateway relays SMSG_DESTROY_OBJECT) and a DIFFERENT inactive point ACTIVATES (a fresh
// game_gameobject row is INSERTed → gateway relays CREATE_OBJECT), so the active set ROTATES across
// the zone — nodes roam; you cannot camp one spot. Higher-tier falls out FOR FREE: a higher-tier
// template is just a low-WEIGHT member, so the weighted pick makes it rare (NO tier-reroll logic).
//
// MODEL A is the lazier choice because the gateway already relays a live GO INSERT (on_go_insert,
// subscriptions.rs:516) symmetric to its DELETE relay (on_go_delete, :525). All pool state lives on
// the TEMPLATE + these two NEW module-only tables; the SPAWN row (GameObject) is UNCHANGED, so the
// gateway's hand-matched BSATN-positional binding (game_object_type.rs) stays byte-identical — zero
// gateway edits. Model B (a `dormant` column on the spawn row) would force that hand-match + a new
// on_go_update closure (the `subscriptions.rs:513` no-on_update note). The reroll's −1 delete / +1
// insert is exactly the path import_gameobjects already exercises (clear → reload).

/// A gathering pool HEADER (cmangos `pool_template` subset). Public + SQL-loadable (no Timestamp) —
/// the gateway never subscribes to or reads this; it's module-only pool state. `max_active` member
/// POINTS are active (have a `game_gameobject` row) at any instant; the reroll holds that invariant.
#[table(accessor = game_gameobject_pool, public)]
pub struct GameObjectPool {
    #[primary_key]
    pub pool_id: u32,
    /// Exactly this many member points are active (have a live `game_gameobject` row) at any time.
    pub max_active: u32,
    // END-APPENDED defaulted column (additive auto-migration, same discipline as the GameObjectTemplate
    // defaulted cols). The POOL's KIND, which decides the gather DEPLETE timing (NOT `max_active` — that
    // is NOT a safe signal: `pool_max_active(3)==pool_max_active(4)==1`, so a small roaming pool collides
    // with the `max_active==1` tier signal). false = a ROAMING pool (the importer's single-tier spatial
    // pools): gather → INSTANT `reroll_pool` + return, so the node vanishes here and a sibling appears
    // elsewhere (wander-to-find). true = an IN-PLACE TIER point:
    // `max_active==1`, members CO-LOCATED (identical x,y,z) but of differing TIERS (e.g. Copper w85 / Tin
    // w15); gather flips state + arms the respawn timer (exactly the standalone path) and
    // `pass_gameobject_respawn` weighted-RE-ROLLS the tier IN PLACE (co-located ⇒ no wander, repeats
    // allowed ⇒ the common tier recurs). Defaults false so EVERY existing/importer pool is byte-identical.
    #[default(false)]
    pub in_place: bool,
}

/// A pool member POINT (cmangos `pool_gameobject` subset). A *potential* spawn location: a point is
/// ACTIVE iff a `game_gameobject` row exists at `pool_point_guid(point_id)`, INACTIVE otherwise — the
/// active/inactive distinction is the EXISTENCE OF A ROW, not a column (so the spawn row is untouched).
/// `template_entry` chooses which GO spawns here (a higher-tier template = a rarer member); `weight` is
/// the relative selection weight (0 = never auto-selected). Public + SQL-loadable (no Timestamp).
#[table(accessor = game_gameobject_pool_member, public,
        index(accessor = by_pool, btree(columns = [pool_id])))]
pub struct GameObjectPoolMember {
    #[primary_key]
    #[auto_inc]
    pub point_id: u64,
    pub pool_id: u32,
    pub template_entry: u32, // the GO template to spawn at this point (higher-tier template = rarer member)
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub weight: u32, // relative selection weight; 0 = never auto-selected
}

/// Bit 47 of the low-48 marks a guid as a POOL point. cmangos `gameobject.guid` db_guids and the GO
/// seed ordinals are small integers (all well under 2^47), so this tag bit is ALWAYS free for them —
/// pool points live in a DISJOINT sub-namespace. Without it, a pool `point_id` (auto_inc from 1) collided
/// with a standalone GO at the same low-48 (seeded Copper Vein = `GO_HIGH | 3` == pool point 3), so arming
/// a pool silently deleted/clobbered real nodes and a standalone respawn misrouted into a foreign pool.
pub const POOL_TAG: u64 = 1 << 47;

/// Derive a pool point's `game_gameobject` guid from its `point_id` — HIGHGUID_GAMEOBJECT (0xF110 in bits
/// 48..63, the same scheme as `debug_spawn_gameobject` and the GO seed) | [`POOL_TAG`] | point_id. The tag
/// bit keeps the point's live row findable/deletable by the reroll while staying disjoint from standalone
/// GO guids (see [`pool_point_id_of`] — the standalone-vs-pooled discriminator).
pub fn pool_point_guid(point_id: u64) -> u64 {
    (0xF110u64 << 48) | POOL_TAG | (point_id & 0x7FFF_FFFF_FFFF)
}

/// Recover the `point_id` from a guid IFF it is a tagged pool point (HIGHGUID_GAMEOBJECT + [`POOL_TAG`]).
/// `None` for a standalone GO (tag bit clear) → the respawn pass never even queries the member table for it
/// and always takes the in-place branch. A STRUCTURAL test, collision-proof — not a member-row lookup.
pub fn pool_point_id_of(guid: u64) -> Option<u64> {
    if (guid >> 48) == 0xF110 && (guid & POOL_TAG) != 0 {
        Some(guid & 0x7FFF_FFFF_FFFF)
    } else {
        None
    }
}

/// Pure WEIGHTED-PICK over a pool's inactive candidates (ctx-free, unit-tested): given each candidate's
/// `weight` and a `roll` in 0..total (total = Σweights), return the index of the picked candidate via
/// cumulative-weight bands (`roll < cumulative` → that band) — the same shape as `loot::group_pick`,
/// but over an arbitrary total (not basis-points), so a higher-weight member occupies a wider band.
/// `roll` is always < total at the call site (`ctx.random::<u32>() % total`), so this never returns the
/// past-the-end `None` band; isolated so the deterministic "seeded index → chosen member" is testable.
pub(crate) fn weighted_pick(weights: &[u32], roll: u32) -> Option<usize> {
    let mut cum = 0u32;
    for (i, &w) in weights.iter().enumerate() {
        cum = cum.saturating_add(w);
        if roll < cum {
            return Some(i);
        }
    }
    None
}

/// RE-ROLL a pool (MODEL A) after one of its points was gathered and its respawn fired. Holds the
/// MAX_ACTIVE invariant as a strict −1 / +1: DEACTIVATE the gathered point (DELETE its row, the −1)
/// then ACTIVATE one currently-INACTIVE weighted-chosen point (INSERT a row, the +1) → the active
/// count is CONSERVED. The gateway relays the delete as SMSG_DESTROY_OBJECT and the insert as
/// CREATE_OBJECT, so in-range clients see the node ROAM. Collect-then-mutate (snapshot candidates
/// before any insert) so two due rerolls in one pass never race the same slot and we never write the
/// `game_gameobject` table while iterating it.
///
/// Edge case (no inactive member): if EVERY member is already active (`max_active >= eligible point
/// count`), in-place respawn the gathered point instead of dropping below `max_active` — re-insert it
/// at its own guid so the active count never dips. A misconfigured pool (every member `weight == 0`)
/// has no candidate at all; the gathered point stays deleted (the safe fail), which is not an invariant
/// break of a *correctly configured* pool.
pub(crate) fn reroll_pool(ctx: &ReducerContext, pool_id: u32, gathered_guid: u64) {
    let gos = ctx.db.game_gameobject();
    let members = ctx.db.game_gameobject_pool_member();
    // 1. DEACTIVATE the gathered point (the −1). The just-freed point is itself eligible again below.
    gos.guid().delete(gathered_guid);
    // 2. Candidate set = pool members with weight > 0 that are currently INACTIVE (no live row at their
    //    derived guid). The just-deleted gathered point is now inactive → eligible to be re-picked (a
    //    small pool can legitimately re-roll the same spot; MAX_ACTIVE still holds).
    //    LOAD-BEARING INVARIANT (tier-variety): the delete-THEN-collect order RE-INCLUDES the freed point
    //    in the candidate set. For an in_place `max_active==1` tier pool that is the WHOLE mechanism —
    //    every member is inactive after the delete, so the weighted pick spans ALL tiers and the common
    //    tier CAN repeat. Do NOT "optimize" this to exclude the gathered point: that would silently turn a
    //    co-located {Copper,Tin} tier pool into a DETERMINISTIC Copper↔Tin FLIP (guaranteed Tin every other
    //    gather), destroying the ~85/15 weighting.
    let candidates: Vec<GameObjectPoolMember> = members
        .by_pool()
        .filter(&pool_id)
        .filter(|m| m.weight > 0 && gos.guid().find(pool_point_guid(m.point_id)).is_none())
        .collect();
    if candidates.is_empty() {
        // Misconfigured pool (all members weight==0) → no candidate. Leave the gathered point deleted;
        // a correctly configured pool always has ≥1 candidate (the freed point itself), so this branch
        // is only the degenerate fail. (The "all active" no-rotate case lands in step 3 via re-pick.)
        return;
    }
    // 3. WEIGHTED pick over the inactive candidates (reuse the loot weighting shape). `total > 0` here
    //    because every candidate has weight > 0, so the `% total` is safe and `weighted_pick` always hits.
    let weights: Vec<u32> = candidates.iter().map(|m| m.weight).collect();
    let total: u32 = weights.iter().sum();
    let roll = ctx.random::<u32>() % total;
    let chosen =
        &candidates[weighted_pick(&weights, roll).expect("roll < total ⇒ a band always hits")];
    // 4. ACTIVATE the chosen point (the +1) — INSERT a fresh ready row at its derived guid (gateway
    //    relays CREATE_OBJECT → it spawns live). state 0 = ready, no pending respawn.
    activate_point(ctx, chosen);
}

/// ACTIVATE a pool point — insert its READY `game_gameobject` row at the derived guid. Shared by
/// `arm_pool` (initial fill) and `reroll_pool` (rotation). Idempotent-by-guid: the caller guarantees
/// the point is currently inactive (no row at its guid), so this is a plain insert.
fn activate_point(ctx: &ReducerContext, m: &GameObjectPoolMember) {
    ctx.db.game_gameobject().insert(GameObject {
        guid: pool_point_guid(m.point_id),
        template_entry: m.template_entry,
        map_id: m.map_id,
        x: m.x,
        y: m.y,
        z: m.z,
        orientation: m.orientation,
        state: 0,
        created_at: ctx.timestamp,
        respawn_at_micros: 0,
        instance_id: 0, // pools are open-world machinery (190 slice 2: never instanced),
        grid_x: lyracore_shared::spatial::grid_cell(m.x, m.y).0,
        grid_y: lyracore_shared::spatial::grid_cell(m.x, m.y).1,
        cell: lyracore_shared::spatial::cell_id_at(m.x, m.y),
        // Pool members carry no spawn quaternion (cmangos pool_gameobject has none either) — the
        // codec derives a yaw-only quaternion from `orientation` for the all-zero case.
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    });
}

/// Every map id this database currently holds REAL imported spatial content for, read off
/// `game_terrain_chunk`/`game_nav_chunk` — the same ground-truth signal the fresh-shard guard
/// uses in `importer/scripts/import-world.sh`'s `db_imported_probe`. `init` seeds neither table (only a real
/// `importer --apply` run ever writes to them; see both files' module doc comments), so an EMPTY
/// result here means this database has never received a real import — everything it holds is `init`'s
/// own demo/test scaffolding.
fn imported_terrain_maps(ctx: &ReducerContext) -> HashSet<u32> {
    ctx.db
        .game_terrain_chunk()
        .iter()
        .map(|c| c.map_id)
        .chain(ctx.db.game_nav_chunk().iter().map(|c| c.map_id))
        .collect()
}

/// Whether a pool member's map is one this database's real imported content covers (pure, unit-tested
/// — see `imported_terrain_maps` for how `imported_maps` is derived from live tables). An EMPTY
/// `imported_maps` means nothing has been imported here yet, so `init`'s own publish-time `arm_pool`
/// call (and a bare-dev-DB `debug_setup_gather_pool`) must still arm every pool — this admits
/// everything in that case. Once ANY map has real terrain/nav content, a member on a DIFFERENT map is
/// foreign: refusing it is what stops `arm_all_pools` resurrecting another continent's fixture after
/// that continent's own import already wiped it (the false REGRESSION a clean map-1 import
/// used to end with, because the demo tier-pool's map-0 Northshire point got re-armed regardless).
pub(crate) fn pool_member_map_is_live(member_map: u32, imported_maps: &HashSet<u32>) -> bool {
    imported_maps.is_empty() || imported_maps.contains(&member_map)
}

/// The exact candidate filter `arm_pool` splices into its `&&` chain each iteration — weight>0, on a
/// live map (`pool_member_map_is_live`), and not already an active point (`active_point_guids`, the
/// pool's currently-armed point guids at `pool_point_guid`). Pulled out to a pure fn (no
/// `ReducerContext`, `members`/`active_point_guids` are plain owned/borrowed values a unit test can
/// construct directly) SPECIFICALLY so this splice point — not just the isolated
/// `pool_member_map_is_live` predicate — has an automated mutation-testing guard (review: the
/// original review disclosed this wiring as unreachable without a live `ReducerContext`; it isn't —
/// `arm_pool` below now calls this directly, so mutating IT mutates the real production code path).
pub(crate) fn eligible_pool_candidates(
    members: Vec<GameObjectPoolMember>,
    active_point_guids: &HashSet<u64>,
    imported_maps: &HashSet<u32>,
) -> Vec<GameObjectPoolMember> {
    members
        .into_iter()
        .filter(|m| {
            m.weight > 0
                && pool_member_map_is_live(m.map_id, imported_maps)
                && !active_point_guids.contains(&pool_point_guid(m.point_id))
        })
        .collect()
}

/// ARM a pool to exactly `max_active` live rows: clear any existing live rows at this pool's points,
/// then activate `max_active` weighted-distinct INACTIVE points (the only place the active count is
/// SET; every later reroll conserves it). If the pool has fewer eligible (weight>0) points than
/// `max_active`, all eligible points activate (the achievable max) — never duplicates a point.
pub(crate) fn arm_pool(ctx: &ReducerContext, pool_id: u32) {
    let gos = ctx.db.game_gameobject();
    let members = ctx.db.game_gameobject_pool_member();
    let imported_maps = imported_terrain_maps(ctx);
    let max_active = ctx
        .db
        .game_gameobject_pool()
        .pool_id()
        .find(pool_id)
        .map(|p| p.max_active)
        .unwrap_or(0);
    // Clear any prior live rows at THIS pool's points (idempotent arming — a re-setup re-fills cleanly).
    for m in members.by_pool().filter(&pool_id) {
        gos.guid().delete(pool_point_guid(m.point_id));
    }
    // Activate max_active weighted-distinct points: re-query the inactive eligible set each iteration (a
    // chosen point becomes active → excluded next iteration via `active_point_guids` → no point activates
    // twice). Re-querying (not a cloned snapshot) also keeps the table row type Clone-free.
    for _ in 0..max_active {
        let pool_members: Vec<GameObjectPoolMember> = members.by_pool().filter(&pool_id).collect();
        let active_point_guids: HashSet<u64> = pool_members
            .iter()
            .map(|m| pool_point_guid(m.point_id))
            .filter(|guid| gos.guid().find(guid).is_some())
            .collect();
        let candidates =
            eligible_pool_candidates(pool_members, &active_point_guids, &imported_maps);
        if candidates.is_empty() {
            break; // fewer eligible points than max_active → activate all there are
        }
        let weights: Vec<u32> = candidates.iter().map(|m| m.weight).collect();
        let total: u32 = weights.iter().sum();
        let roll = ctx.random::<u32>() % total;
        let chosen =
            &candidates[weighted_pick(&weights, roll).expect("roll < total ⇒ a band always hits")];
        activate_point(ctx, chosen);
    }
}

/// ARM every loaded pool to its `max_active` live rows — the import-time fill. `seed::init` does NOT
/// re-run on a plain (auto-migrate) publish, and the importer writes the pool HEADER + MEMBER rows via
/// SQL but no live `game_gameobject` rows (those carry a Timestamp), so something must turn members into
/// active spawns once the SQL load lands. The ETL calls this after the load (importer/scripts/import-world.sh),
/// exactly like the creature-tick rearm folded into `debug_repair_after_publish`. Idempotent: `arm_pool` clears each pool's prior live rows
/// first, so re-running it never drifts the count. Operator-gated (a world-open privileged reducer).
/// Collect the ids first (house style — though `arm_pool` mutates `game_gameobject`, not the pool header).
#[reducer]
pub fn arm_all_pools(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let pool_ids: Vec<u32> = ctx
        .db
        .game_gameobject_pool()
        .iter()
        .map(|p| p.pool_id)
        .collect();
    for pid in pool_ids {
        arm_pool(ctx, pid);
    }
    Ok(())
}

/// Max distance to use a gameobject — reuse the loot range (10 yd)²; the client walks into range itself
/// before sending CMSG_GAMEOBJ_USE, so this only rejects clearly-out-of-range abuse (mirrors loot/vendor).
const USE_RANGE_SQ: f32 = crate::loot::LOOT_RANGE_SQ;

/// Gather-node respawn window FALLBACK: a depleted GATHER node with `respawn_secs == 0` becomes gatherable
/// again this long after it's gathered. Per-template variance now overrides it (`respawn_secs`); this is
/// the default every existing node uses (so they're byte-identical). cmangos respawn RANGES are DEFERRED.
const RESPAWN_WINDOW_MICROS: u64 = 180_000_000; // 3 min

/// Pure respawn-window lookup (ctx-free, unit-tested): the micros a depleted node stays down, from its
/// template's `respawn_secs`. 0 ⇒ the `RESPAWN_WINDOW_MICROS` fallback (existing nodes, byte-identical);
/// else the per-template seconds scaled to micros. Isolated so the fallback-vs-scaled choice is testable.
pub(crate) fn respawn_window_micros(respawn_secs: u32) -> u64 {
    if respawn_secs == 0 {
        RESPAWN_WINDOW_MICROS
    } else {
        respawn_secs as u64 * 1_000_000
    }
}

/// Pure respawn-due predicate (ctx-free, unit-tested): a gameobject is due to respawn iff it is DEPLETED
/// (state==1), has an ARMED timer (respawn_at_micros != 0), and that armed time has elapsed (now >=
/// respawn_at_micros). A never-armed node (respawn_at_micros==0) and a not-yet-due node both return false.
/// This is exactly the filter the respawn pass branches on; isolated so the timer logic is testable.
pub(crate) fn respawn_due(state: u8, respawn_at_micros: u64, now: u64) -> bool {
    state == 1 && respawn_at_micros != 0 && now >= respawn_at_micros
}

/// Does the lock `lock_id` require opening before a CHEST/GOOBER/DOOR use? True iff `game_lock`
/// holds a REAL opener row for it, either a skill line to meet or a key item. A lock
/// with ONLY degenerate `property==0` slots (unmapped LockTypes = open-by-hand) is NOT gated: it opened
/// freely pre-119 and still does, so widening the import's lock coverage never bricks a hand-open chest.
fn lock_requires_opening(ctx: &ReducerContext, lock_id: u32) -> bool {
    ctx.db
        .game_lock()
        .by_lock()
        .filter(&lock_id)
        .any(|r| is_real_lock_opener(r.kind, r.property))
}

/// Is the GO `go_guid` with template `lock_id` locked shut for the use-gate? True iff it
/// carries a lock_id with a real opener AND no `game_gameobject_unlocked` row exists yet. The shared
/// gate for the CHEST, GOOBER and DOOR/BUTTON arms. GATHER and QUESTGIVER carry lock_id 0.
fn locked_shut(ctx: &ReducerContext, go_guid: u64, lock_id: u32) -> bool {
    lock_id != 0
        && lock_requires_opening(ctx, lock_id)
        && ctx
            .db
            .game_gameobject_unlocked()
            .go_guid()
            .find(go_guid)
            .is_none()
}

/// The GO target-acquisition gate shared by [`apply_pick_lock`] and [`apply_use_gameobject`]:
/// resolve the GameObject + its template, resolve the caster's live entity, and confirm they
/// share a map+instance and are within [`USE_RANGE_SQ`]. Every error string below is preserved verbatim
/// from both call sites (the two ~30-line blocks were byte-identical before this extraction).
fn usable_go(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(GameObject, GameObjectTemplate), String> {
    let go = ctx
        .db
        .game_gameobject()
        .guid()
        .find(go_guid)
        .ok_or_else(|| "no such gameobject".to_string())?;
    let player = crate::helpers::live_entity(ctx, caster_guid)
        .map_err(|_| "user not in world".to_string())?;
    if player.map_id != go.map_id {
        return Err("gameobject on another map".to_string());
    }
    if player.instance_id != go.instance_id {
        return Err("gameobject in another instance".to_string());
    }
    let (dx, dy, dz) = (go.x - player.x, go.y - player.y, go.z - player.z);
    if dx * dx + dy * dy + dz * dz > USE_RANGE_SQ {
        return Err("gameobject out of range".to_string());
    }
    let tmpl = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(go.template_entry)
        .ok_or_else(|| format!("no gameobject template {}", go.template_entry))?;
    Ok((go, tmpl))
}

/// Pick the lock on a locked GameObject (work-item 119 — Pick Lock 1804, gateway-intercepted by the
/// E_OPEN_LOCK effect kind). Gated same-map/instance + range like [`apply_use_gameobject`] (the caster
/// walked up to click it). Reads the GO template's `lock_id`, then scans `game_lock` for a SATISFIABLE
/// real opener — a SKILL row (kind 2, `property != 0`) the caster's `game_player_skill(property)` meets,
/// or an ITEM row (kind 1) whose key item the caster holds. On success: record the GO unlocked (the
/// use-gate then admits it) + climb the satisfied skill line one step (`gain_profession_skill` — a
/// Lockpicking lock skills up on use). Emits NO game_spell_cast_event — the gateway sends the
/// START/OK/GO handshake itself (the enchant/fish precedent). [entity]
pub(crate) fn apply_pick_lock(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    let (_go, tmpl) = usable_go(ctx, caster_guid, go_guid)?;
    if tmpl.lock_id == 0 {
        return Err("it is not locked".to_string());
    }
    // Idempotent: re-casting Pick Lock on an already-open lock is a no-op success, not an error.
    if ctx
        .db
        .game_gameobject_unlocked()
        .go_guid()
        .find(go_guid)
        .is_some()
    {
        return Ok(());
    }
    // Scan the lock's up-to-8 parallel openers for the FIRST the caster satisfies. `climb` carries the
    // satisfied SKILL line (Some) to step up on success; an ITEM opener climbs nothing (None).
    let mut satisfied = false;
    let mut climb: Option<(u32, u32)> = None; // (skill_line, required_skill)
    for row in ctx.db.game_lock().by_lock().filter(&tmpl.lock_id) {
        if !is_real_lock_opener(row.kind, row.property) {
            continue; // degenerate property==0 slot / unmodeled kind — Pick Lock can't use it
        }
        if row.kind == LOCK_KIND_SKILL {
            let current = ctx
                .db
                .game_player_skill()
                .by_character()
                .filter(&caster_guid)
                .find(|s| s.skill_line == row.property)
                .map(|s| s.current as u32)
                .unwrap_or(0);
            if current >= row.required_skill {
                satisfied = true;
                climb = Some((row.property, row.required_skill));
                break;
            }
        } else if row.kind == LOCK_KIND_ITEM
            && crate::items::item_count(ctx, caster_guid, row.property) > 0
        {
            satisfied = true;
            break;
        }
    }
    if !satisfied {
        return Err("your Lockpicking is too low".to_string());
    }
    ctx.db
        .game_gameobject_unlocked()
        .insert(GameObjectUnlocked { go_guid });
    // Skill-up the satisfied SKILL line one step: orange = the required floor (always skills below it),
    // gray = 0 (the always-skill sentinel → a deterministic +1 each pick, like a default-band gather
    // node). A no-op when the line has no row (an item opener, or an un-learned line). [entity]
    if let Some((line, required)) = climb {
        crate::skill::gain_profession_skill(ctx, caster_guid, line, required, 0);
    }
    Ok(())
}

/// Shared use-a-gameobject core (player + debug paths). Gates same-map + range, then dispatches by
/// template type. CHEST → roll its single `data0` drop into `game_corpse_loot` keyed on the GO guid,
/// ONCE (state flips 0→1 so a re-use can't re-roll), so the existing loot window/take path serves it.
/// GOOBER requires its opener, then grants USE_GAMEOBJECT quest credit. Other types are not usable
/// yet. [entity]
pub(crate) fn apply_use_gameobject(
    ctx: &ReducerContext,
    caster_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    // Map + instance gated (190 slice 2 — GO rows carry `instance_id` now): a player may only use
    // a GO in THEIR OWN instance (dungeon doors/chests are per-instance copies; the static
    // instance-0 originals on a dungeon map are the copy SOURCES, unreachable from inside a run).
    let (mut go, tmpl) = usable_go(ctx, caster_guid, go_guid)?;
    // Snapshot for the on_go_used notify hook fired at the success exit below — `go` is moved into
    // an update (or deleted by a pool reroll) inside the dispatch arms. The instance is the GO
    // ROW'S OWN (190 slice 2, per 228's documented splice note; equal to the user's after the gate
    // above, but the row is the authoritative source).
    let (used_go_guid, used_go_entry, used_go_instance) =
        (go.guid, go.template_entry, go.instance_id);
    match tmpl.type_id {
        go_type::CHEST => {
            // LOCK gate (work-item 119): a CHEST whose lock has a REAL opener (`locked_shut`) refuses
            // the loot roll until Pick Lock (`apply_pick_lock`) records a `game_gameobject_unlocked` row.
            // lock_id 0 / a hand-open (property==0-only) lock opens freely (unchanged). BEFORE the loot.
            if locked_shut(ctx, go.guid, tmpl.lock_id) {
                return Err("it is locked".to_string());
            }
            if go.state != 0 {
                return Err("chest already looted".to_string());
            }
            // DATA-DRIVEN (work-item 210): `data1` names a `game_gameobject_loot` loot id (the REAL
            // cmangos `gameobject_template.Data1`) — roll ALL independent rows + one pick per group as
            // real multi-slot `game_corpse_loot` rows, keyed on the GO guid so the existing CMSG_LOOT/
            // take-loot path serves it unchanged. Quest-only rows roll UNCONDITIONALLY now (work-item
            // 187 slice 0 — no user/killer gate; visibility/takability are per-viewer/per-taker
            // downstream, same as the creature/pickpocket families). `data1 == 0` (unimported, or the
            // seed/demo chest) OR a table whose roll produced NOTHING falls back to the legacy single
            // guaranteed `data0` drop, so existing seed/demo data + tests stay byte-identical.
            let raw: Vec<(u32, u32, u32, u32, bool)> = if tmpl.data1 != 0 {
                ctx.db
                    .game_gameobject_loot()
                    .by_loot()
                    .filter(&tmpl.data1)
                    .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id, r.quest_only))
                    .collect()
            } else {
                Vec::new()
            };
            let winners = crate::loot::roll_loot_rows_quest_aware(ctx, raw);
            if winners.is_empty() {
                if tmpl.data0 != 0 {
                    ctx.db.game_corpse_loot().insert(CorpseLoot {
                        id: 0,
                        corpse_guid: go.guid,
                        slot: 0,
                        item_entry: tmpl.data0,
                        count: 1,
                        quest_only: false,
                        reserved_for: 0,
                        // GO/chest loot is never group-loot-gated (work-item 187 scoped to creature
                        // corpses) — always the FFA defaults.
                        designated_looter_guid: 0,
                        master_only: false,
                        withheld: false,
                    });
                }
            } else {
                for (slot, (item_entry, count, quest_only)) in winners.into_iter().enumerate() {
                    ctx.db.game_corpse_loot().insert(CorpseLoot {
                        id: 0,
                        corpse_guid: go.guid,
                        slot: slot as u8,
                        item_entry,
                        count,
                        quest_only,
                        reserved_for: 0,
                        designated_looter_guid: 0,
                        master_only: false,
                        withheld: false,
                    });
                }
            }
            go.state = 1;
            ctx.db.game_gameobject().guid().update(go);
        }
        go_type::GOOBER => {
            if locked_shut(ctx, go.guid, tmpl.lock_id) {
                return Err("it is locked".to_string());
            }
            crate::quest::on_gameobject_used(ctx, caster_guid, go.template_entry);
        }
        go_type::GATHER => {
            // GATHER node (mining vein / herb bush): skill-gate → DIRECT-grant the ore/herb → climb the
            // gather skill → DEPLETE. This is the skinning core's shape (`loot::skin_corpse`) reused on the
            // already-present GO `state` marker instead of a per-table `skinned` bool.
            //
            // DEPLETION = the chest's `state` flip (no new lifecycle code): a depleted node stays `state=1`
            // FOREVER — the GO system has no respawn machinery by design (no spawn table / respawn tick), so
            // re-testing a depleted node needs a re-spawn (debug lever) or a re-import. RESPAWN is DEFERRED.
            if go.state != 0 {
                return Err("node already gathered".to_string());
            }
            // SKILL-GATE: the char must have LEARNED the node's gather skill at >= the required level
            // (data1). Unlike skinning there is NO auto-learn here — Mining/Herbalism is learned at the
            // trainer first, so a fresh char without the skill row fails the `learned == 0` gate (the
            // intentional negative case). The pure decision lives in `can_gather` (unit-tested).
            let line = tmpl.gather_skill_line;
            let learned = ctx
                .db
                .game_player_skill()
                .by_character()
                .filter(&caster_guid)
                .find(|s| s.skill_line == line)
                .map(|s| s.current as u32)
                .unwrap_or(0);
            can_gather(learned, tmpl.data1)?;
            // DIRECT-grant (skinning-style, NOT the loot window). `?`-rollback: a full bag returns Err HERE,
            // BEFORE the `state` flip, so the whole tx rolls back and the node stays gatherable — byte-identical
            // to `skin_corpse`'s leather grant (loot.rs:315) and the cooking reagent gate.
            crate::items::grant_item(ctx, caster_guid, tmpl.data0, 1)?;
            // Skill-up — ROLL the node's difficulty band: orange = `data1` (the required floor, already
            // gated above), gray = `gather_gray` (0 ⇒ the always-skill sentinel → byte-identical +1 for
            // existing nodes; a real table with gray>0 makes the climb taper as it nears the ceiling).
            crate::skill::gain_profession_skill(
                ctx,
                caster_guid,
                line,
                tmpl.data1,
                tmpl.gather_gray,
            );
            // DEPLETE — a THREE-WAY split on the structural POOL_TAG discriminator + the pool's `in_place`
            // KIND (collision-proof, NOT a bare low-48 lookup):
            //
            // (1) ROAMING pool (a pooled point whose pool is `in_place == false`) → the rotation IS the
            //     respawn: `reroll_pool` NOW (delete this point, activate a DIFFERENT inactive one) and RETURN
            //     — no state flip, no armed timer (reroll_pool deleted the row, so pass_gameobject_respawn
            //     never sees this guid → no double-respawn). Deleting the point relays SMSG_DESTROY_OBJECT
            //     (vanishes on gather = correct feedback) and the new point's INSERT relays CREATE_OBJECT (the
            //     node roams visible now). Active count stays max_active. DEBUG-ONLY: vanilla Elwynn is not
            //     spatially pooled, so nothing in production emits `in_place=false` pools — only the
            //     `debug_setup_gather_pool` lever does (spatial wander was rejected for in-place tier variety).
            //
            // (2) IN-PLACE TIER point (a pooled point whose pool is `in_place == true` — a `max_active==1`
            //     co-located multi-tier point) → the SAME tail as a standalone: flip state + arm the timer.
            //     pass_gameobject_respawn fires `reroll_pool` when the timer elapses, which (because
            //     `max_active==1` ⇒ every member inactive after the delete) WEIGHTED-rolls over ALL the
            //     point's CO-LOCATED tiers, repeats allowed, IN PLACE (identical x,y,z ⇒ NO wander). So a
            //     Copper point mostly re-presents Copper and ~15% presents Tin. Rolling at timer-fire (not
            //     now) reuses the already-wired pass branch, preserves authentic downtime, and cannot
            //     double-respawn (the depleted row is the one reroll_pool deletes before insert).
            //
            // (3) STANDALONE node (tag bit clear → never queries the member table) → the EXISTING in-place
            //     state-flip + armed respawn timer: gatherable again `respawn_window_micros(tmpl.respawn_secs)`
            //     from now (per-template window, default 3 min), flipped back to state 0 by
            //     pass_gameobject_respawn (the sense tick). UNCHANGED.
            //
            // (2) and (3) share the flip+arm tail (the `_ =>` arm). For BOTH, the state 0→1 flip is not
            // relayed live (no GO on_update relay), so the depleted node stays visible client-side until its
            // timer fires (then the reroll's DELETE/INSERT, or relog, refreshes it) — cosmetic, pre-existing.
            // respawn_at_micros is a defaulted u64 (NOT a Timestamp — that would abort the live-table publish).
            match pool_point_id_of(go.guid)
                .and_then(|pid| ctx.db.game_gameobject_pool_member().point_id().find(pid))
            {
                // (1) ROAMING pool → instant reroll + return (the importer's wander pools).
                Some(member)
                    if !ctx
                        .db
                        .game_gameobject_pool()
                        .pool_id()
                        .find(member.pool_id)
                        .map(|p| p.in_place)
                        .unwrap_or(false) =>
                {
                    reroll_pool(ctx, member.pool_id, go.guid);
                }
                // (2) in-place TIER point OR (3) standalone node → flip + arm the respawn timer; the
                // respawn pass re-rolls the tier in place (pooled) or flips state 0 (standalone).
                _ => {
                    go.state = 1;
                    go.respawn_at_micros = (ctx.timestamp.to_micros_since_unix_epoch() as u64)
                        + respawn_window_micros(tmpl.respawn_secs);
                    ctx.db.game_gameobject().guid().update(go);
                }
            }
        }
        go_type::DOOR | go_type::BUTTON => {
            // DOOR/BUTTON toggle (work-item 211): open↔closed is state 0↔1, the same field the
            // gateway already relays in the CREATE_OBJECT (`gameobject_state`) — see the new
            // `on_go_update` relay (subscriptions.rs) that re-emits it live, mirroring the corpse
            // body→bones re-emit. LOCK gate (work-item 119): a DOOR/BUTTON whose lock has a REAL opener
            // (`locked_shut`) refuses the toggle until Pick Lock records a `game_gameobject_unlocked`
            // row. lock_id 0 / a hand-open (property==0-only) lock toggles freely (unchanged, per 211).
            if locked_shut(ctx, go.guid, tmpl.lock_id) {
                return Err("it is locked".to_string());
            }
            go.state = toggle_state(go.state);
            ctx.db.game_gameobject().guid().update(go);
        }
        _ => {
            // INERT type (work-item 211 widened import): every in-scope cmangos GO type now imports
            // (template + spawn), but only CHEST/GOOBER/GATHER/DOOR/BUTTON dispatch above — everything
            // else (SPELL_FOCUS, MAILBOX until work-item 068, etc.) is a benign no-op so the object is
            // at least usable/clickable without a hard error, not a silent capability promise.
        }
    }
    // Encounter kernel notify hook (work-item 228): every SUCCESSFUL use — the gates above (range,
    // already-looted, skill) `?`-returned before this line, so a rejected use never fires.
    crate::hooks::fire_on_go_used(
        ctx,
        &crate::hooks::GoUsedPayload {
            go_guid: used_go_guid,
            go_entry: used_go_entry,
            user_guid: caster_guid,
            instance_id: used_go_instance,
        },
    );
    Ok(())
}

pub(crate) fn despawn_from_relay(ctx: &ReducerContext, go_guid: u64) -> Result<(), String> {
    if ctx.db.game_gameobject().guid().find(go_guid).is_none() {
        return Err(format!("relay gameobject {go_guid} is missing"));
    }
    crate::loot::reap_corpse_loot_family(ctx, go_guid);
    ctx.db.game_gameobject_unlocked().go_guid().delete(go_guid);
    ctx.db
        .game_gameobject_trap_cooldown()
        .go_guid()
        .delete(go_guid);
    ctx.db.game_gameobject().guid().delete(go_guid);
    Ok(())
}

pub(crate) fn activate_trap_from_relay(
    ctx: &ReducerContext,
    source_guid: u64,
    go_guid: u64,
) -> Result<(), String> {
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .filter(|entity| !entity.dead)
        .ok_or_else(|| format!("relay trap user {source_guid} is unavailable"))?;
    let object = ctx
        .db
        .game_gameobject()
        .guid()
        .find(go_guid)
        .filter(|object| object.map_id == source.map_id && object.instance_id == source.instance_id)
        .ok_or_else(|| format!("relay trap {go_guid} is unavailable"))?;
    let template = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(object.template_entry)
        .filter(|template| template.type_id == go_type::TRAP)
        .ok_or_else(|| format!("relay object {go_guid} is not a trap"))?;
    let trap = ctx
        .db
        .game_gameobject_trap()
        .entry()
        .find(template.entry)
        .filter(|trap| trap.spell_id != 0)
        .ok_or_else(|| format!("trap metadata {} is missing", template.entry))?;
    if ctx
        .db
        .game_gameobject_trap_cooldown()
        .go_guid()
        .find(go_guid)
        .is_some_and(|cooldown| cooldown.ready_at > ctx.timestamp)
    {
        return Ok(());
    }
    crate::spell::start_creature_spell(
        ctx,
        crate::spell::CreatureSpellStart {
            caster_guid: source_guid,
            caster_level: source.level as u8,
            spell_id: trap.spell_id,
            mode: crate::spell::CreatureSpellStartMode::Triggered,
            target: crate::spell::CreatureSpellTarget::Unit(source_guid),
            interrupt_previous: false,
            admission: crate::spell::CreatureSpellCasterAdmission::Living,
        },
    )?;
    let cooldown_secs = trap_cooldown_secs(trap.cooldown_secs);
    let ready_at = ctx
        .timestamp
        .checked_add(TimeDuration::from_micros(
            i64::from(cooldown_secs) * 1_000_000,
        ))
        .unwrap_or(ctx.timestamp);
    let cooldowns = ctx.db.game_gameobject_trap_cooldown();
    let row = GameObjectTrapCooldown {
        go_guid,
        map_id: object.map_id,
        instance_id: object.instance_id,
        ready_at,
    };
    if cooldowns.go_guid().find(go_guid).is_some() {
        cooldowns.go_guid().update(row);
    } else {
        cooldowns.insert(row);
    }
    Ok(())
}

fn trap_cooldown_secs(source_cooldown_secs: u32) -> u32 {
    if source_cooldown_secs == 0 {
        4
    } else {
        source_cooldown_secs
    }
}

pub(crate) fn clear_relay_trap_cooldowns_for_instance(ctx: &ReducerContext, instance_id: u64) {
    let cooldowns = ctx.db.game_gameobject_trap_cooldown();
    for row in cooldowns
        .by_instance()
        .filter(&instance_id)
        .collect::<Vec<_>>()
    {
        cooldowns.go_guid().delete(row.go_guid);
    }
}

/// Pure DOOR/BUTTON open/closed TOGGLE (ctx-free, unit-tested): the same binary `state` field a CHEST
/// uses for looted/unlooted, just flipped both directions instead of one-way. Total over every `u8`
/// input (not just 0/1) so a corrupt/foreign state value can never wedge the reducer — anything
/// nonzero is treated as "open" and flips closed; only exact 0 flips to 1.
pub(crate) fn toggle_state(state: u8) -> u8 {
    if state == 0 {
        1
    } else {
        0
    }
}

/// Pick the lock on the FIRST spawned gameobject of template `go_entry` as `character_guid` — same core
/// as `pick_lock` but resolves by template entry so `spacetime call` can drive it with SMALL args (it
/// mangles guids > 2^53), mirroring `debug_use_gameobject_entry`. The headless Pick Lock verify lever.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_pick_lock_entry(
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
    apply_pick_lock(ctx, character_guid, go.guid)
}

/// Replace the live gameobject set with an imported content slice (see `importer/`). CLEARS every
/// `game_gameobject` (the gateway relays those deletes as DESTROY), then loads the batch. Unlike
/// creatures, a `game_gameobject` row IS the live object (no separate spawn table + respawn tick), so
/// the gateway's on_insert relay sends each CREATE_OBJECT directly. Loads via a reducer (not SQL)
/// because `game_gameobject` has a `created_at: Timestamp`. Mirrors `import_creature_spawns`.
///
/// `packed`: rows separated by `;`, fields by `,`, in the order
/// `guid,template_entry,map_id,x,y,z,orientation,initial_state,rot0,rot1,rot2,rot3`. `created_at` =
/// `ctx.timestamp`. `initial_state` (work-item 211, END field — widened alongside
/// `import_gameobjects_append` in the SAME commit, not a table migration: this is a string protocol
/// between the importer and this reducer, not a reducer arg) lets a DOOR/BUTTON spawn already-open
/// (cmangos `startOpen`); every other type's importer row carries 0 (ready/closed), byte-identical to
/// the pre-211 always-0 shape. `rot0..3` (END fields — the cmangos spawn quaternion) ride
/// AFTER `initial_state` rather than beside `orientation` so every pre-515 packed-row builder in this
/// file's tests keeps working unmodified; a hand-built row that omits them fails loudly (field-count
/// check below) rather than silently importing a zero quaternion.
#[reducer]
pub fn import_gameobjects(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let gos = ctx.db.game_gameobject();
    let guids: Vec<u64> = gos.iter().map(|g| g.guid).collect();
    for g in guids {
        gos.guid().delete(g);
    }
    if load_go_batch(ctx, &packed)? == 0 {
        return Err("import payload was empty".to_string());
    }
    Ok(())
}

/// Append a batch of packed gameobjects WITHOUT the clear — paired with `import_gameobjects` (which
/// clears ONCE + loads batch 0), so a whole zone can load across several `spacetime call`s. note: no
/// clear here, so re-calling this alone never wipes the set.
#[reducer]
pub fn import_gameobjects_append(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_go_batch(ctx, &packed)?;
    Ok(())
}

/// Parse `packed` (`;`-separated rows of
/// `guid,template_entry,map,x,y,z,o,initial_state,rot0,rot1,rot2,rot3`) into `game_gameobject` rows
/// (stamped at `ctx.timestamp`), returning the count loaded. Shared by both reducers above.
/// `initial_state` is work-item 211's DOOR/BUTTON `startOpen` carry — every non-door/button importer
/// row still sends 0, so existing content is byte-identical. `rot0..3` is the cmangos
/// spawn quaternion; the importer sends the dump's literal `0,0,0,0` for any row it doesn't have one
/// for, so this reducer never needs to derive a fallback itself — that derivation is the WIRE codec's
/// job (`gateway/src/codec/gameobject.rs`), not storage's.
fn load_go_batch(ctx: &ReducerContext, packed: &str) -> Result<u32, String> {
    let gos = ctx.db.game_gameobject();
    let now = ctx.timestamp;
    let mut loaded = 0u32;
    for row in packed.split(';').filter(|r| !r.is_empty()) {
        let f: Vec<&str> = row.split(',').collect();
        if f.len() != 12 {
            return Err(format!(
                "gameobject import row needs 12 fields, got {}: {row}",
                f.len()
            ));
        }
        let pu64 = |s: &str| s.parse::<u64>().map_err(|_| format!("bad u64: {s}"));
        let pu32 = |s: &str| s.parse::<u32>().map_err(|_| format!("bad u32: {s}"));
        let pf32 = |s: &str| s.parse::<f32>().map_err(|_| format!("bad f32: {s}"));
        let pu8 = |s: &str| s.parse::<u8>().map_err(|_| format!("bad u8 state: {s}"));
        // Parse the position ONCE: the grid columns used to re-parse `f[3]`/`f[4]` per
        // component, and `cell` would have made that four parses of the same two fields. Hoisting
        // also makes it structurally impossible for `grid_x`, `grid_y` and `cell` below to be
        // derived from different coordinates.
        let (gx_src, gy_src) = (pf32(f[3])?, pf32(f[4])?);
        gos.try_insert(GameObject {
            guid: pu64(f[0])?,
            template_entry: pu32(f[1])?,
            map_id: pu32(f[2])?,
            x: pf32(f[3])?,
            y: pf32(f[4])?,
            z: pf32(f[5])?,
            orientation: pf32(f[6])?,
            state: pu8(f[7])?,
            created_at: now,
            respawn_at_micros: 0, // a freshly-imported node is ready (no pending respawn)
            instance_id: 0, // imported static rows are open-world (dungeon copies are runtime, 190 slice 2)
            // note: the new respawn_secs/gather_gray cols live on the TEMPLATE (GameObjectTemplate), not on
            // this SPAWN row — so this GameObject literal is otherwise unchanged this slice.,
            grid_x: lyracore_shared::spatial::grid_cell(gx_src, gy_src).0,
            grid_y: lyracore_shared::spatial::grid_cell(gx_src, gy_src).1,
            cell: lyracore_shared::spatial::cell_id_at(gx_src, gy_src),
            rotation_0: pf32(f[8])?,
            rotation_1: pf32(f[9])?,
            rotation_2: pf32(f[10])?,
            rotation_3: pf32(f[11])?,
        })
        .map_err(|e| format!("gameobject insert failed (dup guid?): {e}"))?;
        loaded += 1;
    }
    Ok(loaded)
}

// ===========================================================================================
//  Tests [server] — the gather-node slice's non-trivial pieces (the live arm composes these)
// ===========================================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_trap_uses_the_source_default_only_for_zero_cooldown() {
        assert_eq!(trap_cooldown_secs(0), 4);
        assert_eq!(trap_cooldown_secs(1), 1);
        assert_eq!(trap_cooldown_secs(30), 30);
    }
    use crate::skill::{next_skill, skill_line, APPRENTICE_CAP};

    /// The GATHER type id is wire-facing (it rides into the client's UpdateGameObject `type_id`), so guard
    /// the literal against a silent edit and confirm it can't collide with the two real cmangos types.
    #[test]
    fn gather_type_id_is_the_synthetic_marker_25() {
        assert_eq!(go_type::GATHER, 25);
        assert_ne!(go_type::GATHER, go_type::CHEST);
        assert_ne!(go_type::GATHER, go_type::GOOBER);
        // DOOR/BUTTON (work-item 211) sit at cmangos' real 0/1 — guard them too, and confirm none of
        // the five LIVE type ids collide with each other.
        assert_eq!(go_type::DOOR, 0);
        assert_eq!(go_type::BUTTON, 1);
        let live = [
            go_type::DOOR,
            go_type::BUTTON,
            go_type::QUESTGIVER,
            go_type::CHEST,
            go_type::GOOBER,
            go_type::GATHER,
        ];
        for i in 0..live.len() {
            for j in (i + 1)..live.len() {
                assert_ne!(
                    live[i], live[j],
                    "LIVE go_type ids must be pairwise distinct"
                );
            }
        }
    }

    /// Work-item 041: `QUESTGIVER` is re-exported from `lyracore_shared::constants::go_type` (not a local
    /// literal) so the module and gateway copies can never drift — pin BOTH the cmangos value and the
    /// identity with the shared source in one assertion.
    #[test]
    fn questgiver_type_id_matches_the_shared_constant_and_is_cmangos_type_2() {
        assert_eq!(go_type::QUESTGIVER, 2);
        assert_eq!(
            go_type::QUESTGIVER,
            lyracore_shared::constants::go_type::QUESTGIVER
        );
    }

    /// DOOR/BUTTON TOGGLE (work-item 211) — the pure decision `apply_use_gameobject`'s DOOR|BUTTON arm
    /// calls. Total over every `u8` (not just 0/1): only exact 0 flips to 1, anything else (a corrupt/
    /// foreign value) flips to 0 rather than panicking or wedging the reducer.
    #[test]
    fn toggle_state_flips_zero_and_one_and_is_total() {
        assert_eq!(toggle_state(0), 1, "closed -> open");
        assert_eq!(toggle_state(1), 0, "open -> closed");
        // A never-expected third value still flips deterministically (never panics).
        assert_eq!(toggle_state(2), 0);
        assert_eq!(toggle_state(255), 0);
    }

    /// LOCK opener classification (work-item 119) — the pure decision the use-gate + pick path share. A
    /// SKILL row (kind 2) is a REAL opener only with a real SkillLine (`property != 0`): the importer
    /// stores an UNMAPPED LockType.dbc id as `property 0` (LOCKTYPE_OPEN/blasting/disarm), which must NOT
    /// gate a hand-open chest and which Pick Lock can't satisfy. An ITEM row (kind 1) always opens; an
    /// unmodeled kind never does. This is exactly the property=0 escape-hatch the live locks (5/23/24)
    /// carry ALONGSIDE their real 633 gate — the 633 row still gates, the property=0 rows are ignored.
    #[test]
    fn is_real_lock_opener_ignores_property_zero_skill_slots() {
        assert!(
            is_real_lock_opener(LOCK_KIND_SKILL, 633),
            "Lockpicking 633 skill row is a real opener"
        );
        assert!(
            is_real_lock_opener(LOCK_KIND_SKILL, 186),
            "any real SkillLine (Mining) is a real opener"
        );
        assert!(
            !is_real_lock_opener(LOCK_KIND_SKILL, 0),
            "property==0 skill slot (unmapped LockType) is NOT an opener"
        );
        assert!(
            is_real_lock_opener(LOCK_KIND_ITEM, 5397),
            "an item-key row is a real opener"
        );
        assert!(
            !is_real_lock_opener(0, 633),
            "an unmodeled lock kind is never an opener"
        );
        assert!(
            !is_real_lock_opener(9, 633),
            "an unmodeled lock kind is never an opener"
        );
        // A lock carrying BOTH a 633 row AND a property==0 slot (the live 5/23/24 shape) still has a real
        // opener → it stays gated; the property==0 slot does not make it "already openable".
        assert!(
            is_real_lock_opener(LOCK_KIND_SKILL, 633) && !is_real_lock_opener(LOCK_KIND_SKILL, 0)
        );
    }

    /// PIECE 1 — the skill-GATE admits a learned-and-high-enough skill and rejects both the not-learned and
    /// too-low cases (the live arm's first decision, `can_gather(learned, data1)`).
    #[test]
    fn can_gather_admits_learned_and_rejects_unlearned_or_too_low() {
        // PASS: a freshly-learned 1/75 skill clears a required-level-1 node (the seeded data1=1).
        assert!(can_gather(1, 1).is_ok());
        assert!(can_gather(75, 1).is_ok());
        // REJECT — never learned (no skill row → learned == 0): the negative case the verify recipe drives.
        assert_eq!(
            can_gather(0, 1).unwrap_err(),
            "you have not learned this gathering skill"
        );
        // REJECT — learned but below the node's required level.
        assert_eq!(
            can_gather(5, 25).unwrap_err(),
            "your skill is too low to gather this node"
        );
    }

    /// PIECE 2 (re-scoped, was `gather_grants_one_and_climbs_the_skill_one_step`) — only the REAL-fn half:
    /// the skill GATE passes for a freshly-learned char on a required-1 node (`can_gather`), and the
    /// skill-up climbs toward the Apprentice cap via the shared `next_skill`. The "grants one" half (the
    /// live arm's `grant_item(.., tmpl.data0, 1)` call) is a ctx fn — not exercised here, so it's dropped
    /// rather than modeled as a bare local (`let granted = 1; assert_eq!(granted, 1)` asserted nothing).
    #[test]
    fn gather_gate_passes_and_skill_climbs_toward_apprentice_cap() {
        let cap = APPRENTICE_CAP as u32; // 75 — the profession cap, NOT level*5
                                         // Gate passes for a learned 1/75 char on a required-1 node.
        assert!(can_gather(1, 1).is_ok());
        // The skill-up: 1 -> 2 (one step, capped at the apprentice 75 — independent of level).
        assert_eq!(next_skill(1, cap), 2);
        // Mining is a real profession line, climbs past the level-1 combat cap (5) toward 75.
        assert_eq!(skill_line::MINING, 186);
        assert_eq!(next_skill(74, cap), 75);
        assert_eq!(next_skill(75, cap), 75); // at the apprentice cap → no further climb
    }

    /// PIECE 5 (respawn variance) — the per-template RESPAWN-WINDOW lookup. A node with `respawn_secs == 0`
    /// (every EXISTING node, default-migrated) keeps the 3-min `RESPAWN_WINDOW_MICROS` fallback (byte-
    /// identical); a node with a seeded `respawn_secs` uses its OWN window, scaled seconds→micros.
    #[test]
    fn respawn_window_micros_defaults_to_the_const_else_scales_seconds() {
        // Default (0) ⇒ the 3-min const fallback — the regression guard for existing nodes.
        assert_eq!(respawn_window_micros(0), RESPAWN_WINDOW_MICROS);
        assert_eq!(respawn_window_micros(0), 180_000_000);
        // Seeded seconds ⇒ scaled to micros (the verify recipe seeds a 30s node).
        assert_eq!(respawn_window_micros(30), 30_000_000);
        assert_eq!(respawn_window_micros(1), 1_000_000);
        assert_eq!(respawn_window_micros(180), RESPAWN_WINDOW_MICROS); // 180s == the const, no surprise
    }

    /// `arm_pool`'s map-fence. An EMPTY imported-maps set (nothing has ever been really
    /// imported into this database) must admit every member regardless of its map: that's `init`'s own
    /// publish-time arm, and a bare-dev-DB `debug_setup_gather_pool`, and both must keep working exactly
    /// as before this fence existed.
    #[test]
    fn pool_member_map_is_live_admits_everything_before_any_real_import() {
        let none_imported: HashSet<u32> = HashSet::new();
        assert!(
            pool_member_map_is_live(0, &none_imported),
            "map 0 admitted pre-import"
        );
        assert!(
            pool_member_map_is_live(1, &none_imported),
            "map 1 admitted pre-import"
        );
        assert!(
            pool_member_map_is_live(36, &none_imported),
            "any map admitted pre-import"
        );
    }

    /// Once a real import has landed terrain/nav content for at least one map, a pool member on THAT
    /// map is live, and a member on any OTHER map is foreign — the exact defect this issue reported:
    /// a map-1 shard's `arm_all_pools` call must not resurrect `init`'s map-0 Northshire tier point.
    #[test]
    fn pool_member_map_is_live_gates_by_map_once_something_is_really_imported() {
        let map_1_imported: HashSet<u32> = [1].into_iter().collect();
        assert!(
            pool_member_map_is_live(1, &map_1_imported),
            "the imported map itself stays live"
        );
        assert!(
            !pool_member_map_is_live(0, &map_1_imported),
            "init's map-0 fixture is foreign now"
        );
        assert!(
            !pool_member_map_is_live(36, &map_1_imported),
            "any other foreign map is refused too"
        );
        // A database that legitimately hosts more than one map (e.g. a continent + its --include-map
        // instance interior) admits every one of them, not just the first.
        let multi_map: HashSet<u32> = [0, 36].into_iter().collect();
        assert!(pool_member_map_is_live(0, &multi_map));
        assert!(pool_member_map_is_live(36, &multi_map));
        assert!(!pool_member_map_is_live(1, &multi_map));
    }

    /// `eligible_pool_candidates` — the ACTUAL splice `arm_pool` calls each iteration (review:
    /// the disclosed gap was that only the isolated `pool_member_map_is_live` predicate had a unit test,
    /// not the wiring; this exercises the real production filter — weight>0 AND live-map AND not-already-
    /// active — together, exactly as `arm_pool` composes them).
    #[test]
    fn eligible_pool_candidates_applies_weight_map_and_already_active_together() {
        fn member(point_id: u64, map_id: u32, weight: u32) -> GameObjectPoolMember {
            GameObjectPoolMember {
                point_id,
                pool_id: 2,
                template_entry: 1731,
                map_id,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                orientation: 0.0,
                weight,
            }
        }
        let live_map: HashSet<u32> = [1].into_iter().collect();
        let members = vec![
            member(1, 1, 5), // live map, weighted, inactive → ELIGIBLE
            member(2, 1, 0), // live map, weight 0 → excluded
            member(3, 0, 5), // foreign map (init's map-0 fixture) → excluded
            member(4, 1, 5), // live map, weighted, but already active → excluded
        ];
        let active_point_guids: HashSet<u64> = [pool_point_guid(4)].into_iter().collect();
        let out = eligible_pool_candidates(members, &active_point_guids, &live_map);
        let ids: Vec<u64> = out.iter().map(|m| m.point_id).collect();
        assert_eq!(
            ids,
            vec![1],
            "only the live-map, weighted, inactive member survives all three gates"
        );
    }

    /// PIECE 4 — the RESPAWN-DUE predicate the sense-tick pass branches on. A depleted node
    /// with an armed timer flips ONLY once its time has elapsed; a not-yet-due node, a never-armed node
    /// (respawn_at_micros==0), and a ready node (state 0) all stay put.
    #[test]
    fn respawn_due_flips_only_a_depleted_armed_and_elapsed_node() {
        let armed_at = 1_000u64;
        // DUE — depleted, armed, and the armed time has passed (now == and now > both flip).
        assert!(
            respawn_due(1, armed_at, armed_at),
            "due exactly at the armed time"
        );
        assert!(
            respawn_due(1, armed_at, armed_at + 1),
            "due once past the armed time"
        );
        // NOT due — armed but the window has not elapsed yet (the pass must NOT flip it early).
        assert!(
            !respawn_due(1, armed_at, armed_at - 1),
            "before the armed time → not due"
        );
        // NOT due — a depleted node with NO armed timer (respawn_at_micros == 0): never respawns on its own.
        assert!(
            !respawn_due(1, 0, u64::MAX),
            "never-armed depleted node stays depleted"
        );
        // NOT due — a READY node (state 0) is irrelevant to the respawn pass regardless of the timer/now.
        assert!(
            !respawn_due(0, armed_at, armed_at + 1),
            "a ready node is never a respawn candidate"
        );
    }

    /// POOL PIECE A — the WEIGHTED pick is deterministic given a seeded roll index. The cumulative-band
    /// math must put a higher-weight member in a wider band and land each roll in the right band, exactly
    /// like the reroll's `ctx.random::<u32>() % total` feeds it. This is the "weighted pick is
    /// deterministic given a seeded index" guarantee.
    #[test]
    fn weighted_pick_lands_each_roll_in_its_cumulative_band() {
        // Five base members at weight 20 + one higher-tier at weight 2 → total 102. Bands: [0,20),
        // [20,40), [40,60), [60,80), [80,100) are the bases; [100,102) is the rare higher-tier.
        let w = [20u32, 20, 20, 20, 20, 2];
        assert_eq!(weighted_pick(&w, 0), Some(0)); // band start
        assert_eq!(weighted_pick(&w, 19), Some(0)); // band end (half-open)
        assert_eq!(weighted_pick(&w, 20), Some(1)); // next band start
        assert_eq!(weighted_pick(&w, 99), Some(4)); // last BASE band
        assert_eq!(weighted_pick(&w, 100), Some(5)); // the rare higher-tier band start
        assert_eq!(weighted_pick(&w, 101), Some(5)); // …and its (narrow) end → only 2/102 rolls hit it
                                                     // A zero-weight member occupies NO band — it is never chosen (the "weight 0 = never auto-selected"
                                                     // rule the candidate filter also enforces). Here member 1 (weight 0) is skipped entirely.
        let w0 = [10u32, 0, 10];
        assert_eq!(weighted_pick(&w0, 9), Some(0));
        assert_eq!(weighted_pick(&w0, 10), Some(2)); // jumps PAST the zero-weight member 1
                                                     // A roll past the total lands in no band (None) — the reroll never feeds such a roll (% total),
                                                     // but the predicate is honest about it.
        assert_eq!(weighted_pick(&[5u32, 5], 10), None);
    }

    /// POOL PIECE C — the STANDALONE-vs-POOLED discriminator is a STRUCTURAL guid test (the [`POOL_TAG`]
    /// bit), collision-proof against standalone GO guids. A pooled point round-trips point_id <-> guid via
    /// `pool_point_guid` / `pool_point_id_of`; a STANDALONE node's guid (`0xF110<<48 | small db_guid/seed
    /// ordinal`, tag bit CLEAR) yields `None` → it never queries the member table → in-place respawn (the
    /// no-regression guarantee). Pins the exact namespace collision the tag fixes.
    #[test]
    fn pool_point_guid_tagged_namespace_is_disjoint_from_standalone() {
        const GO_HIGH: u64 = 0xF110u64 << 48;
        for point_id in [1u64, 2, 3, 4, 42, 9999] {
            let guid = pool_point_guid(point_id);
            assert_eq!(
                guid >> 48,
                0xF110,
                "carries HIGHGUID_GAMEOBJECT in bits 48..63"
            );
            assert_ne!(guid & POOL_TAG, 0, "tagged as a pool point");
            assert_eq!(
                pool_point_id_of(guid),
                Some(point_id),
                "round-trips to point_id"
            );
        }
        // The exact bug: a seeded standalone Copper Vein at GO_HIGH|3 shares point_id 3's OLD namespace.
        // With the tag bit it is NOT a pool guid (None) → in-place respawn, never clobbered/misrouted; and
        // its guid differs from the real pooled point 3.
        for standalone in [GO_HIGH | 3, GO_HIGH | 4, GO_HIGH | 50102, GO_HIGH | 1234] {
            assert_eq!(
                pool_point_id_of(standalone),
                None,
                "standalone GO is not a pool point"
            );
            assert_ne!(
                standalone,
                pool_point_guid(standalone & 0x7FFF_FFFF_FFFF),
                "disjoint from pooled"
            );
        }
    }

    /// POOL PIECE D — the GATHER deplete branch ROUTES on the structural POOL_TAG discriminator AND (for a
    /// pooled point) the pool's `in_place` KIND, a THREE-WAY split: (1) a ROAMING pooled point (in_place
    /// FALSE) → reroll-and-return (the node WANDERS, no state flip, no armed timer, so
    /// `pass_gameobject_respawn` never double-respawns it); (2) an IN-PLACE TIER pooled point (in_place
    /// TRUE) AND (3) a STANDALONE node (`None`, tag bit clear) → the SAME flip + armed respawn-timer tail
    /// (the `_ =>` arm). This pins the STRUCTURAL half (pooled-vs-standalone); the in_place half is a pool
    /// header lookup the live branch does. The live count-conservation of the reroll is the arithmetic in
    /// `reroll_conserves_the_active_count` + the server verify recipe.
    #[test]
    fn gather_deplete_routes_pooled_to_reroll_and_standalone_to_in_place() {
        // A pooled point (the importer's base-1_000_000 point_id, tag bit set) → the member-table lookup
        // succeeds; whether it rerolls (roaming) or arms a timer (in-place tier) is then the pool's in_place.
        let pooled = pool_point_guid(1_000_000);
        assert_eq!(
            pool_point_id_of(pooled),
            Some(1_000_000),
            "pooled point → consult the pool's in_place"
        );
        // A standalone imported gather node (HIGHGUID_GAMEOBJECT | small db_guid, tag CLEAR) → in-place.
        let standalone = (0xF110u64 << 48) | 50102;
        assert_eq!(
            pool_point_id_of(standalone),
            None,
            "standalone gather node → in-place respawn timer"
        );
        // …and its in-place tail still arms a positive future window (the standalone branch is unchanged):
        // a 300s (real vanilla) node arms 300_000_000 micros out; the default-0 node keeps the 3-min const.
        assert_eq!(
            respawn_window_micros(300),
            300_000_000,
            "standalone vein arms its 300s window"
        );
        assert_eq!(
            respawn_window_micros(0),
            RESPAWN_WINDOW_MICROS,
            "default node keeps the 3-min fallback"
        );
    }

    /// TIER-VARIETY A — the demonstrator's ~85/15 Copper/Tin weighting, and the load-bearing REPEAT-ALLOWED
    /// property that distinguishes a tier RE-ROLL from a deterministic flip. The in-place tier reroll feeds
    /// `weighted_pick([85, 15], roll)` with `roll = ctx.random() % 100`: Copper (member 0) owns the wide
    /// band [0,85) and Tin (member 1) the narrow [85,100) — so over many rolls Tin presents ~15% of the
    /// time. Crucially a roll already inside the Copper band re-selects Copper (member 0): the common tier
    /// CAN repeat (this is the whole point — a co-located pool does NOT flip Copper↔Tin every gather).
    #[test]
    fn tier_variety_weighted_reroll_is_eightyfive_fifteen_and_repeats_the_common_tier() {
        let w = [85u32, 15]; // Copper w85, Tin w15 → total 100
                             // Copper owns [0,85): every roll there re-presents Copper (REPEAT-ALLOWED — not a forced flip).
        assert_eq!(weighted_pick(&w, 0), Some(0), "low roll → Copper");
        assert_eq!(
            weighted_pick(&w, 84),
            Some(0),
            "band end (half-open) → still Copper"
        );
        // Tin owns the narrow [85,100): only ~15/100 rolls present the rarer tier.
        assert_eq!(weighted_pick(&w, 85), Some(1), "the Tin band starts at 85");
        assert_eq!(
            weighted_pick(&w, 99),
            Some(1),
            "…and ends at 99 → 15/100 rolls hit Tin"
        );
        // COUNT the Copper band width over the full 0..100 roll space = exactly 85 (the ~85% the verify
        // recipe samples), and Tin = 15. This is the deterministic distribution the seeded roll produces.
        let copper = (0..100u32)
            .filter(|&r| weighted_pick(&w, r) == Some(0))
            .count();
        let tin = (0..100u32)
            .filter(|&r| weighted_pick(&w, r) == Some(1))
            .count();
        assert_eq!(
            (copper, tin),
            (85, 15),
            "the band split IS the weight split (no flip, repeats allowed)"
        );
        // Co-located members differ ONLY in template_entry/weight, so whichever index the roll picks, the
        // SPAWNED coords are identical → the node never wanders. Model the two members' coords and assert
        // the picked coord is invariant across BOTH possible picks (the "same coords" guarantee).
        let coords = [(-9620.11f32, -46.3336f32, 47.3641f32); 2]; // {Copper, Tin} co-located
        for roll in [0u32, 50, 84, 85, 99] {
            let idx = weighted_pick(&w, roll).unwrap();
            assert_eq!(
                coords[idx], coords[0],
                "a rolled tier spawns at the SAME co-located coord (no wander)"
            );
        }
    }

    /// TIER-VARIETY B — the SKILL-GATE still blocks a rolled higher tier for a low char (the authentic
    /// "richer node you can't tap yet" teaser). The demonstrator's tiers carry the real vanilla required
    /// Mining: Copper req 1, Tin req 65. A Mining-50 char can always work a rolled Copper but is rejected
    /// off a rolled Tin until Mining reaches 65 — the EXISTING `can_gather`, unchanged by tier-variety.
    #[test]
    fn tier_variety_skill_gate_blocks_a_rolled_tin_for_a_low_char() {
        let (copper_req, tin_req) = (1u32, 65u32); // the demonstrator's real Mining requirements
                                                   // A Mining-50 char: a rolled Copper (req 1) gathers; a rolled Tin (req 65) is gated.
        assert!(
            can_gather(50, copper_req).is_ok(),
            "a rolled Copper is always tappable"
        );
        assert_eq!(
            can_gather(50, tin_req).unwrap_err(),
            "your skill is too low to gather this node",
            "a rolled Tin teases a low char until Mining 65"
        );
        // Once the char reaches Mining 65 the rolled Tin opens up (the gate is `learned >= required`).
        assert!(
            can_gather(65, tin_req).is_ok(),
            "Mining 65 unlocks the rolled Tin"
        );
        // A never-learned char fails BOTH tiers (the learned==0 case), tier-variety irrelevant.
        assert_eq!(
            can_gather(0, copper_req).unwrap_err(),
            "you have not learned this gathering skill"
        );
    }
}
