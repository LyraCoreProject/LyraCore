//! Encounter kernel (work-item 228) — the mangos `InstanceScript` analogue as a PACKAGE-SUPPORT
//! layer. Ships ZERO encounter content: dungeons' choreography lives in drop-in packages
//! (`packages/<dungeon>/`, first consumer: 227 Deadmines), which consume three things from here:
//!
//! 1. **State** — `game_encounter_state`, one row per `(instance_id, encounter_id)` (the playbook's
//!    PK-pair pattern: synthetic `#[auto_inc]` PK + a btree on the pair, like
//!    `game_player_reputation`'s `(character_guid, faction_id)`). `state` is the mangos 4-state
//!    machine (NotStarted/InProgress/Done/Failed), `payload` the `SetData`/`GetData` u32. NOT
//!    gateway-subscribed — packages and core read it server-side only (verified absent from
//!    `gateway/src/stdb/connection.rs`'s subscription list; nothing client-side needs it).
//! 2. **Hooks** — three new notify-hook events on the EXISTING build.rs marker-scan substrate
//!    (`game_hook!` + `HOOK_EVENTS` row + payload struct in hooks.rs — NOT a second mechanism):
//!    `on_creature_death` (entry-carrying, instance-stamped; fired from `combat::kill_creature`),
//!    `on_go_used` (fired from `gameobject::apply_use_gameobject`'s success exit), and
//!    `on_hp_threshold` (fired from THIS module's `encounter_hp_probe`, itself a `game_hook!` on
//!    the existing `on_damage_taken` event — the damage chokepoint needed no new dispatch line).
//!    `on_hp_threshold` fires ONCE per watched threshold per instance (heal-and-re-cross never
//!    re-fires until reset), deduped via a RESERVED `game_encounter_state` row (see
//!    [`hp_fired_key`]). ZERO-COST when unused: the probe's first instruction bails on the
//!    compile-time-const `GAME_HOOKS_ON_HP_THRESHOLD.is_empty()`, so a tree with no registered
//!    consumer pays one predictable branch per damage event and nothing else.
//! 3. **Primitives** — thin package-callable choreography verbs: [`open_door`], [`spawn_wave`]
//!    (with despawn-on-reset bookkeeping via `game_encounter_spawn`), [`equip_swap`] (durable
//!    virtual-item display projection), [`move_to_point`] (the
//!    existing creature move-event emission pattern, used directly — work-item 181's shared leg
//!    chokepoint is NOT built yet; fold this in when it lands), and [`encounter_reset`].
//!
//! Reset/sweep: 190 slice 3 (instance reap) deletes per-instance kernel state via
//! [`sweep_encounter_state`], called from `instance::teardown_instance_inner` right before the
//! `game_instance` row itself goes (the splice point is documented on the fn, mirroring 229's
//! `debug_arm_instance_tick` splice notes). [server]

use spacetimedb::{table, ReducerContext, Table};

use lyracore_shared::{constants, spatial};

use crate::game_item_template;

use crate::{
    game_creature_spawn, game_creature_spline, game_creature_template, game_gameobject,
    game_gameobject_template, game_instance, game_world_entity,
};

/// Package-owned encounter selected by the EventAI import boundary.
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncounterBinding {
    BlackfathomDeepsKelris,
    BlackrockDepthsTombOfSeven,
    DireMaulAlzzin,
    RazorfenKraulWardKeepers,
    ShadowfangKeepRethilgore,
    ShadowfangKeepFenrus,
    ShadowfangKeepNandos,
    SunkenTempleAvatar,
    WailingCavernsAnacondra,
    WailingCavernsCobrahn,
    WailingCavernsPythas,
    WailingCavernsSerpentis,
    WailingCavernsMutanus,
    ZulGurubOhgan,
}

pub(crate) type EncounterPackageHandler =
    fn(&ReducerContext, u64, EncounterSignal) -> Result<(), String>;

impl EncounterBinding {
    pub const ALL: [Self; 14] = [
        Self::BlackfathomDeepsKelris,
        Self::BlackrockDepthsTombOfSeven,
        Self::DireMaulAlzzin,
        Self::RazorfenKraulWardKeepers,
        Self::ShadowfangKeepRethilgore,
        Self::ShadowfangKeepFenrus,
        Self::ShadowfangKeepNandos,
        Self::SunkenTempleAvatar,
        Self::WailingCavernsAnacondra,
        Self::WailingCavernsCobrahn,
        Self::WailingCavernsPythas,
        Self::WailingCavernsSerpentis,
        Self::WailingCavernsMutanus,
        Self::ZulGurubOhgan,
    ];

    pub fn map_id(self) -> u32 {
        match self {
            Self::BlackfathomDeepsKelris => 48,
            Self::BlackrockDepthsTombOfSeven => 230,
            Self::DireMaulAlzzin => 429,
            Self::RazorfenKraulWardKeepers => 47,
            Self::ShadowfangKeepRethilgore
            | Self::ShadowfangKeepFenrus
            | Self::ShadowfangKeepNandos => 33,
            Self::SunkenTempleAvatar => 109,
            Self::WailingCavernsAnacondra
            | Self::WailingCavernsCobrahn
            | Self::WailingCavernsPythas
            | Self::WailingCavernsSerpentis
            | Self::WailingCavernsMutanus => 43,
            Self::ZulGurubOhgan => 309,
        }
    }

    pub fn encounter_id(self) -> u32 {
        match self {
            Self::BlackfathomDeepsKelris => 1,
            Self::BlackrockDepthsTombOfSeven => 4,
            Self::DireMaulAlzzin => 0,
            Self::RazorfenKraulWardKeepers => 1,
            Self::ShadowfangKeepRethilgore => 2,
            Self::ShadowfangKeepFenrus => 3,
            Self::ShadowfangKeepNandos => 4,
            Self::SunkenTempleAvatar => 4,
            Self::WailingCavernsAnacondra => 0,
            Self::WailingCavernsCobrahn => 1,
            Self::WailingCavernsPythas => 2,
            Self::WailingCavernsSerpentis => 3,
            Self::WailingCavernsMutanus => 5,
            Self::ZulGurubOhgan => 5,
        }
    }
}

