//! KINDS / MECHANICS / param-tag / target / combat-field / stat / resistance taxonomy + the
//! spell-power scaling tuning constants. `mod.rs` re-exports these (`pub(crate) use taxonomy::*`)
//! so `crate::spell::<CONST>` resolves.

// ===========================================================================================
//  KINDS — the deduplicated effect/aura taxonomy (dispatch key). Split by MECHANISM, folded by
//  DATA. High bit `KIND_AURA_BIT` => the effect places a persistent aura (vs an instant one-shot).
//  The importer maps mangos (Effect, AuraName) pairs onto these; unmapped → E_SCRIPTED no-op.
// ===========================================================================================

/// High bit on a `kind`: this effect creates a persistent `game_aura` row (vs an instant one-shot).
pub(crate) const KIND_AURA_BIT: u8 = 0x80;

/// `Spell.attributes` bit: a PASSIVE spell (vanilla `SPELL_ATTR_PASSIVE`). Server-enforced, never
/// player-cast or player-cancelable — talent passives set it so `cancel_aura` refuses to strip them.
pub(crate) const SPELL_ATTR_PASSIVE: u32 = 0x40;

// --- Rogue-slice cast-gate flags (our OWN bits in the DEDICATED `Spell.cast_flags` mask — NOT the raw
// vanilla `Spell.attributes`, whose bits are densely used and would collide. The importer sets these BY
// NAME; the gates in resolve_cast_at read them — keyed on the flag, never a spell id). ---
/// The spell may only be cast while the caster is BEHIND the target (rear 180° hemisphere) — Backstab.
/// Gate: reject unless `is_behind(caster, target)`.
pub(crate) const SPELL_ATTR_REQ_BEHIND: u32 = 0x0001;
/// The spell may only be cast while the caster is STEALTHED (an opener) — Sap.
/// Gate: reject unless `is_stealthed(ctx, caster_guid)`.
pub(crate) const SPELL_ATTR_REQ_STEALTH: u32 = 0x0002;
/// Casting this spell does NOT break the caster's stealth (Sap / Pick Pocket stay stealthed — vanilla).
/// The `break_stealth` chokepoint in the cast path skips a spell carrying this bit.
pub(crate) const SPELL_ATTR_STEALTH_SAFE: u32 = 0x0004;
/// A combo-FINISHER whose aura DURATION scales with the combo points spent (Slice and Dice). At
/// aura-apply, the aura's lifetime is `finisher_duration_ms(hdr.duration_ms, combo)` read off the
/// caster's combo pool on the ENEMY cast target — then the combo is spent EXACTLY ONCE (one aura effect
/// per such spell). The aura still lands on its own target kind (SnD's haste is T_SELF → the caster);
/// only its expiry is recomputed. Gate: `aura_apply` reads this bit, never a spell id. The importer
/// sets it BY NAME (Slice and Dice).
pub(crate) const SPELL_ATTR_FINISHER_DURATION: u32 = 0x0008;
/// A Sap-shaped INCAPACITATE OPENER: in addition to REQ_STEALTH, the target must be OUT of combat AND a
/// HUMANOID (vanilla Sap). Split out of REQ_STEALTH so a non-Sap stealth opener (Garrote — usable on ANY
/// creature type, in or out of combat) carries REQ_STEALTH WITHOUT these constraints. The importer sets
/// it BY NAME (Sap only); the gate in resolve_cast_at reads this bit, never a spell id.
pub(crate) const SPELL_ATTR_INCAP_OPENER: u32 = 0x0010;
/// Overpower: castable ONLY in the ~5s window after one of the caster's melee swings was DODGED. The attack
/// table (resolve_swing) stamps `overpower_until_ms` on a HIT_DODGE; the gate rejects unless that window is
/// still open. The importer sets it BY NAME (Overpower); the gate reads this bit, never a spell id.
pub(crate) const SPELL_ATTR_REQ_OVERPOWER: u32 = 0x0020;
/// Revenge: castable ONLY in the ~5s window after the caster DODGED / PARRIED / BLOCKED an incoming swing.
/// The attack table stamps `revenge_until_ms` on a HIT_DODGE/PARRY/BLOCK against this unit; the gate rejects
/// unless that window is open. The importer sets it BY NAME (Revenge); the gate reads this bit, never an id.
pub(crate) const SPELL_ATTR_REQ_REVENGE: u32 = 0x0040;
/// A CHANNELED spell (Arcane Missiles): the cast holds the caster for the spell's `duration_ms` and TICKS a
/// per-tick effect each `period_ms` (the channel = a self-`A_PERIODIC_TRIGGER` aura, ticked by `tick_auras`).
/// The channel BREAKS early on the caster moving / starting another cast / being CC'd / the channel target
/// dying — all converging on `break_channel`. The importer sets it from the DBC `AttributesEx1` CHANNELED
/// bit (`0x44`), with a by-NAME fallback (Arcane Missiles); it ALSO drives the importer's reclassification of
/// the periodic-trigger effect (`PeriodicTriggerSpell`/`E_TRIGGER` with `period_ms>0` + a T_SELF target) into
/// `A_PERIODIC_TRIGGER`. The engine reads this bit / the kind, NEVER a spell id.
pub(crate) const SPELL_ATTR_CHANNELED: u32 = 0x0080;
/// Backstab: castable ONLY while the caster has a DAGGER (weapon subclass 15) equipped in the MAIN-HAND
/// slot (15) — vanilla's melee-weapon-type restriction. Gate: join the caster's `game_item_instance` in
/// the main-hand slot -> `game_item_template` -> `subclass == weapon_subclass::DAGGER`; a missing/broken/
/// non-dagger main-hand rejects. Player-only (a caster with no item rows, i.e. a creature, always fails
/// this join and is rejected too — Backstab is a rogue-only ability so this is baseline-safe in practice).
/// The importer sets it BY NAME (Backstab); the gate in resolve_cast_at reads this bit, never a spell id.
pub(crate) const SPELL_ATTR_REQ_DAGGER: u32 = 0x0100;

