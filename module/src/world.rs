//! The live in-world player: the field-sync `WorldEntity` row (also reused for creatures, type
//! Unit), the per-recipient movement-relay event table, and the login/movement/death cores the
//! `gw::gw_*` reducers drive (#483 — the sender-path twins are gone) plus `on_disconnect`.
//! [entity]/[event]

use lyracore_shared::constants::sheath_state;
use lyracore_shared::spatial;
use spacetimedb::{
    reducer, table, Identity, ReducerContext, Table,
};

use crate::faction::game_faction_template;
// Graveyard resolution (work-item 209/226) lives in `graveyard.rs` (issue #385 extraction) — this
// alias keeps every `graveyard::...` call site below byte-identical.
use crate::graveyard;
use crate::helpers::entity_by_owner;
use crate::spell::game_resurrect_request;
use crate::{game_character, game_character_buyback, game_corpse, game_instance};

// ===========================================================================================
//  Live in-world entity [entity] — public, field-sync source
// ===========================================================================================

/// A player currently in the world. Created at login, deleted at disconnect. [entity]
#[table(
    accessor = game_world_entity,
    public,
    index(accessor = by_map, btree(columns = [map_id])),
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y])),
    // #456: the AOI subscription's index. Exactly 3 columns, all matched by equality terms —
    // the only shape SpacetimeDB 2.7.1's subscription planner can serve (see the `cell` column).
    // `by_grid` stays: the MODULE reaches it through the generated index accessor, not SQL, so
    // the 3-column planner limit does not apply to `helpers::entities_near`.
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell])),
    // `entity_by_owner` is the auth prologue of ~77 player reducer call sites; without this it was a
    // full table scan per transaction (perf catalog 1.2). `owner_identity` never changes for a live
    // row, so maintenance is insert/delete-only.
    index(accessor = by_owner, btree(columns = [owner_identity]))
)]
pub struct WorldEntity {
    #[primary_key]
    pub guid: u64, // OBJECT_FIELD_GUID
    pub owner_identity: Identity,
    pub account_id: u64,

    // spatial (not a client field; visibility + movement block)
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub grid_x: i32,
    pub grid_y: i32,
    pub last_move_ms: u32,

    // object block
    pub type_mask: u32, // OBJECT_FIELD_TYPE = 0x19 for players
    pub entry: u32,     // OBJECT_FIELD_ENTRY = 0
    pub scale_x: f32,   // OBJECT_FIELD_SCALE_X = 1.0

    // unit block
    pub health: u32,
    pub max_health: u32,
    pub power: u32,
    pub max_power: u32,
    pub level: u32,
    pub faction_template: u32,
    pub unit_bytes_0: u32, // race|class|gender|powertype
    pub display_id: u32,
    pub native_display_id: u32,
    pub unit_flags: u32,
    pub base_attack_time_ms: u32, // UNIT_FIELD_BASEATTACKTIME; the melee swing interval
    pub dynamic_flags: u32,       // UNIT_DYNAMIC_FLAGS (corpse/lootable bits)
    pub dead: bool,               // a creature corpse lingering through its decay window

    // player block
    pub player_bytes: u32,
    pub player_bytes_2: u32,
    pub player_bytes_3: u32,
    pub player_flags: u32,
    pub xp: u32,            // PLAYER_XP; 0 for creatures
    pub next_level_xp: u32, // PLAYER_NEXT_LEVEL_XP; 0 for creatures

    // current target (UNIT_FIELD_TARGET); 0 = none. Set by `set_target` (CMSG_SET_SELECTION).
    pub target_guid: u64,

    // money in copper. For a PLAYER this is PLAYER_FIELD_COINAGE (their purse); for a
    // creature CORPSE it is the lootable money rolled on the killing blow (0 once looted / while
    // alive). `#[default(0)]` + appended at the struct END so `publish` auto-migrates existing rows
    // (the migration rule: column-add needs a default annotation AND end-append, else publish aborts).
    #[default(0)]
    pub money: u32,

    // UNIT_FIELD_BYTES_1: byte 3 carries UNIT_VIS_FLAG_GHOST while the player is a ghost
    // (the semi-transparent render). 0 = normal. `#[default(0)]` + end-appended (migration rule).
    #[default(0)]
    pub unit_bytes_1: u32,

    // The five base attributes (UNIT_FIELD_STAT0..4) for a PLAYER — STR, AGI, STA, INT, SPI — set at
    // login from the cmangos `game_level_stats` curve so the character sheet (C) shows real numbers.
    // The same `game_level_stats` row feeds `max_health_for`/`max_power_for`,
    // so the stamina/intellect here stay consistent with the HP/mana pool. 0 for creatures (a Unit
    // has no character sheet). `#[default(0)]` + end-appended so `publish` auto-migrates existing rows.
    #[default(0)]
    pub strength: u32,
    #[default(0)]
    pub agility: u32,
    #[default(0)]
    pub stamina: u32,
    #[default(0)]
    pub intellect: u32,
    #[default(0)]
    pub spirit: u32,

    // UNIT_NPC_FLAGS (gossip / vendor / questgiver / trainer …) for a creature — drives the client's
    // interact cursor + minimap/overhead icons. Carried over from the imported creature template at spawn;
    // 0 for players (not NPCs) and for un-flagged creatures. Static (CREATE-only — never relayed as a
    // VALUES change). `#[default(0)]` + end-appended so `publish` auto-migrates existing rows.
    #[default(0)]
    pub npc_flags: u32,

    // Armor (UNIT_FIELD_RESISTANCES[0], physical): the server-authoritative source of truth for the
    // character-sheet Armor line AND physical damage mitigation (combat/ `armor_mitigation_pct`).
    // Players: agility*2 base (classic 2 armor/agi) set at login; creatures: 0 for now (no creature
    // armor data yet). `#[default(0)]` + end-appended so `publish` auto-migrates existing rows.
    #[default(0)]
    pub armor: u32,

    // Creature-movement cursor + in-flight leg ETA (mangos-parity smooth movement). `leg_ends_ms` = the
    // `now_ms` at which the creature's current SEGMENT leg (patrol/wander) lands; 0 = idle/no leg — the
    // segment passes skip re-emitting while `now_ms < leg_ends_ms` (no re-throw/dither). `wp_target` =
    // the `game_creature_waypoint.id` the patrol is walking TO (ordered route cursor); 0 = unset →
    // re-acquire the nearest waypoint. Both reset to 0 for free on respawn (the row is rebuilt by
    // `build_creature_entity`). `#[default(0)]` + end-appended so `publish` auto-migrates existing rows.
    #[default(0)]
    pub leg_ends_ms: u32,
    #[default(0u64)]
    // u64 default MUST be typed (a bare `0` encodes 4 bytes; publish needs 8 — migration rule)
    pub wp_target: u64,
    // Live MovementFlags — the leading u32 of the last movement packet's MovementInfo. Lets a peer's
    // CREATE_OBJECT spawn it in its CURRENT move state (running/strafing) instead of idle, fixing the
    // "peer enters AOI range mid-run → idle-floats" bug (the observer missed the MSG_MOVE_START sent
    // while the peer was out of range). 0 = standing still. `#[default(0u32)]` + end-appended so
    // `publish` auto-migrates existing rows.
    #[default(0u32)]
    pub movement_flags: u32,
    // Time (ms since unix epoch) until which this unit is IN COMBAT. Set/refreshed at every hostile
    // action (`combat::enter_combat`); the tick's combat-drop pass clears `UNIT_FLAG_IN_COMBAT` once
    // `now_ms >= combat_until_ms`. Full u64 ms (NOT the u32 `now_ms` — avoids the 49-day wrap). 0 = never
    // in combat. `#[default(0u64)]` (typed — u64 needs 8 bytes) + end-appended so `publish` auto-migrates.
    #[default(0u64)]
    pub combat_until_ms: u64,
    // True once this creature spawn has been pickpocketed (E_PICKPOCKET) — gates a second attempt so the
    // same spawn's pockets can't be drained twice per life. Reset to false for FREE on respawn (the row is
    // rebuilt by `build_creature_entity`, exactly like `leg_ends_ms`/`wp_target`). Players never set it
    // (pickpocket targets creatures only). `#[default(false)]` + end-appended so `publish` auto-migrates
    // existing rows (migration rule).
    #[default(false)]
    pub pickpocketed: bool,
    // The spell id QUEUED onto this unit's next melee swing (Heroic Strike 78 / Cleave 845) — the vanilla
    // "on next swing" mechanic. The cast (E_NEXT_SWING) charges rage + sets this; the next melee swing that
    // FIRES reads it, adds the queued spell's E_NEXT_SWING base_points as bonus damage to a LANDED swing,
    // and CLEARS it back to 0. 0 = no queued strike (the common path). Players only (creatures don't queue).
    // Re-casting while one is queued just overwrites (vanilla's single mutually-exclusive next-swing slot).
    // `#[default(0)]` + end-appended so `publish` auto-migrates existing rows (migration rule).
    #[default(0)]
    pub next_swing_spell: u32,
    // Time (ms since unix epoch) until which this unit may cast Overpower — ARMED when one of its melee
    // swings is DODGED (resolve_swing stamps it). 0 = no window. The cast gate (SPELL_ATTR_REQ_OVERPOWER)
    // refuses Overpower unless `overpower_until_ms > now`. Full u64 ms (NOT the u32 `now_ms` — avoids the
    // 49-day wrap), mirroring combat_until_ms. `#[default(0u64)]` (typed — u64 needs 8 bytes) + end-appended
    // so `publish` auto-migrates existing rows (migration rule).
    #[default(0u64)]
    pub overpower_until_ms: u64,
    // Time (ms since unix epoch) until which this unit may cast Revenge — ARMED when it DODGES / PARRIES /
    // BLOCKS an incoming swing (resolve_swing stamps it). 0 = no window. The cast gate
    // (SPELL_ATTR_REQ_REVENGE) refuses Revenge unless `revenge_until_ms > now`. Mirrors overpower_until_ms.
    // `#[default(0u64)]` (typed) + end-appended so `publish` auto-migrates existing rows (migration rule).
    #[default(0u64)]
    pub revenge_until_ms: u64,
    // The unit's active stance/form, 0-based per the taxonomy STANCE_* convention block (THE definition
    // site): Warrior Battle 0 (default) / Defensive 1 / Berserker 2, Druid Bear 3 / Cat 4 / DireBear 5
    // (156). Written by the E_SET_STANCE effect (stance/form spells, importer name-rescued). The cast
    // gate reads it (Spell.dbc Stances usability mask → `stance_allows`); the combat folds key the
    // Defensive mitigation/threat off it directly (a pure function of this field, so a switch clears
    // the old stance's effect for FREE — no aura cleanup). 0 for classes with no stance mechanic AND
    // for a druid in caster form (form recast toggles back to 0), so every existing unit's combat is
    // byte-identical (baseline-safe). `#[default(0)]` + end-appended → `publish` auto-migrates existing
    // rows (the migration rule). Single mutually-exclusive scalar — a u8, mirroring next_swing_spell /
    // combat_until_ms, NOT an aura (no O(auras) scan on the hot path).
    #[default(0)]
    pub stance: u8,
    // PET OWNERSHIP (Tier 3b — Warlock pet): the controlling player's guid for a SUMMONED PET creature
    // (Summon Imp → an Imp owned by the warlock). 0 = NOT a pet (every existing player and creature). A
    // pet is a normal creature `game_world_entity` (no PLAYER bit) with this set — it rides the existing
    // chase/melee/swing/relay machinery, with two extra tick branches keyed on `owner_guid != 0`: follow
    // the owner when the owner is idle, engage the owner's target when the owner is in combat. The pet has
    // NO `game_creature_spawn` row, so the respawn/decay/return-home passes never touch it; it is deleted
    // (despawned) on the owner's logout/death and on a re-summon. `#[default(0u64)]` (typed — a bare 0
    // encodes 4 bytes, publish needs 8) + end-appended so `publish` auto-migrates existing rows (the
    // migration rule). Mirrors the per-character `owner_guid` naming on game_item_instance / game_corpse.
    #[default(0u64)]
    pub owner_guid: u64,
    // True once this beast creature's CORPSE has been skinned (Skinning profession) — gates a
    // second skin so the same corpse can't be looted for leather twice. The exact `pickpocketed` precedent:
    // a per-spawn bool marker, reset to false for FREE on respawn (the row is rebuilt by
    // `build_creature_entity`). Players never set it (skinning targets beast corpses only). The skinned
    // corpse decays normally. `#[default(false)]` + end-appended so `publish` auto-migrates existing rows
    // (migration rule); the gateway binding ends before this column and tolerates the trailing field.
    #[default(false)]
    pub skinned: bool,
    // FSR (Five-Second Rule) mana regen gate: the unix epoch ms UNTIL WHICH spirit-based mana regen
    // is paused. Stamped to `now_ms + 5000` whenever a player SPENDS mana (hdr.cost > 0 cast path in
    // spell/cast.rs). Mana regens when `now_ms >= mana_regen_paused_until_ms` — IN or OUT of combat —
    // implementing vanilla's 5-second rule. 0 = never paused (every existing row's default). Full u64
    // ms (NOT u32 — same rationale as combat_until_ms). `#[default(0u64)]` (typed — u64 needs 8 bytes)
    // + end-appended so `publish` auto-migrates existing rows (migration rule).
    #[default(0u64)]
    pub mana_regen_paused_until_ms: u64,
    // The death-streak deadline (`corpse::escalated_reclaim`'s durable state), stamped on every
    // `do_repop`. Dying again before this
    // deadline climbs the reclaim delay (30s → 60s → 120s, capped); dying after it has lapsed resets
    // to the 30s base. 0 = never died (every existing row's default) — `escalated_reclaim` treats
    // `now >= 0` as "past expiry", so a fresh player's first death is the 30s base, same as today.
    // Player-only (creatures don't have corpses/reclaim). `#[default(0i64)]` (typed — an i64 column
    // needs an explicitly-typed literal, same rationale as the u64 fields above) + end-appended so
    // `publish` auto-migrates existing rows (migration rule).
    #[default(0i64)]
    pub death_expire_micros: i64,
    // Dungeon/instance isolation key (work-item 190 slice 1 — pure plumbing, always 0 this slice):
    // 0 = open world; a nonzero value will (slice 2+) identify one `game_instance` row's private
    // population. Folded into the by_grid index alongside map_id so spatial scans can never cross
    // an instance boundary once slice 2 creates real instances; every entity-vs-entity gate that
    // today compares `map_id` gains the matching `instance_id` equality check in the same commit.
    // `#[default(0u64)]` (typed — u64 needs 8 bytes) + end-appended so `publish` auto-migrates
    // existing rows (migration rule). Gateway-subscribed → hand-synced in world_entity_type.rs.
    #[default(0u64)]
    pub instance_id: u64,
    /// GM playtest run-speed multiplier in basis points (10000 = 1.0×, work-item 223's `.speed`).
    /// Live-only (NOT threaded through `Character` — a relog resets it to 1×, an accepted
    /// simplification for a solo-playtest toggle). The gateway relays a change as
    /// `SMSG_FORCE_RUN_SPEED_CHANGE` (`entity_update_to_outbound`), mirroring the existing aura-based
    /// `A_MOD_SPEED(MOVE)` relay (`run_speed_packet`) — player movement is client-authoritative, so
    /// the server-side field alone never speeds the client up. `#[default(10000)]` + END-appended so
    /// `publish` auto-migrates existing rows to "no speed change" (byte-identical to before this
    /// column existed). Gateway-subscribed → hand-synced in `world_entity_type.rs` + widened parity.
    #[default(10000)]
    pub run_speed_mult_bp: u32,
    /// GM playtest godmode (work-item 223's `.god`): INCOMING damage no-ops on a godmode entity
    /// (`spell::apply_target_damage` + both melee swing-resolution sites in `combat/mod.rs`);
    /// OUTGOING damage is unaffected. Live-only (NOT threaded through `Character`, same rationale as
    /// `run_speed_mult_bp` — a relog resets it off). `#[default(false)]` + END-appended so `publish`
    /// auto-migrates existing rows to "not godmode" (byte-identical to before this column existed).
    /// Gateway-subscribed → hand-synced in `world_entity_type.rs` + widened parity (no wire relay: no
    /// client field maps to it, it's a pure server-side gate).
    #[default(false)]
    pub godmode: bool,
    /// Rest state (196): the LIVE resting flag, so `check_rest_state` can detect an inn threshold
    /// crossing with an in-memory compare against the already-loaded mover row — no per-heartbeat
    /// `game_character` lookup (only a threshold FLIP touches the DB). Restored from `Character.resting`
    /// at spawn; persisted back at logout. `#[default(false)]` + END-appended → `publish` auto-migrates.
    /// Gateway-subscribed → hand-synced in `world_entity_type.rs` + widened parity (no wire relay of the
    /// bool itself — the rest byte ships via PLAYER_BYTES_2 in `game_rest_state_event`).
    #[default(false)]
    pub resting: bool,
    /// #456: `(grid_x, grid_y)` packed into ONE indexed value — the AOI subscription's cell key.
    ///
    /// SpacetimeDB 2.7.1's subscription planner can only serve a query from an index when EVERY
    /// column of that index is matched by an equality term, and it skips any index with more than 3
    /// columns outright (`MAX_EXACT_INDEX_COLS`); range predicates are never index-served at all
    /// (`IndexProbe::Range` — "we currently never construct this variant") and an `OR` is evaluated
    /// row-by-row. So the four-column `by_grid` index is unreachable from SQL, and a
    /// `grid_x BETWEEN .. AND grid_y BETWEEN ..` box degrades to a full partition scan — 1.1 BILLION
    /// rows examined on `game_world_entity` in a 445-player measurement, 53% of all writer time.
    /// Folding the two grid columns into one makes `by_cell` a 3-column all-equality index, which the
    /// planner CAN serve, and the AOI box becomes 25 point probes instead of a scan.
    ///
    /// ALWAYS written from `spatial::grid_cell_id(grid_x, grid_y)` in the SAME statement that writes
    /// `grid_x`/`grid_y` — a stale value here does not merely slow a query down, it puts the row in
    /// the wrong cell and shows players the wrong world. `module/src/tripwires.rs::grid_cell_tripwire`
    /// is the enforcement.
    ///
    /// `#[default(0i64)]` (typed — an i64 column needs an explicitly-typed literal) + END-appended so
    /// `publish` auto-migrates. **The default is cell (0, 0), not "unset"**, so every pre-existing row
    /// is mis-addressed until `backfill_cell_ids` re-stamps it — see that reducer for the
    /// post-publish step this migration REQUIRES.
    #[default(0i64)]
    pub cell: i64,
    /// Character-sheet numbers (#517): `spell::recompute_sheet` is the SINGLE chokepoint that writes
    /// these nine fields (base + `A_MOD_STAT`/`A_MOD_COMBAT(ATTACK_POWER)` aura + equipped gear incl.
    /// enchants — the exact same folds `combat::swing_range_ctx` rolls against), so the gateway's
    /// `build_sheet_stats_values` is a plain row read, never a second copy of aura/gear semantics. A
    /// SIGNED bonus per attribute (`effective = base ± this`, e.g. `strength + sheet_str_bonus`); AP is
    /// split at the source into base/mods because vanilla renders it through two DIFFERENT wire fields
    /// (`UNIT_FIELD_ATTACK_POWER` / `_ATTACK_POWER_MODS`). 0 for every existing row (no bonus) and for
    /// creatures (which never call `recompute_sheet`, having no sheet). `#[default(0)]` + END-appended
    /// so `publish` auto-migrates existing rows (the migration rule).
    #[default(0)]
    pub sheet_str_bonus: i32,
    #[default(0)]
    pub sheet_agi_bonus: i32,
    #[default(0)]
    pub sheet_sta_bonus: i32,
    #[default(0)]
    pub sheet_int_bonus: i32,
    #[default(0)]
    pub sheet_spi_bonus: i32,
    #[default(0)]
    pub sheet_ap_base: u32,
    #[default(0)]
    pub sheet_ap_mods: i32,
    #[default(0)]
    pub sheet_dmg_min: u32,
    #[default(0)]
    pub sheet_dmg_max: u32,
    /// Melee crit chance in basis points (#532) — a plain copy of `combat::effective_crit_bp`'s
    /// output, the SAME fold the swing table rolls against (flat base + agility-derived,
    /// level-suppressed + gear crit rating + `A_MOD_COMBAT(CRIT)` auras). No second formula: this
    /// column exists only so the gateway can relay `PLAYER_CRIT_PERCENTAGE` without recomputing crit
    /// itself. 0 for every existing row and for creatures (no sheet). `#[default(0)]` + END-appended
    /// so `publish` auto-migrates.
    #[default(0)]
    pub sheet_crit_bp: u32,
    /// `UNIT_FIELD_BYTES_2` (#101): byte 0 is the SHEATH STATE — 0 = weapons stowed, 1 = melee drawn,
    /// 2 = ranged drawn. Written only by `set_sheathed` (the `CMSG_SETSHEATHED` the client sends on
    /// `Z`), read by the gateway create block + relay so PEERS see a weapon drawn or stowed at all.
    /// Distinct from `unit_bytes_1` (stand state / shapeshift / ghost vis) and from `player_bytes_2`
    /// (facial hair / rest state) — three different wire fields, easy to confuse.
    /// The remaining bytes (1 = PvP flags, 2 = pet flags, 3 = shapeshift) stay 0 until something
    /// needs them; the whole u32 is stored so they don't each cost a migration.
    /// `#[default(0)]` + END-appended so `publish` auto-migrates existing rows (weapons stowed, which
    /// is what every existing row renders as today).
    #[default(0)]
    pub unit_bytes_2: u32,
}

