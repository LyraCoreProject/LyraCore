//! Spell/aura/cooldown table structs (the [static] definition tables + the [event]/[entity]/[server]
//! runtime tables). The `#[table]` macro generates the per-table accessor traits here; `mod.rs`
//! re-exports them (`pub use tables::*`) so every `crate::spell::<Table>` / `ctx.db.game_<table>()`
//! path resolves.

use spacetimedb::{
    table, Identity, ReducerContext, ScheduleAt, Table, Timestamp,
};

// The `scheduled(..)` table macros below reference the reducer callbacks by name; those reducers live in
// `scheduler.rs` and are re-exported by `mod.rs`, so pull them into scope for the macro to resolve.
use super::{fire_pending_cast, fire_spell_impact, tick_auras, tick_ground_areas};
use crate::WorldEntity;

// ===========================================================================================
//  Spell definition tables [static] — hand-authored OR importer-filled from Spell.dbc. Public
//  (reference data the client/gateway read). No Timestamp → SQL/seed-loadable.
// ===========================================================================================

/// A castable spell's **header** — the whole-spell scalars (mapped ~1:1 from `SpellEntry`). The
/// per-effect behavior lives in `game_spell_effect`; this row carries only what's shared by every
/// effect of the spell (duration, school, dispel category, GCD/cost/cast-time, stacking bound). [static]
#[table(accessor = game_spell, public)]
pub struct Spell {
    #[primary_key]
    pub spell_id: u32,
    pub name: String,
    pub power_type: u8, // which pool `cost` drains: 0=mana 1=rage 3=energy 255=health
    pub cost: u32,
    pub cast_time_ms: u32,
    pub gcd_ms: u32,
    pub cooldown_ms: u32, // per-spell cooldown, distinct from the GCD (post-spike)
    pub range_yd: u32,
    pub duration_ms: u32, // aura lifetime (DBC DurationIndex is per-spell, shared by all aura effects)
    pub school_mask: u8,  // 1=phys 2=holy 4=fire 8=nature 16=frost 32=shadow 64=arcane
    pub dispel_type: u8,  // what dispels THIS spell: 0 none/1 magic/2 curse/3 disease/4 poison
    pub mechanic: u8,     // whole-spell mechanic (immunity matching)
    pub max_stacks: u8,   // StackAmount; 0/1 = non-stacking
    pub aura_interrupt: u16, // bit0=break_on_damage, bit1=break_on_move (the only bits we honor now)
    pub attributes: u32,     // folded subset of Spell.dbc Attributes we branch on
    pub spell_level: u8,     // scaling floor (per_level scales from here)
    pub max_level: u8,       // scaling cap (0 = no cap)
    pub is_negative: bool,   // resolved buff/debuff polarity (drives dispel + UI frame)
    // OUR OWN cast-gate flags (SPELL_ATTR_REQ_BEHIND/REQ_STEALTH/STEALTH_SAFE) — a DEDICATED bitmask, NOT
    // folded into `attributes` (the raw vanilla Spell.dbc Attributes bits are densely used and would
    // collide). END-appended + `#[default(0)]` so existing rows auto-migrate to 0 (safe column-add, per the
    // publish-migration rule). The importer sets these BY NAME; the gates in resolve_cast_at read them.
    #[default(0)]
    pub cast_flags: u32,
    // Spell.dbc `Stances` (ShapeshiftMask) bitmask — which Warrior stances this ability is usable in, as
    // OUR 0-based stance bits (bit `1 << stance`: bit0=Battle, bit1=Defensive, bit2=Berserker). The importer
    // translates the vanilla form-bit mask (`1 << (formId-1)`, forms 17/18/19) onto these. 0 = NO stance
    // requirement (usable in ANY stance — every non-stance-gated spell, every non-warrior spell), so the
    // gate is a no-op for all existing rows (baseline-safe). The cast gate in resolve_cast_at (`stance_allows`)
    // rejects an ability whose mask != 0 and excludes the caster's current `stance`. END-appended +
    // `#[default(0)]` → publish auto-migrates existing rows (the migration rule). Keyed on the bit, never a
    // spell id.
    #[default(0)]
    pub stances: u8,
    /// SpellFamilyName (Spell.dbc `spell_class_set` — MAGE=3, WARRIOR=4, …; 0 = generic). With
    /// `family_flags` this is how a spell MODIFIER names its affected spells (264): a modifier aura
    /// applies to a cast iff family_name matches AND `family_flags & modifier_mask != 0`.
    /// END-appended + `#[default(0)]` → auto-migrates. [data]
    #[default(0)]
    pub family_name: u8,
    /// SpellFamilyFlags (Spell.dbc `spell_class_mask`, the low 32 bits carry every vanilla family
    /// bit — stored u64 for headroom). END-appended + `#[default(0u64)]` → auto-migrates. [data]
    #[default(0u64)]
    pub family_flags: u64,
    /// Spell.dbc `procFlags` — the vanilla combat-event mask a Proc on this spell fires off, stored
    /// VERBATIM (like `family_flags`), so the engine names only the bits it fires and there is no
    /// translation table to keep in lockstep. See `spell::proc`'s `PROC_FLAG_*`. 0 = never procs.
    /// END-appended + `#[default(0u32)]` → auto-migrates. [data]
    #[default(0u32)]
    pub proc_flags: u32,
    /// Spell.dbc `procChance` — the flat percent a Proc on this spell fires at, once its event and
    /// filters match. 100 (or above) always fires. Frozen onto the aura row at apply. [data]
    #[default(0u8)]
    pub proc_chance: u8,
    /// Spell.dbc `procCharges` — how many times a Proc on this spell may fire before the whole buff
    /// comes off. 0 = unlimited (Frost Armor); 3 = Lightning Shield. [data]
    #[default(0u8)]
    pub proc_charges: u8,
}