/// Named notification delivered to the encounter package that owns [`EncounterBinding`].
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncounterSignal {
    Begin,
    Fail,
    Complete,
    BreakAlzzinCrumbleWall,
    InterruptAvatarSuppression,
    SendMandokirDownstairs,
}

/// Route one EventAI notification through encounter authority. The binding checks both package
/// ownership and map scope before any state is changed.
pub(crate) fn notify_from_eventai(
    ctx: &ReducerContext,
    source_guid: u64,
    binding: EncounterBinding,
    signal: EncounterSignal,
) -> Result<(), String> {
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .ok_or_else(|| format!("encounter notification source {source_guid} is missing"))?;
    if source.map_id != binding.map_id() {
        return Err(format!(
            "encounter binding {binding:?} belongs to map {}, not source map {}",
            binding.map_id(),
            source.map_id
        ));
    }
    if source.instance_id == 0 {
        return Err(format!(
            "encounter binding {binding:?} needs an instance-scoped source"
        ));
    }
    let instance = ctx
        .db
        .game_instance()
        .instance_id()
        .find(source.instance_id)
        .ok_or_else(|| {
            format!(
                "encounter source instance {} is missing",
                source.instance_id
            )
        })?;
    if instance.map_id != source.map_id {
        return Err(format!(
            "encounter source map {} does not match instance {} map {}",
            source.map_id, source.instance_id, instance.map_id
        ));
    }
    let handler = package_handler_for(binding, signal, crate::GAME_ENCOUNTER_PACKAGES)?;
    handler(ctx, source.instance_id, signal)
}

fn package_handler_for(
    binding: EncounterBinding,
    signal: EncounterSignal,
    installed: &[(EncounterBinding, EncounterPackageHandler)],
) -> Result<EncounterPackageHandler, String> {
    let mut handlers = installed
        .iter()
        .filter(|(installed, _)| *installed == binding)
        .map(|(_, handler)| *handler);
    let handler = handlers.next().ok_or_else(|| {
        format!("encounter binding {binding:?} has no installed package authority for {signal:?}")
    })?;
    if handlers.next().is_some() {
        return Err(format!(
            "encounter binding {binding:?} has more than one installed package authority"
        ));
    }
    Ok(handler)
}

#[cfg(test)]
fn encounter_state_for_signal(signal: EncounterSignal) -> Option<u8> {
    match signal {
        EncounterSignal::Begin => Some(ENCOUNTER_IN_PROGRESS),
        EncounterSignal::Fail => Some(ENCOUNTER_FAILED),
        EncounterSignal::Complete => Some(ENCOUNTER_DONE),
        EncounterSignal::BreakAlzzinCrumbleWall
        | EncounterSignal::InterruptAvatarSuppression
        | EncounterSignal::SendMandokirDownstairs => None,
    }
}

// ===========================================================================================
//  Encounter state machine values (mangos EncounterState analogue)
// ===========================================================================================

pub const ENCOUNTER_NOT_STARTED: u8 = 0;
pub const ENCOUNTER_IN_PROGRESS: u8 = 1;
pub const ENCOUNTER_DONE: u8 = 2;
pub const ENCOUNTER_FAILED: u8 = 3;

/// Encounter ids with this bit set are RESERVED for kernel bookkeeping (today: the per-entry
/// HP-threshold fired-marks, [`hp_fired_key`]). Packages must key their encounters below it —
/// vanilla creature entries are < 2^24, so the namespaces can never collide.
pub const RESERVED_ENCOUNTER_ID_BIT: u32 = 0x8000_0000;

/// Sentinel for "no HP threshold has fired yet" in a reserved dedup row's `payload` — one past the
/// highest possible percentage, so every watch `pct` (0..=100) compares strictly below it.
pub const HP_FIRED_NONE: u32 = 101;

// ===========================================================================================
//  Tables (all server-side only — none is gateway-subscribed)
// ===========================================================================================

/// Per-(instance, encounter) state + scratch data — the `InstanceScript::SetData/GetData` store.
/// Logical key `(instance_id, encounter_id)` via an `#[auto_inc]` PK + the `by_pair` btree (the
/// PK-pair pattern; see `game_player_reputation`). A missing row reads as
/// (`ENCOUNTER_NOT_STARTED`, payload 0). `encounter_id`s with [`RESERVED_ENCOUNTER_ID_BIT`] set
/// are kernel bookkeeping rows (HP dedup), not package encounters. Swept per instance by
/// [`sweep_encounter_state`] (190 slice 3). [server]
#[table(accessor = game_encounter_state, index(accessor = by_pair, btree(columns = [instance_id, encounter_id])))]
pub struct EncounterState {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub instance_id: u64,
    pub encounter_id: u32,
    pub state: u8,
    pub payload: u32,
}

/// A registered HP-threshold watch: "fire `on_hp_threshold` when a creature of `entry` drops to
/// `pct`% or below". GLOBAL (template-scoped, not per-instance — the per-instance once-only dedup
/// lives in the reserved `game_encounter_state` rows), registered idempotently by packages via
/// [`watch_hp_threshold`]. Never swept with an instance. [server]
#[table(accessor = game_encounter_hp_watch, index(accessor = by_entry, btree(columns = [entry])))]
pub struct EncounterHpWatch {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub entry: u32,
    pub pct: u8,
}

/// Despawn-on-reset bookkeeping for [`spawn_wave`]: one row per wave-spawned creature, so
/// [`encounter_reset`] / [`sweep_encounter_state`] can tear the adds (entity + spawn row + legs +
/// loot) back down. CRITICAL: wave adds get real `game_creature_spawn` rows (so corpse decay works)
/// which are NOT instance-tagged — an untracked leftover spawn row would eventually respawn into
/// instance 0 (`pass_respawn` builds at instance 0), so every reset path MUST delete via this
/// table. [server]
#[table(accessor = game_encounter_spawn, index(accessor = by_instance, btree(columns = [instance_id])))]
pub struct EncounterSpawn {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub instance_id: u64,
    pub encounter_id: u32,
    pub guid: u64,
}