/// cmangos `CREATURE_TYPE_HUMANOID` — the creature_type value the Sap incapacitate-opener requires on the
/// target.
pub(crate) const HUMANOID_TYPE: u8 = 7;

/// cmangos `CREATURE_TYPE_BEAST` — the creature_type value a corpse must carry to be SKINNABLE (Skinning
/// profession, slice 2). Imported verbatim by the importer (`ct::CREATURE_TYPE`), so Elwynn wolves/boars
/// arrive as type 1. Shared by `loot::skin_corpse`'s beast gate + the `debug_skin_nearest` nearest-search.
pub(crate) const BEAST_TYPE: u8 = 1;

/// cmangos `CREATURE_TYPE_CRITTER` — the creature_type value that never yields XP/honor. Gates the
/// Drain Soul soul-shard grant: a critter kill mints no shard regardless of level.
pub(crate) const CRITTER_TYPE: u8 = 8;

// --- instant effects (high bit clear) ---
pub(crate) const E_DAMAGE: u8 = 0x01; // direct damage (lethal-for-creatures via kill_creature)
pub(crate) const E_HEAL: u8 = 0x02; // direct heal (clamp to max)
pub(crate) const E_ENERGIZE: u8 = 0x03; // restore power (p0 = power type)
pub(crate) const E_DISPEL: u8 = 0x04; // strip auras (p0 = dispel category, 0 = all foreign)
pub(crate) const E_TRIGGER: u8 = 0x05; // cast trigger_spell now
pub(crate) const E_TAUNT: u8 = 0x06; // force-aggro: top the caster's threat on the target creature
pub(crate) const E_CREATE_ITEM: u8 = 0x07; // create item(s) in the caster's inventory (conjure / quest item); p0 = item entry, base_points = count
pub(crate) const E_WEAPON_STRIKE: u8 = 0x08; // physical melee ability: damage = weapon swing roll + base_points (Mortal Strike et al.)
pub(crate) const E_CHARGE: u8 = 0x09; // rush the caster into melee of the target (position change; +rage rides a separate E_ENERGIZE)
pub(crate) const E_CONVERT_RESOURCE: u8 = 0x0A; // drain the caster's health into power 1:1 (Life Tap); from/to pools generalize via params later
pub(crate) const E_JUDGEMENT: u8 = 0x0B; // unleash the caster's active SEAL: a holy hit derived from the seal, then consume it
pub(crate) const E_ADD_COMBO: u8 = 0x0C; // GENERATOR: +1 combo point on the target (Sinister Strike / Gouge / Backstab), capped at 5 — game_combo_point
pub(crate) const E_FINISHER_DAMAGE: u8 = 0x0D; // FINISHER: damage = base_points (per-point) × the target's combo points, then spend them (Eviscerate)
pub(crate) const E_RESURRECT: u8 = 0x0E; // revive a DEAD ally: restore base_points% of max hp/power + clear death/ghost (Resurrection)
pub(crate) const E_SCRIPTED: u8 = 0x0F; // a registered Rust fn (script_id)
pub(crate) const E_PICKPOCKET: u8 = 0x10; // grant the rogue a small copper pocket from a creature WITHOUT engaging (Pick Pocket)
pub(crate) const E_INTERRUPT: u8 = 0x11; // cancel the target's in-progress (timed) cast (Kick) — calls interrupt_cast; no-op if not casting. Kick ALSO locks the cast's SCHOOL for ~5s (game_school_lockout, Rogue Slice 3)
pub(crate) const E_REDUCE_THREAT: u8 = 0x12; // one-time reduction of the CASTER's CURRENT threat by base_points across all its source rows (Feint), floored at 0 — DISTINCT from Fade's A_MOD_COMBAT(COMBAT_THREAT) FUTURE-threat percent
pub(crate) const E_NEXT_SWING: u8 = 0x13; // QUEUE the strike onto the caster's next melee swing (Heroic Strike/Cleave): the cast sets next_swing_spell + charges rage (no instant damage); the next LANDED swing (resolve_swing) adds this effect's base_points as bonus damage and clears the field
pub(crate) const E_SET_STANCE: u8 = 0x14; // set the caster's stance/form to p0, per the STANCE_* convention block below (THE definition site): Warrior 0/1/2, Druid Bear/Cat/DireBear 3/4/5 (156) — a one-shot state write to WorldEntity.stance, mutually exclusive (the old stance is overwritten, no aura cleanup). The importer reclassifies the stance/form spells' inert ModShapeshift→A_FLAG marker to this kind BY NAME, with p0 remapped from the vanilla form id via form_to_stance (the lockstep inverse of client_form_for_stance — NOT a fixed −17 offset since druid forms landed). Recasting the ACTIVE druid form toggles back to 0 (form_recast_toggles_off); a warrior same-stance recast stays a no-op. Switching automatically clears the prior stance's mitigation/threat because those are FOLDS keyed on the field, not auras
pub(crate) const E_SUMMON_PET: u8 = 0x15; // summon a persistent PET creature owned by the caster (Summon Imp): despawn any existing pet, then spawn the creature `p0` (p0_kind = P_ENTRY, a game_creature_template.entry) at the caster with owner_guid = caster. ONE pet per owner (re-summon replaces). The pet is a normal creature entity (no spawn row) that rides the behavior cycle's pet phase (follow the owner when idle / engage the owner's target in combat) + the existing chase/melee. The importer maps the raw vanilla Summon effect (56) to this kind with the misc_value (the summoned creature entry) routed into p0. Despawned on owner logout/death
pub(crate) const E_HEAL_MAX_HEALTH: u8 = 0x16; // heal the target to FULL max health (Lay on Hands): the vanilla HealMaxHealth effect (67) heals to max regardless of base_points (its DBC base is ~0), so it canNOT route through E_HEAL (which heals `points`, i.e. 0). Sets t.health = t.max_health and credits the full heal as threat. The importer splits effect 67 out of E_HEAL onto this kind (E_HEAL stays effect 10 only). Spell-power-agnostic (a max-heal needs no scaling)
pub(crate) const E_TAME_CREATURE: u8 = 0x20; // completed Hunter tame: convert the explicit wild target into one durable Hunter identity plus a live Hunter pet
pub(crate) const E_FEED_PET: u8 = 0x21; // consume an explicit item target after Hunter-pet care validation
pub(crate) const E_ENCHANT_ITEM: u8 = 0x17; // apply a permanent enchant to the cast's ITEM target: p0 = the enchant_id (p0_kind = P_ENCHANT_ID), looked up in the module ENCHANTS overlay for the stat bonus. The spell never runs through resolve_cast (it targets an item GUID, not a unit) — the GATEWAY intercepts CMSG_CAST_SPELL, sees this effect kind, resolves the item GUID→bag slot, and calls enchant_item_on_slot(slot, p0). Routing is by KIND + p0, NOT a hardcoded spell-id list, so a new enchant is a data row
pub(crate) const E_DISENCHANT: u8 = 0x18; // disenchant the cast's ITEM target into reagents: same gateway-intercept path as E_ENCHANT_ITEM, dispatched to disenchant_item(slot). No params (the module validates disenchantability + yields dust by item)
pub(crate) const E_PERSISTENT_AREA: u8 = 0x1B; // GROUND-AoE (118, Consecration/Blizzard/Rain of Fire/Flamestrike-patch): an INSTANT effect that, on cast, spawns a fixed-position `game_ground_area` row at the anchor (caster pos for a self/target-0 area; the clicked ground dest once dest-coords are plumbed). Its own scheduled `tick_ground_areas` re-scans the radius each `period_ms` and applies `amount` `school` damage to every hostile inside via the shared apply_resistance→apply_target_damage path (threat/kill/absorb/break-CC reused), reaping the row at `expires_at`. DIRECT per-tick damage (no per-unit aura churn) — the clean-slate choice over mangos' DynamicObject+SpellAuraHolder-add/remove, equivalent for pure-damage areas. The importer reclassifies the ground A_PERIODIC_DAMAGE effect to this kind BY NAME. A future non-damage ground field (heal/slow) extends the emitted descriptor
pub(crate) const E_FISH: u8 = 0x1C; // Fishing (060): gateway-intercepted like E_ENCHANT_ITEM/E_DISENCHANT — CMSG_CAST_SPELL for a spell carrying this kind routes to the `fish` reducer (instant-resolve alpha catch; the bobber/channel flow is the deferred follow-up). Inert in-module (no resolve arm); exists so the gateway routes by DATA, never a spell-id list.
pub(crate) const E_OPEN_LOCK: u8 = 0x1D; // Pick Lock (119): gateway-intercepted like E_FISH (0x1C) — CMSG_CAST_SPELL for a spell carrying this kind routes to the `pick_lock` reducer (unlock a locked GameObject, gated on the caster's Lockpicking 633 skill vs the game_lock required_skill). Inert in-module (NO resolve arm in cast.rs); exists so the gateway routes by DATA, never a spell-id list. 0x1E is reserved for a future E_SUMMON_PORTAL — do NOT reuse.
pub(crate) const E_BLINK: u8 = 0x1A; // teleport the caster ~20yd FORWARD along its facing (Mage Blink, 116): a self-cast position change reusing the teleport core (like E_CHARGE), clamped to the furthest nav-LoS-clear point so it doesn't cross geometry. Root/snare removal rides a separate A_IMMUNITY effect. The importer name-rescues the dead SCRIPT teleport effect (raw 29) to this kind
pub(crate) const E_RECALL_HOME: u8 = 0x1F; // teleport the caster to its bound HOME (Hearthstone, #387): a self-cast recall reusing the shared `world::recall_to_home` core, always to instance 0 regardless of the caster's current instance. Data-driven — a consumable's `spellid_1` naming a spell that carries this kind IS "a recall item"; `items::ops::apply_item_use` reads that (not a hardcoded item entry) to skip the normal stack-consumption a used-up consumable takes, since a recall trinket is never consumed. No cost/cooldown gate yet (the vanilla ~10s cast + 1hr CD is the same later follow-up E_BLINK's forward-teleport already defers)
pub(crate) const E_DUEL: u8 = 0x22; // Duel (raw effect 83): request a server-authoritative Duel; p0 is the duel-flag gameobject template entry
/// Remove the target's active LAND MOUNT (Dazed's mount-removal half). An instant effect that calls the
/// one shared `mount::dismount` — idempotent, a silent no-op on an unmounted target, and never touched by
/// an unlanded cast (it resolves like any other instant effect). The importer translates a raw
/// `DISPEL_MECHANIC` effect whose parameter is the mount mechanic onto this kind, so the runtime never
/// implements a generic mechanic dispel and never branches on spell 1604 or a spell name.
pub(crate) const E_DISMOUNT: u8 = 0x23;
pub(crate) const E_POWER_BURN: u8 = 0x19; // drain N mana from the target and deal a fraction of it as damage (Mana Burn): MANA-power-type gate read off the target's `unit_bytes_0` byte 3 (same read as `is_rage_user`) — a rage/energy target is a silent no-op (power AND health untouched), matching vanilla's behaviour of skipping the effect entirely. drained = min(base_points, target.power) (floor-at-available; an empty/low pool just burns less, never fails the cast). damage = drained * p1 / 100 (p1 = the effect's ratio in basis-points — vanilla Mana Burn is EffectMultipleValue=0.5 -> p1=50 -> half the drained mana as Shadow damage); `p1<=0` (unauthored data) defaults to 100 (1:1), so a missing p1 never silently zeroes all burn damage. Dealt via the shared `apply_target_damage` (threat/kill/absorb reuse, no new wire work)