impl WorldEntity {
    /// Is this entity a player (vs a server-authored creature/Unit)? Tests the PLAYER bit of
    /// `OBJECT_FIELD_TYPE`. The single spelling of this check — drives damage amount, the
    /// death-vs-floor decision in the swing tick, the loot/corpse owner test, etc.
    pub fn is_player(&self) -> bool {
        self.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0
    }

    /// The entity's RACE id — byte 0 of `unit_bytes_0` (race|class|gender|powertype). The one
    /// spelling of the unpack; the packing layout itself is pinned by lyracore-shared's
    /// `unit_bytes_0` tests.
    pub fn race(&self) -> u8 {
        (self.unit_bytes_0 & 0xFF) as u8
    }

    /// The entity's CLASS id — byte 1 of `unit_bytes_0`. The single spelling of this shift; callers
    /// should use it rather than hand-unpacking the bitfield.
    pub fn class(&self) -> u8 {
        ((self.unit_bytes_0 >> 8) & 0xFF) as u8
    }
}

/// **The per-mover motion row (perf catalog 2.1).** One row per moving entity, UPDATED IN PLACE,
/// carrying the same `(opcode, movement_info)` payload the old per-recipient `game_movement_event`
/// relay table used to carry (dropped — #350; nothing wrote it any more after this table replaced
/// it).
///
/// Why it exists: the old table inserted one row PER NEARBY PLAYER per movement, so a crowd cost
/// O(C²) event inserts per second through the serialized writer, plus O(C²) rows the reaper deleted
/// a second later. Measured at 200 co-located players that was **70,568 inserts/s and 67,753
/// reaps/s**, and subscription DELIVERY of those rows was 36% of the writer budget — the single
/// largest cost in the system.
///
/// Meanwhile the gateway ALREADY has a grid-scoped per-player subscription that knows exactly who
/// should see this mover (the AOI tracker). Recipient selection is therefore computed twice: once on
/// the writer, once by the subscription engine. This table keeps only the second one — one indexed
/// UPDATE per heartbeat regardless of crowd size, with the AOI box query doing the fan-out.
///
/// `seq` increments per write so a recipient can order (and de-duplicate) motions from one mover;
/// the row is a single PK, so per-mover ordering is preserved by construction.
///
/// A NEW table, so `movement_info: Vec<u8>` is fine — the `Drop`-type restriction in
/// danger-zones §1.6 applies to END-APPENDING a column to an existing table, not to a new one.
#[table(
    accessor = game_entity_motion,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y])),
    // #456: the AOI cell index — see the `cell` column's doc comment.
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct EntityMotion {
    #[primary_key]
    pub guid: u64,
    // The SAME grid address `game_world_entity` carries, so the AOI tracker can subscribe this table
    // with the identical 5×5 box query it already builds for entities.
    pub map_id: u32,
    pub instance_id: u64,
    pub grid_x: i32,
    pub grid_y: i32,
    pub opcode: u16,
    pub movement_info: Vec<u8>,
    pub seq: u32,
    /// #456: `(grid_x, grid_y)` packed into ONE indexed value — the AOI subscription's cell key.
    ///
    /// SpacetimeDB 2.7.1's subscription planner can only serve a query from an index when EVERY
    /// column of that index is matched by an equality term, and it skips any index with more than 3
    /// columns outright (`MAX_EXACT_INDEX_COLS`); range predicates are never index-served at all
    /// (`IndexProbe::Range` — "we currently never construct this variant") and an `OR` is evaluated
    /// row-by-row. So the four-column `by_grid` index is unreachable from SQL, and a
    /// `grid_x BETWEEN .. AND grid_y BETWEEN ..` box degrades to a full partition scan — 1.1 BILLION
    /// rows examined on `game_world_entity` in a 445-player measurement, 53% of all writer time.
    /// Folding the two grid columns into one makes `by_cell` a 3-column all-equality index, which the
    /// planner CAN serve, and the AOI box becomes 25 point probes instead of a scan.
    ///
    /// ALWAYS written from `spatial::grid_cell_id(grid_x, grid_y)` in the SAME statement that writes
    /// `grid_x`/`grid_y` — a stale value here does not merely slow a query down, it puts the row in
    /// the wrong cell and shows players the wrong world. `module/src/tripwires.rs::grid_cell_tripwire`
    /// is the enforcement.
    ///
    /// `#[default(0i64)]` (typed — an i64 column needs an explicitly-typed literal) + END-appended so
    /// `publish` auto-migrates. **The default is cell (0, 0), not "unset"**, so every pre-existing row
    /// is mis-addressed until `backfill_cell_ids` re-stamps it — see that reducer for the
    /// post-publish step this migration REQUIRES.
    #[default(0i64)]
    pub cell: i64,
}

// ── Anti-cheat: movement plausibility (255, tier 1 — DETECT-AND-FLAG, never reject inline) ────────────
// The mangos-anticheat lesson: rubber-banding a false positive is worse than the cheat. We LOG anomalies
// and leave the position write untouched; a GM tool (205) surfaces the flags. Per-character score = the
// COUNT of a guid's rows (no separate counter table — a detect-and-flag MVP doesn't need O(1) reads).
pub const MOVE_VIOLATION_SPEED: u8 = 1; // observed speed over the client's own elapsed time > allowed
pub const MOVE_VIOLATION_TELEPORT: u8 = 2; // a single heartbeat delta larger than any legit step
                                           // Follow-up kinds (fall/gravity, fly/under-world z, wall-clip via nav find_leg) are reserved 3..=5.

/// Speed slack over the effective max: covers latency jitter, diagonal/z movement the 2D check ignores,
/// and packet-time rounding. A real 3× speedhack still trips this; a legit session never should.
const SPEED_LEEWAY: f32 = 1.8;
/// A single heartbeat delta this large is never a legit step (server-side teleport/blink/charge write the
/// stored position FIRST, so the following client delta is small — they're auto-exempt, not special-cased).
const TELEPORT_MAX_YD: f32 = 60.0;
/// Ignore sub-50ms samples for the speed check — the dt is too small to divide by without amplifying jitter
/// into a false spike. The teleport-delta check (distance-only) still applies.
const MIN_DT_S: f32 = 0.05;

/// Score ONE movement delta. Pure (unit-tested). `dist_2d` yd, `dt_s` the CLIENT's own elapsed seconds
/// between this and the prior heartbeat (client-authoritative time → a speedhack's dist/dt still spikes),
/// `max_speed` the mover's effective allowed speed (RUN through `effective_move_speed`, so a snare lowers
/// and Sprint raises it). Returns `(kind, magnitude)` for the worst violation, or `None` when plausible.
/// Magnitude: teleport = the jump distance (yd); speed = the observed multiple of normal (×).
pub fn movement_violation(dist_2d: f32, dt_s: f32, max_speed: f32) -> Option<(u8, f32)> {
    // Teleport ceiling first (distance-only, applies even with an unknown dt).
    if dist_2d > TELEPORT_MAX_YD {
        return Some((MOVE_VIOLATION_TELEPORT, dist_2d));
    }
    if dt_s >= MIN_DT_S && max_speed > 0.0 {
        let observed = dist_2d / dt_s;
        if observed > max_speed * SPEED_LEEWAY {
            return Some((MOVE_VIOLATION_SPEED, observed / max_speed));
        }
    }
    None
}

/// Is the character behind `guid` a GM (`gm_level != 0`)? Used to EXEMPT GMs from anti-cheat movement
/// flagging (255) — a GM's `.speed`/`.tele` are legitimate. A non-player guid (creature) has no
/// `game_character` row → `false` (and creatures aren't scored anyway). Mirrors `gm_command`'s lookup.
fn is_gm_character(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_character()
        .guid()
        .find(guid)
        .map(|c| c.gm_level != 0)
        .unwrap_or(false)
}

/// One heartbeat's raw position/time delta since the mover's last PERSISTED heartbeat — the anti-cheat
/// scorer's whole input, and what `plan_movement` derives `moved` from. Issue #385: bundled instead of
/// 7 loose positional floats/u32s (`score_and_log_movement` used to take 9 arguments counting
/// `ctx`/`guid`; `debug_score_movement` builds one of these from its own flat wire args to call it).
pub(crate) struct MovementDelta {
    pub old_x: f32,
    pub old_y: f32,
    pub old_z: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub old_move_ms: u32,
    pub move_time_ms: u32,
}

impl MovementDelta {
    /// A real translation vs a pure-turn/stationary heartbeat — vanilla breaks a channel on movement,
    /// not on turning in place. Both `movement_update` call sites that used to re-derive this
    /// independently (`break_channel`'s guard and the anti-cheat scoring guard) now read the one
    /// value computed here.
    pub(crate) fn moved(&self) -> bool {
        (self.x - self.old_x).powi(2)
            + (self.y - self.old_y).powi(2)
            + (self.z - self.old_z).powi(2)
            > 0.0001
    }
}

/// Score ONE persisted movement delta against speed/teleport plausibility and, if anomalous, LOG a
/// `game_movement_violation` row (never rejects — the move already persisted). Shared by `movement_update`
/// (the live path) and `debug_score_movement` (server-side verification by explicit guid). `dt_s` uses the
/// CLIENT's own move-time delta (0 when `move_time_ms <= old_move_ms`, which skips only the speed check).
/// Allowed speed is the mover's effective RUN so a snare lowers / Sprint raises the bar.
///
/// EXEMPTIONS (the single policy chokepoint): only PLAYERS are scored, never godmode, and never a GM
/// character (`gm_level != 0`) — a GM's `.speed`/`.tele` are legitimate, so flagging them is always a
/// false positive. Creatures/godmode/GM return without touching the table.
pub(crate) fn score_and_log_movement(ctx: &ReducerContext, guid: u64, delta: &MovementDelta) {
    // Perf catalog 1.22: the three table reads (entity re-fetch, GM character row, aura speed fold)
    // used to run BEFORE the pure math on every moving heartbeat, even though the check almost always
    // returns None. Reordered so the free arithmetic gates them. Byte-identical: the exemptions only
    // decide whether a violation gets LOGGED, and nothing is logged when there is no violation.
    let dist_2d = ((delta.x - delta.old_x).powi(2) + (delta.y - delta.old_y).powi(2)).sqrt();
    let dt_s = if delta.move_time_ms > delta.old_move_ms {
        (delta.move_time_ms - delta.old_move_ms) as f32 / 1000.0
    } else {
        0.0
    };
    // No violation is reachable for ANY `max_speed` here (see `movement_violation`), so skip the reads.
    if dist_2d <= TELEPORT_MAX_YD && dt_s < MIN_DT_S {
        return;
    }
    let max_speed =
        crate::combat::effective_move_speed(ctx, guid, lyracore_shared::constants::speeds::RUN);
    let Some((kind, magnitude)) = movement_violation(dist_2d, dt_s, max_speed) else {
        return; // plausible — the common path, now one indexed aura probe instead of three reads
    };
    match ctx.db.game_world_entity().guid().find(guid) {
        Some(e) if e.is_player() && !e.godmode => {}
        _ => return, // creature / godmode / gone → not scored
    }
    if is_gm_character(ctx, guid) {
        return; // GM: `.speed`/`.tele` are legitimate — never flag
    }
    ctx.db.game_movement_violation().insert(MovementViolation {
        id: 0,
        guid,
        kind,
        magnitude,
        x: delta.x,
        y: delta.y,
        z: delta.z,
        created_at: ctx.timestamp,
    });
}