/// Client-visible virtual-item display slots for a creature. [`equip_swap`] resolves authored item
/// entries through `game_item_template` before writing this projection. The gateway relays the three
/// display fields through its crash-safe partial VALUES path and replays them after a peer CREATE.
/// Swept per instance ([`sweep_encounter_state`]) and per tracked despawn. [entity]
#[table(accessor = game_encounter_equip, public, index(accessor = by_instance, btree(columns = [instance_id])))]
pub struct EncounterEquip {
    #[primary_key]
    pub creature_guid: u64,
    pub instance_id: u64,
    pub main_hand: u32,
    pub off_hand: u32,
    pub ranged: u32,
}

// ===========================================================================================
//  Pure decisions (unit-tested below)
// ===========================================================================================

/// The reserved `encounter_id` carrying the per-instance HP-threshold fired-mark for creature
/// `entry` (its `payload` = lowest pct fired so far, [`HP_FIRED_NONE`] when virgin). Pure.
pub fn hp_fired_key(entry: u32) -> u32 {
    RESERVED_ENCOUNTER_ID_BIT | entry
}

/// Is `encounter_id` in the kernel-reserved namespace? Pure.
pub fn is_reserved_encounter_id(encounter_id: u32) -> bool {
    encounter_id & RESERVED_ENCOUNTER_ID_BIT != 0
}

/// Current HP as an integer percentage (floor), u64 math so `health * 100` can never wrap.
/// `max == 0` (malformed row) reads as 100% — nothing fires. Pure.
pub fn hp_pct(health: u32, max_health: u32) -> u32 {
    if max_health == 0 {
        return 100;
    }
    ((health as u64 * 100) / max_health as u64) as u32
}

/// THE crossing-dedup decision: which watched thresholds fire now that the creature sits at
/// `new_pct`%, given `lowest_fired` (the lowest pct already fired this instance, or
/// [`HP_FIRED_NONE`]). A watch `w` fires iff `new_pct <= w` (crossed) AND `w < lowest_fired`
/// (never fired — this is the once-per-instance memory: a heal back above `w` and a re-drop
/// changes nothing, because `lowest_fired` only ratchets DOWN). A single huge hit crossing several
/// watches fires them all, ordered high→low (66 before 33 — phase order). Duplicate watch rows
/// collapse to one firing. Returns the fired pcts (desc). Pure — unit-tested.
pub fn crossed_thresholds(watches: &[u8], lowest_fired: u32, new_pct: u32) -> Vec<u8> {
    let mut fired: Vec<u8> = watches
        .iter()
        .copied()
        .filter(|&w| new_pct <= w as u32 && (w as u32) < lowest_fired)
        .collect();
    fired.sort_unstable_by(|a, b| b.cmp(a));
    fired.dedup();
    fired
}

/// Wave-spawn guid: `HIGHGUID_UNIT` high bits + `entry` in 24..47 + a unique low 24 — the same
/// namespace layout the importer/seed/debug spawns use, so the 5875 client classifies the add as a
/// Unit and the low part can't collide with an existing spawn of the same entry. Pure.
pub fn wave_guid(entry: u32, next_low: u64) -> u64 {
    const HIGHGUID_UNIT: u64 = 0xF130;
    (HIGHGUID_UNIT << 48) | ((entry as u64) << 24) | (next_low & 0x00FF_FFFF)
}

/// The creature ENTRY carried in bits 24..47 of every unit guid this codebase mints (importer,
/// seed, debug, wave, instance-population — all share the layout above). Lets cleanup code
/// identify a guid's creature even after the entity row is gone (dead + decayed/reaped), which an
/// entity-table lookup cannot (227 review: the Smite equip reset silently no-op'd on exactly the
/// corpse-decayed case it existed for). Pure.
pub fn entry_of_unit_guid(guid: u64) -> u32 {
    ((guid >> 24) & 0x00FF_FFFF) as u32
}

/// Deterministic per-add spread around the wave anchor so a multi-add wave doesn't stack on one
/// point: a compact 3-wide grid, 2 yd pitch, row-major. Pure.
pub fn wave_offset(index: usize) -> (f32, f32) {
    (2.0 * ((index % 3) as f32 - 1.0), 2.0 * ((index / 3) as f32))
}

/// `[V]` The `game_gameobject.state` value that renders a DOOR/BUTTON as OPEN on the 5875 client.
/// The in-repo convention (gameobject.rs `toggle_state`'s test labels, work-item 211) is
/// 0 = closed/ready, 1 = open/used, and the gateway relays the raw value
/// (`set_gameobject_state(go.state)`); no live client in this sandbox to confirm the rendering —
/// if a live check shows doors inverted, flip THIS const (one place).
pub const DOOR_OPEN_STATE: u8 = 1;

// ===========================================================================================
//  State accessors (SetData / GetData)
// ===========================================================================================

fn state_row(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) -> Option<EncounterState> {
    ctx.db
        .game_encounter_state()
        .by_pair()
        .filter((instance_id, encounter_id))
        .next()
}

/// Upsert the full row (internal — the public accessors preserve the other column).
fn upsert_state_row(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    state: u8,
    payload: u32,
) {
    let t = ctx.db.game_encounter_state();
    match state_row(ctx, instance_id, encounter_id) {
        Some(mut row) => {
            row.state = state;
            row.payload = payload;
            t.id().update(row);
        }
        None => {
            t.insert(EncounterState {
                id: 0,
                instance_id,
                encounter_id,
                state,
                payload,
            });
        }
    }
}

/// Current state of `(instance_id, encounter_id)` — `ENCOUNTER_NOT_STARTED` when no row exists.
pub fn get_encounter_state(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) -> u8 {
    state_row(ctx, instance_id, encounter_id)
        .map(|r| r.state)
        .unwrap_or(ENCOUNTER_NOT_STARTED)
}