// --- aura effects (high bit set) ---
pub(crate) const A_PERIODIC_DAMAGE: u8 = 0x90; // DoT
pub(crate) const A_PERIODIC_HEAL: u8 = 0x91; // HoT
pub(crate) const A_PERIODIC_ENERGIZE: u8 = 0x92; // periodic power
                                                 // A CHANNEL's per-tick trigger: a self-aura that, each tick, CASTS its frozen trigger spell (the channel's
                                                 // per-bolt missile — e.g. Arcane Missiles 5143 → 7268's E_DAMAGE) at the FROZEN channel target. Unlike the
                                                 // DoT/HoT/energize kinds it folds NO vitals pool of its own — it routes each tick through `resolve_cast_at`,
                                                 // so the bolt gets spell-power scaling / crit / resist / threat / the kill path FOR FREE. The trigger spell
                                                 // id is frozen in `eff_p1`; the channel target (the enemy guid) in `channel_target`. Reaped on duration like
                                                 // any aura (N ticks = duration_ms/period_ms); broken EARLY by `break_channel` (move/new-cast/CC/target-death).
pub(crate) const A_PERIODIC_TRIGGER: u8 = 0x93; // channel tick: cast the frozen trigger spell at channel_target
pub(crate) const A_MOD_STAT: u8 = 0xA0; // p0 = stat 0..4 (0xFF = all)
pub(crate) const A_MOD_RESISTANCE: u8 = 0xA1; // p0 = school mask (armor = bit0)
pub(crate) const A_ABSORB: u8 = 0xA2; // p0 = pool school. `p1` is unused by A_ABSORB itself; on ANY aura
                                      // kind, `p1` GENERICALLY doubles as the LINKED-DEBUFF spell id (see `apply_linked_debuff`/the refusal
                                      // gate in cast.rs); PW:Shield's own `p1` names Weakened Soul (6788).