/// One **effect** of a spell (1..3 per spell, ordered by `effect_index`). The `kind` (§ KINDS) is the
/// single dispatch tag; the magnitude is `base_points (+die_sides random) + per_level·(level-spell_level)`;
/// `p0`/`p1` are the **union-killed** typed params whose meaning is fixed by `kind` and asserted via
/// `p0_kind` (the importer resolves `EffectMiscValue` into these ONCE, so the runtime never asks "what
/// does this int mean"). `id` is DETERMINISTIC `(spell_id<<2)|effect_index` (the seeder computes it, NOT
/// auto_inc) so seed/SQL rows are stable and `game_aura.effect_id` is reproducible. [static]
#[table(accessor = game_spell_effect, public, index(accessor = by_spell, btree(columns = [spell_id])))]
pub struct SpellEffect {
    #[primary_key]
    pub id: u64, // = (spell_id<<2)|effect_index, computed by the author/importer
    pub spell_id: u32,
    pub effect_index: u8,
    pub kind: u8, // KIND_* discriminator (high bit KIND_AURA_BIT => persistent aura)
    pub base_points: i32, // magnitude base (DBC +1 already applied at import)
    pub die_sides: i32, // random 0..die_sides added (0 = deterministic)
    pub per_level: f32, // × (clamped level - spell_level)
    pub period_ms: u32, // periodic tick interval (0 = non-periodic)
    pub target: u8, // TargetKind (our small taxonomy)
    pub radius_yd: f32, // 0 = single-target
    pub chain_targets: u8, // 0/1 = single
    pub trigger_spell: u32, // for E_TRIGGER (0 = none)
    pub effect_mechanic: u8, // per-effect mechanic override (0 = inherit header)
    pub p0: i32,  // typed param A (meaning fixed by kind / asserted by p0_kind)
    pub p0_kind: u8, // P_* tag: what p0 means (import-correctness tripwire + self-doc)
    pub p1: i32,  // typed param B
    pub script_id: u32, // 0 = pure data; >0 = a registered Rust fn (kind = E_SCRIPTED)
    /// Set true so this effect ENTERS the caster into combat when it fires — the E_ENERGIZE /
    /// A_PERIODIC_ENERGIZE arms read it instead of an id check, so the granted resource isn't
    /// out-of-combat-decayed. Bloodrage's grant + trickle opt in via data; ANY energize can. END-appended
    /// `#[default(false)]` → additive migration.
    #[default(false)]
    pub enters_combat: bool,
}

/// One REAGENT a spell consumes (work-item 282: the real multi-reagent recipe model that replaced
/// the hardcoded `cast::RECIPES` map). Filled by the importer from `Spell.dbc` `Reagent[1-8]` /
/// `ReagentCount[1-8]` during the wholesale spell import — every real recipe (and any reagent-
/// consuming spell) carries its true mats here, keyed by the REAL vanilla spell id, so the craft
/// gate resolves by data instead of a hardcoded id list. Module-private (the craft reducer reads
/// it directly; the client's own `Spell.dbc` feeds the tradeskill UI, the gateway never needs it).
/// `id` is DETERMINISTIC `(spell_id<<3)|slot` (slot 0..7) so the importer's clear+reload is
/// idempotent, mirroring `SpellEffect`. [static]
#[table(accessor = game_spell_reagent, index(accessor = by_spell, btree(columns = [spell_id])))]
pub struct SpellReagent {
    #[primary_key]
    pub id: u64, // = (spell_id<<3)|reagent_slot
    pub spell_id: u32,
    pub item_entry: u32,
    pub count: u32,
}

/// The classic-db `spell_proc_event` **overlay** for one spell: everything a Proc needs that Spell.dbc
/// has no column for. Vanilla's spell data carries a proc's event mask, flat chance and charge count
/// and nothing else — no procs-per-minute rate, no internal cooldown, no hit-quality rule and no
/// filter on which spells may trigger it. Those live here, keyed by spell id, and an ABSENT row means
/// "the header's values, and neutral zeros for the rest".
///
/// Read once, at apply: `spell::proc::freeze_profile` folds this row and the header into the profile
/// the aura freezes, so the proc pass never re-joins either. Module-private — the client has its own
/// Spell.dbc and the gateway has no use for proc policy, so there is no binding to hand-sync. No
/// `Timestamp`, so the importer loads it as plain SQL. [static]
#[table(accessor = game_spell_proc_event)]
pub struct SpellProcEvent {
    #[primary_key]
    pub spell_id: u32,
    /// `procFlags` OVERRIDE — replaces the header's mask when non-zero. 0 = use the header's.
    pub proc_flags: u32,
    /// `procEx`, verbatim. The engine reads only the normal-hit / critical-hit bits.
    pub proc_ex: u32,
    /// School filter on the triggering spell; 0 = any school.
    pub school_mask: u8,
    /// Spell-family filter (name half); 0 = any family.
    pub family_name: u8,
    /// Spell-family filter (flags half); paired with `family_name`.
    pub family_flags: u64,
    /// Procs-per-minute rate; replaces the chance for a Carrier that dealt the hit. 0 = flat chance.
    pub ppm_rate: f32,
    /// `CustomChance` — the flat percent, replacing the header's `procChance` when non-zero.
    pub custom_chance: u8,
    /// Internal cooldown; the Proc fires at most once per window. 0 = none.
    pub icd_ms: u32,
}