/// A logged movement-plausibility anomaly (255). One row per flagged delta; the flag is advisory (the
/// move was NOT rejected). Not `public` — server-internal until the GM console (205) surfaces it. These
/// are recent diagnostics, reaped after the shared `EVENT_TTL_MICROS` window so a benchmark's intentional
/// speeders cannot grow the table without bound (issue #211). Query a live character with
/// `SELECT * FROM game_movement_violation WHERE guid = :guid`; durable forensics belongs in reducer logs.
#[table(accessor = game_movement_violation)]
pub struct MovementViolation {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub guid: u64,
    pub kind: u8,       // MOVE_VIOLATION_*
    pub magnitude: f32, // teleport: jump yd; speed: ×normal
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub created_at: spacetimedb::Timestamp,
}

/// A pending teleport for one recipient — the reusable teleport core. A normal position write only moves
/// the entity row; the player's CLIENT camera follows ONLY when the gateway sends the teleport handshake
/// (`MSG_MOVE_TELEPORT_ACK` same-map / `SMSG_TRANSFER_PENDING`+`SMSG_NEW_WORLD` cross-map). So
/// `teleport_player` writes the new position AND inserts one of these; the gateway relays it (branching on
/// whether the live entity is still present post-transaction — see `subscriptions.rs`'s `on_teleport`).
/// `map_id`/`x`/`y`/`z`/`o` are the target — the gateway builds BOTH possible relays from this ONE row
/// (no schema change needed; it already carried everything `SMSG_NEW_WORLD` needs). Used by graveyard
/// release, `debug_teleport`, and any future teleport (hearthstone/.tele/summon).  [event]
#[table(accessor = game_teleport_event, public, index(accessor = by_recipient, btree(columns = [recipient_identity])))]
pub struct TeleportEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub mover_guid: u64,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    /// Insert time (µs since epoch) for the event reaper (gc.rs) — these are transient relay rows. A u64
    /// with a default (not a `Timestamp`) so adding it to the already-published table auto-migrates; a
    /// non-defaulted `Timestamp` column-add aborts the migration.
    #[default(0u64)]
    pub created_micros: u64,
    /// AUTHORITATIVE same-map/cross-map signal for the gateway's `on_teleport` relay (ACK vs
    /// TRANSFER_PENDING/NEW_WORLD). The gateway MUST NOT re-derive this from live-entity presence: with
    /// AOI on, a FAR same-map teleport moves the self entity out of the viewer's grid-scoped subscription,
    /// so it reads absent post-txn and the old proxy wrongly chose the cross-map (loading-screen) path,
    /// hanging the client. Set from `is_cross_map_teleport` here (the module knows both source + dest map).
    #[default(false)]
    pub cross_map: bool,
}

/// Whether a `teleport_player` target lands on a DIFFERENT map than the entity's current one — the
/// single decision point behind both the module's same-map-in-place-update vs. cross-map-despawn branch
/// below, and (mirrored gateway-side via live-entity presence post-transaction, since the two can't share
/// code across the module/gateway boundary) the `on_teleport` relay's ACK vs. TRANSFER_PENDING/NEW_WORLD
/// choice. Pure. [190/224]
pub(crate) fn is_cross_map_teleport(current_map_id: u32, target_map_id: u32) -> bool {
    current_map_id != target_map_id
}

// The teleport primitive's full destination (map/instance/x/y/z/o) plus the actor; the shape mirrors `game_teleport_event`'s columns.
#[allow(clippy::too_many_arguments)]
/// The reusable teleport core: move `player_guid` to `(map_id, instance_id, x, y, z, o)` authoritatively
/// AND emit a `game_teleport_event` so the gateway sends the client the teleport handshake (without it the
/// entity moves but the player's camera stays put). `instance_id` stamps `Character.pending_instance_id`
/// (190's dungeon-instancing substrate) — every current caller passes 0 (open world), so behavior is
/// unchanged until 190 slice 2 lands instanced destinations.
///
/// SAME map (`is_cross_map_teleport` false): byte-identical to before — updates the live entity in place
/// (position + grid cell + instance_id) and the durable `game_character` row so a relog resumes there.
///
/// CROSS map: the client needs a full reload (`SMSG_TRANSFER_PENDING`/`SMSG_NEW_WORLD`, gateway-side), so
/// the live entity is DESPAWNED here rather than moved — `build_player_entity` rebuilds it fresh on the
/// far side once the gateway drives `MSG_MOVE_WORLDPORT_ACK` (mirroring `player_login`'s own entity
/// build). Progression is persisted BEFORE the despawn (`persist_entity`, the same discipline
/// `remove_from_world`/logout uses) so nothing earned on the old map is lost; combat is disengaged and
/// stealth broken for the same reason `remove_from_world` does it on logout — an orphan `game_melee_attack`
/// row or a stale `A_STEALTH` aura would otherwise survive the entity's deletion and corrupt the arrival
/// (see `remove_from_world`'s identical rationale). The durable `Character` row is updated in BOTH
/// branches (single source of truth for the WORLDPORT_ACK rebuild).
///
/// No-op if the player isn't in world.
pub(crate) fn teleport_player(
    ctx: &ReducerContext,
    player_guid: u64,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) {
    let entities = ctx.db.game_world_entity();
    let Some(e) = entities.guid().find(player_guid) else {
        return;
    };
    let recipient_identity = e.owner_identity;
    let cross_map = is_cross_map_teleport(e.map_id, map_id);
    // #461: a movement packet staged before the teleport describes the OLD position. Left queued, the
    // next `publish_motion` firing would relay it up to a tick AFTER the teleport landed and snap
    // every nearby peer's view of this player back to where they were. Drop it in both branches —
    // the authoritative position is the destination, and the player's next heartbeat relays it.
    crate::motion::drop_pending(ctx, player_guid);

    if cross_map {
        // Persist current progression (old position/vitals) BEFORE despawning — matches
        // `remove_from_world`'s persist-then-delete order (logout) so a cross-map hop never loses a
        // freshly-earned ding/loot/HP just because the live row disappears.
        persist_entity(ctx, &e, false);
        // Combat/visibility cleanup mirrors logout — see the doc comment above.
        crate::combat::disengage(ctx, player_guid);
        crate::spell::break_stealth(ctx, player_guid);
        entities.guid().delete(player_guid);
    } else {
        let (grid_x, grid_y) = spatial::grid_cell(x, y);
        let mut e = e;
        e.map_id = map_id;
        e.x = x;
        e.y = y;
        e.z = z;
        e.orientation = o;
        e.grid_x = grid_x;
        e.grid_y = grid_y;
        e.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
        e.instance_id = instance_id;
        entities.guid().update(e);
    }

    // Durable character row — updated in BOTH branches: survives relog, and for a cross-map hop is the
    // ONLY source `build_player_entity` reads from when the gateway rebuilds the entity on WORLDPORT_ACK.
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(player_guid) {
        c.map_id = map_id;
        c.x = x;
        c.y = y;
        c.z = z;
        c.orientation = o;
        c.pending_instance_id = instance_id;
        // Zone follows the destination (the persist_entity stamp's teleport twin): a cross-map
        // hop persisted the OLD position's zone above, and the char-select label reads this row.
        if let Some(zone) = crate::terrain::zone_id_at(ctx, map_id, x, y) {
            c.zone_id = zone;
        }
        chars.guid().update(c);
    }

    // Relay the client handshake — the gateway's `on_teleport` branches same-map (MSG_MOVE_TELEPORT_ACK)
    // vs cross-map (SMSG_TRANSFER_PENDING + SMSG_NEW_WORLD) on the `cross_map` flag stamped here (NOT on
    // live-entity presence — that proxy breaks under AOI for a far same-map teleport, see the field doc).
    ctx.db.game_teleport_event().insert(TeleportEvent {
        id: 0,
        recipient_identity,
        mover_guid: player_guid,
        map_id,
        x,
        y,
        z,
        orientation: o,
        created_micros: ctx.timestamp.to_micros_since_unix_epoch() as u64,
        cross_map,
    });
}

/// Bind a character's hearthstone home to its live entity's CURRENT position — the slice's "make this inn
/// your home" (vanilla binds to the inn's fixed point; we bind where the player stands). `home_zone` is
/// kept from the durable row (the entity carries no zone; ≈ the live zone in single-zone Elwynn). No-op
/// if not in world. [entity]
pub(crate) fn set_home(ctx: &ReducerContext, guid: u64) {
    let Some(e) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(guid) {
        c.home_map = e.map_id;
        c.home_x = e.x;
        c.home_y = e.y;
        c.home_z = e.z;
        c.home_zone = c.zone_id;
        chars.guid().update(c);
    }
}

/// Recall a character to its hearthstone home — an IMMEDIATE teleport via the shared core (the vanilla
/// ~10s channel/cast is a follow-up). No-op if the character row is gone. [entity]
///
/// REFUSE verdict (issue #30). This is the ONE `teleport_player` caller that needs no live entity —
/// it resolves the home coords straight off the durable row — so it is the only route by which
/// `teleport_player`'s unconditional durable-row write (map_id/x/y/z/orientation/pending_instance_id,
/// FIVE `ExportBlob` fields plus the id `in_transit_instances` reads) can land on an escrowed
/// character. Its player path (`items::ops` hearthstone use) already resolves a live entity first, so
/// the fence only bites the by-guid harness twin `debug_use_hearthstone`; fenced HERE rather than
/// there so a future by-guid caller inherits it.
pub(crate) fn recall_to_home(ctx: &ReducerContext, guid: u64) {
    if let Some(c) = crate::helpers::character_by_guid(ctx, guid) {
        // Hearthstone always returns to the open world (instance 0) — even from inside a dungeon.
        teleport_player(
            ctx,
            guid,
            c.home_map,
            0,
            c.home_x,
            c.home_y,
            c.home_z,
            c.orientation,
        );
    }
}

// ===========================================================================================
//  Enter world
// ===========================================================================================

/// The login core, actor-explicit (#468 stage 4d): everything the old sender-path `player_login`
/// did after resolving WHOSE login this is. `owner` is the identity stamped onto the live entity
/// and the character's owner-RLS rows — on the gateway path (`gw::gw_player_login`, the only
/// remaining caller, #483) the account's BOUND identity, so the rows a per-player connection would
/// see stay owned by the identity that connection would present.
pub(crate) fn apply_player_login(
    ctx: &ReducerContext,
    account: &crate::Account,
    character_guid: u64,
    owner: spacetimedb::Identity,
) -> Result<(), String> {
    let chars = ctx.db.game_character();
    let mut character = chars
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no such character: {character_guid}"))?;
    if character.account_id != account.id {
        return Err("character does not belong to caller".to_string());
    }

    // IN-TRANSIT FENCE (issue #16): `begin_transfer` deleted this character's live entity, so every
    // targeting/aggro/threat/AOI gate already cannot see it — login is the one path that could
    // materialise it again, which on a shard the character has left is exactly the dual-liveness
    // dupe the escrow exists to prevent. Refuse until `finish_transfer` (or the reaper) clears the
    // ledger. See `transfer::login_allowed`.
    if crate::transfer::is_in_transit(ctx, character_guid) {
        return Err(format!(
            "character {character_guid} is in transit between shards"
        ));
    }

    // Re-own this character's durable owner-RLS rows (items / learned spells / skills / talents / quest
    // log) to the CURRENT connection identity. The gateway opens a per-player SpacetimeDB connection that
    // mints a FRESH node identity on each (re)connect — notably after every gateway restart — so without
    // this a relog leaves those rows stamped with a PREVIOUS identity and the owner-RLS filter
    // (`owner_identity = :sender`) hides them from the new connection: the player logs in to an empty bag,
    // no learned talents/spells, default skills. Mirrors how the live entity + character row below are
    // rebuilt under `ctx.sender()`. Idempotent — only rewrites rows whose identity already differs.
    restamp_owned_data(ctx, character_guid, owner);

    let entities = ctx.db.game_world_entity();
    // Ghost relog: a stale live entity for this guid (the previous session's logout/disconnect
    // hasn't fired yet) must have its progression persisted back to the character BEFORE we delete
    // and re-insert — otherwise freshly-looted coin / a ding / the latest position would be lost,
    // since the new entity is built from the character row. Persist, delete, then re-read the
    // character so the re-insert reflects the just-saved state.
    if let Some(stale) = entities.guid().find(character_guid) {
        persist_entity(ctx, &stale, false);
        entities.guid().delete(character_guid);
        character = chars.guid().find(character_guid).unwrap_or(character);
    }

    // STRANDING GUARD (work-item 190 slice 3, the design's biggest trap): a login whose
    // `pending_instance_id` names a REAPED instance (logged out inside, the 30min-empty/reset reap
    // ran while offline) must NEVER rebuild into a dead id — the module queries and relay gates
    // would isolate them into an empty phantom world. Fall back per `instance::stranding_fallback`:
    // a known dungeon map → its entrance const at instance 0 (the design doc §3 rule); a
    // non-dungeon map (the dev-map fixture) → in place at instance 0; a dungeon map with no
    // entrance arm (pinned-never by test) → hearthstone home. Alive-or-ghost is PRESERVED per
    // 226's rules (`pending_ghost` untouched — the ghost branch below still applies): a ghost
    // whose corpse was reaped WITH the instance simply has nothing left to reclaim — the
    // vanilla-consistent outcome is SPIRIT HEALER ONLY (same as an expired corpse), documented in
    // the 190 runbook. The rewritten fields persist via the character update at the end of login.
    // NOTE (190 review nit): this checks row EXISTENCE only — a relog into a live instance whose
    // reset_requested flag is set rides the condemned instance and, by occupying it, PINS the
    // reset until they leave (the reaper requires empty). Safe by construction (never reaped out
    // from under a player), just a coherence wrinkle: the resetting leader waits them out.
    if character.pending_instance_id != 0
        && ctx
            .db
            .game_instance()
            .instance_id()
            .find(character.pending_instance_id)
            .is_none()
    {
        let reaped_id = character.pending_instance_id;
        match crate::instance::stranding_fallback(
            crate::instance::entrance_fallback(character.map_id),
            crate::instance::is_dungeon_map(character.map_id),
        ) {
            crate::instance::StrandingFallback::Entrance(m, x, y, z, o) => {
                character.map_id = m;
                character.x = x;
                character.y = y;
                character.z = z;
                character.orientation = o;
            }
            crate::instance::StrandingFallback::InPlaceOpenWorld => {}
            crate::instance::StrandingFallback::HearthstoneHome => {
                spacetimedb::log::warn!(
                    "player_login: dungeon map {} has no entrance_fallback arm — diverting {} to hearthstone",
                    character.map_id, character.guid
                );
                character.map_id = character.home_map;
                character.x = character.home_x;
                character.y = character.home_y;
                character.z = character.home_z;
                character.orientation = 0.0;
            }
        }
        character.pending_instance_id = 0;
        spacetimedb::log::info!(
            "player_login: instance {reaped_id} was reaped — {} falls back to map {} at instance 0",
            character.guid,
            character.map_id
        );
    }
    // A relog comes back ALIVE (we don't persist ghost state across a REAL logout), so clear any
    // leftover corpse — else it orphans (rendered with no owning ghost, with a stale reclaim marker
    // that MSG_CORPSE_QUERY keeps offering for a now-alive player). Idempotent (no-op if none).
    // EXCEPTION (work-item 226): `pending_ghost` means this world entry is the rebuild half of a
    // released ghost's despawn (a cross-map graveyard release, or a reconnect that raced the ghost's
    // corpse run) — the corpse IS the ghost's reclaim target and MUST survive the rebuild, or a
    // Deadmines death would silently resurrect corpseless at the Westfall graveyard.
    if !character.pending_ghost {
        ctx.db
            .game_corpse()
            .guid()
            .delete(crate::corpse::corpse_guid_for(character_guid));
    }

    // Build the live entity from the durable character row — the SAME construction
    // `debug_spawn_player_entity` runs, factored into `build_player_entity`. `owner_identity` is the
    // connection's bound identity (`ctx.sender()`); `account.id` is identical to `character.account_id`
    // (gated above), so the builder reads `account_id` from the character row.
    let mut entity = crate::build_player_entity(ctx, &character, owner);
    // Work-item 226: a preserved ghost rebuilds AS a ghost — `build_player_entity` always builds
    // alive (it's the shared player construction, creature-side code), so the released-ghost state
    // `persist_entity` stamped is re-applied here, the one player-owned call site. Health 1 matches
    // both `do_repop`'s release and the `persist_entity` clamp (`health.max(1)`).
    if character.pending_ghost {
        let (dead, health, player_flags, unit_bytes_1) =
            ghost_restored_fields(entity.player_flags, entity.unit_bytes_1);
        entity.dead = dead;
        entity.health = health;
        entity.player_flags = player_flags;
        entity.unit_bytes_1 = unit_bytes_1;
    }
    entities.insert(entity);

    // Starter-loadout safety net: creation grants the loadout (so char-select shows gear), and
    // this idempotent call (no-op if the character owns ANY item) covers characters created
    // before that change. It also re-scopes nothing — owner_identity here is the live connection,
    // matching what the restamp sweep would set anyway.
    crate::items::grant_starter_item(ctx, character.guid, owner);

    // Weapon skill (rank 14): lazily seed this character's skill lines (Defense + Unarmed + the now-equipped
    // weapon's line) at the level cap, owner-scoped for the RLS. AFTER grant_starter_item so the starter
    // weapon's line is seeded too. Idempotent (no-op if rows exist), so a relog never duplicates. Seeding at
    // cap keeps a fresh login baseline (skill_diff vs an equal-level foe = 0 → combat byte-identical).
    crate::skill::ensure_player_skills(
        ctx,
        character.guid,
        owner,
        character.level as u32,
        character.class,
    );

    // Talents (rank 27): re-apply every learned passive talent's aura at login (idempotent — refreshes by
    // effect_id, never stacks). A character with no learned talents applies nothing (baseline-safe).
    crate::talent::apply_learned_talents(ctx, character.guid, owner, character.level as u32);

    // Racial passives (per-race createinfo): apply the always-on racial auras (Human → The Human Spirit
    // +Spirit, weapon specs, Diplomacy). Reads SPELL_ATTR_PASSIVE from the imported spell, so an unimported
    // or ACTIVE racial (Perception) is skipped. BEFORE recompute_vitals so the +stat racial folds into vitals.
    crate::spell::apply_racial_passives(ctx, character.guid, character.race, character.level);

    // Parity #8: fold the now-equipped starter gear's Stamina/Intellect into max HP/mana. The entity was
    // built from the bare level curve; grant_starter_item + apply_learned_talents have since equipped gear
    // and applied talent auras, so re-derive vitals once here (a no-talent newbie still gets its gear HP;
    // recompute_vitals no-ops if nothing moved). Health may sit a hair under the new max until the first
    // regen tick — negligible at newbie gear levels. (Deliberate simplification: full-heal-to-new-max on login if it ever bites.)
    crate::spell::recompute_vitals(ctx, character.guid);
    crate::spell::recompute_sheet(ctx, character.guid);

    // Rested XP (rank 30): accrue rested XP for the time spent logged out (5%/8h, capped 1.5 levels),
    // then consume the logout stamp so a re-login can't double-accrue. A never-logged-out character
    // (stamp 0) accrues nothing. Folded into the single character update below.
    let (rested, consumed) = crate::xp::accrue_rested_on_login(
        ctx.timestamp.to_micros_since_unix_epoch(),
        character.last_logout_micros,
        character.rested_xp,
        character.level as u32,
        character.resting, // 196: full offline rate if logged out in a rest area, else 1/4 (in-field)
    );
    character.rested_xp = rested;
    if consumed {
        character.last_logout_micros = 0;
    }
    // Rest state (196): resume the LIVE accrual clock if this character logs back in still flagged
    // resting (logged out in an inn). The first movement heartbeat re-detects and clears it if they've
    // since walked out. The spawn baked the RESTED byte into PLAYER_BYTES_2 from `character.resting`.
    if character.resting {
        character.rested_since_micros = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    }

    // Played time (/played): stamp the start of this fresh session so `persist_entity` can fold its
    // elapsed span into `played_total_secs` on the next logout/disconnect/ghost-relog. Overwrites any
    // stale stamp unconditionally — a live session (session_start_micros already nonzero) means the
    // prior logout never persisted cleanly, but `player_login`'s ghost-relog branch above already
    // called `persist_entity` on the stale entity before we got here, so that span is already folded
    // in; re-stamping here just starts the clock for this new session.
    character.session_start_micros = ctx.timestamp.to_micros_since_unix_epoch() as u64;

    character.online = true;
    character.first_login = false;
    chars.guid().update(character);
    // Notify-hook: the login is fully committed (entity live, rows restamped, character row
    // updated). Server-side bots don't fire this — they enter via their own spawn path.
    crate::hooks::fire_on_login(ctx, &crate::hooks::LoginPayload { character_guid });
    Ok(())
}