pub(crate) const A_MOD_COMBAT: u8 = 0xA3; // p0 = combat field (AP/crit/hit/dmg/haste)
pub(crate) const A_MOD_SPEED: u8 = 0xA4; // p0 = speed kind (move/swing/cast), signed pct
pub(crate) const A_MOD_HEALTH_POWER: u8 = 0xA5; // +max hp/power
pub(crate) const A_MOD_DAMAGE_TAKEN: u8 = 0xA6; // signed % modifier to INCOMING damage (−75 = Shield Wall's 75% reduction; + = a vulnerability debuff). p0 reserved for a school mask (all-school in v1)
pub(crate) const A_SEAL: u8 = 0xA7; // proc-on-swing holy-damage seal (Seal of Righteousness): a landed melee swing reads it + adds holy; amount = per-swing seal value (weapon-speed-scaled); single active seal; consumed by E_JUDGEMENT
pub(crate) const A_STEALTH: u8 = 0xA8; // presence marker: the unit is stealthed — creatures skip it as an aggro target; removed (broken) by the caster's own swing/non-Stealth cast
/// Passive aura: X% of normal OUT-of-combat health regen continues DURING COMBAT. `amount` = the
/// percent (10 for Troll Regeneration racial). The cycle's regeneration phase sums all active
/// `A_COMBAT_HEALTH_REGEN_PCT` auras on the entity → allows `pct%` of `health_regen_per_tick`
/// during combat. 0 active auras → 0 combat regen (today's behaviour). Composed additively (two
/// such auras at 10% each grant 20%); percent is capped at 100 before the multiply. Data only —
/// no race read, no spell-id check; the same kind on any class composes identically.
pub(crate) const A_COMBAT_HEALTH_REGEN_PCT: u8 = 0xA9;
/// PERCENT stat modifier (distinct from the FLAT `A_MOD_STAT`): p0 = stat 0..4 (0xFF = all), amount =
/// percent (e.g. The Human Spirit = +5% Spirit). `recompute_vitals` folds it as a multiplier on the
/// resolved stat — `effective = (base + flat) * (100 + pct) / 100` — so it scales the base, not a flat add.
pub(crate) const A_MOD_STAT_PCT: u8 = 0xAA;
/// A Proc that starts a **Triggered Cast** of its trigger spell (vanilla `Spell.dbc` aura 42,
/// ProcTriggerSpell). `eff_p1` freezes the trigger spell id at apply, mirroring `A_PERIODIC_TRIGGER`'s
/// own `eff_p1` freeze (see `aura_apply`); the proc profile (event mask, chance, charges, internal
/// cooldown) freezes onto the same row. Sits on the CARRIER; `proc::run_proc_pass` — the one proc pass,
/// called from `combat::apply_hit` — decides whether it fires and casts the trigger spell from the
/// Carrier at the Counterparty. A carrier with no proc aura is unaffected.
pub(crate) const A_PROC_TRIGGER: u8 = 0xAB;
/// A Proc that deals its frozen `amount` as damage of the proc spell's school (vanilla `Spell.dbc`
/// aura 43, ProcTriggerDamage). Same proc profile and same one pass as [`A_PROC_TRIGGER`]; `p0` is the
/// school mask ([`P_SCHOOL_MASK`]). The firing arm is not wired yet — the pass logs and spends nothing.
pub(crate) const A_PROC_DAMAGE: u8 = 0xB4;
pub(crate) const A_SPELLMOD_FLAT: u8 = 0xAC; // spell modifier, FLAT (264 — DBC aura 107 AddFlatModifier): p0 (P_SPELLMOD_OP) = the SpellModOp, p1 = the 32-bit affected-spell FAMILY mask (DBC EffectItemType), amount = the SIGNED flat value (Improved Fireball: op 10 cast-time, −100ms/rank). Pulled by `spell_mod` at the cast-time/damage seams — a talent passive is pure data, zero per-spell code
pub(crate) const A_SPELLMOD_PCT: u8 = 0xAD; // spell modifier, PERCENT (DBC aura 108 AddPctModifier): same shape, amount = a signed percent
/// DISARM (Warrior Disarm, DBC AuraMod 67 ModDisarm): while active on an ENEMY, the disarmed unit's melee
/// swing loses its main-hand weapon — a PLAYER drops to UNARMED damage, a CREATURE (no weapon/unarmed split)
/// keeps only a documented fraction of its swing (`DISARM_SWING_RETAINED_PCT`). A generic aura placed via the
/// KIND_AURA_BIT path (natural expiry); `control::is_disarmed` presence-reads it at the swing-range seam
/// (`combat::swing_range_ctx`), never a spell id. 0 auras → not disarmed (baseline-safe).
pub(crate) const A_DISARM: u8 = 0xAE;
/// RETALIATION (Warrior Retaliation, self-buff): while active on a unit, any REAL incoming melee hit provokes
/// ONE free main-hand counter-swing back at the attacker (`combat::retaliate_on_hit`, hooked in
/// `break_auras_on_damage`'s melee block). A generic aura placed via the KIND_AURA_BIT path; the counter-swing
/// routes through `apply_target_damage` (attacker sentinel 0) so it can never itself re-trigger retaliation —
/// no recursion. 0 auras → no counter-swing (baseline-safe).
pub(crate) const A_RETALIATE: u8 = 0xAF;
pub(crate) const A_CONTROL: u8 = 0xB0; // CC: p0 = mechanic (stun/poly/fear/root/...)
pub(crate) const A_IMMUNITY: u8 = 0xB1; // p0 = school mask OR mechanic per p0_kind
/// DETECT-RANGE modifier (Priest Mind Soothe, DBC AuraMod 91 ModDetectRange): while active on a hostile
/// CREATURE, its proximity aggro / detection radius is shifted by the aura's SIGNED `amount` YARDS — Mind
/// Soothe's amount is NEGATIVE (-10), so it SHRINKS the radius (`math::detect_range_mod` sums the active
/// auras; the behavior cycle's aggro phase ADDS the sum to the creature's aggro radius, clamped ≥ 0 — a
/// soothed mob notices the player only from closer, or not at all). A generic aura via the KIND_AURA_BIT
/// path; 0 auras → full aggro radius (baseline-safe). Read on the soothed unit.
pub(crate) const A_MOD_DETECT_RANGE: u8 = 0xB2;
/// LAND MOUNT (vanilla Mounted aura): the STATE OF RECORD for a player's ground-mounted state. `p0` is
/// the resolved creature display id (`p0_kind` = [`P_DISPLAY_ID`]), frozen at apply and projected onto
/// `WorldEntity.mount_display_id` for the client's `UNIT_FIELD_MOUNTDISPLAYID`. The projection is derived
/// from this aura set by `mount::recompute_mount`, never a second state machine — zero `A_MOUNTED` auras
/// means display 0. A normal cancelable self aura: expiry, `CMSG_CANCEL_AURA`, dispel and `E_DISMOUNT`
/// all converge on the same recompute.
pub(crate) const A_MOUNTED: u8 = 0xB3;
pub(crate) const A_FLAG: u8 = 0xBE; // passive marker aura (no tick), p0 = flag id