/// Set the state, preserving `payload`. Upserts. Packages own the transition semantics (the
/// kernel enforces none — mangos likewise lets a script move Failed→InProgress on a retry).
/// REFUSES a reserved (bit-31) encounter_id: writing one would silently corrupt an hp-fired
/// ratchet row (`hp_fired_key(entry)` shares the table) — review finding; refuse, never corrupt.
pub fn set_encounter_state(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    state: u8,
) -> Result<(), String> {
    if is_reserved_encounter_id(encounter_id) {
        return Err(format!(
            "encounter_id {encounter_id:#x} is in the kernel-reserved bit-31 namespace (hp-fired ratchets)"
        ));
    }
    let payload = get_encounter_data(ctx, instance_id, encounter_id);
    upsert_state_row(ctx, instance_id, encounter_id, state, payload);
    Ok(())
}

/// `GetData`: the row's scratch u32 — 0 when no row exists.
pub fn get_encounter_data(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) -> u32 {
    state_row(ctx, instance_id, encounter_id)
        .map(|r| r.payload)
        .unwrap_or(0)
}

/// `SetData`: set the scratch u32, preserving `state`. Upserts. Same reserved-namespace refusal
/// as [`set_encounter_state`].
pub fn set_encounter_data(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    data: u32,
) -> Result<(), String> {
    if is_reserved_encounter_id(encounter_id) {
        return Err(format!(
            "encounter_id {encounter_id:#x} is in the kernel-reserved bit-31 namespace (hp-fired ratchets)"
        ));
    }
    let state = get_encounter_state(ctx, instance_id, encounter_id);
    upsert_state_row(ctx, instance_id, encounter_id, state, data);
    Ok(())
}

// ===========================================================================================
//  HP-threshold watch + probe
// ===========================================================================================

/// Register "fire `on_hp_threshold` when a creature of `entry` reaches `pct`% or below" — the
/// package-side half of the hook (the `game_hook!(on_hp_threshold, ...)` handler is the other).
/// Idempotent (check-before-insert, the `learn_spell` grant idiom): packages typically call this
/// from their seed/fixture reducer or an `on_creature_spawn` handler, either of which may run
/// repeatedly. `pct` must be 1..=99 (0 is the death hook's job; 100 would fire on any scratch).
pub fn watch_hp_threshold(ctx: &ReducerContext, entry: u32, pct: u8) -> Result<(), String> {
    if !(1..=99).contains(&pct) {
        return Err(format!("hp watch pct must be 1..=99, got {pct}"));
    }
    if entry & RESERVED_ENCOUNTER_ID_BIT != 0 {
        // hp_fired_key(entry) FOLDS bit 31 — two entries >= 2^31 would share one ratchet row
        // (review finding). No vanilla entry is remotely close; refuse garbage loudly.
        return Err(format!(
            "hp watch entry {entry:#x} has bit 31 set (reserved-key fold)"
        ));
    }
    let t = ctx.db.game_encounter_hp_watch();
    if t.by_entry().filter(&entry).any(|w| w.pct == pct) {
        return Ok(()); // already registered
    }
    t.insert(EncounterHpWatch { id: 0, entry, pct });
    Ok(())
}

/// Forget the per-instance fired-marks for `entry` so its thresholds can fire again — a package
/// calls this from its wipe/evade reset alongside [`encounter_reset`] (the boss healed to full;
/// the next attempt's crossings must re-fire). Instance-scoped: other instances' marks are
/// untouched. The instance reap sweeps these rows wholesale ([`sweep_encounter_state`]).
pub fn reset_hp_fired(ctx: &ReducerContext, instance_id: u64, entry: u32) {
    let t = ctx.db.game_encounter_state();
    if let Some(row) = state_row(ctx, instance_id, hp_fired_key(entry)) {
        t.id().delete(row.id);
    }
}

// The kernel's own damage-side probe, registered on the EXISTING `on_damage_taken` event via the
// same marker-scan substrate packages use — so the damage chokepoint (`spell::break_auras_on_damage`,
// where the target's post-damage health is already committed: every caller updates the row first,
// and same-transaction read-your-writes holds) needed NO new dispatch line. ZERO-COST GUARD: the
// first instruction bails on the compile-time-const registry — a tree where no package registered
// an `on_hp_threshold` handler pays one predictable branch per damage event, no table reads.
crate::game_hook!(on_damage_taken, fn encounter_hp_probe(ctx, payload) {
    if crate::GAME_HOOKS_ON_HP_THRESHOLD.is_empty() {
        return; // no consumer anywhere in the build — free.
    }
    probe_hp_thresholds(ctx, payload.target_guid);
});

/// The ctx half of the HP-threshold hook: read the (already-committed) post-damage health, join it
/// against the entry's registered watches, dedup via the reserved state row, and fire
/// `on_hp_threshold` once per newly-crossed watch (high→low). The fired-mark is written BEFORE the
/// handlers run (mark-before-roll, playbook §2): a handler that itself deals damage re-enters this
/// probe with the ratchet already advanced, so it can never double-fire.
fn probe_hp_thresholds(ctx: &ReducerContext, target_guid: u64) {
    let Some(e) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return;
    };
    // Creatures only (players/pets are never encounter bosses); `on_damage_taken` already implies
    // the target survived, but a dead row is re-checked for safety.
    if e.is_player() || e.owner_guid != 0 || e.dead || e.max_health == 0 {
        return;
    }
    let watches: Vec<u8> = ctx
        .db
        .game_encounter_hp_watch()
        .by_entry()
        .filter(&e.entry)
        .map(|w| w.pct)
        .collect();
    if watches.is_empty() {
        return; // registered consumers exist, but not for this entry — one btree probe spent.
    }
    let key = hp_fired_key(e.entry);
    let lowest_fired = state_row(ctx, e.instance_id, key)
        .map(|r| r.payload)
        .unwrap_or(HP_FIRED_NONE);
    let fired = crossed_thresholds(&watches, lowest_fired, hp_pct(e.health, e.max_health));
    let Some(&new_lowest) = fired.last() else {
        return; // nothing newly crossed
    };
    // Ratchet FIRST (see fn doc), then notify.
    upsert_state_row(
        ctx,
        e.instance_id,
        key,
        ENCOUNTER_NOT_STARTED,
        new_lowest as u32,
    );
    for pct in fired {
        crate::hooks::fire_on_hp_threshold(
            ctx,
            &crate::hooks::HpThresholdPayload {
                creature_guid: target_guid,
                entry: e.entry,
                instance_id: e.instance_id,
                pct,
            },
        );
    }
}