// ===========================================================================================
//  Aura + cast-event + schedule tables [event/entity]
// ===========================================================================================

/// An active aura on a unit. Public (buffs are visible unit state); the gateway relays apply/clear as a
/// partial `UNIT_FIELD_AURA` VALUES update, reading `slot`/`spell_id`/`level`/`flags`. The original
/// columns (id..expires_at) are UNCHANGED so that relay keeps working; the appended `#[default(0)]`
/// columns are the **frozen typed snapshot** (computed once at apply) so periodic ticks + combat
/// stat-reads are self-contained — NO template/effect re-join on the hot path. [event]
///
/// `by_expiry` (work-item 232) is a plain INDEX ADD over the already-existing `expires_at` column — no
/// new column, no default, no data migration; a schema/metadata-only change on this gateway-SUBSCRIBED
/// table (verified by `gateway/tests/schema_parity.rs`, which checks columns/bindings, not indexes).
/// `scheduler::tick_auras`'s expiry pass range-scans `by_expiry().filter(..=now)` instead of `.iter()`ing
/// every aura row to find the handful past their deadline.
#[table(
    accessor = game_aura,
    public,
    index(accessor = by_target, btree(columns = [target_guid])),
    index(accessor = by_expiry, btree(columns = [expires_at])),
    // Creature fear movement runs every firing. This exact pair finds its sparse candidates without
    // reading unrelated buffs, passive effects, periodic effects, or Procs.
    index(accessor = by_kind_param, btree(columns = [eff_kind, eff_p0])),
    // Perf catalog 1.9: the same recipe as `by_expiry`, for the PERIODIC pass. `next_tick_micros == 0`
    // is the non-periodic sentinel, so a `1..=now` range skips every buff/passive for free.
    index(accessor = by_next_tick, btree(columns = [next_tick_micros]))
)]
pub struct Aura {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target_guid: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub slot: u8,
    pub level: u8,
    pub flags: u8,
    pub applied_at: Timestamp,
    pub expires_at: Timestamp,
    // --- frozen typed snapshot (END-appended, #[default(0)] → additive migration) ---
    #[default(0u64)]
    pub effect_id: u64, // FK → game_spell_effect (the SPECIFIC effect; distinguishes a multi-aura spell's auras)
    #[default(0)]
    pub eff_kind: u8, // copy of effect.kind — the dispatch + combat-filter key
    #[default(0)]
    pub amount: i32, // resolved magnitude at apply (POSITIVE; the kind decides direction)
    #[default(0)]
    pub eff_p0: i32, // frozen typed param A (stat/school/combat-field/mechanic)
    #[default(0)]
    pub eff_p0_kind: u8,
    #[default(0)]
    pub eff_p1: i32,
    #[default(0)]
    pub period_ms: u32, // 0 = non-periodic
    #[default(0)]
    pub amount_remaining: i32, // A_ABSORB pool left (post-spike; 0 otherwise)
    #[default(0)]
    pub stacks: u8, // 1..max_stacks (combat reads amount*stacks)
    // Per-aura periodic cadence (micros-since-epoch). i64 (not Timestamp) so it's `#[default(0)]`-able
    // for the additive migration; 0 = no pending tick. Advanced by `period_ms` each tick so a 3 s DoT
    // and a 1 s HoT tick independently, each on its own cadence.
    #[default(0i64)]
    pub next_tick_micros: i64,
    // The CHANNEL target (the enemy guid each per-tick missile flies at). Only meaningful for an
    // `A_PERIODIC_TRIGGER` channel aura — the aura's own `target_guid` is the CASTER (a T_SELF self-channel),
    // so the missile destination must ride here separately (a u64 guid can't fit the i32 `eff_p*` slots). The
    // per-tick trigger spell id rides in `eff_p1`. 0 for every non-channel aura. END-appended + `#[default(0u64)]`
    // → additive auto-migration (per the publish-migration rule).
    #[default(0u64)]
    pub channel_target: u64,
    /// Frozen from the source effect's `enters_combat` at apply, so the periodic-energize tick reads it
    /// without a spell-id check (Bloodrage's over-time trickle holds the caster in combat).
    #[default(false)]
    pub enters_combat: bool,
    // --- the frozen PROC PROFILE (A_PROC_TRIGGER / A_PROC_DAMAGE rows; zeros on every other kind) ---
    // Frozen at apply the same way kind/amount/params are, so the proc pass reads one row and never
    // re-joins the header or the overlay. `proc_charges` and `proc_ready_micros` are the two MUTABLE
    // fields: a fire spends a charge and stamps the internal cooldown. END-appended + typed defaults.
    /// The Carrier's frozen combat-event mask (vanilla `procFlags`, verbatim). 0 = this row never procs.
    #[default(0u32)]
    pub proc_flags: u32,
    /// Frozen flat percent. Used unless `proc_ppm` applies (dealer-side procs-per-minute).
    #[default(0u8)]
    pub proc_chance: u8,
    /// Frozen procs-per-minute rate. Non-zero only from the `spell_proc_event` overlay; the chance is
    /// derived from the Carrier's attack time at proc time, so it cannot be folded into `proc_chance`.
    #[default(0.0f32)]
    pub proc_ppm: f32,
    /// Frozen vanilla `procEx` mask, verbatim — the engine reads only the normal-hit / critical-hit bits.
    #[default(0u32)]
    pub proc_ex: u32,
    /// Frozen school filter: when non-zero, a triggering SPELL must share a school bit.
    #[default(0u8)]
    pub proc_school_mask: u8,
    /// Frozen spell-family filter (name half); 0 = no family filter.
    #[default(0u8)]
    pub proc_family_name: u8,
    /// Frozen spell-family filter (flags half); paired with `proc_family_name`.
    #[default(0u64)]
    pub proc_family_flags: u64,
    /// Charges LEFT. 0 = unlimited. A fire on the last charge removes every aura row of the spell.
    #[default(0u8)]
    pub proc_charges: u8,
    /// Frozen internal-cooldown length. 0 = no cooldown (fire as often as the roll allows).
    #[default(0u32)]
    pub proc_icd_ms: u32,
    /// Micros-since-epoch this Proc is ready again. 0 = ready now; stamped `now + proc_icd_ms` on a fire.
    #[default(0i64)]
    pub proc_ready_micros: i64,
}