/// Documented tuning approximation for `combat::swing_range_ctx`: a DISARMED creature has no weapon/unarmed
/// damage split (creature swings roll a flat template range), so instead of zeroing its damage we retain this
/// PERCENT of its swing while disarmed — a middle-ground stand-in for "lost its weapon" that keeps the
/// mechanic meaningful without a per-creature unarmed model. Players ignore this (they fall back to their real
/// unarmed range). Tunable.
pub(crate) const DISARM_SWING_RETAINED_PCT: u32 = 50;

// --- MECHANICS (the value of `p0` for an A_CONTROL aura when `p0_kind == P_MECHANIC`; the CC kind) ---
// Our OWN taxonomy (NOT the mangos Mechanics.dbc numbering) — they need only be internally consistent
// between the seed data and the gates that read them. Small distinct values starting at 1 so 0 stays a
// reserved "no mechanic" sentinel (a frozen A_CONTROL aura whose `eff_p0` is 0 matches NO gate → inert,
// exactly the pre-CC behavior). All four crowd-control mechanics are wired to gates, split on the two
// independent axes — ACT (perform a swing/aggro/cast) and MOVE (drive own position):
//   - M_STUN — cannot ACT, cannot MOVE. The full lock-down (frozen + silenced).
//   - M_ROOT — CAN ACT (keeps swinging/aggroing/casting) but cannot MOVE. Snared in place.
//   - M_POLY — cannot ACT, cannot MOVE (incapacitate/polymorph — frozen like stun, the action-only twin).
//   - M_FEAR — cannot ACT, but is force-MOVED: the fear-flee pass walks it AWAY from the fear source each
//     tick ("flees in terror"). The one mechanic that MOVES the unit rather than freezing it.
pub(crate) const M_STUN: i32 = 1;
pub(crate) const M_ROOT: i32 = 2;
pub(crate) const M_FEAR: i32 = 3;
pub(crate) const M_POLY: i32 = 4;

// --- p0_kind: what `p0` means (single-meaning tag; import-correctness tripwire) ---
pub(crate) const P_NONE: u8 = 0;
pub(crate) const P_STAT_ID: u8 = 1;
pub(crate) const P_SCHOOL_MASK: u8 = 2;
pub(crate) const P_MECHANIC: u8 = 3;
pub(crate) const P_POWER_TYPE: u8 = 4;
pub(crate) const P_COMBAT_FIELD: u8 = 5;
pub(crate) const P_SPEED_KIND: u8 = 6;
pub(crate) const P_FLAG: u8 = 7;
pub(crate) const P_ITEM_ENTRY: u8 = 8; // p0 is a game_item_template entry (E_CREATE_ITEM)
pub(crate) const P_ENTRY: u8 = 9; // p0 is a game_creature_template entry (E_SUMMON_PET — the summoned pet's creature entry)
pub(crate) const P_ENCHANT_ID: u8 = 10; // p0 is an enchant id (E_ENCHANT_ITEM — the enchant applied to the item)
                                        // SpellModOp values the engine consumes (mangos SpellModOp; the rest import fine and stay inert until a seam folds them).