// ===========================================================================================
//  Choreography primitives
// ===========================================================================================

/// Open every DOOR/BUTTON gameobject of `go_entry` (Rhahk'Zor's door on his death, the iron door
/// on the cannon blast): set `state` to [`DOOR_OPEN_STATE`] the way a player `use` does (same
/// row update — the gateway's `on_go_update` relay re-emits the CREATE_OBJECT, work-item 211),
/// but SET-OPEN rather than toggle so a re-fired death hook can never swing the door shut again.
/// Idempotent. `instance_id` filters to THAT instance's per-instance GO copies (190 slice 2 landed
/// the `game_gameobject.instance_id` column — the splice this doc used to defer): a boss death in
/// instance 7 opens instance 7's door and nobody else's; `instance_id` 0 targets the open-world
/// (static) rows. Errs when no such GO exists in that instance or its template isn't a DOOR/BUTTON
/// (a chest's `state` means "looted" — refusing the type keeps this verb from corrupting loot state).
pub fn open_door(ctx: &ReducerContext, go_entry: u32, instance_id: u64) -> Result<u32, String> {
    let tmpl = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(go_entry)
        .ok_or_else(|| format!("no gameobject template {go_entry}"))?;
    if tmpl.type_id != crate::gameobject::go_type::DOOR
        && tmpl.type_id != crate::gameobject::go_type::BUTTON
    {
        return Err(format!(
            "gameobject {go_entry} is type {} — open_door flips DOOR/BUTTON only",
            tmpl.type_id
        ));
    }
    let gos = ctx.db.game_gameobject();
    // ONE pass collects both the flip set and existence (review nit: was two full iters).
    // 190 slice 2: the instance-equality filter this loop's splice comment documented — landed.
    let mut any_of_entry = false;
    let mut targets: Vec<u64> = Vec::new();
    for g in gos.iter() {
        if g.template_entry == go_entry && g.instance_id == instance_id {
            any_of_entry = true;
            if g.state != DOOR_OPEN_STATE {
                targets.push(g.guid);
            }
        }
    }
    if !any_of_entry {
        return Err(format!(
            "no spawned gameobject with entry {go_entry} in instance {instance_id}"
        ));
    }
    let opened = targets.len() as u32;
    for guid in targets {
        if let Some(mut g) = gos.guid().find(guid) {
            g.state = DOOR_OPEN_STATE;
            gos.guid().update(g);
        }
    }
    Ok(opened)
}

// A wave's full placement (entries, count, anchor position, instance, encounter id).
#[allow(clippy::too_many_arguments)]
/// Spawn a wave of adds (VanCleef's bodyguards) at `(x, y, z)` facing `orientation`, tracked for
/// despawn-on-reset. Each add gets a REAL `game_creature_spawn` row (so the normal corpse-decay
/// pass reaps its body) with `respawn_secs = u32::MAX` (~136 years — decay arms the respawn that
/// far out, so a dead add NEVER respawns; the instance is reaped long before) and a
/// `game_encounter_spawn` tracking row so [`encounter_reset`]/[`sweep_encounter_state`] can tear
/// everything down. Spawns via the shared `build_creature_entity`/`insert_creature_entity` path
/// (so `on_creature_spawn` fires per add) at `instance_id`, spread on a 2 yd grid
/// ([`wave_offset`]). Aggro/leash then ride the normal proximity-aggro + pursuit-timer leash
/// passes — a per-wave leash RADIUS is not representable today (the engine's chase/leash radii are
/// global consts, and the return anchor is the spawn row, i.e. the wave anchor); a custom radius needs a
/// `CreatureSpawn` column (deferred, noted in the work item). Entries missing a template are
/// skipped with a log line. Returns the spawned guids in `entries` order.
pub fn spawn_wave(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    map_id: u32,
    entries: &[u32],
    x: f32,
    y: f32,
    z: f32,
    orientation: f32,
) -> Vec<u64> {
    let mut guids = Vec::with_capacity(entries.len());
    for (i, &entry) in entries.iter().enumerate() {
        let Some(tmpl) = ctx.db.game_creature_template().entry().find(entry) else {
            spacetimedb::log::warn!("spawn_wave: no creature template {entry} — skipped");
            continue;
        };
        // Unique low 24 bits: one past the current max for this entry (the debug_spawn_at_feet
        // scheme). Recomputed per add — a wave of N identical entries gets consecutive lows.
        let next_low = ctx
            .db
            .game_creature_spawn()
            .iter()
            .filter(|s| s.entry == entry)
            .map(|s| s.guid & 0x00FF_FFFF)
            .max()
            .unwrap_or(0)
            + 1;
        let guid = wave_guid(entry, next_low);
        let (dx, dy) = wave_offset(i);
        let spawn = crate::creatures::CreatureSpawn {
            guid,
            entry,
            map_id,
            x: x + dx,
            y: y + dy,
            z,
            orientation,
            // FAR-FUTURE, not `ctx.timestamp` (review MEDIUM): a past-armed respawn_at is
            // respawn-ELIGIBLE the instant the entity vanishes by any path that bypasses decay —
            // debug_clear_creatures deletes entities and deliberately keeps spawn rows, and
            // pass_respawn rebuilds at INSTANCE 0 — resurrecting the wave into the open world.
            // A far-future stamp is equally "never matches while alive" and stays safe after any
            // entity delete; decay's re-arm (respawn_secs = u32::MAX) keeps it far-future forever.
            respawn_at: ctx
                .timestamp
                .checked_add(spacetimedb::TimeDuration::from_micros(
                    u32::MAX as i64 * 1_000_000,
                ))
                .unwrap_or(ctx.timestamp),
            despawn_at: crate::creatures::timer_never(ctx), // not armed until this add actually dies
            movement_type: crate::creatures::MOVEMENT_IDLE, // holds its post until aggro
            respawn_secs: u32::MAX,                         // "never" — see the fn doc
            life_seq: 0,
        };
        let entity =
            crate::creatures::build_creature_entity(&spawn, &tmpl, ctx.random(), instance_id);
        ctx.db.game_creature_spawn().insert(spawn);
        crate::creatures::insert_creature_entity(ctx, entity);
        ctx.db.game_encounter_spawn().insert(EncounterSpawn {
            id: 0,
            instance_id,
            encounter_id,
            guid,
        });
        guids.push(guid);
    }
    guids
}