// UNIT-keyed character-owned sweep (the warm-handoff hot-state audit). `Aura`'s columns
// name ANY unit (`target_guid`/`caster_guid` — creatures have auras too), never `character_guid`/
// `player_guid`/`owner_guid`, so `tripwires.rs`'s `character_owned_tripwire` never flags this table and no
// marker was mandatory. That silence is exactly why the TRANSFER half went missing while the DELETE
// half got hand-rolled straight into `world::cascade_delete_character` (see that fn's own comment,
// "auras ON the character... the character_owned tripwire exempts the tables"): a warm handoff carried
// every manifest table but dropped a mid-fight buff, a DoT, a HoT, or a Rogue's Stealth on the floor —
// silently, because nothing here ever claimed otherwise. Scoped to `target_guid == character_guid`,
// the exact predicate `cascade_delete_character`'s block already uses; a mob's own aura, or a debuff
// the character CAST on someone else, is untouched either way (`caster_guid` never gates this sweep).
crate::character_owned!(delete, fn sweep_delete_game_aura(ctx, character_guid) {
    let auras = ctx.db.game_aura();
    let ids: Vec<u64> = auras.by_target().filter(&character_guid).map(|a| a.id).collect();
    for id in ids {
        auras.id().delete(id);
    }
});
// CROSS-DATABASE transport: `id` is `#[auto_inc]` — meaningless, and possibly
// COLLIDING, on the destination — reset to 0 so the insert mints a fresh one, mirroring
// `sweep_transfer_game_character_talent`'s identical re-mint of its own surrogate PK. Every OTHER
// column, including `applied_at`/`expires_at`, rides unchanged: both are ABSOLUTE `Timestamp`s (a
// point in wall-clock time), never a duration, so a 10-minute buff that has 4 left resumes with those
// same 4 minutes on the destination — the receiving side just compares the same deadline against its
// own clock. `caster_guid`/`channel_target` may now name a guid that does not exist on this database
// (the caster stayed behind); nothing on the read path panics on that — it is the same "unit vanished"
// case the periodic/expiry ticks already tolerate for a caster who simply logged out.
crate::character_owned!(transfer, fn sweep_transfer_game_aura(ctx, character_guid, io) {
    table = game_aura,
    by = by_target,
    remint = id,
});

/// `SpellCastEvent::failure_reason` — our own small taxonomy of WHY a cast failed, carried on an
/// `is_interrupted` row so the gateway can name the reason to the client. The gateway maps these onto
/// the vanilla `CastFailureReason` byte; the module holds no wire values. `NONE` means "no concrete
/// reason", which is the plain cast-bar teardown every existing interruption already sends.
pub const CAST_FAIL_NONE: u8 = 0;
/// The caster could not pay the cost. The gateway reports it as `SPELL_FAILED_NO_POWER`
/// ("Not enough rage"), which is what releases the client's lit on-next-swing button.
pub const CAST_FAIL_NO_POWER: u8 = 1;