pub(crate) const SPELLMOD_OP_DAMAGE: i32 = 0;
pub(crate) const SPELLMOD_OP_CASTING_TIME: i32 = 10;
// Slot 11 of the p0_kind taxonomy. Not read by any engine seam yet (the spell-mod fold reaches the
// op through `A_SPELLMOD_*`'s own `p0`), but the NUMBER is a stored data contract: the importer
// writes p0_kind values into `game_spell_effect`, so deleting the name would leave 11 undocumented
// and free for a future kind to collide with. Named here, deliberately unused.
#[allow(dead_code)]
pub(crate) const P_SPELLMOD_OP: u8 = 11; // p0 is a SpellModOp (A_SPELLMOD_*: 0=damage, 7=crit, 10=cast time, 14=cost); p1 carries the 32-bit affected-spell family mask
/// The effect's `amount` is a PERCENT of the caster's MAX power, not an absolute figure (Mage Evocation's
/// A_PERIODIC_ENERGIZE restores 15% of max mana per tick). `aura_apply` reads this tag and converts the
/// stored percent to an absolute per-tick amount (`caster.max_power * amount / 100`) BEFORE freezing it onto
/// the Aura row, so the generic energize tick (`energized_value`) restores a real number. Every other p0_kind
/// leaves `amount` verbatim (baseline-safe).
pub(crate) const P_PCT_MAX_POWER: u8 = 12;
pub(crate) const P_GAMEOBJECT_ENTRY: u8 = 13; // p0 is a game_gameobject_template entry (E_DUEL)
/// `p0` is a creature DISPLAY id — the `UNIT_FIELD_MOUNTDISPLAYID` value an [`A_MOUNTED`] aura projects.
/// The importer resolves a mount spell's creature template to its display once, at import, and freezes
/// the result here; the runtime only reads it.
pub(crate) const P_DISPLAY_ID: u8 = 14;
pub(crate) const P_RAW: u8 = 255; // scripted / unresolved

// --- TargetKind: who the effect resolves onto ---
pub(crate) const T_SELF: u8 = 0;
pub(crate) const T_TARGET_ENEMY: u8 = 1;
pub(crate) const T_TARGET_ALLY: u8 = 2;
pub(crate) const T_TARGET_ANY: u8 = 3;
pub(crate) const T_AREA_ENEMY: u8 = 4;
pub(crate) const T_AREA_ALLY: u8 = 5;
pub(crate) const T_CHAIN_ENEMY: u8 = 6;
pub(crate) const T_SCRIPTED: u8 = 7;

// --- combat fields (the value of `p0` for an A_MOD_COMBAT aura; the combat module reads these) ---
pub(crate) const COMBAT_ATTACK_POWER: u8 = 0;
pub(crate) const COMBAT_CRIT: u8 = 1;
pub(crate) const COMBAT_HIT: u8 = 2;
pub(crate) const COMBAT_DMG_DONE: u8 = 3;
// (field 4 RETIRED — melee haste was COMBAT_HASTE, now A_MOD_SPEED(SPEED_SWING). The importer never
// emitted COMBAT_HASTE — all haste/attack-speed auras route to A_MOD_SPEED — so swing speed has ONE
// representation. 4 stays a reserved hole to keep COMBAT_SPELL_POWER's value stable.)
/// Spell power / healing power — the caster-side damage/heal scaling field. An `A_MOD_COMBAT` aura with
/// `p0 == COMBAT_SPELL_POWER` is a flat spell-power buff (e.g. a "Spell Power" trinket/food), summed
/// alongside the INT-derived bonus by `spell_power` and folded into E_DAMAGE / E_HEAL magnitudes. Reuses
/// the existing A_MOD_COMBAT aura plumbing (`combat_field_bonus`) — NO new aura kind, NO schema change.
pub(crate) const COMBAT_SPELL_POWER: u8 = 5;
// Defender/utility combat fields (talent folds): an A_MOD_COMBAT aura with one of these p0s
// adds to the matching attack-table band / skill / threat, summed by `combat_field_bonus`. They enable
// passive talents (Deflection→parry, Shield Spec→block, Anticipation→defense, Defiance→threat) and any
// future +avoidance buff. Defender-side (read on the TARGET) except THREAT (read on the threat SOURCE).
pub(crate) const COMBAT_PARRY: u8 = 6; // + parry-chance basis points (defender)
pub(crate) const COMBAT_DODGE: u8 = 7; // + dodge-chance basis points (defender)
pub(crate) const COMBAT_BLOCK: u8 = 8; // + block-chance basis points (shielded defender)
pub(crate) const COMBAT_DEFENSE: u8 = 9; // + defense SKILL points (raises avoidance vs the attacker)
pub(crate) const COMBAT_THREAT: u8 = 10; // + threat generated, signed PERCENT (threat source)

// --- speed kinds (the value of `p0` for an A_MOD_SPEED aura; signed PERCENT amount: + faster, − slower).
// Mirrors the importer's `resolve_aura_params` mapping (ModMeleeHaste→SWING, ModDecreaseSpeed→MOVE, …).
// SWING folds into `combat::effective_swing_time`; MOVE folds into `combat::effective_move_speed` (the
// creature movement passes). CAST/MOUNTED are schema-ready, not yet read (a cast-time / mount-speed fold
// is a follow-up). A signed sum lets a haste + a slow net out. ---
pub(crate) const SPEED_MOVE: u8 = 0;
pub(crate) const SPEED_SWING: u8 = 1;
pub(crate) const SPEED_CAST: u8 = 2;
pub(crate) const SPEED_MOUNTED: u8 = 3;