/// Write a creature's virtual-item slots (Mr. Smite: sword+board → two-hander → dual maces).
/// Authored values are item entries; the durable row carries the corresponding display ids expected
/// by `UNIT_VIRTUAL_ITEM_SLOT_DISPLAY`. Zero unequips a slot. Upserts; errs for a missing creature
/// or item template so a relay never reports success without an observable projection.
pub fn equip_swap(
    ctx: &ReducerContext,
    creature_guid: u64,
    main_hand: u32,
    off_hand: u32,
    ranged: u32,
) -> Result<(), String> {
    let e = crate::helpers::live_entity(ctx, creature_guid)
        .map_err(|_| format!("no live entity {creature_guid}"))?;
    if e.is_player() {
        return Err("equip_swap targets creatures only".to_string());
    }
    let display = |entry: u32| -> Result<u32, String> {
        if entry == 0 {
            return Ok(0);
        }
        ctx.db
            .game_item_template()
            .entry()
            .find(entry)
            .map(|item| item.display_id)
            .ok_or_else(|| format!("virtual equipment item {entry} is missing"))
    };
    let (main_hand, off_hand, ranged) = (display(main_hand)?, display(off_hand)?, display(ranged)?);
    let t = ctx.db.game_encounter_equip();
    match t.creature_guid().find(creature_guid) {
        Some(mut row) => {
            row.instance_id = e.instance_id;
            row.main_hand = main_hand;
            row.off_hand = off_hand;
            row.ranged = ranged;
            t.creature_guid().update(row);
        }
        None => {
            t.insert(EncounterEquip {
                creature_guid,
                instance_id: e.instance_id,
                main_hand,
                off_hand,
                ranged,
            });
        }
    }
    Ok(())
}

/// Send a creature on a single movement leg to `(x, y, z)` (Smite's run to the weapon rack): the
/// EXISTING creature move-event emission pattern (chase/pet-follow legs), used directly — emit one
/// `CreatureMoveEvent` (relayed as `SMSG_MONSTER_MOVE`) and snap the server-side position/grid to
/// the destination, exactly like a chase leg. Duration from the snare-aware
/// `combat::effective_move_speed` over the 2D distance. NOTE (future cleanup, work-item 181): when
/// the shared leg chokepoint lands, route this through it instead. Errs for a missing/player/dead
/// mover; a zero-length leg is a no-op Ok.
pub fn move_to_point(
    ctx: &ReducerContext,
    creature_guid: u64,
    x: f32,
    y: f32,
    z: f32,
    run: bool,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(creature_guid) else {
        return Err(format!("no live entity {creature_guid}"));
    };
    if e.is_player() {
        return Err("move_to_point targets creatures only".to_string());
    }
    if e.dead {
        return Err("mover is dead".to_string());
    }
    let base = if run {
        constants::speeds::RUN
    } else {
        constants::speeds::WALK
    };
    let speed = crate::combat::effective_move_speed(ctx, creature_guid, base);
    let (dx, dy) = (x - e.x, y - e.y);
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= f32::EPSILON || speed <= 0.0 {
        return Ok(()); // already there (or immobilized) — no zero-length leg
    }
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    // STRICTLY-INCREASING spline id (227 review HIGH): the codec contract says a non-increasing
    // spline_id per creature is IGNORED by the client (gateway/src/codec/movement.rs). Every
    // tick-pass emitter is once-per-creature-per-tick so `now_ms` suffices there, but a package
    // calling this twice in ONE transaction (same timestamp) would have its second leg silently
    // client-dropped while the server position moved — take max(now_ms, last+1) over the mover's
    // pending legs so back-to-back calls stay renderable.
    // The spline row is per-mover and updated in place, so the "two legs in one transaction" guard
    // reads the LIVE row's id rather than scanning an append-only event table.
    let spline_id = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(creature_guid)
        .map_or(now_ms, |last| now_ms.max(last.spline_id.wrapping_add(1)));
    // ONE relay path (perf 2.3) — see `creatures::tick::emit_move_spline`. This site kept writing
    // `game_creature_move_event` after the gateway stopped subscribing it, so scripted encounter
    // movement moved the server and nothing else.
    crate::creatures::tick::emit_move_spline(
        ctx,
        creature_guid,
        (e.x, e.y, e.z),
        (x, y, z),
        ((dist / speed) * 1000.0) as u32,
        run,
        spline_id,
        e.map_id,
        e.instance_id,
        (e.grid_x, e.grid_y),
    );
    let (gx, gy) = spatial::grid_cell(x, y);
    e.x = x;
    e.y = y;
    e.z = z;
    e.grid_x = gx;
    e.grid_y = gy;
    e.cell = spatial::grid_cell_id(gx, gy);
    e.last_move_ms = now_ms;
    entities.guid().update(e);
    Ok(())
}