/// Re-stamp every owner-RLS-scoped row this character owns to `identity` (the caller's current bound
/// identity). Iterates `CHARACTER_OWNED_RESTAMP_SWEEPS` — the build-time-generated
/// registry of every table that declared itself via a `character_owned` `restamp` marker
/// colocated with its own definition (today: items, learned spells, skills, talents, and the quest
/// log). Each sweep fn is a no-op for rows already owned by `identity`, so a normal relog within one
/// gateway process costs nothing; it only does real work after the per-player identity changed (a
/// gateway restart). See the call site in `player_login` for the why. [entity]
fn restamp_owned_data(ctx: &ReducerContext, character_guid: u64, identity: Identity) {
    for sweep in crate::CHARACTER_OWNED_RESTAMP_SWEEPS {
        sweep(ctx, character_guid, identity);
    }
}

/// Cascade-delete `character_guid`: the character row AND every owner-scoped row it owns + its
/// in-world entity. The owner-scoped rows are swept via `CHARACTER_OWNED_DELETE_SWEEPS` — the
/// build-time-generated registry of every table that declared itself via a
/// `character_owned` `delete` marker colocated with its own definition (today: items, learned
/// spells, skills, talents, quest log, buyback, reputation). This is what makes a freed guid safe to
/// REUSE: a fully-deleted guid owns nothing, so a new character created onto it (guid reuse) inherits
/// nothing (a raw SQL delete that left these rows behind is what caused the Warrior-wore-Warlock-gear
/// bug). The CMSG_CHAR_DELETE handler calls this after an ownership check.
///
/// ENFORCEMENT: there is no schema-level FK/cascade in SpacetimeDB, so a NEW durable table keyed by a
/// character guid still needs a `character_owned` marker in its own file (build.rs's
/// scan generates the registry from those markers; a tripwire test source-scans for character-keyed
/// `#[table]` structs missing one). Corpse/world-entity/character themselves stay hardcoded below —
/// they're the character's own identity rows, not "owned data" the registry covers. Transient
/// combat/event rows (auras, combo points, lockouts, pending casts) are exempt from all of this —
/// they die with the live entity/session, not the durable character. [entity]
pub(crate) fn cascade_delete_character(ctx: &ReducerContext, character_guid: u64) {
    // Issue #59 defect 1: floor the guid allocator at `character_guid` BEFORE it disappears, on
    // EVERY delete path (this is the one chokepoint all of them share — transfer, CMSG_CHAR_DELETE,
    // debug_delete_character). A lazily-seeded allocator that has never been touched yet seeds
    // itself from a scan of the SURVIVING rows the moment `next_character_guid` first runs — which,
    // without this, is a scan that no longer sees the guid this call is about to delete, so the
    // very first local `create_character` after an untouched database's first-ever transfer could
    // re-issue it. Ratcheting here closes the window regardless of seed timing: this call always
    // floors the mark at the exact guid leaving the table, so the scan never gets a chance to miss it.
    crate::auth::bump_guid_high_water(ctx, character_guid);
    for sweep in crate::CHARACTER_OWNED_DELETE_SWEEPS {
        sweep(ctx, character_guid);
    }
    // UNIT-keyed transient rows the marker system can't see (their columns name ANY unit —
    // target_guid/owner_guid — so the character_owned tripwire never flags the tables): combo
    // rows. Without this a recycled guid inherits them — the exact "non-item case" auth.rs's
    // allocation comment predicted: a fresh character spawned wearing a despawned bot's
    // resurrection sickness (live find, 2026-07-19).
    //
    // Auras ON the character used to be hand-deleted here too, for the identical reason
    // (`target_guid` is not one of the tripwire's magic field names). Issue #72's hot-state audit
    // gave `game_aura` a real `character_owned!` marker instead (`sweep_delete_game_aura` /
    // `sweep_transfer_game_aura`, `spell/tables.rs`) — the delete half above already runs it via
    // `CHARACTER_OWNED_DELETE_SWEEPS`, and the marker is also what lets a warm handoff carry the
    // rows across a database boundary, which this hand-roll never could.
    {
        use crate::combo::game_combo_point;
        let combos = ctx.db.game_combo_point();
        let stale: Vec<u64> = combos
            .iter()
            .filter(|c| c.owner_guid == character_guid || c.target_guid == character_guid)
            .map(|c| c.id)
            .collect();
        for id in stale {
            combos.id().delete(id);
        }
    }
    // Corpse: deterministic guid (one per player), same lookup pattern as the other corpse_guid_for call sites.
    // MELEE + THREAT are unit-keyed exactly like the combo rows above (attacker/target, creature/source),
    // so the `character_owned` marker system cannot see them either — and unlike combo points they were
    // never handled here. A deleted character therefore left every creature that had been fighting it
    // ENGAGED WITH A GHOST: the aggro pass skips any creature that is already an attacker, so it could
    // never be re-armed, and nothing ever disengaged it. Despawning 300 bots orphaned ~100 creatures
    // this way (live, 2026-07-29) and they piled onto the only player left standing.
    //
    // Issue #365: this used to hand-roll `combat::disengage` inline (delete the outgoing row, free
    // attackers via `by_target`, clear_target each, `threat::clear_for_unit`) — the same three steps
    // the logout path (below) and the cross-map teleport path call the real helper for. The hand-roll
    // had drifted: `disengage` ALSO drops IN_COMBAT (+ zeroes `combat_until_ms`) on any attacker left
    // with no remaining engagement (the 249 rule), which the copy silently omitted — attackers of a
    // deleted character kept the flag set forever. Routing through the canonical helper closes that
    // gap and keeps this chokepoint from drifting again.
    crate::combat::disengage(ctx, character_guid);
    ctx.db
        .game_corpse()
        .guid()
        .delete(crate::corpse::corpse_guid_for(character_guid));
    crate::motion::drop_pending(ctx, character_guid); // #461: staged-but-unpublished motion too
    ctx.db.game_entity_motion().guid().delete(character_guid); // see the despawn path
    ctx.db.game_world_entity().guid().delete(character_guid);
    ctx.db.game_character().guid().delete(character_guid);
}

/// Debug: cascade-delete a character (test cleanup). The real CMSG_CHAR_DELETE path reuses
/// `cascade_delete_character` behind an ownership check; this debug entry is otherwise gate-free for
/// the harness.
///
/// REFUSE verdict (issue #30) — the SAME verdict `auth::delete_character` carries, for the same
/// reason: this destroys a durable copy another shard holds a claim on. It is strictly worse here
/// than there, because `cascade_delete_character` runs the `character_owned!` sweep, which includes
/// `sweep_delete_game_transfer_out` — so an unfenced call deletes the character AND both escrow rows
/// in one transaction. Cross-database that leaves the destination's arrival copy with no source
/// out-row, so `recovery` answers `Hold` forever and the character is wedged frozen. Returns `Err`
/// rather than silently no-opping so a harness script that hits the window sees why.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_delete_character(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    if crate::helpers::character_by_guid(ctx, character_guid).is_none()
        && ctx
            .db
            .game_character()
            .guid()
            .find(character_guid)
            .is_some()
    {
        return Err("CHAR_IN_TRANSIT".to_string());
    }
    cascade_delete_character(ctx, character_guid);
    Ok(())
}

// ===========================================================================================
//  Movement: persist + relay
// ===========================================================================================

/// #110 ground truth. The gateway reports submitting ~400 movement calls/s at 200 players and
/// receiving a completion for every one, while `spacetime_num_txns_total{reducer="movement_update"}`
/// reports ~200/s. Exactly one of those is wrong, and no amount of gateway-side instrumentation can
/// say which — this counts ENTRIES TO THE REDUCER ITSELF, which is the only unambiguous witness.
///
/// Logged every 2000 entries rather than per call: a per-call log would dominate the very hot path
/// being measured. A wasm module instance is single-threaded, so `Relaxed` is free here.
static MOVEMENT_ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How far the STORED position may lag the client's before a heartbeat must be persisted.
///
/// 4 yards, not the catalog's suggested ~10: everything server-authoritative reads this row — melee
/// reach, interact range, aggro radius — and a range gate that disagrees with the client by more
/// than its own reach is a gameplay bug, not a performance trade.
pub(crate) const PERSIST_MAX_DRIFT_YD: f32 = 4.0;

/// **Does this heartbeat have to be written to the ~60-column entity row?** (perf catalog 2.2)
///
/// `movement_update` used to rewrite the full public row on EVERY heartbeat, and SpacetimeDB then
/// evaluated that row against every in-box subscription and shipped a delta to each. Since 2.1 peers
/// are animated from `game_entity_motion`, not from this row, so most of that work produced nothing
/// any client rendered.
///
/// The row is persisted only when something OBSERVABLE changed:
///
///   - **grid cell** — AOI correctness; also the gate `check_area_exploration` rides;
///   - **movement flags** — a late observer's CREATE must encode the right move state;
///   - **resting / health** — real state, not position (see the caller's mutator audit);
///   - **any non-heartbeat opcode** — start/stop/turn/jump/fall edges, which is what makes a pure
///     facing change (`MSG_MOVE_SET_FACING`) persist even though it moves the unit nowhere;
///   - **drift past [`PERSIST_MAX_DRIFT_YD`]**.
///
/// ⚠ **The deliberate consequence: `last_move_ms` goes stale for a SLOW mover**, because it only
/// advances on a persisted heartbeat. Two consumers read it as "is this unit moving right now" —
/// `CHASE_TARGET_MOVING_MS` (700 ms; a mob plants and swings instead of chasing) and
/// `MELEE_LEEWAY_WINDOW_MS` (1200 ms; a chase's +8/3 yd swing reach). At run speed (7 yd/s) 4 yards
/// is ~570 ms, inside both windows, so a running target is never mistaken for a stationary one. A
/// WALKING target can exceed them — and reading a 2.5 yd/s walker as stationary is arguably the more
/// correct answer for both consumers, since the mob is already inside its reach. Anything that
/// tightens those windows below ~600 ms, or raises this threshold, has to revisit that.
pub(crate) fn snapshot_needs_persist(
    opcode: u16,
    grid_changed: bool,
    flags_changed: bool,
    state_changed: bool,
    drift_sq: f32,
) -> bool {
    opcode as u32 != lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT
        || grid_changed
        || flags_changed
        || state_changed
        || drift_sq > PERSIST_MAX_DRIFT_YD * PERSIST_MAX_DRIFT_YD
}

/// Environmental damage, absorbed out of `movement_update`'s own inline block (issue #385): given the
/// shared curve's damage figure (`lyracore_shared::env::fall_damage`, already computed by the caller
/// from the client's airborne time + max_health) and the mover's CURRENT health, decide the health to
/// carry forward and whether the landing is lethal. A lethal fall does NOT subtract here — the
/// position persists first (if `MovementPlan::persist_entity` says so) and `combat::kill_player`
/// re-fetches fresh (the shared death funnel: channel teardown, durability, on_death hooks — identical
/// to a melee death, so release/reclaim works). `dmg == 0` (a soft landing) is a no-op pass-through.
/// Pure/testable.
pub(crate) fn resolve_environmental_damage(dmg: u32, health: u32) -> (u32, bool) {
    if dmg == 0 {
        return (health, false);
    }
    if health <= dmg {
        (health, true) // lethal: caller does not subtract; kill_player re-fetches
    } else {
        (health - dmg, false)
    }
}