// --- STANCES / FORMS (the value WorldEntity.stance holds + the value `p0` carries on an E_SET_STANCE
// effect; 0-based, our OWN small taxonomy — THE DEFINITION SITE of the stance-id convention). 0 = Battle
// (the login/default stance — every non-stance class also sits at 0, so they're "in Battle" inertly but
// never gate on it). Work-item 156 widened the space past the Warrior trio to the Druid COMBAT forms
// (Bear/Cat/Dire Bear — the shapeshift switches the importer name-rescues to E_SET_STANCE); the druid
// non-combat forms (Aquatic/Travel/Moonkin/Tree) remain out of scope, their markers stay inert A_FLAGs.
// The ONE convention, end to end:
//   * `WorldEntity.stance` and E_SET_STANCE `p0` carry the 0-based id below;
//   * the `game_spell.stances` usability mask carries bit `1 << stance` per allowed stance (a u8, ids
//     0..7 — 6 assigned, 2 spare), translated by the importer from the vanilla form-bit mask
//     (`1 << (formId-1)`) via its `form_to_stance` (LOCKSTEP with this table — vanilla ShapeshiftForm
//     ids: Battle=17 / Defensive=18 / Berserker=19 / Bear=5 / Cat=1 / DireBear=8);
//   * the client UNIT_FIELD_BYTES_1[2] form byte is derived back by `client_form_for_stance` (math.rs).
// `stance_allows` (the cast gate) and the combat folds all key on this one 0-based id. ---
pub(crate) const STANCE_BATTLE: u8 = 0;
pub(crate) const STANCE_DEFENSIVE: u8 = 1;
pub(crate) const STANCE_BERSERKER: u8 = 2;
pub(crate) const STANCE_BEAR: u8 = 3; // Druid Bear Form (vanilla form 5) — the tank form
pub(crate) const STANCE_CAT: u8 = 4; // Druid Cat Form (vanilla form 1)
pub(crate) const STANCE_DIRE_BEAR: u8 = 5; // Druid Dire Bear Form (vanilla form 8)

// --- Defensive Stance combat magnitudes (Option B — FIELD-keyed folds, NOT auras, so a stance switch
// clears them for free). Tuning lives in ONE place. Signed PERCENTs folded by the combat chokepoints:
// DR is added to `damage_taken_bonus` (read on the DEFENDER, both melee+spell paths); the dmg-done penalty
// is folded at the two OUTGOING-damage sites (read on the ATTACKER); the threat bonus rides `add_threat`
// (read on the threat SOURCE). All three are pure functions of `WorldEntity.stance`, so the moment
// E_SET_STANCE rewrites the field the old stance's contribution vanishes — no aura row to create/refresh/
// delete. Vanilla Defensive Stance: −10% damage taken, −10% damage dealt, +threat. ---
pub(crate) const STANCE_DEFENSIVE_DR_PCT: i32 = -10; // incoming-damage percent while in Defensive (−10% taken)
pub(crate) const STANCE_DEFENSIVE_DMG_DONE_PCT: i32 = -10; // outgoing-damage percent while in Defensive (−10% dealt)
pub(crate) const STANCE_DEFENSIVE_THREAT_PCT: i32 = 30; // threat percent bonus while in Defensive (+30% threat generated)

// --- projectile travel time ---
/// Generic projectile missile speed (yards/sec) for a ranged `E_DAMAGE` bolt (Shadow Bolt, Frostbolt, …).
/// Vanilla assigns each spell its own `Spell.dbc` `Speed` value; the importer does not currently surface
/// that column (see `importer/src/spell.rs`'s documented schema-bug remap, which repurposes the same wire
/// slot for `RangeIndex`), so this is one documented flat approximation shared by every ranged bolt rather
/// than a per-spell DBC value (Shadow Bolt ≈ 21 yd/s).
/// Melee/self/instant effects never read this (only a ranged `E_DAMAGE` effect does — see
/// `projectile_travel_ms`), so every non-projectile cast is unaffected.
pub(crate) const PROJECTILE_SPEED_YPS: f32 = 21.0;

// --- spell-power scaling tuning (documented + unit-tested; chosen baseline-safe) ---
/// INT below this floor contributes ZERO spell power. Set to the L2 starter's intellect (~20) so the
/// existing low-INT player/creatures (no buff, no spell-power gear) derive 0 INT spell power and every
/// seed spell's magnitude stays byte-identical to today. Vanilla INT does not grant spell power at all;
/// this above-baseline ramp is our slice's modest stand-in (it only adds when INT clearly exceeds the
/// starter floor — e.g. a high-level caster), so the gating is EXPLICIT: scaling appears only with a real
/// spell-power source (a high INT OR a `COMBAT_SPELL_POWER` aura).
pub(crate) const INT_SPELL_POWER_BASE: i32 = 20;
/// How many points of effective INT *above the floor* yield 1 point of spell power. A large divisor keeps
/// the INT ramp modest (a L60-ish caster at ~150 INT → (150-20)/5 = 26 spell power).
pub(crate) const INT_PER_SPELL_POWER: i32 = 5;
/// Per-effect spell-power coefficient, in PERCENT (100 = 1.0 = full spell power added once). The slice
/// uses a flat 1.0 for the wired instant direct effects (E_DAMAGE / E_HEAL) — a modest, documented choice
/// (the vanilla cast-time/3.5 coefficient is a later refinement). At 0 spell power this adds nothing, so
/// the baseline magnitude is unchanged.
pub(crate) const SPELL_POWER_COEFF_PCT: i32 = 100;

// --- base attributes (the value of `p0` for an A_MOD_STAT aura; matches the UNIT_FIELD_STAT order) ---
pub(crate) const STAT_STR: u8 = 0;
pub(crate) const STAT_AGI: u8 = 1;
pub(crate) const STAT_STA: u8 = 2;
pub(crate) const STAT_INT: u8 = 3;
pub(crate) const STAT_SPI: u8 = 4;
pub(crate) const STAT_ALL: u8 = 0xFF; // an A_MOD_STAT effect that buffs every attribute (e.g. Mark of the Wild)