/// Reset one encounter: state → `ENCOUNTER_NOT_STARTED` (payload zeroed — the `SetData` scratch is
/// per-attempt), and despawn this encounter's tracked wave spawns (entity + spawn row + legs +
/// corpse loot + equip + tracking row). Does NOT touch the HP fired-marks — those are keyed by
/// creature ENTRY, not encounter id, so the package's reset handler calls [`reset_hp_fired`] for
/// its boss entries alongside this (documented on that fn).
pub fn encounter_reset(ctx: &ReducerContext, instance_id: u64, encounter_id: u32) {
    upsert_state_row(ctx, instance_id, encounter_id, ENCOUNTER_NOT_STARTED, 0);
    let tracked: Vec<EncounterSpawn> = ctx
        .db
        .game_encounter_spawn()
        .by_instance()
        .filter(&instance_id)
        .filter(|s| s.encounter_id == encounter_id)
        .collect();
    despawn_tracked(ctx, &tracked);
}

/// [`encounter_reset`] + [`reset_hp_fired`] for the named boss entries in ONE call — the verb a
/// wipe handler almost always wants (review finding: a package that calls the bare reset but
/// forgets the ratchets gets a re-pulled boss whose 66/33 events never fire, with no error
/// anywhere; mangos' `Reset()` reconstructs phase state implicitly, so the trap is easy to walk
/// into). Prefer this from 227-style wipe/evade handlers; the split verbs remain for the rare
/// asymmetric reset.
pub fn encounter_reset_full(
    ctx: &ReducerContext,
    instance_id: u64,
    encounter_id: u32,
    boss_entries: &[u32],
) {
    encounter_reset(ctx, instance_id, encounter_id);
    for &entry in boss_entries {
        reset_hp_fired(ctx, instance_id, entry);
    }
}

/// Tear down tracked wave spawns: the canonical creature-despawn checklist
/// ([`crate::creatures::despawn_creature_entity`] — engagement/threat/taunt lock, spline + motion
/// rows, the whole loot family, then the live entity, whose delete relays `SMSG_DESTROY_OBJECT`;
/// it may already be decay-deleted, which is fine), plus this path's OWN extras: the spawn row (the
/// row that would otherwise respawn into instance 0 — see [`EncounterSpawn`]), any equip row, and the
/// tracking row itself.
///
/// Issue #359: this used to carry a private copy of the checklist, and that copy deleted every
/// `game_corpse_loot` row on the guid with NO `!withheld` filter — the #50 invariant, which the other
/// two teardown paths honour. Wave adds are ordinary group-killable creatures and `encounter_reset` is
/// the wipe handler, so a mid-roll wipe hit exactly this delete and silently ate the winner's item.
fn despawn_tracked(ctx: &ReducerContext, tracked: &[EncounterSpawn]) {
    for t in tracked {
        crate::creatures::despawn_creature_entity(ctx, t.guid);
        ctx.db.game_creature_spawn().guid().delete(t.guid);
        ctx.db.game_encounter_equip().creature_guid().delete(t.guid);
        ctx.db.game_encounter_spawn().id().delete(t.id);
    }
}

/// Sweep EVERY kernel row for a dying instance: tracked wave spawns (full teardown — this is what
/// keeps their untagged `game_creature_spawn` rows from ever respawning into instance 0), all
/// `game_encounter_state` rows (package encounters AND the reserved HP fired-marks), and all
/// equip rows. HP WATCHES are global template config and deliberately survive.
///
/// SPLICE POINT (190 slice 3, instance reap — mirrors 229's `debug_arm_instance_tick` splice
/// notes): the reap's population sweep calls `crate::encounter::sweep_encounter_state(ctx,
/// instance_id)` right next to its entity/corpse/GO-copy deletes, before the `game_instance` row
/// itself goes (`instance::teardown_instance_inner`). Also exercised directly by the debug-feature
/// lever `debug_sweep_encounter_state`.
pub(crate) fn sweep_encounter_state(ctx: &ReducerContext, instance_id: u64) {
    let tracked: Vec<EncounterSpawn> = ctx
        .db
        .game_encounter_spawn()
        .by_instance()
        .filter(&instance_id)
        .collect();
    despawn_tracked(ctx, &tracked);
    let states = ctx.db.game_encounter_state();
    // by_pair prefix scan (instance_id is the leading column) — not a full-table iter (review nit).
    let stale: Vec<u64> = states
        .by_pair()
        .filter(&instance_id)
        .map(|r| r.id)
        .collect();
    for id in stale {
        states.id().delete(id);
    }
    let equips = ctx.db.game_encounter_equip();
    let stale: Vec<u64> = equips
        .by_instance()
        .filter(&instance_id)
        .map(|r| r.creature_guid)
        .collect();
    for guid in stale {
        equips.creature_guid().delete(guid);
    }
}