/// What `movement_update` decided to DO with one heartbeat — computed once every impure,
/// DB-touching mutator on the path (exploration/rest, which can flip `resting`/`health`/`xp`) has
/// already run against `mover`. From here on the reducer does nothing but execute this: no further
/// branching on raw movement state happens after `plan_movement` returns.
///
/// Issue #385: this replaces the reducer's own formatted source as what the "relay is never gated on
/// the persist decision" invariant pins against — a whitespace-collapsed exact-body match that broke
/// on every rename or rustfmt re-wrap. `relay_motion` is unconditionally `true` (see `plan_movement`'s
/// doc) regardless of every other field, which is exactly the shape `#109` regressed on once (only the
/// entity row gated; the per-mover motion relay never is) — and it is now an ordinary value-level unit
/// test (`plan_movement`'s tests, below) instead of a source scan.
pub(crate) struct MovementPlan {
    /// Write the (possibly-mutated) entity row back — gated on [`snapshot_needs_persist`].
    pub persist_entity: bool,
    /// Relay this heartbeat's motion to nearby peers via `game_entity_motion`. Always `true` by
    /// construction — see the struct doc.
    pub relay_motion: bool,
    /// A lethal fall (see [`resolve_environmental_damage`]): the position already persisted (if
    /// `persist_entity`), `combat::kill_player` runs next.
    pub fall_lethal: bool,
    /// A real translation, not a pure turn ([`MovementDelta::moved`]) — gates the channel break and
    /// the anti-cheat scorer.
    pub moved: bool,
}

/// Build the plan `movement_update` executes. Pure — no `ReducerContext` — so every decision this
/// used to make inline (and that the old source-scan pin existed to guard) is now a plain function of
/// its inputs, directly unit-tested below.
pub(crate) fn plan_movement(
    opcode: u16,
    grid_changed: bool,
    flags_changed: bool,
    state_changed: bool,
    drift_sq: f32,
    fall_lethal: bool,
    delta: &MovementDelta,
) -> MovementPlan {
    MovementPlan {
        persist_entity: snapshot_needs_persist(
            opcode,
            grid_changed,
            flags_changed,
            state_changed,
            drift_sq,
        ),
        relay_motion: true,
        fall_lethal,
        moved: delta.moved(),
    }
}

/// The movement core, actor-explicit (#468 stage 4): everything the old sender-path `movement_update`
/// did after resolving WHO moved. The trusted gateway path (`gw::gw_movement_update`) delegates here
/// — same shape as every `actor.rs` verb, factored out per that file's own rule for still-inlined cores.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_movement_update(
    ctx: &ReducerContext,
    mut mover: WorldEntity,
    opcode: u16,
    movement_info: Vec<u8>,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    move_time_ms: u32,
) -> Result<(), String> {
    // #110 ground truth — see MOVEMENT_ENTRIES. Counted in the CORE, not the sender reducer, so
    // the gateway path's heartbeats land in the same total.
    {
        let n = MOVEMENT_ENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n.is_multiple_of(2000) {
            spacetimedb::log::info!("MOVEMENT_ENTRIES total={n}");
        }
    }
    let entities = ctx.db.game_world_entity();

    // A dead player on the Release Spirit screen can't move — drop the heartbeat so the
    // body's authoritative position doesn't walk and peers don't see a 0-HP unit glide. EXCEPTION:
    // a GHOST (dead + PLAYER_FLAGS_GHOST) CAN move — it has to run back to its corpse — so
    // only the not-yet-released dead state is frozen.
    if mover.dead && mover.player_flags & lyracore_shared::constants::player_flags::GHOST == 0 {
        return Ok(());
    }

    // Drop stale / out-of-order heartbeats.
    if move_time_ms != 0 && move_time_ms <= mover.last_move_ms {
        return Ok(());
    }

    // Reject non-finite coordinates. A malicious client sending NaN/Inf would poison the stored
    // position AND the AOI grid (spatial::grid_cell floors x/y → an undefined i32 cell), and peers
    // would see the unit jump to garbage. Drop the heartbeat like a stale one (don't error-spam).
    if !x.is_finite() || !y.is_finite() || !z.is_finite() || !o.is_finite() {
        return Ok(());
    }

    // Capture the PRE-MOVE position so we can tell a real translation (which breaks a channel) from a
    // pure-turn / stationary heartbeat (which does not) — vanilla breaks a channel on movement, not on
    // turning in place. Compared below, after the row is persisted.
    let (old_x, old_y, old_z) = (mover.x, mover.y, mover.z);
    // Exploration (200): the pre-move grid cell, to gate the area check to a real cell crossing below.
    let (old_gx, old_gy) = (mover.grid_x, mover.grid_y);
    // Anti-cheat (255): the mover's prior heartbeat time, for the speed check's dt. Read here before the
    // row overwrites last_move_ms below. The player/godmode/GM exemptions live in score_and_log_movement.
    let old_move_ms = mover.last_move_ms;
    // SNAPSHOT PERSISTENCE (perf catalog 2.2): the fields whose change forces the row write below.
    // Verified against each mutator on this path rather than assumed:
    //   - `rest::check_rest_state` early-returns unless `resting` flips, and writes `player_bytes_2`
    //     only inside that branch — so watching `resting` covers the rest byte too;
    //   - `health` is fall damage's (a LETHAL fall defers to `kill_player`, which re-fetches);
    //   - `exploration::check_area_exploration` can move xp/level, but only fires on a GRID CHANGE,
    //     which is itself a persist trigger.
    // So these three plus grid/flags/opcode/drift cover every mutator between here and the write.
    let (old_flags, old_resting, old_health) = (mover.movement_flags, mover.resting, mover.health);

    // (state) persist position so a late observer's CREATE_OBJECT is correct.
    let (grid_x, grid_y) = spatial::grid_cell(x, y);
    mover.x = x;
    mover.y = y;
    mover.z = z;
    mover.orientation = o;
    mover.grid_x = grid_x;
    mover.grid_y = grid_y;
    mover.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
    mover.last_move_ms = move_time_ms;
    // Stamp the live movement flags (leading u32 of the MovementInfo wire layout, LE) so a peer's
    // CREATE encodes the current move state instead of idle (see WorldEntity::movement_flags).
    mover.movement_flags = movement_info
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(0);
    let mover_guid = mover.guid;
    let map_id = mover.map_id;
    let instance_id = mover.instance_id;
    // FALL DAMAGE (058): a landing packet carries the client's airborne time — fold it through the
    // shared curve (lyracore_shared::env, the SAME one the gateway's flavor-log line uses). The
    // health/lethal DECISION is `resolve_fall_damage` (pure, tested); this block only computes the
    // curve's raw damage figure and applies the decision. Godmode and already-dead movers skip.
    let mut fall_lethal = false;
    if opcode as u32 == lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND
        && !mover.dead
        && !mover.godmode
        && mover.is_player()
    {
        if let Some(ft) = lyracore_shared::env::fall_time_from_movement_info(&movement_info) {
            let dmg = lyracore_shared::env::fall_damage(ft, mover.max_health);
            let (health, lethal) = resolve_environmental_damage(dmg, mover.health);
            mover.health = health;
            fall_lethal = lethal;
        }
    }
    // Exploration (200): on crossing into a new grid cell, award discovery XP if this is a fresh
    // subzone. Gated on the grid change so the area lookup + dedup run ~once per 50yd, not per
    // heartbeat; folds any XP/ding into the single `update` below (mutates `mover`).
    if mover.is_player() && !mover.dead && (grid_x != old_gx || grid_y != old_gy) {
        crate::exploration::check_area_exploration(ctx, &mut mover);
    }
    // Rest state (196): not grid-gated (an inn is smaller than a 50yd cell) but THROTTLED to
    // ~1Hz per mover (#482): gate on the heartbeat clock crossing a second boundary. At run
    // speed that is a check every ~7yd — still finer than any inn — and it removes a per-
    // heartbeat rest evaluation from the hottest path on the server (measured: the per-move
    // hook chain was ~58µs at 10k moves/s).
    if mover.is_player() && !mover.dead && (move_time_ms / 1000 != old_move_ms / 1000) {
        crate::rest::check_rest_state(ctx, &mut mover);
    }
    // Breath shares the ~1 Hz movement gate with rest state, but only records the underwater edge;
    // its own scheduled tick advances the non-spatial timer while a player is standing still.
    if mover.is_player()
        && !mover.dead
        && !mover.godmode
        && mover.player_flags & lyracore_shared::constants::player_flags::GHOST == 0
        && (move_time_ms / 1000 != old_move_ms / 1000)
    {
        let liquid_level = crate::terrain::liquid_level_at(ctx, mover.map_id, mover.x, mover.y);
        let submerged = lyracore_shared::env::is_submerged(
            mover.z,
            liquid_level.unwrap_or_default(),
            liquid_level.is_some(),
            mover.movement_flags,
        );
        crate::breath::update_breath_edge(ctx, &mover, submerged);
    }
    // `old_x/old_y/old_z` are the last PERSISTED position, so this drift is exactly how far the
    // stored row has fallen behind the client.
    let drift_sq = (x - old_x).powi(2) + (y - old_y).powi(2) + (z - old_z).powi(2);
    let delta = MovementDelta {
        old_x,
        old_y,
        old_z,
        x,
        y,
        z,
        old_move_ms,
        move_time_ms,
    };
    // THE PLAN: every remaining decision on this heartbeat, made ONCE, purely (see `MovementPlan`'s
    // doc for why the reducer applies it rather than deciding anything itself from here on).
    let plan = plan_movement(
        opcode,
        grid_x != old_gx || grid_y != old_gy,
        mover.movement_flags != old_flags,
        mover.resting != old_resting || mover.health != old_health,
        drift_sq,
        fall_lethal,
        &delta,
    );

    if plan.persist_entity {
        entities.guid().update(mover);
    }
    if plan.fall_lethal {
        // Killer = self: no kill credit, no loot — an environmental death.
        crate::combat::kill_player(ctx, mover_guid, mover_guid);
    }

    // BREAK the mover's CHANNEL on a real move (Arcane Missiles stops if you walk). Only on an actual
    // translation — a pure-turn / stationary heartbeat (same x/y/z, only orientation changed) does NOT
    // break it, matching vanilla. `break_channel` is a no-op for a mover with no channel/cast (the common
    // path), so the `plan.moved` guard is just to skip the table scan on a non-move heartbeat.
    if plan.moved {
        crate::spell::break_channel(ctx, mover_guid);
    }

    // Anti-cheat (255): score this delta for speed / teleport plausibility and LOG (never reject) any
    // anomaly (exemptions — player-only, godmode, GM — are enforced inside the helper). Server-side
    // teleport/blink/charge write the stored position first, so the following client delta is small and
    // auto-exempt (no special-case).
    if plan.moved {
        score_and_log_movement(ctx, mover_guid, &delta);
    }

    // PER-MOVER MOTION ROW (perf catalog 2.1) — the ONLY movement relay path. Gated on
    // `plan.relay_motion` — which `plan_movement`'s own tests prove is ALWAYS `true` — never on
    // `plan.persist_entity` or any raw movement state re-derived here: recreating that coupling is
    // exactly the #109 regression this used to guard against with a much larger, rename-brittle scan.
    //
    // This replaced a 25-probe recipient scan plus one `game_movement_event` INSERT per nearby
    // player: O(C) writes per heartbeat, so O(C²) per second zone-wide, plus the same again for the
    // reaper a second later (measured at 200 co-located: 70,568 inserts/s, 67,753 reaps/s). The
    // gateway's AOI tracker already subscribes this exact 5×5 box, so recipient selection was being
    // computed twice — once here, once by the subscription engine. Only the second one remains.
    //
    // #461: it no longer writes the PUBLIC `game_entity_motion` row inline. SpacetimeDB pays its
    // subscription sweep per TRANSACTION, so one transaction per movement packet cost 7.8 ms/call at
    // 500 players (98.5% of it subscription work, 1.5% our wasm). `queue_motion` stages the payload
    // in the PRIVATE `game_entity_motion_pending` table — no subscribers, therefore no sweep — and
    // `motion::publish_motion` republishes every staged mover into the public relay in ONE
    // transaction at 20 Hz. Everything above this line, `persist_entity`/`snapshot_needs_persist`
    // included, is unchanged.
    //
    // `instance_id`/`map_id`/grid come from the mover we just persisted, so the motion row and the
    // entity row can never disagree about which box they belong to (that agreement is what stops the
    // "spawned-but-frozen" desync the old shared-GridBox comment described).
    if plan.relay_motion {
        crate::motion::queue_motion(
            ctx,
            mover_guid,
            map_id,
            instance_id,
            grid_x,
            grid_y,
            opcode,
            movement_info,
        );
    }

    Ok(())
}

// ===========================================================================================
//  Targeting
// ===========================================================================================

/// The target-write core, actor-explicit (#468 stage 4a) — `gw::gw_set_target` delegates here.
pub(crate) fn apply_set_target(
    ctx: &ReducerContext,
    mut player: WorldEntity,
    target_guid: u64,
) -> Result<(), String> {
    player.target_guid = target_guid;
    ctx.db.game_world_entity().guid().update(player);
    Ok(())
}

/// Max distance to inspect another player's equipment (`CMSG_INSPECT`) — (10 yd)², the same
/// interaction-range convention as the vendor/quest-giver/loot gates (`QUEST_GIVER_RANGE_SQ` et al).
const INSPECT_RANGE_SQ: f32 = 100.0;

/// The pure INSPECT gate: may `target` (a PLAYER, `same_map`, `dist_sq` away, and `friendly` to the
/// caller) be inspected? Factored out of the `inspect` reducer (like `loot::can_skin`) so the gate is
/// unit-testable without a live `ReducerContext`. Returns the reject reason, or `Ok(())` to proceed —
/// the gateway's success reply is `SMSG_INSPECT(target_guid)`; a rejection is silently ignored (the
/// client just never opens the window).
pub(crate) fn can_inspect(
    target_is_player: bool,
    same_map: bool,
    dist_sq: f32,
    friendly: bool,
) -> Result<(), String> {
    if !target_is_player {
        return Err("inspect target is not a player".to_string());
    }
    if !same_map {
        return Err("inspect target on another map".to_string());
    }
    if dist_sq > INSPECT_RANGE_SQ {
        return Err("inspect target out of range".to_string());
    }
    if !friendly {
        return Err("inspect target is not friendly".to_string());
    }
    Ok(())
}

/// Inspect another player (`CMSG_INSPECT`): validate the target is a real in-world player, on the
/// caller's map, in range, and friendly (vanilla never lets you inspect an enemy-faction player).
/// Stateless — no mutation, no persisted row (mirrors `quest::validate_giver`'s read-only gate); a
/// success just lets the gateway reply `SMSG_INSPECT(target_guid)` so the client opens the paperdoll
/// (full equipment display additionally needs the target's visible-item fields, tracked separately).
/// The faction gate is SKIPPED when `FactionTemplate` data isn't loaded (table empty), the same
/// fail-open convention as `combat::start_attack`'s friendly-gate, so a dev/test server without an
/// imported `FactionTemplate.dbc` never blocks inspect.
///
/// The inspect core, actor-explicit (#479) — same split as [`apply_set_target`].
pub(crate) fn apply_inspect(
    ctx: &ReducerContext,
    inspector: crate::WorldEntity,
    target_guid: u64,
) -> Result<(), String> {
    let target = crate::helpers::live_entity(ctx, target_guid)
        .map_err(|_| "no such inspect target".to_string())?;
    let dist_sq = crate::helpers::dist_sq(&inspector, &target);
    let friendly = ctx.db.game_faction_template().count() == 0
        || crate::faction::is_friendly(ctx, inspector.faction_template, target.faction_template);
    can_inspect(
        target.is_player(),
        target.map_id == inspector.map_id && target.instance_id == inspector.instance_id,
        dist_sq,
        friendly,
    )
}

// ===========================================================================================
//  Death / revive
// ===========================================================================================