// --- resistance schools (the value of `p0` for an A_MOD_RESISTANCE aura; a bitmask, armor = bit 0) ---
pub(crate) const RESIST_ARMOR: u8 = 0x01; // physical armor (bit 0 of the school mask)

/// Canonical, ordered list of every INSTANT (`E_*`) effect kind — the deduplicated effect taxonomy the
/// importer maps mangos `Effect` ids onto. This is the SINGLE source of truth for "every instant kind
/// that exists": `tests.rs`'s `instant_kind_wire_values_exhaustive` loops it (never a hand-copied
/// duplicate) so a kind is exhaustively covered by construction, and referencing every `E_*` const here
/// keeps an as-yet-unwired one from tripping `dead_code` (CI's clippy gate runs `-D warnings`, so a
/// kind added to the taxonomy but left OFF this slice fails the build, not just a test). #367: this
/// replaces the old `_TAXONOMY` scaffold + four separately hand-copied `E_*`/`A_*` lists in `tests.rs`,
/// one of which had drifted (E_BLINK/E_PERSISTENT_AREA/E_FISH/E_OPEN_LOCK were missing from it).
/// `#[allow(dead_code)]`: like `_TAXONOMY` before it, this binding is read only from `#[cfg(test)]`
/// code (`tests.rs`), so the non-test `lib` build never itself "reads" `ALL_INSTANT_KINDS` — but every
/// `E_*` const named INSIDE it is still counted as used, which is the array's actual job.
#[allow(dead_code)]
pub(crate) const ALL_INSTANT_KINDS: &[u8] = &[
    E_DAMAGE,
    E_HEAL,
    E_ENERGIZE,
    E_DISPEL,
    E_TRIGGER,
    E_TAUNT,
    E_CREATE_ITEM,
    E_WEAPON_STRIKE,
    E_CHARGE,
    E_CONVERT_RESOURCE,
    E_JUDGEMENT,
    E_ADD_COMBO,
    E_FINISHER_DAMAGE,
    E_RESURRECT,
    E_SCRIPTED,
    E_PICKPOCKET,
    E_INTERRUPT,
    E_REDUCE_THREAT,
    E_NEXT_SWING,
    E_SET_STANCE,
    E_SUMMON_PET,
    E_HEAL_MAX_HEALTH,
    E_ENCHANT_ITEM,
    E_DISENCHANT,
    E_POWER_BURN,
    E_BLINK,
    E_PERSISTENT_AREA,
    E_FISH,
    E_OPEN_LOCK,
    E_RECALL_HOME,
    E_DUEL,
    E_TAME_CREATURE,
    E_FEED_PET,
    E_DISMOUNT,
];

/// Canonical, ordered list of every AURA (`A_*`) kind — same rationale and same #367 fix as
/// [`ALL_INSTANT_KINDS`] above (that older hand-copied list had drifted too, missing
/// A_SPELLMOD_FLAT/A_SPELLMOD_PCT). Same `#[allow(dead_code)]` rationale as `ALL_INSTANT_KINDS` above.
#[allow(dead_code)]
pub(crate) const ALL_AURA_KINDS: &[u8] = &[
    A_PERIODIC_DAMAGE,
    A_PERIODIC_HEAL,
    A_PERIODIC_ENERGIZE,
    A_PERIODIC_TRIGGER,
    A_MOD_STAT,
    A_MOD_RESISTANCE,
    A_ABSORB,
    A_MOD_COMBAT,
    A_MOD_SPEED,
    A_MOD_HEALTH_POWER,
    A_MOD_DAMAGE_TAKEN,
    A_SEAL,
    A_STEALTH,
    A_COMBAT_HEALTH_REGEN_PCT,
    A_MOD_STAT_PCT,
    A_PROC_TRIGGER,
    A_PROC_DAMAGE,
    A_SPELLMOD_FLAT,
    A_SPELLMOD_PCT,
    A_DISARM,
    A_RETALIATE,
    A_CONTROL,
    A_IMMUNITY,
    A_MOD_DETECT_RANGE,
    A_MOUNTED,
    A_FLAG,
];

// The rest of the taxonomy vocabulary (param tags, target kinds, combat fields, speed kinds, stances,
// stats) is not KIND-shaped, already has its own hand-picked (non-"exhaustive") pin in tests.rs
// (`param_tag_wire_values_exhaustive`, `mechanic_wire_values`, `kind_wire_values`, …), and has never
// drifted the way the two lists above did — so it does not need a canonical, loop-tested slice of its
// own. A subset of it has no OTHER production call site yet though (only a test-only reference, same as
// every `E_*`/`A_*` kind before `ALL_INSTANT_KINDS`/`ALL_AURA_KINDS` existed), so it still needs SOME
// scaffold to keep `dead_code` from tripping on the non-test `lib` build — this is that scaffold, sized
// to just the residue (down from `_TAXONOMY`'s ~100 entries to these 16).
#[allow(dead_code)]
const _RESERVED_NON_KIND_TAXONOMY: &[u8] = &[
    P_NONE,
    P_STAT_ID,
    P_SCHOOL_MASK,
    P_POWER_TYPE,
    P_COMBAT_FIELD,
    P_SPEED_KIND,
    P_FLAG,
    P_ITEM_ENTRY,
    P_ENTRY,
    P_ENCHANT_ID,
    P_DISPLAY_ID,
    P_RAW,
    T_SCRIPTED,
    SPEED_CAST,
    SPEED_MOUNTED,
    STANCE_BATTLE,
    STANCE_BERSERKER,
];

// (No `_MECHANICS` dead-code scaffold needed: all four mechanics are wired into real gates — M_STUN and
// M_ROOT via their single-mechanic checkers `is_stunned`/`is_rooted`, and M_POLY/M_FEAR via the composite
// predicates `is_incapacitated`(M_POLY) / `is_feared`(M_FEAR), which the action/movement gates
// (`is_action_blocked`, `is_movement_blocked`, `is_self_movement_suppressed`) then compose.)