/// A spell cast (the visual). Separate from the aura row so a re-cast always replays the cast
/// animation/SFX (`SMSG_SPELL_GO`) even when the aura only refreshes its timer. Reaped by the event GC.
#[table(
    accessor = game_spell_cast_event,
    public,
    // perf catalog 2.3: AOI-box scoping instead of a global `SELECT *`.
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct SpellCastEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub created_at: Timestamp,
    // The spell's primary target — so the gateway aims SMSG_SPELL_GO at it (the missile flies at the
    // mob) instead of self. END-appended + defaulted → auto-migrates.
    #[default(0u64)]
    pub target_guid: u64,
    // >0 on the cast-START event (a timed spell), so the gateway relays SMSG_SPELL_START with this as the
    // cast-bar duration; 0 on the cast-GO event (instant or completion). END-appended + defaulted.
    #[default(0u32)]
    pub cast_time_ms: u32,
    // true on a TIMED-cast COMPLETION (the GO event from resolve_cast_at via fire_pending_cast); false on a
    // begin-START, a genuine instant cast, a channel/creature cast. CURRENTLY UNUSED on the relay path: the
    // gateway emits the SAME START(0)+GO+COOLDOWN close sequence for a completion and a genuine instant (the
    // 5875 client closes its cast state on a START→GO pair, like the instant-cast + channel-tick relays).
    // Kept (additive, harmless) for a future relay that wants to distinguish them. END-appended +
    // #[default(false)] → auto-migrates.
    #[default(false)]
    pub is_completion: bool,
    // Total post-mitigation spell damage dealt by this cast (summed across E_DAMAGE/E_WEAPON_STRIKE
    // effects on the primary target). 0 = no damage log (begin-START, heals, buffs). Set on the cast-GO
    // event so the gateway relays SMSG_SPELLNONMELEEDAMAGELOG (the floating damage number). END-appended +
    // #[default(0u32)] → auto-migrates.
    #[default(0u32)]
    pub damage: u32,
    // SpellSchool INDEX (0=phys/normal..6=arcane) for the damage log — derived from the spell header's
    // school_mask BITMASK via trailing_zeros so the gateway stays dumb (no mask math). Only meaningful when
    // damage>0. END-appended + #[default(0u8)] → auto-migrates.
    #[default(0u8)]
    pub school: u8,
    // True iff this cast's E_DAMAGE roll CRIT (×1.5). Drives the damage log's hit_info=CriticalHit so the
    // floating number renders as a crit (yellow, "Critical"). Only meaningful when damage>0. END-appended +
    // #[default(false)] → auto-migrates (additive column-add, per the publish-migration rule).
    #[default(false)]
    pub is_crit: bool,
    // Magic damage RESISTED off this cast's hit (sum across damage effects on the primary target). Drives the
    // damage log's `resisted` field ("(N resisted)"). 0 for a physical/unresisted hit. END-appended + defaulted.
    #[default(0u32)]
    pub resisted: u32,
    // Damage ABSORBED by the target's A_ABSORB shields before the health write (sum across damage effects on the
    // primary target). Drives the damage log's `absorbed_damage` field ("(N absorbed)"). END-appended + defaulted.
    #[default(0u32)]
    pub absorbed: u32,
    // True on an INTERRUPT signal row (the victim's mid-cast timed spell was cancelled by direct damage,
    // or an on-next-swing strike could not pay its cost at the swing). The gateway relays
    // SMSG_SPELL_FAILURE{spell, result=Interrupted} to the caster, plus the failed cast result when
    // `failure_reason` names one. Such a row carries no cast-START/GO/COOLDOWN sequence. END-appended
    // + #[default(false)] → additive auto-migration (the publish-migration rule).
    #[default(false)]
    pub is_interrupted: bool,
    // The spell's per-spell cooldown (ms) FROZEN onto the cast-GO row, so the gateway sends
    // SMSG_SPELL_COOLDOWN ONLY for a spell that actually HAS a cooldown (Mortal Strike 6s, Judgement 10s) —
    // and with the REAL value. A 0-cooldown spell (Demon Skin, Shadow Bolt, most 1-10 casts) gets NO cooldown
    // packet: mangos doesn't send one per cast, and spamming SMSG_SPELL_COOLDOWN(0) after EVERY cast left the
    // client's action button stuck ("yellow casting outline" + "another action is in progress" — can only
    // cast each spell once). END-appended + #[default(0u32)] → additive auto-migration.
    #[default(0u32)]
    pub cooldown_ms: u32,
    // PUSHBACK signal (work-item 039): >0 on a pushback row (direct damage slid the caster's in-progress
    // TIMED cast's fire time by this many ms) — the gateway relays SMSG_SPELL_DELAYED{guid, delay_time} so
    // the client's cast bar visibly shifts. This is the ONLY field set on such a row besides
    // caster_guid/spell_id (cast_time_ms/is_completion stay 0/false — it is neither a START nor a GO).
    // `game_spell_cast_event` is gateway-subscribed (per-player, `stdb/subscriptions.rs`), so this
    // END-appended column needs the binding hand-synced (`gateway/src/stdb/bindings/spell_cast_event_type.rs`)
    // — a hand-maintained binding not mirrored on a schema change breaks live row decode silently.
    // END-appended + #[default(0u32)] → additive auto-migration (the publish-migration rule).
    #[default(0u32)]
    pub delay_ms: u32,
    // EFFECTIVE health restored to the primary target by this cast (overheal excluded; summed
    // across E_HEAL effects). >0 on the cast-GO row → the gateway relays SMSG_SPELLHEALLOG (the
    // green floating number + combat-log line, work-item 251). Binding hand-synced (see the
    // delay_ms note above). END-appended + #[default(0u32)] → additive auto-migration.
    #[default(0u32)]
    pub healed: u32,
    // PROC-LOG signal (114): true on a swing-proc damage line (Seal of Righteousness holy riding a
    // landed melee swing). The gateway sends ONLY SMSG_SPELLNONMELEEDAMAGELOG — no START/GO/cooldown
    // (nothing "casts"; the seal aura is already up). Distinct from an on-next-swing FIRE (Heroic
    // Strike), which rides the normal is_completion=true relay (CAST_RESULT(OK)+GO alone) because the
    // client holds a pending cast for it. Binding hand-synced (see the delay_ms note above).
    // END-appended + #[default(false)] -> additive auto-migration.
    #[default(false)]
    pub is_proc_log: bool,
    // The melee swing outcome an on-next-swing FIRE row rode (114): CombatEvent hit_info codes
    // (0 normal, 1 crit, 2 miss, 3 dodge, 4 parry, ...). The relay shapes the SMSG_SPELL_GO miss
    // list from it when damage == 0 — the client then prints the yellow "Your Heroic Strike
    // missed/was dodged/was parried" line instead of a white MISS. 0 (normal) on every other row.
    // Binding hand-synced. END-appended + #[default(0u8)] -> additive auto-migration.
    #[default(0u8)]
    pub swing_hit_info: u8,
    // True ONLY for a CLIENT-INITIATED instant/channel cast (the CMSG_CAST_SPELL reducer path,
    // 088): the gateway already sent that caster START/RESULT/GO SYNCHRONOUSLY, so the relay must
    // suppress the duplicate to the caster. Every OTHER instant row (channel-tick missile, on-hit
    // trigger, creature cast, item-use) got NO synchronous send — the relay must deliver the
    // caster's visual (the old !is_completion&&self gate over-suppressed those: Arcane Missiles'
    // per-tick missile was invisible to its own caster). Binding hand-synced (delay_ms note).
    // END-appended + #[default(false)] -> additive auto-migration.
    #[default(false)]
    pub client_initiated: bool,
    // --- AOI columns (perf catalog 2.3), END-appended + TYPED defaults (a bare `0` on a u64
    // encodes as 4 bytes and fails the publish). Stamped from the actor via `helpers::grid_of`;
    // (0,0,0,0) means "no live actor", which matches no box and is correctly never delivered.
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
    // Why this cast failed (`CAST_FAIL_*`), on an `is_interrupted` row. A deferred on-next-swing
    // strike that cannot pay its cost at the swing carries `CAST_FAIL_NO_POWER`, so the gateway
    // follows the teardown with a failed SMSG_CAST_RESULT naming the queued spell — without it the
    // 1.12 client keeps the ability latched as its current melee spell. `CAST_FAIL_NONE` on every
    // other row keeps the plain teardown. Binding hand-synced (see the delay_ms note above).
    // END-appended + #[default(0u8)] -> additive auto-migration.
    #[default(0u8)]
    pub failure_reason: u8,
}