/// Release Spirit after death (`CMSG_REPOP_REQUEST` — the "Release Spirit" click): become a ghost at
/// 1 HP and teleport to the nearest graveyard, leaving a corpse behind at the death spot (see
/// `do_repop`). The client leaves the death screen purely from `UNIT_FIELD_HEALTH > 0` (vanilla 1.12
/// has no "you are alive" opcode — death and revive are both field replication). No-op if the caller
/// isn't dead.
///
/// The `repop` body keyed off an explicit guid — shared by `gw::gw_repop` (resolves the actor guid
/// from the shared connection) and `debug::debug_repop` (a CLI `spacetime call` drives the
/// ghost/release transition by guid, so the death loop is verifiable headless). No-op if the entity
/// is missing.
pub(crate) fn do_repop(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    use lyracore_shared::constants::{player_flags, unit_vis_flags};
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead {
        return Ok(()); // alive — release is a no-op
    }
    if player.player_flags & player_flags::GHOST != 0 {
        return Ok(()); // already a ghost — release already happened (idempotent)
    }

    // Leave the dead body at the death location as a CORPSE object the ghost runs back to.
    // CORPSE_FIELD_BYTES_1/2 use a CORPSE-specific layout (race/gender/skin), NOT the PLAYER_BYTES
    // layout — sending player_bytes verbatim makes the 5875 client read the *face* id as the race,
    // null-deref the body-model lookup, and crash (verified). Repack from the player's appearance:
    // race/gender are in unit_bytes_0 (byte0/byte2); skin/face/hair in player_bytes; facialhair in
    // player_bytes_2 byte0. (The corpse-appearance layout the 5875 client requires.)
    let (corpse_bytes_1, corpse_bytes_2) = crate::corpse::corpse_appearance_bytes(
        player.unit_bytes_0,
        player.player_bytes,
        player.player_bytes_2,
    );
    // Escalate the reclaim delay off the player's own death-streak state: a death inside the
    // previous streak window steps the delay
    // up the 30s/60s/120s ladder; a death after it has lapsed resets to the 30s base. `new_expire` is
    // stamped back onto the player row below (so the NEXT death escalates from it); `delay_micros` is
    // stamped onto the corpse row just inserted (and is what the gateway reports to the client).
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let (new_expire, delay_micros) =
        crate::corpse::escalated_reclaim(player.death_expire_micros, now_micros);

    let corpse_guid = crate::corpse::corpse_guid_for(player.guid);
    let corpses = ctx.db.game_corpse();
    corpses.guid().delete(corpse_guid); // idempotent: clear any stale corpse for this player
    corpses.insert(crate::Corpse {
        guid: corpse_guid,
        owner_guid: player.guid,
        map_id: player.map_id,
        x: player.x,
        y: player.y,
        z: player.z,
        orientation: player.orientation,
        display_id: player.native_display_id, // the race/gender body model (CORPSE_FIELD_DISPLAY_ID)
        bytes_1: corpse_bytes_1,
        bytes_2: corpse_bytes_2,
        created_at: ctx.timestamp,
        reclaim_delay_micros: delay_micros,
        is_bones: false,
        // 190 slice 2: the corpse stays in the instance the death happened in — the ghost
        // corpse-runs back through the portal (which re-binds them to this same instance).
        instance_id: player.instance_id,
    });

    // Become a GHOST: health 1 (not 0 — vanilla), PLAYER_FLAGS_GHOST + the UNIT_VIS_FLAG_GHOST render
    // bit. Keep `dead = true` — a ghost is still "dead" (no regen, non-attackable) until it reclaims;
    // the movement gate has a ghost exception so the ghost can still run to its corpse.
    player.health = 1;
    player.player_flags |= player_flags::GHOST;
    player.unit_bytes_1 |= unit_vis_flags::GHOST;
    player.death_expire_micros = new_expire; // stamp the escalation state for the NEXT death
    let player_guid = player.guid;
    // Capture before the move into `update` below (the graveyard resolution needs all four).
    let (death_x, death_y, death_map_id, death_race) =
        (player.x, player.y, player.map_id, player.race());
    entities.guid().update(player);

    // Tier 3b (Warlock pet): a warlock's death dismisses its pet (vanilla — the Imp despawns when the
    // master dies). Done on release (the death→ghost transition) so the pet is gone for the corpse run.
    // `pass_pet` ALSO despawns a pet whose owner is dead, so this is belt-and-suspenders (prompt; immediate).
    crate::creatures::despawn_pets(ctx, player_guid);

    // Teleport the ghost to the graveyard `graveyard::resolve_graveyard` (work-item 209) resolves for
    // this death: prefer a zone-linked graveyard (imported `game_graveyard`/`game_graveyard_zone` —
    // cmangos WorldSafeLocs + game_graveyard_zone), falling back to the nearest of every imported
    // graveyard on the map, falling back to the five hardcoded Elwynn/Westfall consts
    // (`graveyard::nearest`) when nothing is imported at all (this sandbox's default state):
    //   105 Northshire Abbey  (-8935, -188)          — zone 12 Elwynn
    //   106 Goldshire          (-9339,  171)          — zone 12 Elwynn
    //   854 Eastvale Logging Camp (-9552, -1374)      — zone 12 Elwynn
    //   ≈80 Sentinel Hill (-10650, 1180) [V]          — zone 40 Westfall (work-item 206)
    //   ≈81 Westfall coast (-11390, 1590) [V]           (seed.rs also row-seeds these five, see 209)
    // The corpse was just inserted at (death_x, death_y) — use those coords + the player's map/race
    // to resolve the release point, then teleport_player emits MSG_MOVE_TELEPORT_ACK.
    let gy = graveyard::resolve_graveyard(ctx, death_map_id, death_x, death_y, death_race);
    // Graveyard release always lands in the open world (instance 0) — the resolved graveyard is never
    // itself inside a dungeon instance.
    teleport_player(ctx, player_guid, gy.map, 0, gy.x, gy.y, gy.z, gy.o);

    // Releasing to spirit resolves the death outside of accepting a pending resurrect offer —
    // drop any outstanding `game_resurrect_request` for this target so a stale offer doesn't resurface
    // as a phantom SMSG_RESURRECT_REQUEST on a future reconnect. Idempotent (no-op if none pending).
    ctx.db
        .game_resurrect_request()
        .target_guid()
        .delete(player_guid);
    Ok(())
}

/// Should `pending_ghost` be stamped onto the durable Character row for this persist? True only for
/// a RELEASED ghost (`dead` + `PLAYER_FLAGS_GHOST` — the exact pair `do_repop` sets) persisting for a
/// reason OTHER than a real logout: a cross-map hop (`teleport_player`'s persist-then-despawn) or the
/// stale-entity cleanup in `player_login`. A real logout/disconnect (`set_offline`) always clears it —
/// the established "relog comes back alive" rule (`remove_from_world` deletes the corpse right after
/// its persist, so a preserved ghost there would have nothing to reclaim). A dead-but-UNRELEASED
/// player (death screen, no ghost flag yet) deliberately does NOT preserve: the only cross-map paths
/// a dead-unreleased player can take are GM/debug teleports (movement is frozen pre-release), and
/// rebuilding those alive-at-1-HP matches the pre-226 behavior for that GM edge. Pure. [226]
pub(crate) fn persisted_pending_ghost(dead: bool, player_flags: u32, set_offline: bool) -> bool {
    !set_offline && dead && player_flags & lyracore_shared::constants::player_flags::GHOST != 0
}

/// 1× run speed in basis points — the `.speed`-off value, shared by the live entity's own default and
/// the durable carry column below so "no GM speed" is spelled ONE way. [289]
pub(crate) const RUN_SPEED_BP_1X: u32 = 10_000;

/// What `(pending_godmode, pending_run_speed_mult_bp)` should be stamped onto the durable Character
/// row for this persist — the SCOPE DECISION of work-item 289, in one pure function.
///
/// `!set_offline` (a cross-map teleport, a shard-transfer freeze, the stale-entity cleanup in
/// `player_login`) CARRIES the live values, because the entity is about to be rebuilt from this row
/// within the same continuous session — the client is looking at a loading screen, not a login
/// screen, and dropping `.god` there is the bug (a GM arrived in Durotar mortal and was killed
/// repeatedly with nothing printed).
///
/// A real logout/disconnect (`set_offline`) RESETS both to their off values. That is deliberate and
/// is the whole reason these are `pending_*` carry fields rather than plain durable GM settings: a
/// GM who forgets `.god` off should not stay invulnerable — and invisible to every creature's aggro
/// pass (`creatures::ai::is_aggro_candidate`) — across future sessions, with no in-game indication.
/// A session boundary is the natural place for that safety reset, and it costs a GM one `.god`
/// re-issue per login. Exactly the `persisted_pending_ghost` discipline, same `set_offline` seam.
/// Pure. [289]
pub(crate) fn persisted_gm_playtest(
    godmode: bool,
    run_speed_mult_bp: u32,
    set_offline: bool,
) -> (bool, u32) {
    if set_offline {
        (false, RUN_SPEED_BP_1X)
    } else {
        (godmode, run_speed_mult_bp)
    }
}

/// The field overrides that restore RELEASED-GHOST state onto a freshly-rebuilt player entity —
/// `build_player_entity` (creature-owned code, `creatures/spawn.rs`) always builds alive, so the
/// ghost re-application lives HERE, at `player_login`'s call site. Returns
/// `(dead, health, player_flags, unit_bytes_1)`: exactly the four fields `do_repop` sets on release
/// (dead stays true, health 1 — vanilla's ghost HP — plus the GHOST player flag and the
/// UNIT_VIS_FLAG_GHOST render bit OR'd over whatever the fresh build carried, e.g. a Warrior's
/// Battle-Stance byte in `unit_bytes_1`). Pure. [226]
pub(crate) fn ghost_restored_fields(player_flags: u32, unit_bytes_1: u32) -> (bool, u32, u32, u32) {
    use lyracore_shared::constants::{player_flags as pf, unit_vis_flags};
    (
        true,
        1,
        player_flags | pf::GHOST,
        unit_bytes_1 | unit_vis_flags::GHOST,
    )
}

/// Draw or stow the actor's weapons — the `CMSG_SETSHEATHED` the client sends when a player presses
/// `Z` or starts an attack. Writes byte 0 of `UNIT_FIELD_BYTES_2`; the gateway's entity-update relay
/// turns the row change into a VALUES packet, which is the ONLY way an observer learns that someone
/// else drew or stowed a weapon. Without this the field stays 0 forever and every peer renders every
/// player permanently unarmed. Where a stowed weapon hangs is a different field — the per-item
/// `item_template.sheath` byte in the item query. [#101]
pub(crate) fn apply_set_sheathed(
    ctx: &ReducerContext,
    mut actor: WorldEntity,
    state: u8,
) -> Result<(), String> {
    if !sheath_state::is_valid(state) {
        return Err(format!("invalid sheath state {state}"));
    }
    let packed = sheath_state::packed_with(actor.unit_bytes_2, state);
    // No-op guard: the client re-sends the CURRENT state on every weapon swap and on some ability
    // presses. Writing the identical row anyway would fire the entity-update relay, which broadcasts
    // a VALUES packet to every observer in range — a swap-spam amplifier on a busy cell.
    if packed == actor.unit_bytes_2 {
        return Ok(());
    }
    actor.unit_bytes_2 = packed;
    ctx.db.game_world_entity().guid().update(actor);
    Ok(())
}

/// Resurrection Sickness debuff spell id (vanilla 15007). Seeded in `seed.rs` as a single negative
/// A_MOD_STAT(STAT_ALL) aura and landed by `do_spirit_healer_res` via the shared aura engine.
pub const RESURRECTION_SICKNESS_SPELL: u32 = 15007;

/// The vitals a Spirit-Healer res restores: 50% of max health (floored, but at least 1 — a ghost is
/// never res'd back to 0 hp) and 50% of max mana (0 for a rage/energy class, whose `max_power` is 0).
/// Pure so the percent + the `.max(1)` floor are unit-tested without a `ReducerContext` (the exact
/// thing the verify recipe asserts on the live entity row). Mirrors `reclaim_corpse`'s 50% health.
fn spirit_res_vitals(max_health: u32, max_power: u32) -> (u32, u32) {
    ((max_health / 2).max(1), max_power / 2)
}

/// Spirit Healer resurrection (`CMSG_SPIRIT_HEALER_ACTIVATE`): a GHOST activates the graveyard Spirit
/// Healer (npc_flags SPIRITHEALER 0x20) to res IN PLACE at reduced vitals + a 10-min Resurrection
/// Sickness debuff — the alternative to the corpse run (which res's at the body with no sickness). The
/// healer arg (the healer's guid) is IGNORED like `reclaim_corpse`'s `_corpse_guid`: the res targets
/// the actor named by guid (the client only sends this from the healer dialog, which the
/// SPIRITHEALER flag already ghost-gates).
///
/// The `do_spirit_healer_res` body keyed off an explicit player guid — shared by `gw::gw_spirit_res`
/// (resolves the actor guid from the shared connection) and `debug::debug_spirit_healer_res` (a CLI
/// `spacetime call` drives the res by guid, so the feature is verifiable without the mouse-only
/// healer dialog). Gates on the entity
/// being a ghost (mirroring `reclaim_corpse`); res's IN PLACE at 50% health + 50% mana (no range/delay/
/// corpse check — that's the corpse-run path), clears the ghost/dead state (health > 0 + cleared flags
/// replicate → the client leaves the death screen, exactly like `reclaim_corpse`; vanilla has no
/// "alive" opcode), deletes any leftover corpse, and lands Resurrection Sickness through the aura engine.
pub(crate) fn do_spirit_healer_res(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    use lyracore_shared::constants::{player_flags, unit_vis_flags};
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead || player.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".to_string());
    }
    // Res at 50% health + 50% mana (the vanilla graveyard-res percent). Clearing dead + the GHOST
    // flags + restoring health replicates to the client, which leaves the ghost/death state. 50% of a
    // rage/energy class's power pool is 0/harmless (max_power is 0 for non-mana classes here).
    let (health, power) = spirit_res_vitals(player.max_health, player.max_power);
    let player_level = player.level;
    player.health = health;
    player.power = power;
    player.dead = false;
    player.player_flags &= !player_flags::GHOST;
    player.unit_bytes_1 &= !unit_vis_flags::GHOST;
    entities.guid().update(player);
    // Clear the body the ghost would have run back to (irrelevant after a graveyard res); idempotent —
    // a no-op if the player never released far enough to leave a corpse.
    ctx.db
        .game_corpse()
        .guid()
        .delete(crate::corpse::corpse_guid_for(guid));
    // Resurrection Sickness: vanilla exempts characters below level 11 (CONFIG_UINT32_DEATH_SICKNESS_LEVEL=11).
    // Reuse the aura engine (the same path talents/buffs land through). level/rank are nominal (the debuff
    // is a flat −10 all-stats; rank 1 → the seeded base_points verbatim).
    if player_level >= 11 {
        crate::spell::apply_spell_auras(ctx, RESURRECTION_SICKNESS_SPELL, guid, 1, 1);
    }
    // This resolves the death outside of accepting a pending resurrect offer — drop any
    // outstanding `game_resurrect_request` for this target so a stale offer doesn't resurface as a
    // phantom SMSG_RESURRECT_REQUEST on a future reconnect. Idempotent (no-op if none pending).
    ctx.db.game_resurrect_request().target_guid().delete(guid);
    Ok(())
}

// ===========================================================================================
//  Leave world
// ===========================================================================================

/// Fold the elapsed span since `session_start_micros` into `played_total_secs`, whole-seconds
/// truncated (matching vanilla's own played-time granularity). `session_start_micros == 0` means no
/// live session to close (e.g. `persist_entity` called twice back-to-back) — returns the pool
/// unchanged. Pure/testable, mirrors `xp::accrue_rested_on_login`'s shape.
fn accrue_played_on_persist(now_micros: i64, session_start_micros: u64, pool: u32) -> u32 {
    if session_start_micros == 0 {
        return pool;
    }
    let elapsed_secs = (now_micros as u64).saturating_sub(session_start_micros) / 1_000_000;
    pool.saturating_add(elapsed_secs as u32)
}