// ===========================================================================================
//  Tests — every pure decision above
// ===========================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_has_exactly_one_installed_package_authority() {
        let mut from_enum: Vec<String> = EncounterBinding::ALL
            .iter()
            .map(|binding| format!("{binding:?}"))
            .collect();
        from_enum.sort();
        assert_eq!(crate::GAME_ENCOUNTER_PACKAGE_BINDING_NAMES, from_enum);
        assert_eq!(
            crate::GAME_ENCOUNTER_PACKAGE_BINDING_NAMES,
            [
                "BlackfathomDeepsKelris",
                "BlackrockDepthsTombOfSeven",
                "DireMaulAlzzin",
                "RazorfenKraulWardKeepers",
                "ShadowfangKeepFenrus",
                "ShadowfangKeepNandos",
                "ShadowfangKeepRethilgore",
                "SunkenTempleAvatar",
                "WailingCavernsAnacondra",
                "WailingCavernsCobrahn",
                "WailingCavernsMutanus",
                "WailingCavernsPythas",
                "WailingCavernsSerpentis",
                "ZulGurubOhgan",
            ]
        );
    }

    #[test]
    fn package_lookup_refuses_missing_and_duplicate_authority() {
        fn handler(
            _ctx: &ReducerContext,
            _instance_id: u64,
            _signal: EncounterSignal,
        ) -> Result<(), String> {
            Ok(())
        }

        let binding = EncounterBinding::BlackfathomDeepsKelris;
        let missing = package_handler_for(binding, EncounterSignal::Complete, &[])
            .err()
            .expect("missing authority must refuse");
        assert!(missing.contains("no installed package authority"));

        let duplicate = [(binding, handler as EncounterPackageHandler); 2];
        let duplicate = package_handler_for(binding, EncounterSignal::Complete, &duplicate)
            .err()
            .expect("duplicate authority must refuse");
        assert!(duplicate.contains("more than one installed package authority"));
    }

    #[test]
    fn crossing_fires_each_watch_once_high_to_low_and_never_refires_after_heal_redrop() {
        // Smite's 66/33 pattern. First hit to 60%: 66 fires, 33 doesn't.
        let watches = [66u8, 33u8];
        assert_eq!(crossed_thresholds(&watches, HP_FIRED_NONE, 60), vec![66]);
        // Ratchet now 66. Heal to 90%, re-drop to 60%: NOTHING re-fires (once per instance).
        assert_eq!(crossed_thresholds(&watches, 66, 60), Vec::<u8>::new());
        // Drop on to 30%: only 33 fires.
        assert_eq!(crossed_thresholds(&watches, 66, 30), vec![33]);
        // Ratchet 33: nothing left.
        assert_eq!(crossed_thresholds(&watches, 33, 5), Vec::<u8>::new());
    }

    #[test]
    fn one_giant_hit_crossing_several_watches_fires_all_desc_and_dedups_duplicates() {
        // 100% → 10% in one hit, with a duplicate 66 row registered twice.
        let watches = [33u8, 66u8, 66u8, 90u8];
        assert_eq!(
            crossed_thresholds(&watches, HP_FIRED_NONE, 10),
            vec![90, 66, 33]
        );
    }

    #[test]
    fn crossing_boundary_is_at_or_below_and_higher_watch_never_fires_after_lower_ratchet() {
        let watches = [66u8];
        // Exactly 66% counts as crossed (mangos HealthBelowPct-with-equal semantics).
        assert_eq!(crossed_thresholds(&watches, HP_FIRED_NONE, 66), vec![66]);
        // 67% does not.
        assert_eq!(
            crossed_thresholds(&watches, HP_FIRED_NONE, 67),
            Vec::<u8>::new()
        );
        // A 66 watch with the ratchet already at 33 (fired during a burst) can never fire late.
        assert_eq!(crossed_thresholds(&[66u8, 33u8], 33, 20), Vec::<u8>::new());
    }

    #[test]
    fn hp_pct_floors_uses_u64_math_and_treats_zero_max_as_full() {
        assert_eq!(hp_pct(664, 1000), 66); // floor, not round
        assert_eq!(hp_pct(0, 1000), 0);
        assert_eq!(hp_pct(1000, 1000), 100);
        assert_eq!(hp_pct(u32::MAX, u32::MAX), 100); // health*100 would wrap u32
        assert_eq!(hp_pct(5, 0), 100); // malformed row → nothing fires
    }

    #[test]
    fn reserved_hp_key_namespace_is_disjoint_from_package_encounter_ids() {
        // Every vanilla creature entry (< 2^24) maps into the reserved namespace...
        assert!(is_reserved_encounter_id(hp_fired_key(1)));
        assert!(is_reserved_encounter_id(hp_fired_key(646))); // Mr. Smite
        assert_eq!(hp_fired_key(646), 0x8000_0000 | 646);
        // ...and no sane package encounter id (below the bit) is ever reserved.
        assert!(!is_reserved_encounter_id(0));
        assert!(!is_reserved_encounter_id(1));
        assert!(!is_reserved_encounter_id(0x7FFF_FFFF));
        // Distinct entries map to distinct keys (bit-or, no folding).
        assert_ne!(hp_fired_key(639), hp_fired_key(646));
    }

    #[test]
    fn state_constants_are_the_mangos_four_and_hp_fired_none_sits_above_every_pct() {
        assert_eq!(ENCOUNTER_NOT_STARTED, 0);
        assert_eq!(ENCOUNTER_IN_PROGRESS, 1);
        assert_eq!(ENCOUNTER_DONE, 2);
        assert_eq!(ENCOUNTER_FAILED, 3);
        // The virgin sentinel strictly exceeds any pct a watch may hold (1..=99) AND 100,
        // so `w < HP_FIRED_NONE` is true for every legal watch — the first crossing always fires.
        const { assert!(HP_FIRED_NONE > 100) };
    }

    #[test]
    fn named_source_states_translate_to_the_internal_encounter_contract() {
        assert_eq!(encounter_state_for_signal(EncounterSignal::Fail), Some(3));
        assert_eq!(
            encounter_state_for_signal(EncounterSignal::Complete),
            Some(2)
        );
    }

    #[test]
    fn wave_guid_carries_highguid_unit_entry_bits_and_a_bounded_low() {
        let g = wave_guid(636, 7); // a Defias Miner add, low 7
        assert_eq!(
            g >> 48,
            0xF130,
            "HIGHGUID_UNIT so the 5875 client classifies it as a Unit"
        );
        assert_eq!((g >> 24) & 0xFF_FFFF, 636);
        assert_eq!(g & 0xFF_FFFF, 7);
        // A low that would overflow 24 bits is masked, never corrupting the entry bits.
        assert_eq!((wave_guid(636, 0x0100_0000 + 5) >> 24) & 0xFF_FFFF, 636);
        assert_eq!(wave_guid(636, 0x0100_0000 + 5) & 0xFF_FFFF, 5);
    }

    #[test]
    fn wave_offsets_spread_adds_on_a_three_wide_two_yard_grid() {
        assert_eq!(wave_offset(0), (-2.0, 0.0));
        assert_eq!(wave_offset(1), (0.0, 0.0));
        assert_eq!(wave_offset(2), (2.0, 0.0));
        assert_eq!(wave_offset(3), (-2.0, 2.0)); // second row
                                                 // No two of the first 9 collide.
        let pts: Vec<(i32, i32)> = (0..9)
            .map(|i| {
                let (x, y) = wave_offset(i);
                (x as i32, y as i32)
            })
            .collect();
        let mut dedup = pts.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), pts.len());
    }
}