impl SpellCastEvent {
    /// A baseline `SpellCastEvent` row for `caster_guid`/`spell_id`: `id`=0, `created_at`=`ctx.timestamp`,
    /// and the AOI address (`map_id`/`instance_id`/`grid_x`/`grid_y`) stamped from ONE
    /// [`crate::helpers::grid_of`] lookup — every other field at its neutral zero/false. Replaces the
    /// ~20-field literal + 4× `grid_of` copy-paste this used to require at every call site (perf catalog
    /// audit, 2026-08-06): a call site now overrides only the 2-4 fields that carry real signal, via
    /// struct-update syntax, e.g.
    /// `SpellCastEvent { is_interrupted: true, ..SpellCastEvent::signal(ctx, caster_guid, spell_id) }`.
    ///
    /// Use [`Self::signal_at`] instead when the caster's live [`WorldEntity`] is already in hand — it
    /// skips this lookup entirely (a landed swing carrying a seal proc + a queued strike used to pay the
    /// `game_world_entity` PK lookup up to twelve times over for what is, in every case, the SAME row).
    pub(crate) fn signal(ctx: &ReducerContext, caster_guid: u64, spell_id: u32) -> Self {
        let (map_id, instance_id, grid_x, grid_y) = crate::helpers::grid_of(ctx, caster_guid);
        Self::signal_addr(
            ctx,
            caster_guid,
            spell_id,
            map_id,
            instance_id,
            grid_x,
            grid_y,
        )
    }

    /// Same baseline as [`Self::signal`], stamped from an already-fetched `caster` entity — zero
    /// `game_world_entity` lookups.
    pub(crate) fn signal_at(ctx: &ReducerContext, caster: &WorldEntity, spell_id: u32) -> Self {
        let (map_id, instance_id, grid_x, grid_y) = crate::helpers::entity_addr(caster);
        Self::signal_addr(
            ctx,
            caster.guid,
            spell_id,
            map_id,
            instance_id,
            grid_x,
            grid_y,
        )
    }

    fn signal_addr(
        ctx: &ReducerContext,
        caster_guid: u64,
        spell_id: u32,
        map_id: u32,
        instance_id: u64,
        grid_x: i32,
        grid_y: i32,
    ) -> Self {
        Self {
            id: 0,
            caster_guid,
            spell_id,
            created_at: ctx.timestamp,
            target_guid: 0,
            cast_time_ms: 0,
            is_completion: false,
            damage: 0,
            school: 0,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            is_interrupted: false,
            cooldown_ms: 0,
            delay_ms: 0,
            healed: 0,
            is_proc_log: false,
            swing_hit_info: 0,
            client_initiated: false,
            map_id,
            instance_id,
            grid_x,
            grid_y,
            failure_reason: CAST_FAIL_NONE,
        }
    }
}

/// Per-caster global-cooldown gate: after a successful cast the caster can't cast again until `ready_at`.
/// This is the GCD ONLY (one row per caster) — the per-spell cooldown lives in `game_spell_cd`. Schema is
/// UNCHANGED (the gateway binding for this table stays valid; no regen needed).
#[table(accessor = game_spell_cooldown, public)]
pub struct SpellCooldown {
    #[primary_key]
    pub caster_guid: u64,
    pub ready_at: Timestamp,
}