/// Persist a live entity's mutable progression — position, level/xp, and money
/// — back to its durable Character row. Shared by the clean-logout path AND the ghost-relog cleanup
/// in `player_login`, so neither drops freshly-earned state (looted coin / a ding / movement) when
/// the live entity is deleted. `set_offline` marks the character offline (logout only; a relog is
/// about to re-insert, so it stays online there).
pub(crate) fn persist_entity(ctx: &ReducerContext, entity: &WorldEntity, set_offline: bool) {
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(entity.guid) {
        c.x = entity.x;
        c.y = entity.y;
        c.z = entity.z;
        c.orientation = entity.orientation;
        // Zone label char-select shows (SMSG_CHAR_ENUM reads zone_id): re-derived from the
        // persisted position — it was stamped ONCE at creation, so a character who moved zones
        // kept the start-zone label forever (a Dun Morogh dwarf GM-teleported to Northshire
        // still read "Dun Morogh" at char select). None (off-slice/unimported terrain) keeps
        // the stored zone rather than guessing.
        if let Some(z) = crate::terrain::zone_id_at(ctx, entity.map_id, entity.x, entity.y) {
            c.zone_id = z;
        }
        c.level = entity.level as u8;
        c.xp = entity.xp;
        c.next_level_xp = entity.next_level_xp;
        c.money = entity.money;
        // Persist current health/power so a relog resumes at the same vitals instead
        // of being force-healed to full. Clamp health to >=1: the "relog comes back ALIVE" rule
        // (see the ghost-corpse cleanup in `player_login`) means a player who logged out dead/at 0 HP
        // must come back alive, so persist 1 rather than 0 (0 is also the "no persisted value"
        // sentinel `build_player_entity` uses to mean "spawn at full" — persisting a real 0 would be
        // indistinguishable from that and re-trigger the full-heal path).
        c.health = entity.health.max(1);
        c.power = entity.power;
        // Corpse-reclaim escalation state survives the session boundary — without this, a mere
        // disconnect reset the death ladder to 30s (the review's die-die-relog-die exploit).
        c.death_expire_micros = entity.death_expire_micros;
        // Instance the entity was in survives the session boundary (work-item 190 slice 1, always 0
        // this slice) — the death_expire_micros precedent — so a relog rebuilds into the same instance
        // rather than open world.
        c.pending_instance_id = entity.instance_id;
        // Released-GHOST state survives an entity despawn (work-item 226 — the 224 landmine): a
        // cross-map graveyard release persists-then-deletes the entity via `teleport_player`, and
        // without this stamp the WORLDPORT_ACK rebuild came back ALIVE with no ghost and (via
        // `player_login`'s corpse delete) no corpse — a silent free resurrect. Re-derived from the
        // LIVE entity on every persist so a reclaim/res naturally clears it; FORCED false on a real
        // logout/disconnect (`set_offline`) — the established "relog comes back alive" rule, whose
        // corpse delete lives in `remove_from_world` right after this persist.
        c.pending_ghost = persisted_pending_ghost(entity.dead, entity.player_flags, set_offline);
        // GM playtest state (work-item 289) survives an entity despawn the same way, and for the same
        // reason: a cross-map `.tele` (or a cross-database shard hop, whose `begin_transfer` freeze
        // calls this with `set_offline: false` too) persists-then-deletes the entity, and without this
        // stamp `build_player_entity` rebuilds it with `.god`/`.speed` silently off. Cleared on a real
        // logout/disconnect — see `persisted_gm_playtest` for that policy.
        let (pending_godmode, pending_speed_bp) =
            persisted_gm_playtest(entity.godmode, entity.run_speed_mult_bp, set_offline);
        c.pending_godmode = pending_godmode;
        c.pending_run_speed_mult_bp = pending_speed_bp;
        // Played time (/played): fold this session's elapsed span into the durable total and close
        // the session stamp — on EVERY persist (real logout/disconnect AND the ghost-relog cleanup),
        // so a stale entity's playtime is never lost or double-counted on the next login's re-stamp.
        c.played_total_secs = accrue_played_on_persist(
            ctx.timestamp.to_micros_since_unix_epoch(),
            c.session_start_micros,
            c.played_total_secs,
        );
        c.session_start_micros = 0;
        if set_offline {
            c.online = false;
            // Rested XP (rank 30): stamp the logout time so the next login accrues rested XP from the
            // offline span. Only on a REAL logout/disconnect (set_offline) — the ghost-relog cleanup
            // path passes false, so it never falsely starts the rest clock.
            c.last_logout_micros = ctx.timestamp.to_micros_since_unix_epoch() as u64;
            // Rest state (196): persist the live resting flag (so `player_login` picks the full vs 1/4
            // offline rate) and BANK any live accrual before the offline clock takes over from the
            // logout stamp — otherwise the online stay's rested would be lost.
            c.resting = entity.resting;
            crate::rest::materialize_on_logout(&mut c, ctx.timestamp.to_micros_since_unix_epoch());
        }
        chars.guid().update(c);
    }
}

pub(crate) fn remove_from_world(ctx: &ReducerContext, owner: Identity) {
    let Some(entity) = entity_by_owner(ctx, owner) else {
        return;
    };
    // Notify-hook: the player is leaving (explicit logout AND abrupt disconnect both land
    // here). Fired FIRST, while the live entity row still exists for handlers to read.
    crate::hooks::fire_on_logout(
        ctx,
        &crate::hooks::LogoutPayload {
            character_guid: entity.guid,
        },
    );
    // Persist position + progression back to the character so next login resumes in place with the
    // ding/loot intact.
    persist_entity(ctx, &entity, true);

    // Free any melee engagement involving this entity so no orphan
    // `game_melee_attack` row lingers (the swing tick self-heals within 500ms, but a logout
    // shouldn't leave the player "attacking" or hold a target in combat). `disengage` removes both
    // its own attack and any attacks targeting it (future PvP).
    crate::combat::disengage(ctx, entity.guid);

    // A live Trade Session dies with the leaver — the partner hears `TradeCanceled` (#120).
    crate::trade::cancel_trade_for(ctx, entity.guid);

    // Clear the player's corpse on leaving the world (logout/disconnect) so a dead/ghost
    // player who quits doesn't leave an orphan body behind (corpse decay to bones rides the gc reaper). Idempotent.
    ctx.db
        .game_corpse()
        .guid()
        .delete(crate::corpse::corpse_guid_for(entity.guid));

    // Leaving the world resolves the death outside of accepting a pending resurrect offer —
    // drop any outstanding `game_resurrect_request` for this entity so a stale offer doesn't resurface
    // as a phantom SMSG_RESURRECT_REQUEST on a future reconnect. Idempotent (no-op if none pending).
    ctx.db
        .game_resurrect_request()
        .target_guid()
        .delete(entity.guid);

    // Stealth drops on logout (vanilla): clear A_STEALTH so a stale aura doesn't survive the disconnect.
    // A_STEALTH is never timer-reaped, so otherwise the gateway's stealth create-skip would find the stale
    // aura on relog and hide the returning player from ALL peers permanently. Idempotent (no-op if unstealthed).
    crate::spell::break_stealth(ctx, entity.guid);

    // Tier 3b (Warlock pet): a summoned pet never outlives its owner — despawn it on logout/disconnect so a
    // leaving warlock's Imp doesn't linger ownerless. Frees the pet's melee row + threat, then DESTROYs the
    // entity (the on_delete relay vanishes it). Idempotent (no-op if the player has no pet).
    crate::creatures::despawn_pets(ctx, entity.guid);

    // Vanilla: clear the buyback ring on logout so the tab is empty on next login.
    for row in ctx
        .db
        .game_character_buyback()
        .by_player_guid()
        .filter(&entity.guid)
        .collect::<Vec<_>>()
    {
        ctx.db.game_character_buyback().id().delete(row.id);
    }

    // The per-mover motion row dies with the entity (perf catalog 2.1's named lifecycle gap): a row
    // left behind never updates again, so it relays nothing, but it would leak one row per character
    // that has ever moved and hand a fresh subscriber a stale motion on first sync. #461: the
    // STAGED payload goes with it, so the next `publish_motion` firing cannot re-create the row we
    // are deleting here (the tick's own liveness gate is the second net, not the only one).
    crate::motion::drop_pending(ctx, entity.guid);
    ctx.db.game_entity_motion().guid().delete(entity.guid);
    ctx.db.game_world_entity().guid().delete(entity.guid);
}

