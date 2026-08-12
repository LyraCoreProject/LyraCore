//! The durable Character table. Public but RLS-restricted so a player connection only ever sees
//! its own characters (owner bound at `establish_session`). [entity]

use spacetimedb::{table, Identity};

/// Durable character; exists whether or not online. [entity]
#[table(accessor = game_character, public, index(accessor = by_account, btree(columns = [account_id])))]
pub struct Character {
    #[primary_key]
    pub guid: u64,
    pub account_id: u64,
    pub owner_identity: Identity, // Identity::ZERO until bound at establish_session
    #[unique]
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub level: u8,
    pub xp: u32,            // current XP toward next level
    pub next_level_xp: u32, // XP threshold to ding (0 at cap)
    pub map_id: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub first_login: bool,
    pub online: bool,
    /// Persisted purse in copper; mirrors WorldEntity.money while online. `#[default(0)]`
    /// so adding this column auto-migrates existing rows instead of forcing a `-c` data wipe.
    #[default(0)]
    pub money: u32,
    /// Rested-XP pool in XP points: accrued from offline time at login, drained as it doubles
    /// kill XP in `xp::award_xp`. Durable (the whole point — it builds up while logged out). `#[default(0)]`
    /// → auto-migrates; 0 = no rest bonus, so existing characters are unaffected until they log out/in.
    #[default(0)]
    pub rested_xp: u32,
    /// Unix-epoch micros of this character's last logout, stamped by `persist_entity` on a real
    /// logout/disconnect and consumed (reset to 0) by the login rested-XP accrual. 0 = never logged out
    /// (no accrual on first login). `#[default(0u64)]` — TYPED: a bare `0` is encoded as a 4-byte i32 and
    /// the publish migration rejects it for a u64 column ("data too short for u64") → auto-migrates. [entity]
    #[default(0u64)]
    pub last_logout_micros: u64,
    /// Hearthstone home (the bound recall point — vanilla's innkeeper bind). `use_hearthstone` teleports
    /// here; `bind_home` sets it to the player's current position (an approximation of vanilla's inn
    /// bind point). `create_character` seeds these to the start position so a fresh char recalls home.
    /// Typed defaults + END-appended → auto-migrate (the `last_logout_micros` precedent: a bare `0` is a
    /// 4-byte i32 and the migration rejects it for a u32/f32 column). [entity]
    #[default(0u32)]
    pub home_map: u32,
    #[default(0u32)]
    pub home_zone: u32,
    #[default(0.0f32)]
    pub home_x: f32,
    #[default(0.0f32)]
    pub home_y: f32,
    #[default(0.0f32)]
    pub home_z: f32,
    /// Accrued played-time total in whole seconds, for `CMSG_PLAYED_TIME`/`/played`. Advanced by
    /// `persist_entity` on every persist (real logout AND the ghost-relog cleanup in `player_login`)
    /// using the elapsed time since `session_start_micros`. `#[default(0)]` → auto-migrates existing
    /// rows to "0 played" rather than forcing a wipe. [entity]
    #[default(0)]
    pub played_total_secs: u32,
    /// Unix-epoch micros this character's CURRENT session began, stamped by `player_login` and
    /// consumed (reset to 0) by `persist_entity` once its elapsed span is folded into
    /// `played_total_secs`. 0 = not currently in a live session (offline, or between persist and the
    /// next login). TYPED `0u64` — a bare `0` encodes as a 4-byte i32 and the publish migration
    /// rejects it for a u64 column (the `last_logout_micros` precedent). [entity]
    #[default(0u64)]
    pub session_start_micros: u64,
    /// Persisted current health at last logout, mirrors `WorldEntity.health` while online. `0` is
    /// the sentinel for "no persisted value yet" (a fresh character, or an existing row migrated
    /// before this column existed) — `build_player_entity` treats 0 as "spawn at full health". A real
    /// logout at 1..=max_health persists exactly that value. `#[default(0u32)]` → END-appended,
    /// additive auto-migrate (the `home_map` precedent). [entity]
    #[default(0u32)]
    pub health: u32,
    /// Persisted current power (mana/rage/energy) at last logout, mirrors
    /// `WorldEntity.power`. Same `0` = "no persisted value" sentinel as `health` — 0 is also a
    /// legitimate live value for rage/energy classes, so `build_player_entity` disambiguates using
    /// `health == 0` (a character can never legitimately persist at 0 health; the relog-alive rule
    /// clamps that case to 1 before saving) rather than power alone. [entity]
    #[default(0u32)]
    pub power: u32,
    /// Number of times this character has reset its talents at a trainer (`talent::do_reset_talents`) —
    /// the escalation counter `talent::respec_cost_copper` reads to price the NEXT reset (never decays,
    /// never resettable; a durable lifetime count, like `played_total_secs`). `#[default(0u32)]` — TYPED,
    /// END-appended (the `home_map`/`health`/`power` precedent: a bare `0` is a 4-byte i32 and the publish
    /// migration rejects it for a u32 column) → auto-migrates existing characters to "never respec'd". [entity]
    #[default(0u32)]
    pub respec_count: u32,
    /// Corpse-reclaim escalation deadline (`corpse::escalated_reclaim`'s recurrence state) — DURABLE so a
    /// disconnect/relog cannot reset the death ladder back to 30s (the live entity is rebuilt on every
    /// login; without this column the most common interruption defeated the escalation entirely).
    /// Persisted at logout, threaded back through `build_player_entity`. `#[default(0i64)]` — TYPED,
    /// END-appended. [entity]
    #[default(0i64)]
    pub death_expire_micros: i64,
    /// The instance to enter THIS character into on its next rebuild (work-item 190 slice 1 —
    /// always 0 this slice). Set at teleport-accept time (slice 2's dungeon entry) so a relog
    /// inside a dungeon puts the rebuilt entity back in the right instance rather than open
    /// world; `build_player_entity` reads it, `persist_entity` writes the live entity's
    /// `instance_id` back here on logout. `#[default(0u64)]` (typed — u64 needs 8 bytes) +
    /// END-appended so `publish` auto-migrates existing rows (migration rule). [entity]
    #[default(0u64)]
    pub pending_instance_id: u64,
    /// GM playtest authorization level (work-item 223): `0` = no access to any `.command`; the
    /// operator-only `gm::set_gm_level` reducer is the only writer. Moderation-facing per-level
    /// distinctions beyond "has access at all" are work-item 205's concern, not this one's — every
    /// `gm_command` today only checks `gm_level != 0`. `#[default(0)]` + END-appended so `publish`
    /// auto-migrates existing characters to "no GM access" (safe default). Gateway-subscribed
    /// (`game_character` is in the coordinator's subscription list) → hand-synced in
    /// `character_type.rs` + widened in `gateway/tests/schema_parity.rs`.
    #[default(0)]
    pub gm_level: u8,
    /// Released-GHOST state that must survive an entity despawn/rebuild (work-item 226 — the 224
    /// review-finding-#2 landmine): a cross-map graveyard release (`do_repop` on map 36 → a Westfall
    /// graveyard on map 0) DESPAWNS the live entity, and the `MSG_MOVE_WORLDPORT_ACK` rebuild goes
    /// through `player_login`, whose relog path deleted the corpse and rebuilt `dead: false` — a
    /// silent free resurrect. `persist_entity` stamps this from the live entity's actual ghost state
    /// (`dead` + `PLAYER_FLAGS_GHOST`) on every NON-logout persist (cross-map hop, stale-entity
    /// cleanup) and FORCES it false on a real logout/disconnect (`set_offline` — the established
    /// "relog comes back alive" rule, whose corpse delete already lives in `remove_from_world`);
    /// `player_login` consumes it: skip the corpse delete + re-apply ghost state onto the rebuilt
    /// entity (`world::ghost_restored_fields`). `#[default(false)]` + END-appended so `publish`
    /// auto-migrates existing rows (migration rule). Gateway-subscribed (`game_character` is in the
    /// coordinator's subscription list) → hand-synced in `character_type.rs` + widened in
    /// `gateway/tests/schema_parity.rs` (the `gm_level` precedent).
    #[default(false)]
    pub pending_ghost: bool,
    /// Rest state (196): logged out in a rest area (inn/city)? Stamped by `persist_entity` from the live
    /// entity's `resting` flag at logout, read by `player_login` to pick the offline rested rate (full in
    /// a rest area vs 1/4 in the field) and to spawn the character already showing the zzz/blue-bar byte.
    /// `#[default(false)]` + END-appended → auto-migrates. Gateway-subscribed (`game_character`) →
    /// hand-synced in `character_type.rs` + `schema_parity.rs` (the `gm_level`/`pending_ghost` precedent).
    #[default(false)]
    pub resting: bool,
    /// Live-accrual clock (196): unix-epoch micros from which un-materialized ONLINE rested time is
    /// counted; 0 = not live-accruing (offline, or not in a rest area). The `rested_accrue_pass` tick
    /// grows `rested_xp` from this stamp and re-stamps it once ≥1 XP banks (lossless). `#[default(0u64)]`
    /// — TYPED (a bare `0` is a 4-byte i32 → the u64-column migration rejects it, `last_logout_micros`
    /// precedent) + END-appended → auto-migrates.
    #[default(0u64)]
    pub rested_since_micros: u64,
    /// GM playtest GODMODE carried across an entity REBUILD (work-item 289 — the 226 landmine wearing
    /// a GM hat): a CROSS-MAP `.tele` (and every cross-database shard hop, which rides the same
    /// primitive) DESPAWNS the live entity, and `build_player_entity` rebuilds it from THIS row — so
    /// before this column, `.god` was silently dropped on arrival and the GM was eaten by the local
    /// wildlife with no message. `persist_entity` stamps it from the live entity on every NON-logout
    /// persist (cross-map hop, shard-transfer freeze, stale-entity cleanup) and FORCES it off on a
    /// real logout/disconnect (`set_offline`) — see `persisted_gm_playtest` for the policy and its
    /// rationale (a map change is a loading screen; a session boundary is a deliberate reset).
    /// `#[default(false)]` + END-appended so `publish` auto-migrates existing rows to "not godmode"
    /// (byte-identical to before this column existed). Gateway-subscribed (`game_character` is in the
    /// coordinator's subscription list) → hand-synced in `character_type.rs` + widened in
    /// `gateway/tests/schema_parity.rs` (the `gm_level`/`pending_ghost` precedent).
    #[default(false)]
    pub pending_godmode: bool,
    /// GM playtest RUN-SPEED multiplier (basis points, 10000 = 1.0×) carried across an entity rebuild —
    /// `pending_godmode`'s twin, same stamp/clear policy (`persisted_gm_playtest`), same reason:
    /// `.speed 3` then `.tele valley` used to arrive back at 1×. `#[default(10000)]` (a bare int
    /// literal is 4 bytes, which is exactly a u32 — the `WorldEntity::run_speed_mult_bp` precedent) +
    /// END-appended so `publish` auto-migrates existing rows to 1× (byte-identical to before this
    /// column existed). Gateway-subscribed → hand-synced in `character_type.rs` + widened parity.
    #[default(10000)]
    pub pending_run_speed_mult_bp: u32,
    /// Bank bag slots bought at a banker (0..=6). Mirrors `WorldEntity.bank_bag_slots` while online
    /// exactly as `money` does: the purchase writes the live entity, `persist_entity` writes it back
    /// here. `#[default(0)]` + END-appended → auto-migrates existing characters to "owns none".
    /// Gateway-subscribed (`game_character`) → hand-synced in `character_type.rs` + `schema_parity.rs`.
    #[default(0)]
    pub bank_bag_slots: u8,
}