/// Per-`(caster_guid, spell_id)` cooldown gate — the SEPARATE second gate (distinct from the GCD above).
/// A spell whose `game_spell.cooldown_ms > 0` records its own `ready_at` here on a successful cast; a
/// re-cast of THAT spell by THAT caster is rejected until it elapses, even when the GCD is clear (a spell
/// can be off-GCD yet still on its own cooldown). A `cooldown_ms == 0` spell writes NO row here, so every
/// current seed spell is unaffected (baseline-safe). Logical key is `(caster_guid, spell_id)`; mirrors the
/// `game_aura` idiom — an `#[auto_inc]` PK plus a `by_caster` btree index, the specific spell located with
/// a `.find(|r| r.spell_id == ..)` on the caster's rows (no lossy u64+u32 PK pack). A NEW table → the GCD
/// binding is untouched; this table's binding must be generated for the gateway to relay SMSG_SPELL_COOLDOWN.
#[table(accessor = game_spell_cd, public, index(accessor = by_caster, btree(columns = [caster_guid])))]
pub struct SpellCd {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub ready_at: Timestamp,
}

/// Per-`(caster, school)` cast LOCKOUT — the Kick "school silence". After a cast is INTERRUPTED, the
/// victim's whole SCHOOL of magic is locked for ~5s: a cast whose `game_spell.school_mask` matches a LIVE
/// lockout row for that caster is rejected by `resolve_cast_at` (until `until > now`). Logical key is
/// `(caster_guid, school)`; mirrors the `game_spell_cd` idiom — an `#[auto_inc]` PK plus a `by_caster`
/// btree, the specific school located with `.find(|r| r.school == ..)` on the caster's rows. A NEW public
/// table → no existing binding changes, and a plain `publish` auto-migrates it (additive, no rows). Public
/// so the gateway can later relay an SMSG_SPELL_COOLDOWN-style lock (the server-authoritative gate works
/// without it). Rows are never GC'd (like `game_spell_cd`): the `until > now` predicate makes expired rows
/// inert, and a re-interrupt of the same school refreshes the row in place. No Timestamp-on-load trap (the
/// table is reducer-written only). [entity]
#[table(accessor = game_school_lockout, public, index(accessor = by_caster, btree(columns = [caster_guid])))]
pub struct SchoolLockout {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub school: u8, // game_spell.school_mask of the interrupted cast (1=phys 2=holy 4=fire 8=nat 16=frost 32=shadow 64=arcane)
    pub until: Timestamp, // the lock expires at this time
}

/// Drives the aura-expiry + periodic tick. [server]
#[table(accessor = game_aura_schedule, scheduled(tick_auras))]
pub struct AuraSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// A GROUND-AoE persistent damage area (118): a fixed-position zone spawned by an `E_PERSISTENT_AREA`
/// cast (Consecration / Blizzard / Rain of Fire / Flamestrike's patch). `tick_ground_areas` re-scans
/// `radius_yd` around `(x,y,z)` every `period_ms` and applies `amount` `school_mask` damage to every
/// hostile inside (via the shared apply_resistance→apply_target_damage path), reaping the row at
/// `expires_at`. DIRECT per-tick damage — no per-unit aura churn (see taxonomy `E_PERSISTENT_AREA`).
/// `level` is NOT stored: resist mitigation derives it from the live caster (gone caster → 0%, the
/// baseline-safe default). [server]
#[table(accessor = game_ground_area)]
pub struct GroundArea {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub spell_id: u32,
    pub caster_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub radius_yd: f32,
    pub amount: i32,     // per-tick damage magnitude (pre-resist)
    pub school_mask: u8, // game_spell.school_mask — for magic resist
    pub period_ms: u32,
    pub next_tick_micros: i64, // micros-since-epoch of the next due tick; advanced by period_ms
    pub expires_at: Timestamp,
}

/// The client-visible half of a ground area (118): a vanilla DYNAMICOBJECT — the 5875 client draws
/// Consecration's ground swirl from a DynamicObject CREATE (DYNAMICOBJECT_SPELLID → SpellVisual),
/// NEVER from the cast packets alone (live find: no swirl rendered). One row per live
/// `game_ground_area`, guid = (0xF100 << 48) | area id (HIGHGUID_DYNAMICOBJECT, disjoint from the
/// corpse 0xF101 / GO 0xF110 spaces); inserted by `create_ground_area`, deleted by the tick's reap.
/// PUBLIC — the gateway subscribes and relays CREATE/DESTROY. [entity]
#[table(accessor = game_dynamic_object, public)]
pub struct DynamicObject {
    #[primary_key]
    pub guid: u64,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub radius_yd: f32,
}