/// NOTIFY-ONLY gossip-option chokepoint: the gateway calls this (fire-and-forget, via
/// `gw::gw_gossip_select`) when it handles CMSG_GOSSIP_SELECT_OPTION, BEFORE running its own
/// vendor/innkeeper/close behavior — gossip handling itself lives gateway-side, so without this
/// reducer the module (and therefore packages) would never see the click. Fires `on_gossip_select`
/// and does nothing else; a failure here never blocks the gateway's gossip reply.
/// `debug_gossip_select` drives the same fire by explicit guid for the harness (the CLI identity
/// owns no entity).
///
/// The gossip-notify core, actor-explicit (#479) — same split as [`apply_set_target`], keyed by
/// guid (the hook payload is the only thing the body reads off the actor).
pub(crate) fn apply_gossip_select(
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

/// Abrupt socket drop. Wired to SpacetimeDB's lifecycle so an ungraceful disconnect also
/// removes the entity (acceptance criterion #7 for non-clean disconnects).
#[reducer(client_disconnected)]
pub fn on_disconnect(ctx: &ReducerContext) {
    remove_from_world(ctx, ctx.sender());
}

#[cfg(test)]
mod tests {
    use super::{
        accrue_played_on_persist, can_inspect, ghost_restored_fields, is_cross_map_teleport,
        movement_violation, persisted_gm_playtest, persisted_pending_ghost, plan_movement,
        resolve_environmental_damage, snapshot_needs_persist, spirit_res_vitals, MovementDelta,
        INSPECT_RANGE_SQ, MOVE_VIOLATION_SPEED, MOVE_VIOLATION_TELEPORT, PERSIST_MAX_DRIFT_YD,
        RESURRECTION_SICKNESS_SPELL, RUN_SPEED_BP_1X,
    };

    // ---- perf catalog 2.2: snapshot persistence ----------------------------------------------
    const HEARTBEAT: u16 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;

    /// The whole point: a heartbeat that changed NOTHING a client or a gameplay gate can observe
    /// must not rewrite the ~60-column public row (and make the subscription engine ship a delta of
    /// it to every in-box player). Peers are animated from `game_entity_motion` since 2.1.
    #[test]
    fn a_heartbeat_that_changed_nothing_observable_is_not_persisted() {
        assert!(!snapshot_needs_persist(HEARTBEAT, false, false, false, 0.0));
        // …including one that moved, as long as the stored row is still within the drift bound.
        assert!(!snapshot_needs_persist(
            HEARTBEAT,
            false,
            false,
            false,
            3.9 * 3.9
        ));
    }

    /// Each trigger on its own. Mutation target: drop any arm of the `||` and exactly one of these
    /// goes red — a heartbeat that silently stops persisting real state is the expensive failure
    /// (a stale row is what melee reach, interact and aggro all read).
    #[test]
    fn every_observable_change_forces_the_row_write() {
        assert!(
            snapshot_needs_persist(HEARTBEAT, true, false, false, 0.0),
            "grid cell (AOI)"
        );
        assert!(
            snapshot_needs_persist(HEARTBEAT, false, true, false, 0.0),
            "movement flags"
        );
        assert!(
            snapshot_needs_persist(HEARTBEAT, false, false, true, 0.0),
            "resting/health"
        );
        // A non-heartbeat opcode is an EDGE (start/stop/turn/jump/fall). `MSG_MOVE_SET_FACING`
        // moves the unit nowhere, so drift alone would never persist a turn — and stored
        // orientation is what behind-the-target checks read.
        let facing = lyracore_shared::opcodes::movement::MSG_MOVE_SET_FACING as u16;
        assert!(
            snapshot_needs_persist(facing, false, false, false, 0.0),
            "non-heartbeat opcode"
        );
    }

    /// The bound is a real bound: at the threshold the row still holds, past it it must be written.
    /// 4 yd is chosen against gameplay reach, not against the benchmark — see the fn's doc.
    #[test]
    fn drift_past_the_bound_forces_the_row_write() {
        let at = PERSIST_MAX_DRIFT_YD * PERSIST_MAX_DRIFT_YD;
        assert!(!snapshot_needs_persist(HEARTBEAT, false, false, false, at));
        assert!(snapshot_needs_persist(
            HEARTBEAT,
            false,
            false,
            false,
            at + 0.01
        ));
        const {
            assert!(
                PERSIST_MAX_DRIFT_YD <= 5.0,
                "drift must stay inside melee reach (5 yd)"
            )
        };
    }

    /// The WIRING, and the half that matters most: the gate covers the ENTITY row and must never
    /// reach the per-mover MOTION row. `game_entity_motion` is the only peer-movement relay when
    /// AOI is on — gating it would recreate #109 (peers frozen, server perfectly healthy) as a
    /// performance optimisation.
    // ---- Issue #385: movement_update as plan-then-apply --------------------------------------
    //
    // `plan_movement` (and the `MovementDelta`/`resolve_fall_damage` pieces it's built from) replace
    // the reducer's own formatted source as what the "relay is never gated on the persist decision"
    // invariant pins against. That invariant — and the fall-damage decision the old inline block used
    // to hide — are now ordinary value-level unit tests instead of a whitespace-collapsed exact-body
    // match that broke on every rename or rustfmt re-wrap. What's left below
    // (`movement_update_applies_the_plan_and_never_re_derives_the_persist_gate`) is a MUCH smaller
    // presence/wiring scan for the one thing no pure unit test can reach without a `ReducerContext`
    // harness (playbook §7): does the reducer actually route through the plan, or silently re-derive
    // its own copy of the gate.

    #[test]
    fn resolve_environmental_damage_subtracts_short_of_lethal_and_flags_lethal_without_mutating() {
        assert_eq!(
            resolve_environmental_damage(0, 50),
            (50, false),
            "a soft landing (no damage) is a no-op"
        );
        assert_eq!(
            resolve_environmental_damage(20, 50),
            (30, false),
            "sub-lethal damage subtracts"
        );
        assert_eq!(
            resolve_environmental_damage(50, 50),
            (50, true),
            "exactly-lethal damage is flagged, not subtracted — kill_player re-fetches fresh"
        );
        assert_eq!(
            resolve_environmental_damage(80, 50),
            (50, true),
            "over-lethal damage is flagged, not subtracted either"
        );
    }

    #[test]
    fn plan_movement_mirrors_snapshot_needs_persist_and_never_gates_the_relay() {
        let stationary = MovementDelta {
            old_x: 0.0,
            old_y: 0.0,
            old_z: 0.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            old_move_ms: 0,
            move_time_ms: 0,
        };
        // The persist decision is EXACTLY snapshot_needs_persist's, for every trigger that matters —
        // and for every one of them, the relay is NEVER gated on the outcome. This second assertion,
        // repeated across every persist/no-persist combination below, IS the invariant the old
        // source-scan pin existed to guard.
        for &(grid, flags, state, drift) in &[
            (false, false, false, 0.0),
            (true, false, false, 0.0),
            (false, true, false, 0.0),
            (false, false, true, 0.0),
            (
                false,
                false,
                false,
                PERSIST_MAX_DRIFT_YD * PERSIST_MAX_DRIFT_YD + 1.0,
            ),
        ] {
            let want = snapshot_needs_persist(HEARTBEAT, grid, flags, state, drift);
            let plan = plan_movement(HEARTBEAT, grid, flags, state, drift, false, &stationary);
            assert_eq!(
                plan.persist_entity, want,
                "persist_entity must mirror snapshot_needs_persist exactly"
            );
            assert!(
                plan.relay_motion,
                "the motion relay must never be gated on the persist decision"
            );
        }
        // A lethal fall on an otherwise-quiet heartbeat need not force a persist, and still relays —
        // `fall_lethal` and `relay_motion` are independent fields, not one derived from the other.
        let lethal = plan_movement(HEARTBEAT, false, false, false, 0.0, true, &stationary);
        assert!(lethal.fall_lethal);
        assert!(!lethal.persist_entity);
        assert!(
            lethal.relay_motion,
            "even a lethal-fall heartbeat still relays"
        );
        // `moved` reflects the delta the caller passes in, not the persist gate.
        let moving = MovementDelta {
            old_x: 0.0,
            old_y: 0.0,
            old_z: 0.0,
            x: 10.0,
            y: 0.0,
            z: 0.0,
            old_move_ms: 0,
            move_time_ms: 500,
        };
        assert!(plan_movement(HEARTBEAT, false, false, false, 0.0, false, &moving).moved);
        assert!(!plan_movement(HEARTBEAT, false, false, false, 0.0, false, &stationary).moved);
    }

    #[test]
    fn movement_update_applies_the_plan_and_never_re_derives_the_persist_gate() {
        let body =
            crate::test_scan::code_of(include_str!("world.rs"), "pub(crate) fn apply_movement_update(");
        // The persist/relay/channel/score decisions must route through `plan_movement` — not a
        // second, hand-rolled `snapshot_needs_persist` call inline (that decision, and the "relay is
        // never gated on it" invariant, are pinned directly by the tests above instead).
        assert_eq!(
            body.matches("snapshot_needs_persist(").count(),
            0,
            "movement_update must not call snapshot_needs_persist directly — plan_movement owns \
             that decision now. Body was:\n{body}"
        );
        let plan_at = body
            .find("plan_movement(")
            .expect("movement_update must build its plan via plan_movement");
        let entity_at = body
            .find("entities.guid().update(mover)")
            .expect("the gated entity write");
        assert!(
            plan_at < entity_at,
            "the plan must be built before the entity row write"
        );
        assert_eq!(
            body.matches("entities.guid().update(mover)").count(),
            1,
            "exactly one entity write"
        );
        assert!(
            body.contains("if plan.persist_entity {"),
            "the entity write must be gated on plan.persist_entity, not re-derived inline. Body \
             was:\n{body}"
        );
        // The relay must come after the entity write and be gated on `plan.relay_motion` — never on
        // `plan.persist_entity` or anything else re-derived inline. `plan_movement`'s own tests prove
        // `relay_motion` is ALWAYS true; this only pins that the reducer actually routes the relay
        // through it, which is the one piece no pure unit test (no ReducerContext in this crate) can
        // reach on its own.
        let relay_at = body
            .find("if plan.relay_motion {")
            .expect("the motion relay's own (always-true) gate");
        assert!(
            entity_at < relay_at,
            "the motion relay must come after the gated entity write"
        );
        assert_eq!(
            body.matches("if plan.relay_motion {").count(),
            1,
            "exactly one relay gate"
        );
    }

    // ---- Work-item 255: movement plausibility (detect-and-flag) ------------------------------
    #[test]
    fn movement_violation_flags_speed_and_teleport_but_not_legit_motion() {
        let run = lyracore_shared::constants::speeds::RUN; // 7.0 yd/s
                                                           // Legit: ~run speed over a 0.5s heartbeat (3.5yd) → no flag.
        assert_eq!(movement_violation(3.5, 0.5, run), None);
        // Legit with slack: 1.5× run (jitter/diagonal) is under the 1.8× leeway → no flag.
        assert_eq!(movement_violation(run * 1.5 * 0.5, 0.5, run), None);
        // Speedhack: 3× run over 0.5s → SPEED flag, magnitude ≈ 3×.
        let (k, mag) = movement_violation(run * 3.0 * 0.5, 0.5, run).unwrap();
        assert_eq!(k, MOVE_VIOLATION_SPEED);
        assert!(
            (mag - 3.0).abs() < 1e-3,
            "magnitude is the ×-normal multiple"
        );
        // Teleport: a 100yd single delta → TELEPORT flag regardless of dt (even unknown dt=0).
        let (k, mag) = movement_violation(100.0, 0.0, run).unwrap();
        assert_eq!(k, MOVE_VIOLATION_TELEPORT);
        assert_eq!(mag, 100.0);
        // Sub-50ms sample: dt too small → speed check skipped (a short teleport-under-60yd is plausible).
        assert_eq!(movement_violation(5.0, 0.01, run), None);
        // A snare (lower max_speed) tightens the bar; Sprint (higher) loosens it — both handled by the
        // caller passing effective_move_speed, but verify the boundary scales with max_speed here.
        assert!(
            movement_violation(run * 2.0 * 0.5, 0.5, run / 2.0).is_some(),
            "snared mover flagged sooner"
        );
    }

    // ---- Work-item 224: cross-map teleport decision ------------------------------------------

    #[test]
    fn is_cross_map_teleport_only_when_target_map_differs() {
        assert!(
            !is_cross_map_teleport(0, 0),
            "same map (both open world) is a same-map hop"
        );
        assert!(
            !is_cross_map_teleport(36, 36),
            "same map (both inside the same instance map) is a same-map hop"
        );
        assert!(
            is_cross_map_teleport(0, 36),
            "Elwynn -> Deadmines is cross-map"
        );
        assert!(
            is_cross_map_teleport(36, 0),
            "Deadmines -> Elwynn (exit) is cross-map"
        );
    }

    // ---- Work-item 226: cross-map ghost preservation (the 224 review-finding-#2 landmine) --------

    #[test]
    fn pending_ghost_persists_only_a_released_ghost_and_never_across_a_real_logout() {
        use lyracore_shared::constants::player_flags::GHOST;
        // The landmine case: a released ghost (dead + GHOST flag) persisting for a cross-map hop
        // (set_offline false) MUST carry its ghost state to the durable row — this is exactly the
        // state `teleport_player`'s persist-then-despawn sees mid graveyard release.
        assert!(
            persisted_pending_ghost(true, GHOST, false),
            "cross-map release preserves the ghost"
        );
        // A real logout/disconnect clears it — the relog-comes-back-alive rule (remove_from_world
        // deletes the corpse right after its persist, so a preserved ghost would be corpseless).
        assert!(
            !persisted_pending_ghost(true, GHOST, true),
            "a real logout never preserves ghost state"
        );
        // Alive players never stamp it, whatever their flags claim (flag hygiene).
        assert!(
            !persisted_pending_ghost(false, GHOST, false),
            "alive + stale GHOST flag is not a ghost"
        );
        // Dead but UNRELEASED (death screen, no ghost flag): deliberately NOT preserved — see the
        // fn's doc comment (GM-teleport edge keeps the pre-226 alive-at-1-HP rebuild).
        assert!(
            !persisted_pending_ghost(true, 0, false),
            "dead-unreleased does not preserve"
        );
    }

    // ---- Work-item 289: GM playtest state across a cross-map / cross-shard entity rebuild --------

    /// The POLICY (behavioural, the decision itself — not a scan): carried across a despawn/rebuild
    /// within a session, reset at a session boundary. Mutation targets: flip either branch of
    /// `persisted_gm_playtest` and exactly one half of this goes red.
    #[test]
    fn gm_playtest_state_carries_across_a_map_change_and_resets_on_a_real_logout() {
        // The bug: `.god` + `.speed 3`, then `.tele valley` — the cross-map persist (set_offline
        // false, the same call `begin_transfer`'s shard freeze makes) must carry BOTH to the durable
        // row, because the rebuilt entity is constructed from nothing else.
        assert_eq!(
            persisted_gm_playtest(true, 30_000, false),
            (true, 30_000),
            "a cross-map hop carries godmode AND the speed multiplier — dropping them is work-item 289"
        );
        // A real logout/disconnect resets both: a GM who forgets `.god` off must not stay
        // invulnerable (and unaggroable) across sessions with nothing printed.
        assert_eq!(
            persisted_gm_playtest(true, 30_000, true),
            (false, RUN_SPEED_BP_1X),
            "a real logout resets GM playtest state to off / 1x"
        );
        // Off stays off on a hop (no accidental self-promotion for a non-GM's map change).
        assert_eq!(
            persisted_gm_playtest(false, RUN_SPEED_BP_1X, false),
            (false, RUN_SPEED_BP_1X)
        );
    }

    /// The CALL SITES, pinned by scan because both live inside `&ReducerContext` functions this
    /// crate has no harness for (playbook §7/§8: say so, then pin with the strongest scan). Both
    /// needles are struct/row FIELD ASSIGNMENTS, which is what makes a presence scan load-bearing
    /// here rather than decorative — Rust forbids a duplicate field in a struct literal, and
    /// `persist_entity` writes each column exactly once, so a decoy copy of the needle cannot
    /// coexist with a restored hardcode.
    ///
    /// What it does NOT catch: a `persist_entity` that computes the pair and then never
    /// `chars.guid().update(c)`s (covered by the surrounding write, unchanged here), and the
    /// gateway-side relay of the restored speed to the client (see the work-item note — the create
    /// block hardcodes `speeds::RUN`).
    #[test]
    fn the_rebuild_and_persist_call_sites_thread_the_gm_playtest_carry_columns() {
        let persist =
            crate::test_scan::code_of(include_str!("world.rs"), "pub(crate) fn persist_entity(");
        assert!(
            persist.contains(
                "persisted_gm_playtest(entity.godmode, entity.run_speed_mult_bp, set_offline)"
            ),
            "persist_entity must stamp the carry columns THROUGH the policy fn (passing the live \
             entity's values and this persist's set_offline sense). Body was:\n{persist}"
        );
        assert!(
            persist.contains("c.pending_godmode = pending_godmode;")
                && persist.contains("c.pending_run_speed_mult_bp = pending_speed_bp;"),
            "…and assign BOTH results to the durable row — computing them and dropping them on the \
             floor is the 289 bug with extra steps. Body was:\n{persist}"
        );

        let build = crate::test_scan::code_of(
            include_str!("creatures/spawn.rs"),
            "pub fn build_player_entity(",
        );
        assert!(
            build.contains("godmode: character.pending_godmode,")
                && build.contains("run_speed_mult_bp: character.pending_run_speed_mult_bp,"),
            "build_player_entity (the ONLY constructor the WORLDPORT_ACK rebuild runs) must read the \
             carry columns. Body was:\n{build}"
        );
        assert!(
            !build.contains("godmode: false,") && !build.contains("run_speed_mult_bp: 10_000,"),
            "…and must NOT hardcode them again — that hardcode IS work-item 289. Body was:\n{build}"
        );
    }

    #[test]
    fn ghost_restored_fields_reapply_exactly_the_release_state_over_the_fresh_build() {
        use lyracore_shared::constants::{player_flags, unit_vis_flags};
        // A Warrior's fresh build carries the Battle-Stance byte (17 << 16) in unit_bytes_1 — the
        // ghost render bit must OR over it, not clobber it (and vice versa for player_flags).
        let (dead, health, pf, ub1) = ghost_restored_fields(0, 17u32 << 16);
        assert!(dead, "a preserved ghost rebuilds dead");
        assert_eq!(
            health, 1,
            "ghost HP is 1 (vanilla), matching do_repop's release"
        );
        assert_eq!(pf, player_flags::GHOST, "the GHOST player flag is set");
        assert_eq!(
            ub1,
            (17u32 << 16) | unit_vis_flags::GHOST,
            "the ghost render bit ORs over the stance byte"
        );
        // Idempotent over already-ghosted inputs (a double-application can't corrupt the flags).
        let (_, _, pf2, ub2) = ghost_restored_fields(pf, ub1);
        assert_eq!((pf2, ub2), (pf, ub1));
    }

    #[test]
    fn inspect_gate_admits_only_an_in_range_friendly_player() {
        // The all-pass baseline: a friendly player, same map, point-blank.
        assert!(can_inspect(true, true, 0.0, true).is_ok());
        // In range at exactly the cap (10 yd)² is still OK (inclusive, matching the skin/loot gates).
        assert!(can_inspect(true, true, INSPECT_RANGE_SQ, true).is_ok());

        // Each reject, in the order the gate checks them:
        assert_eq!(
            can_inspect(false, true, 0.0, true).unwrap_err(),
            "inspect target is not a player"
        );
        assert_eq!(
            can_inspect(true, false, 0.0, true).unwrap_err(),
            "inspect target on another map"
        );
        assert_eq!(
            can_inspect(true, true, INSPECT_RANGE_SQ + 1.0, true).unwrap_err(),
            "inspect target out of range"
        );
        assert_eq!(
            can_inspect(true, true, 0.0, false).unwrap_err(),
            "inspect target is not friendly"
        );
    }

    #[test]
    fn accrue_played_folds_elapsed_whole_seconds_into_the_pool() {
        // A never-started session (stamp 0, e.g. a double-persist) leaves the pool untouched.
        assert_eq!(accrue_played_on_persist(5_000_000, 0, 42), 42);
        // 90.5s elapsed → truncates to 90 whole seconds, added to the existing pool.
        let now = 1_000_090_500_000_i64;
        let session_start = 1_000_000_000_000_u64;
        assert_eq!(accrue_played_on_persist(now, session_start, 10), 100);
        // Sub-second span (e.g. an immediate relog) truncates to 0 added.
        assert_eq!(accrue_played_on_persist(1_000_000_500, 1_000_000_000, 0), 0);
    }

    #[test]
    fn spirit_healer_res_restores_half_vitals_and_seeds_the_right_sickness() {
        // The graveyard res lands a player at 50% of each vital pool (the same percent reclaim_corpse
        // uses for health, extended to mana). Even numbers halve exactly.
        assert_eq!(spirit_res_vitals(200, 80), (100, 40));
        // Odd max → integer-floored half (199/2 = 99), like the live `max_health / 2`.
        assert_eq!(spirit_res_vitals(199, 81), (99, 40));
        // A rage/energy class carries max_power 0 → 0 mana restored (the `/ 2` is harmless), and the
        // health floor never returns 0: 1 hp → max(1) keeps the res alive (a ghost is never res'd dead).
        assert_eq!(spirit_res_vitals(1, 0), (1, 0));
        assert_eq!(
            spirit_res_vitals(0, 0),
            (1, 0),
            "the .max(1) floor keeps a 0-max res alive"
        );
        // The sickness debuff id the res applies is the seeded vanilla Resurrection Sickness (15007).
        assert_eq!(RESURRECTION_SICKNESS_SPELL, 15007);
    }

    // ---- Issue #59 defect 1: cascade_delete_character ratchets the guid allocator -------------

    /// `body_of`/`code_of`/`shape_of` are the shared scan primitives in [`crate::test_scan`]
    /// (issue #64 — this used to be six near-identical copies, drifted apart).
    use crate::test_scan::shape_of;

    /// Every character-delete path — transfer-driven, CMSG_CHAR_DELETE, `debug_delete_character` —
    /// funnels through this ONE function, which is why the guid-allocator ratchet lives here rather
    /// than at each call site.
    ///
    /// This asserts the ratchet is the body's FIRST statement — an exact prefix match, not
    /// `.contains()`. `cascade_delete_character` is a large, actively-edited sweep function, so
    /// pinning its ENTIRE shape (the technique `auth.rs` uses for its two small wrappers) would be
    /// too brittle here; a prefix is cheap AND still closes every round-1 defeat: reordering the
    /// ratchet after the sweeps/delete changes what the prefix IS (fails); wrapping the call in a
    /// shadowed `{ let character_guid = 0u64; ... }` block inserts a `let` before it (fails);
    /// commenting the real call out and replacing it with `let _ = ctx;` puts different code first
    /// (fails). `.contains()` on the same body caught none of the three (round-1 review).
    /// A deleted character must leave NO combat rows behind, in EITHER direction. The forward half
    /// (its own outgoing attack) was always covered; the reverse half was not, and that is what
    /// stranded ~100 creatures on a live realm when 300 bots despawned — each still holding a melee
    /// row against a guid that no longer existed, which the aggro pass then refuses to re-arm
    /// (it skips anything already attacking) and nothing ever disengages.
    ///
    /// Issue #365: this used to assert the three hand-rolled needles (`melee.attacker_guid().delete`,
    /// `melee.by_target().filter`, `threat::clear_for_unit`) that `cascade_delete_character` inlined
    /// instead of calling the real `combat::disengage` helper — which is exactly why the hand-roll
    /// was free to drift (it silently omitted `disengage`'s IN_COMBAT clear on freed attackers). Now
    /// that the body routes through the canonical helper, assert THAT call instead of its innards —
    /// pinning the needles again would just let the same drift happen a second time.
    #[test]
    fn cascade_delete_frees_both_directions_of_combat() {
        let body = crate::test_scan::code_of(
            include_str!("world.rs"),
            "pub(crate) fn cascade_delete_character(",
        );
        assert!(
            body.contains("crate::combat::disengage(ctx, character_guid)"),
            "cascade_delete_character no longer routes combat teardown through the canonical \
             `combat::disengage` helper — issue #365 needs this chokepoint to stay routed through \
             the one place that also clears IN_COMBAT on freed attackers, not a hand-rolled copy of \
             its steps. Body was:\n{body}"
        );
    }

    #[test]
    fn cascade_delete_character_ratchets_the_guid_allocator_as_its_first_statement() {
        let shape = shape_of(
            include_str!("world.rs"),
            "pub(crate) fn cascade_delete_character(",
        );
        let want_prefix = "{ crate::auth::bump_guid_high_water(ctx, character_guid);";
        assert!(
            shape.starts_with(want_prefix),
            "cascade_delete_character no longer ratchets the guid allocator as its first \
             statement — issue #59 defect 1 needs it to run before EVERY sweep and the final row \
             delete, not merely somewhere in the body. Shape was:\n{shape}"
        );
    }
}