/// Drives the ground-AoE damage tick (118). [server]
#[table(accessor = game_ground_area_schedule, scheduled(tick_ground_areas))]
pub struct GroundAreaSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// A spell mid-cast (the cast bar): a one-shot scheduled row whose `fire_pending_cast` callback fires
/// when the cast finishes, carrying the cast payload alongside the deadline. [server]
#[table(
    accessor = game_pending_cast,
    scheduled(fire_pending_cast),
    // Perf catalog 1.18: `interrupt_cast` runs off `break_channel` on every moving heartbeat and off
    // every damage pushback; without this it full-scanned the table for one caster's row.
    index(accessor = by_caster, btree(columns = [caster_guid]))
)]
pub struct PendingCast {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub caster_guid: u64,
    pub spell_id: u32,
    pub level: u8,
    pub target_guid: u64,
    // How many times THIS cast has already been pushed back by direct damage (cmangos's
    // two-pushbacks-per-cast convention). `pushback_cast` (cast.rs) adds `CAST_PUSHBACK_MS` to
    // `scheduled_at` and increments this once per landed direct hit, up to `CAST_PUSHBACK_MAX` (2) —
    // the 3rd+ hit neither delays nor cancels the cast (it completes on schedule). `game_pending_cast`
    // carries no `public` (never gateway-subscribed — the cast-bar deadline is server-internal), so this
    // END-appended column needs no binding work at all. END-appended + `#[default(0)]` → additive
    // auto-migration (the publish-migration rule).
    #[default(0)]
    pub pushback_count: u8,
    // GROUND-TARGET dest (118 phase 2): the clicked ground point for a ground-AoE cast (Flamestrike patch,
    // Blizzard, Rain of Fire) so a TIMED ground cast carries its dest from begin_cast to the completion
    // (`fire_pending_cast` → `resolve_cast_at`). `has_dest` distinguishes "no dest" from a legitimate
    // (0,0,0) point. Same END-appended `#[default]` additive auto-migration as `pushback_count`; still no
    // gateway binding (non-public, server-internal).
    #[default(false)]
    pub has_dest: bool,
    #[default(0.0)]
    pub dest_x: f32,
    #[default(0.0)]
    pub dest_y: f32,
    #[default(0.0)]
    pub dest_z: f32,
}

/// A projectile's damage IN FLIGHT: a one-shot scheduled row whose `fire_spell_impact` callback
/// applies the already-resolved damage when the missile reaches the target — `SMSG_SPELL_GO` (the
/// trajectory) already fires synchronously at cast resolution (see `resolve_cast_at`), so this
/// ONLY defers the impact (`apply_target_damage` + the floating damage number), never the visual. Mirrors
/// `PendingCast`'s one-shot shape. `after_resist` is the crit+resist-scaled damage basis ALREADY rolled at
/// cast time (vanilla: the roll happens at cast, only the impact/health-write is delayed) — `fire_spell_impact`
/// runs it through `apply_target_damage` for absorb/kill/threat exactly like the instant path. [server]
#[table(accessor = game_pending_spell_impact, scheduled(fire_spell_impact))]
pub struct PendingSpellImpact {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub caster_guid: u64,
    pub target_guid: u64,
    pub spell_id: u32,
    pub school_index: u8,
    pub is_crit: bool,
    pub resisted: u32,
    pub after_resist: i32, // crit+resist-scaled damage basis, pre-absorb (rolled at cast time)
    // The bolt was launched inside a Triggered Cast, so its IMPACT is a Triggered hit too: it grants
    // nothing and raises no proc event. Without this the origin would be lost across the missile's
    // travel time and a proc's own bolt could fire further procs on landing. `game_pending_spell_impact`
    // is server-internal (never gateway-subscribed), so this END-appended `#[default(false)]` column
    // needs no binding work.
    #[default(false)]
    pub triggered: bool,
}

/// The floating spell-damage number (`SMSG_SPELLNONMELEEDAMAGELOG`) for a PROJECTILE hit that landed AFTER
/// its missile travel time — kept as its OWN public table (never folded into `game_spell_cast_event`)
/// so the gateway's existing `on_cast` cast-visual relay (danger-zone: NOT to be touched) never sees a
/// second GO/START for the same cast. A dedicated listener (subscriptions.rs, additive — sits beside
/// `on_cast`, doesn't modify it) relays ONLY the damage log off this table's insert. Reaped by the shared
/// event GC (`id`/`created_at` shape). [event]
#[table(
    accessor = game_spell_impact_event,
    public,
    // perf catalog 2.3: AOI-box scoping instead of a global `SELECT *`.
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct SpellImpactEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub caster_guid: u64,
    pub target_guid: u64,
    pub spell_id: u32,
    pub created_at: Timestamp,
    pub damage: u32,
    pub school: u8,
    pub is_crit: bool,
    pub resisted: u32,
    pub absorbed: u32,
    // --- AOI columns (perf catalog 2.3), END-appended + TYPED defaults (a bare `0` on a u64
    // encodes as 4 bytes and fails the publish). Stamped from the actor via `helpers::grid_of`;
    // (0,0,0,0) means "no live actor", which matches no box and is correctly never delivered.
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

/// A live SMSG_RESURRECT_REQUEST awaiting the dead target's CMSG_RESURRECT_RESPONSE accept. One
/// row per TARGET (the primary key is `target_guid`, not auto_inc) — a fresh E_RESURRECT cast on the
/// same still-pending target simply REPLACES the row (the newer offer wins, mirroring vanilla's
/// single-outstanding-prompt UX). `points` is the frozen `%` (E_RESURRECT's base_points) to apply if
/// accepted, so the accept path reuses `revive_amount` with the pct resolved at OFFER time, not at
/// accept time (a rescinded/changed spell mid-flight can't retroactively
/// change what an already-issued offer grants). RLS-scoped to the target's OWNER identity (mirrors
/// `game_whisper_event`) so only the dead player's own connection ever sees their pending offer. [event]
#[table(accessor = game_resurrect_request, public)]
pub struct ResurrectRequest {
    #[primary_key]
    pub target_guid: u64,
    pub target_identity: Identity, // the dead player's owner identity — the RLS key
    pub caster_guid: u64,
    pub caster_name: String, // frozen at offer time for the SMSG_RESURRECT_REQUEST name field
    pub points: i32,         // frozen E_RESURRECT base_points (%), applied on accept
    pub created_at: Timestamp,
}
