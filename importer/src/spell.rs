//! Stream 1 (client DBC) — Spell.dbc → `game_spell` + `game_spell_effect` importer. Reads the
//! operator's OWN `Spell.dbc` (+ the SpellCastTimes / SpellRange / SpellDuration / SpellRadius aux
//! tables) IN MEMORY via the same `open_chain`/`read_table` path as `dbc.rs`, maps each spell's
//! header + up to 3 effects onto our deduped effect/aura TAXONOMY (module/src/spell/taxonomy.rs),
//! and emits derived `game_*` rows as chunked clear+reload SQL. It is the stress test of the
//! taxonomy: every effect either maps to a REAL kind or falls back to `E_SCRIPTED` (a queryable
//! no-op), and the run prints a COVERAGE REPORT measuring how much real vanilla spell data the
//! taxonomy absorbs.
//!
//! LICENSING FIREWALL: like the rest of `dbc.rs`, the client bytes stay in memory — NO `.dbc` (or any
//! Blizzard file) is ever written; only derived `game_spell`/`game_spell_effect` rows are emitted.
//!
//! The AuraMod variant names + instant-effect numeric IDs are matched by hand against
//! `wow_world_base::vanilla`. The shared build 5875 reader supplies the numeric columns. Both
//! mapping tables come from the design report; unmapped → `E_SCRIPTED` for graceful coverage.

use std::collections::BTreeMap;

use crate::spell_dbc::{read_spells, Spell};
use anyhow::{Context, Result};
use wow_dbc::vanilla_tables::spell_cast_times::{SpellCastTimes, SpellCastTimesKey};
use wow_dbc::vanilla_tables::spell_duration::{SpellDuration, SpellDurationKey};
use wow_dbc::vanilla_tables::spell_radius::{SpellRadius, SpellRadiusKey};
use wow_dbc::vanilla_tables::spell_range::{SpellRange, SpellRangeKey};
use wow_dbc::{DbcTable, Indexable};
use wow_world_base::vanilla::AuraMod;

use crate::dbc::{open_chain, read_table};
use crate::package_delta;
use crate::spell_snapshot::{write_snapshot, SpellEffectRow, SpellHeaderRow, SpellRows};
use crate::{push_insert, run_sql_statements, stamp_family, Args};
use std::path::Path;

// ===========================================================================================
//  TAXONOMY constants — verbatim from module/src/spell/taxonomy.rs (the emit target). Duplicated
//  here (not shared) because the wasm `module` is NEVER a dependency of this native ETL tool; these
//  are pure numeric tags and the runtime is the source of truth — keep the two lists in lockstep.
// ===========================================================================================

// instant effects (high bit clear)
const E_DAMAGE: u8 = 0x01;
const E_HEAL: u8 = 0x02;
const E_ENERGIZE: u8 = 0x03;
const E_DISPEL: u8 = 0x04;
const E_TRIGGER: u8 = 0x05;
const E_TAUNT: u8 = 0x06;
const E_CREATE_ITEM: u8 = 0x07;
const E_CHARGE: u8 = 0x09; // rush into melee (Charge — vanilla effect 96, reclassified by name); engine teleports the caster
const E_CONVERT_RESOURCE: u8 = 0x0A; // drain caster health -> power (Life Tap); reclassified from SCRIPT_EFFECT
const E_JUDGEMENT: u8 = 0x0B; // unleash the active seal (Judgement); reclassified from SCRIPT_EFFECT
const E_ADD_COMBO: u8 = 0x0C; // rogue combo GENERATOR (Sinister Strike/Gouge/Backstab); reclassified from the inert Dummy effect
const E_FINISHER_DAMAGE: u8 = 0x0D; // rogue FINISHER damage (Eviscerate): scales with combo points; reclassified from E_DAMAGE
const E_RESURRECT: u8 = 0x0E; // revive a dead ally (Resurrection); reclassified from SCRIPT_EFFECT
const E_SCRIPTED: u8 = 0x0F;
const E_PICKPOCKET: u8 = 0x10; // grant the rogue copper from a creature without engaging (Pick Pocket); reclassified from SCRIPT_EFFECT
const E_INTERRUPT: u8 = 0x11; // cancel the target's in-progress cast (Kick); reclassified from the raw vanilla InterruptCast effect (68)
const E_REDUCE_THREAT: u8 = 0x12; // one-time CURRENT-threat drop on the caster (Feint); reclassified from SCRIPT_EFFECT (lockstep with module taxonomy)
const E_NEXT_SWING: u8 = 0x13; // QUEUE the strike onto the caster's next melee swing (Heroic Strike/Cleave); reclassified from E_DAMAGE by name (lockstep with module taxonomy)
const E_SET_STANCE: u8 = 0x14; // set the caster's Warrior stance (Battle/Defensive/Berserker Stance); reclassified from the inert ModShapeshift→A_FLAG marker BY NAME, with p0 = the 0-based stance id (form id − FORM_BATTLE) (lockstep with module taxonomy)
const E_SUMMON_PET: u8 = 0x15; // summon a persistent pet creature owned by the caster (Summon Imp); mapped from the raw vanilla Summon effect (56), with the misc_value (the summoned creature entry) routed into p0 (p0_kind = P_ENTRY) (lockstep with module taxonomy)
const E_HEAL_MAX_HEALTH: u8 = 0x16; // heal the target to FULL max health (Lay on Hands); mapped from the raw vanilla HealMaxHealth effect (67), split out of E_HEAL because its base_points is ~0 (the magnitude is "fill to max", not a flat N) (lockstep with module taxonomy)
const E_TAME_CREATURE: u8 = 0x20; // completed Hunter tame; raw vanilla TameCreature effect (55), lockstep with module taxonomy
const E_FEED_PET: u8 = 0x21; // feed a Hunter pet from the explicit item target; raw effect 101
const E_POWER_BURN: u8 = 0x19; // drain target mana into damage (Mana Burn); mapped from the raw vanilla PowerBurn effect (62), p1 = EffectMultipleValue*100 basis-points (lockstep with module taxonomy, work-items 117)
const E_BLINK: u8 = 0x1A; // teleport the caster ~20yd FORWARD along its facing (Mage Blink, 116); reclassified BY NAME from the dead SCRIPT teleport effect (lockstep with module taxonomy)
const E_PERSISTENT_AREA: u8 = 0x1B; // ground-AoE (118, Consecration): spawns a fixed-position game_ground_area whose tick damages hostiles inside; reclassified BY NAME from the ground A_PERIODIC_DAMAGE effect (lockstep with module taxonomy)
const E_OPEN_LOCK: u8 = 0x1D; // OPEN LOCK (Pick Lock 1804, work-item 119): gateway-intercepted like E_FISH (0x1C)/E_ENCHANT_ITEM — a CMSG_CAST_SPELL for a spell carrying this kind routes to the `pick_lock` reducer (unlock a locked GameObject, gated on the caster's Lockpicking 633 skill). Mapped from the raw vanilla OpenLock (33) / OpenLockItem (59) effects (lockstep with module taxonomy). 0x1E is reserved for a future E_SUMMON_PORTAL — do NOT reuse.
const E_DUEL: u8 = 0x22; // Duel (raw effect 83): p0 is the duel-flag gameobject template entry
const E_DISENCHANT: u8 = 0x18; // DISENCHANT (real Disenchant 13262, work-item 282): gateway-intercepted, routed to the disenchant reducer by kind. Mapped from raw vanilla effect 99 (SPELL_EFFECT_DISENCHANT); no params (the module validates + yields dust by item). Lockstep with the module taxonomy (module/src/spell/taxonomy.rs E_DISENCHANT).
const E_DISMOUNT: u8 = 0x23; // remove the target's active land mount (Dazed's mount-removal half): translated from a raw DISPEL_MECHANIC effect (108) whose misc value names the mount mechanic (21) — see `dismount_effect_kind` below. No params. Lockstep with module taxonomy.
const E_SUMMON_HOSTILE: u8 = 0x24; // temporary ownerless summon; p0 = creature entry, p1 = required spell focus, header duration = lifetime

// cmangos/classic-db cd0c426a3b2ff56dd518bf009025299468e60fdb:
// spell_template 7728/8674 send events 1131/1134; dbscripts_on_event summons
// 5676/5677 for 360000 ms. Their creature_ai_scripts set faction 14 and attack the summoner.
fn binding_summon_entry(spell_id: u32, effect: i32, event: u32, focus: u32) -> Option<i32> {
    match (spell_id, effect, event, focus) {
        (7728, 61, 1131, 83) => Some(5676),
        (8674, 61, 1134, 83) => Some(5677),
        _ => None,
    }
}

// aura effects (high bit set)
const A_PERIODIC_DAMAGE: u8 = 0x90;
const A_PERIODIC_HEAL: u8 = 0x91;
const A_PERIODIC_ENERGIZE: u8 = 0x92;
const A_MOD_STAT: u8 = 0xA0;
const A_MOD_STAT_PCT: u8 = 0xAA; // PERCENT stat mod (The Human Spirit +5% Spirit) — folded as a multiplier
const A_MOD_RESISTANCE: u8 = 0xA1;
const A_ABSORB: u8 = 0xA2;
const A_MOD_COMBAT: u8 = 0xA3;
const A_MOD_SPEED: u8 = 0xA4;
const A_MOD_HEALTH_POWER: u8 = 0xA5;
const A_MOD_DAMAGE_TAKEN: u8 = 0xA6;
const A_SEAL: u8 = 0xA7; // proc-on-swing holy seal (Seal of Righteousness); reclassified from A_FLAG
const A_STEALTH: u8 = 0xA8; // stealth presence marker (Stealth); reclassified from A_FLAG
                            // A Proc that starts a Triggered Cast of its own trigger spell (vanilla Spell.dbc aura 42,
                            // ProcTriggerSpell). Native mapping — no name rescue (lockstep with module taxonomy, which renamed
                            // this wire value from A_PROC_ON_HIT).
const A_PROC_TRIGGER: u8 = 0xAB;
const A_SPELLMOD_FLAT: u8 = 0xAC; // spell modifier FLAT (DBC aura 107 AddFlatModifier — 264): p0 = SpellModOp, p1 = affected-spell family mask
const A_SPELLMOD_PCT: u8 = 0xAD; // spell modifier PERCENT (DBC aura 108 AddPctModifier)
const A_DISARM: u8 = 0xAE; // Warrior Disarm (DBC AuraMod 67 ModDisarm): strips the enemy's main-hand weapon (lockstep with module taxonomy)
const A_RETALIATE: u8 = 0xAF; // Warrior Retaliation self-buff: free counter-swing at any melee attacker; reclassified from the inert A_FLAG marker BY NAME (lockstep with module taxonomy)
const A_CONTROL: u8 = 0xB0;
const A_IMMUNITY: u8 = 0xB1;
const A_MOD_DETECT_RANGE: u8 = 0xB2; // Priest Mind Soothe (DBC AuraMod 91 ModDetectRange): reduces a creature's aggro/detection radius by `amount` yards (lockstep with module taxonomy)
const A_FLAG: u8 = 0xBE;
const A_COMBAT_HEALTH_REGEN_PCT: u8 = 0xA9; // X% of normal health regen continues during combat (lockstep with module taxonomy)
const A_MOUNTED: u8 = 0xB3; // land mount, the state of record (vanilla AuraMod Mounted, 78): p0 = the resolved creature DISPLAY id (p0_kind = P_DISPLAY_ID), frozen at import (lockstep with module taxonomy)
                            // A Proc that deals its frozen amount as damage of the proc spell's own school (vanilla Spell.dbc
                            // aura 43, ProcTriggerDamage). p0 = the header's school_mask, p0_kind = P_SCHOOL_MASK (lockstep with
                            // module taxonomy).
const A_PROC_DAMAGE: u8 = 0xB4;

// MECHANICS (p0 for A_CONTROL when p0_kind == P_MECHANIC)
const M_STUN: i32 = 1;
const M_ROOT: i32 = 2;
const M_FEAR: i32 = 3;
const M_POLY: i32 = 4;
const GOUGE_RANK_IDS: [u32; 5] = [1776, 1777, 8629, 11285, 11286];

// p0_kind tags
const P_NONE: u8 = 0;
const P_STAT_ID: u8 = 1;
const P_SCHOOL_MASK: u8 = 2;
const P_MECHANIC: u8 = 3;
const P_POWER_TYPE: u8 = 4;
const P_COMBAT_FIELD: u8 = 5;
const P_SPEED_KIND: u8 = 6;
const P_FLAG: u8 = 7;
const P_ITEM_ENTRY: u8 = 8;
const P_ENTRY: u8 = 9; // p0 is a game_creature_template entry
const P_SPELLMOD_OP: u8 = 11; // p0 is a SpellModOp (A_SPELLMOD_*)
const P_PCT_MAX_POWER: u8 = 12; // the effect's `amount` is a PERCENT of the caster's max power (Evocation); aura_apply converts it to an absolute per-tick (lockstep with module taxonomy)
const P_GAMEOBJECT_ENTRY: u8 = 13; // p0 is a game_gameobject_template entry (E_DUEL)
const P_DISPLAY_ID: u8 = 14; // p0 is a creature DISPLAY id (A_MOUNTED — the UNIT_FIELD_MOUNTDISPLAYID value)
const P_RAW: u8 = 255;

// TargetKind
const T_SELF: u8 = 0;
const T_TARGET_ENEMY: u8 = 1;
const T_TARGET_ALLY: u8 = 2;
const T_TARGET_ANY: u8 = 3;
const T_AREA_ENEMY: u8 = 4;
const T_AREA_ALLY: u8 = 5;
const T_SCRIPTED: u8 = 7;

// combat fields (p0 for A_MOD_COMBAT)
const COMBAT_ATTACK_POWER: i32 = 0;
const COMBAT_CRIT: i32 = 1;
const COMBAT_HIT: i32 = 2;
const COMBAT_DMG_DONE: i32 = 3;
// (COMBAT_HASTE = 4 is intentionally absent: haste/attack-speed auras route to A_MOD_SPEED with a
// swing/cast speed-kind, per the aura-map design — never A_MOD_COMBAT.)
const COMBAT_SPELL_POWER: i32 = 5;
// defender / threat fields (values lockstep with module/src/spell/taxonomy.rs 101-105)
const COMBAT_DODGE: i32 = 7;
const COMBAT_THREAT: i32 = 10;

// stat ids
const STAT_ALL: i32 = 0xFF;

// speed kinds (p0 for A_MOD_SPEED; signed PERCENT amount) — lockstep with module/src/spell/taxonomy.rs.
const SPEED_MOVE: i32 = 0;
const SPEED_MOUNTED: i32 = 3; // ModIncreaseMountedSpeed family; T4 folds this speed kind at the combat seam

// STANCE/FORM ids (vanilla SpellShapeshiftForm.dbc indices) — the value the ModShapeshift effect's
// `effect_misc_value` carries (so a stance/form spell arrives as A_FLAG with p0 = the form id). Our
// engine uses its OWN small 0-based stance id space; `form_to_stance` below is THE one form→stance
// mapping (work-item 156 widened it past the Warrior trio to the Druid combat forms), consumed by BOTH
// the E_SET_STANCE p0 remap (`stance_p0`) and the `Stances` usability-mask fold (`translate_stance_mask`)
// so the two can never drift. The Spell.dbc `Stances` usability mask is a form-BIT mask
// (`1 << (formId-1)`: bit0=Cat/bit4=Bear/bit7=DireBear/bit16=Battle/bit17=Defensive/bit18=Berserker);
// each mapped form bit folds onto our stance bit (`1 << stance`) for the header `stances` column.
// The full convention (LOCKSTEP with module/src/spell/taxonomy.rs STANCE_* — that comment block is the
// definition site):
//   form 17 Battle    → stance 0        form  5 Bear      → stance 3
//   form 18 Defensive → stance 1        form  1 Cat       → stance 4
//   form 19 Berserker → stance 2        form  8 Dire Bear → stance 5
// Unmapped forms (Aquatic 4 / Travel 3 / Tree 2 / Ghoul 7 / Moonkin 31 / …) stay out of scope: their
// mask bits are DROPPED (a spell usable ONLY in an unmapped form imports with the mapped-form bits it
// has, or 0 = "any stance" — the pre-156 behavior for every non-warrior form).
const FORM_CAT: i32 = 1;
const FORM_BEAR: i32 = 5;
const FORM_DIRE_BEAR: i32 = 8;
const FORM_BATTLE: i32 = 17;
const FORM_DEFENSIVE: i32 = 18;
const FORM_BERSERKER: i32 = 19;

/// THE one vanilla ShapeshiftForm id → our 0-based stance id mapping (see the FORM_* block above; the
/// module's `taxonomy.rs` STANCE_* consts are the lockstep definition site). `None` = an out-of-scope
/// form (Aquatic/Travel/Tree/…) — dropped from usability masks, never produced by the E_SET_STANCE
/// name rescue. Pure. [import]
fn form_to_stance(form_id: i32) -> Option<i32> {
    match form_id {
        FORM_BATTLE => Some(0),
        FORM_DEFENSIVE => Some(1),
        FORM_BERSERKER => Some(2),
        FORM_BEAR => Some(3),
        FORM_CAT => Some(4),
        FORM_DIRE_BEAR => Some(5),
        _ => None,
    }
}

// Rogue cast-gate flags — OUR OWN bits in the DEDICATED game_spell.cast_flags column, set BY NAME below
// (lockstep with module/src/spell/taxonomy.rs SPELL_ATTR_*). A separate column from the raw vanilla
// `attributes` (whose bits are densely used and would collide). The engine gates on these in
// resolve_cast_at.
const SPELL_ATTR_REQ_BEHIND: u32 = 0x0001; // must be cast from behind the target (Backstab)
const SPELL_ATTR_REQ_STEALTH: u32 = 0x0002; // must be cast while stealthed (Sap)
const SPELL_ATTR_STEALTH_SAFE: u32 = 0x0004; // casting it does NOT break stealth (Sap, Pick Pocket)
const SPELL_ATTR_FINISHER_DURATION: u32 = 0x0008; // combo-finisher whose aura DURATION scales with combo points (Slice and Dice)
const SPELL_ATTR_INCAP_OPENER: u32 = 0x0010; // Sap-shaped incapacitate opener: additionally require the target OUT of combat + HUMANOID (Sap). Split OUT of REQ_STEALTH so Garrote (a stealth opener on ANY type, usable in combat) carries REQ_STEALTH without these constraints
const SPELL_ATTR_REQ_OVERPOWER: u32 = 0x0020; // Overpower: castable only in the ~5s window after the caster's swing was DODGED (Tier 2b react window)
const SPELL_ATTR_REQ_REVENGE: u32 = 0x0040; // Revenge: castable only in the ~5s window after the caster DODGED/PARRIED/BLOCKED an incoming swing (Tier 2b react window)
const SPELL_ATTR_CHANNELED: u32 = 0x0080; // CHANNELED (Arcane Missiles): the cast ticks a per-tick effect over duration_ms and breaks on action — set from the DBC AttributesEx1 CHANNELED bit (0x44), by-NAME fallback. Drives the A_PERIODIC_TRIGGER reclassify below
const SPELL_ATTR_REQ_DAGGER: u32 = 0x0100; // Backstab: castable only with a DAGGER equipped in the main hand (lockstep with module/src/spell/taxonomy.rs)
const SPELL_ATTR_RANGED_AUTO_REPEAT: u32 = 0x0200; // Auto Shot / wand Shoot: an AUTO-REPEAT ranged attack, not a one-shot cast. The GATEWAY reads this bit (game_spell.cast_flags) to intercept CMSG_CAST_SPELL and arm the ranged swing loop, instead of a hardcoded `spell == 75 || 5019` id list (work-item 097). Set from the DBC AttributesEx2 AUTOREPEAT bit, by-NAME fallback.

// vanilla `AttributesEx1` CHANNELED bits — IS_CHANNELLED (0x04) | CHANNELED_2 (0x40). cmangos: channeled = AttributesEx1 & 0x44.
const ATTR_EX1_CHANNELED_MASK: u32 = 0x0000_0044;
const ATTR_EX1_EXCLUDE_CASTER: u32 = 0x0008_0000;
// vanilla `AttributesEx2` SPELL_ATTR_EX2_AUTOREPEAT_FLAG (0x20) — marks auto-repeat ranged attacks (Auto Shot, wand Shoot).
const ATTR_EX2_AUTOREPEAT: u32 = 0x0000_0020;

// the A_PERIODIC_TRIGGER aura kind (lockstep with module/src/spell/taxonomy.rs) — a CHANNEL's per-tick trigger.
const A_PERIODIC_TRIGGER: u8 = 0x93;

// vanilla SpellEffect numeric IDs that PLACE an aura → branch to the AuraMod map.
const EFFECT_APPLY_AURA: i32 = 6;
const EFFECT_PERSISTENT_AREA_AURA: i32 = 27;
const EFFECT_APPLY_AREA_AURA_PARTY: i32 = 35;
const EFFECT_APPLY_AREA_AURA_PET: i32 = 119;

/// True when this raw SpellEffect id places an aura (its taxonomy KIND comes from `effect_aura[i]`).
fn is_aura_effect(effect_id: i32) -> bool {
    matches!(
        effect_id,
        EFFECT_APPLY_AURA
            | EFFECT_PERSISTENT_AREA_AURA
            | EFFECT_APPLY_AREA_AURA_PARTY
            | EFFECT_APPLY_AREA_AURA_PET
    )
}

// raw vanilla SPELL_EFFECT_DISPEL_MECHANIC (108) — an INSTANT effect whose misc value names a Mechanic
// to strip; `dismount_effect_kind` below reclassifies the ONE parameter this codebase supports (the
// mount mechanic, 21) to E_DISMOUNT. Every other mechanic parameter is left unmapped by design.
const EFFECT_DISPEL_MECHANIC: i32 = 108;
const MECHANIC_MOUNT: u32 = 21;

/// Translate a raw `DISPEL_MECHANIC` effect (108) whose misc value names the MOUNT mechanic (21) into
/// `E_DISMOUNT` — the one data-driven reclassify the design allows: no generic mechanic-dispel module,
/// no branch on a spell id or name. ANY spell carrying this raw shape gets mount removal for free — the
/// synthetic Dazed spell (A_MOD_SPEED slow + this effect) is just the first curated consumer, not a
/// special case. Any OTHER `DISPEL_MECHANIC` parameter is left
/// unmapped (falls to `E_SCRIPTED`, counted in the existing unmapped-effect coverage) — deliberately
/// out of scope until a real gameplay need shows up in the data. Pure. [import]
fn dismount_effect_kind(effect_id: i32, misc: u32) -> Option<u8> {
    if effect_id == EFFECT_DISPEL_MECHANIC && misc == MECHANIC_MOUNT {
        Some(E_DISMOUNT)
    } else {
        None
    }
}

/// The vanilla DBC "flat amount" convention: `EffectBasePoints` stores the nominal integer amount
/// MINUS one (the same off-by-one every effect's `base_points` already corrects for at the call site).
/// Named here so the mounted-speed normalization (vanilla stores 59/99 for the nominal 60%/100% speed
/// bonus) is an explicit, tested contract rather than an incidental side effect of the generic `+1`.
/// Pure. [import]
fn dbc_flat_amount(raw: i32) -> i32 {
    raw + 1
}

/// Preserve the event mask and clamp chance and charges to the durable byte columns.
fn proc_header_fields(flags: u32, chance: u32, charges: u32) -> (u32, u8, u8) {
    (flags, chance.min(255) as u8, charges.min(255) as u8)
}

/// Translate vanilla damage and movement flags into the Module's compact mask.
fn aura_interrupt_bits(raw: u32) -> u16 {
    u16::from(raw & 0x2 != 0) | (u16::from(raw & 0x8 != 0) << 1)
}

/// Curated load-time flag correction (the Spell.sql analog for header flags): OR our OWN cast-gate bits
/// into `game_spell.attributes` BY NAME. This is the ONE place allowed to name spells; the engine then
/// gates generically on the bit, never on a spell id. Additive — returns only the bits to OR in.
///   - REQUIRES_BEHIND: Backstab — only castable from the target's rear hemisphere.
///   - REQUIRES_STEALTH: Sap — opener; caster must be stealthed (the engine also enforces out-of-combat +
///     humanoid for a REQ_STEALTH spell).
///   - STEALTH_SAFE: Sap + Pick Pocket — casting keeps the rogue stealthed (vanilla).
///   - REQ_DAGGER: Backstab — only castable with a dagger equipped in the main hand.
///
/// Keyed by NAME so BOTH ranks (the LearnSpell/combo wrapper AND the real spell) carry the flag; the gate
/// is harmless on a wrapper (the wrapper's triggered real spell re-runs resolve_cast_at and is gated there).
fn spell_flag_attributes(name: &str) -> u32 {
    let mut bits = 0u32;
    // REQUIRES_BEHIND — Backstab is the must; Garrote/Ambush are the other vanilla behind-only openers.
    if matches!(name, "Backstab" | "Garrote" | "Ambush") {
        bits |= SPELL_ATTR_REQ_BEHIND;
    }
    // REQ_DAGGER — Backstab requires a dagger equipped in the main hand (vanilla melee-weapon-type gate).
    if name == "Backstab" {
        bits |= SPELL_ATTR_REQ_DAGGER;
    }
    // REQUIRES_STEALTH — Sap + Garrote (stealth openers; the caster must be stealthed). Garrote ALSO
    // breaks stealth (NOT STEALTH_SAFE) and works on ANY creature type in or out of combat, so it gets
    // ONLY REQ_STEALTH — NOT the Sap-shaped out-of-combat/humanoid constraints (those ride INCAP_OPENER).
    if matches!(name, "Sap" | "Garrote") {
        bits |= SPELL_ATTR_REQ_STEALTH;
    }
    // INCAP_OPENER — Sap's additional constraints (target OUT of combat + HUMANOID). Split out of the
    // REQ_STEALTH bit so a non-Sap stealth opener (Garrote) does NOT inherit them.
    if name == "Sap" {
        bits |= SPELL_ATTR_INCAP_OPENER;
    }
    // STEALTH_SAFE — Sap + Pick Pocket keep the rogue stealthed when cast (Garrote is NOT here → it breaks
    // stealth via the cast-path break_stealth chokepoint, correctly revealing the rogue).
    if matches!(name, "Sap" | "Pick Pocket") {
        bits |= SPELL_ATTR_STEALTH_SAFE;
    }
    // FINISHER_DURATION — Slice and Dice: the haste aura's DURATION scales with the combo points spent.
    // Both ranks (the LearnSpell/combo wrapper 5175 AND the real 5171) carry the bit harmlessly — the
    // wrapper just triggers 5171, which re-runs the gated aura-apply.
    if name == "Slice and Dice" {
        bits |= SPELL_ATTR_FINISHER_DURATION;
    }
    // REQ_OVERPOWER — Overpower: only castable in the ~5s window after the caster's swing was DODGED (the
    // attack table stamps the window; the gate reads this bit). REQ_REVENGE — Revenge: only castable in the
    // ~5s window after the caster DODGED/PARRIED/BLOCKED an incoming swing. Both keyed BY NAME here (the one
    // place allowed to name spells); the engine gates generically on the bit, never on a spell id.
    if name == "Overpower" {
        bits |= SPELL_ATTR_REQ_OVERPOWER;
    }
    if name == "Revenge" {
        bits |= SPELL_ATTR_REQ_REVENGE;
    }
    bits
}

/// Is this a CHANNELED spell — the cast holds the caster + ticks a per-tick effect over `duration_ms`,
/// breaking on action (Arcane Missiles)? Keyed on the DBC `AttributesEx1` CHANNELED bit (`0x44`, the cmangos
/// test), with a by-NAME fallback for the curated kit in case the bit is not cleanly readable on this client
/// build. This is the ONE place allowed to name a spell; the engine then keys on the `SPELL_ATTR_CHANNELED`
/// bit / the `A_PERIODIC_TRIGGER` kind, never a spell id. Drives BOTH the header flag AND the effect
/// reclassify (the periodic-trigger effect → `A_PERIODIC_TRIGGER`) so the two stay in lockstep.
fn is_channeled(attributes_ex1: u32, name: &str) -> bool {
    attributes_ex1 & ATTR_EX1_CHANNELED_MASK != 0 || matches!(name, "Arcane Missiles" | "Evocation")
}

fn excludes_eventai_caster(attributes_ex1: u32) -> bool {
    attributes_ex1 & ATTR_EX1_EXCLUDE_CASTER != 0
}

/// Is this an AUTO-REPEAT ranged attack (Auto Shot 75 / wand Shoot 5019) — the client fires it on the
/// ranged-weapon timer until stopped, not as a one-shot cast? Keyed on the DBC `AttributesEx2` AUTOREPEAT
/// bit (`0x20`), with a by-NAME fallback for the two player abilities in case the bit is not cleanly
/// readable on this client build. The GATEWAY (never a spell-id list) then routes CMSG_CAST_SPELL on the
/// resulting `SPELL_ATTR_RANGED_AUTO_REPEAT` cast_flags bit (work-item 097).
fn is_ranged_auto_repeat(attributes_ex2: u32, name: &str) -> bool {
    attributes_ex2 & ATTR_EX2_AUTOREPEAT != 0 || matches!(name, "Auto Shot" | "Shoot")
}

/// An effect row with every column at its inert value, for the curated additions to fill in. Naming
/// only the columns that carry meaning is what makes each synthetic row readable.
fn blank_effect(spell_id: u32, effect_index: u8) -> SpellEffectRow {
    SpellEffectRow {
        spell_id,
        effect_index,
        kind: 0,
        base_points: 0,
        die_sides: 0,
        per_level: 0.0,
        period_ms: 0,
        target: T_SELF,
        radius_yd: 0.0,
        chain_targets: 0,
        trigger_spell: 0,
        effect_mechanic: 0,
        p0: 0,
        p0_kind: P_NONE,
        p1: 0,
        script_id: 0,
        enters_combat: false,
    }
}

/// Preserve the curated seal marker and its Judgement amount. The third client effect
/// is a Dummy whose magnitude names another spell, not the Module's flat damage amount.
fn synthetic_seal_effect(spell_id: u32, name: &str, used_index: u8) -> Option<SpellEffectRow> {
    if !(spell_id == 21082 && name == "Seal of the Crusader") {
        return None;
    }
    Some(SpellEffectRow {
        kind: A_SEAL,
        base_points: 20, // flat Judgement-of-the-Crusader holy burst; self aura, no p0
        ..blank_effect(spell_id, used_index)
    })
}

/// The shield's linked Weakened Soul debuff is a gameplay rule absent from its DBC row.
fn power_word_shield_p1_override(spell_id: u32, name: &str, kind: u8, p1: i32) -> i32 {
    if spell_id == 17 && name == "Power Word: Shield" && kind == A_ABSORB {
        return 6788; // Weakened Soul
    }
    p1
}

/// `p1` for an `E_POWER_BURN` effect (117, Mana Burn): the DBC `EffectMultipleValue` (a fraction, 0.5
/// for vanilla Mana Burn) carried onto the effect row as basis-points (0.5 -> 50), since `p1` is a
/// plain `i32`. The module's `mana_burn_damage` treats `<=0` as 100 (1:1) — so a spell whose
/// `EffectMultipleValue` genuinely reads 0 in the DBC (unauthored/placeholder) still deals full
/// drained-mana damage rather than silently zeroing out. Pure (unit-tested).
fn power_burn_ratio_bp(multiple: f32) -> i32 {
    (multiple * 100.0).round() as i32
}

/// Map a vanilla INSTANT SpellEffect numeric id → our KIND (effect-map design unit). Unmapped → the
/// graceful `E_SCRIPTED` no-op. IDs verified against `wow_world_base::vanilla::SpellEffect` (the
/// `from_int` table): SchoolDamage=2, Heal=10, HealMaxHealth=67, Energize=30, Dispel=38,
/// TriggerSpell=64, Threat=63/ThreatAll=91, weapon-damage family 17/58/121/31, Resurrect=18,
/// AddComboPoints=80, ResurrectNew=113, AttackMe=114 (work-item 101 — cross-checked against the
/// vendored `wow_world_base-0.3.0` vanilla/tbc/wrath `SpellEffect::from_int` tables, all three eras
/// agree on these four numeric ids).
/// CONFIRMED 2026-07-14 against the real client Spell.dbc (curated-kit dry-run + targeted --only):
/// Taunt 355 → E_TAUNT, Resurrection 2006 / Redemption 7328 → E_RESURRECT, Cheap Shot 1833 (no
/// name-rescue) → E_ADD_COMBO via this raw-80 arm — so all four raw ids are load-bearing, and the
/// full curated-kit histogram shows ZERO unmapped effects for the trivial set (114/80/18/113 absent
/// from cov.unmapped_effect; only the out-of-scope LearnSpell-36 / ScriptEffect-77 / Dummy-3 tail
/// remains E_SCRIPTED).
fn instant_effect_to_kind(effect_id: i32) -> u8 {
    match effect_id {
        2 | 17 | 58 | 121 | 31 => E_DAMAGE, // SchoolDamage + WeaponDamage(No)School / Normalized / Percent
        10 => E_HEAL,                       // Heal (flat amount = base_points)
        67 => E_HEAL_MAX_HEALTH, // HealMaxHealth (Lay on Hands) — fill to max, base_points ~0 so it can NOT be E_HEAL (which would heal 0)
        30 => E_ENERGIZE,        // Energize
        38 => E_DISPEL,          // Dispel
        64 => E_TRIGGER,         // TriggerSpell
        63 | 91 => E_TAUNT,      // Threat / ThreatAll
        114 => E_TAUNT, // AttackMe (work-item 101) — same force-aggro semantics as Threat/ThreatAll, just a distinct raw id; not known to occur in the curated 1-10 human kit (no Taunt/Mocking Blow/Challenging Shout id is in any IDS_* list), so this widens coverage for a FUTURE (non-Human or higher-level) import, not the curated kit today
        24 => E_CREATE_ITEM, // CreateItem (conjure / quest item) — p0 = item entry
        68 => E_INTERRUPT, // InterruptCast (Kick) — cancel the target's in-progress cast
        55 => E_TAME_CREATURE, // TameCreature — explicit wild target, no spell-id branch
        101 => E_FEED_PET, // FeedPet — explicit item target is routed by the gateway/manual cast seam
        56 => E_SUMMON_PET, // Summon (Summon Imp et al.) — p0 = the summoned creature entry (misc_value)
        62 => E_POWER_BURN, // PowerBurn (Priest Mana Burn) — p1 = EffectMultipleValue*100 (work-items 117)
        83 => E_DUEL,       // Duel — p0 carries the duel-flag gameobject entry
        33 | 59 => E_OPEN_LOCK, // OpenLock (33) / OpenLockItem (59) — Pick Lock (work-item 119): gateway-intercepted, routed to the pick_lock reducer by kind (Pick Lock 1804 carries the raw OpenLock effect; the item-lock variant 59 covers a lockpick-on-item spell)
        99 => E_DISENCHANT, // Disenchant (13262, work-item 282): gateway-intercepted, routed to the disenchant reducer by kind — the AUTOLEARN enchanting ability. Was falling through to E_SCRIPTED (a no-op).
        80 => E_ADD_COMBO, // AddComboPoints (work-item 101) — the curated Rogue generators (Sinister Strike/Backstab/Gouge/Garrote) carry the generic Dummy effect in-kit and are rescued BY NAME in correct_script_effect_kind below, not via this raw id, so this arm is currently unexercised by the curated kit but correct for any spell that DOES carry the raw AddComboPoints effect
        18 | 113 => E_RESURRECT, // Resurrect / ResurrectNew (work-item 101) — the curated kit's two resurrects (Priest Resurrection 2006, Paladin Redemption 7328) are ALSO rescued by name below; if either carries raw effect 18/113 in the real DBC (plausible — that is literally what the effect exists for) this arm now resolves them natively too, moving them out of `cov.unmapped_effect` even though the final kind was already E_RESURRECT via the name rescue either way — unverified without a client DBC dump, so [V]
        _ => E_SCRIPTED,         // remaining vanilla effects: queryable no-op
    }
}

/// Curated load-time correction — our `Spell.sql` analog. A few vanilla spells tag a generic effect as
/// SCRIPT_EFFECT(77) for client-display reasons, so the raw mapping above falls to E_SCRIPTED; mangos
/// reclassifies the same set via its `Spell.sql` fixes. Keyed by spell NAME — this is the ONE place
/// allowed to name spells, deliberately isolated from the engine — and applied ONLY to effects that fell
/// to E_SCRIPTED, so it never disturbs a correctly-mapped effect. Returns the corrected kind (or input).
fn correct_script_effect_kind(name: &str, kind: u8) -> u8 {
    // Reclassify the inert no-op residue — SCRIPT_EFFECT instants (E_SCRIPTED) AND inert MARKER auras
    // (A_FLAG) that are really a generic mechanic. Only the residue, so a correctly-mapped effect is
    // never disturbed.
    // Rogue FINISHER: a finisher's DAMAGE effect scales with + spends combo points (read at runtime; the
    // base/die are the PER-COMBO-POINT damage). Fires on the real E_DAMAGE effect, before the residue guard.
    if kind == E_DAMAGE && name == "Eviscerate" {
        return E_FINISHER_DAMAGE;
    }
    // On-next-swing QUEUE (Tier 2b): Heroic Strike / Cleave encode a normal weapon-damage effect (→ E_DAMAGE)
    // in the DBC, but vanilla QUEUES them onto the next melee swing rather than hitting instantly. Reclassify
    // that E_DAMAGE to E_NEXT_SWING (the cast charges rage + stamps next_swing_spell; the next swing adds the
    // base_points as bonus damage). Fires on the real weapon-damage effect, before the residue guard — the
    // Eviscerate→E_FINISHER_DAMAGE precedent. base_points (the flat bonus) carries through untouched.
    if kind == E_DAMAGE && matches!(name, "Heroic Strike" | "Cleave") {
        return E_NEXT_SWING;
    }
    // Feint (Rogue Slice 3): vanilla encodes it as the native Threat effect (→ E_TAUNT) with a NEGATIVE
    // base (−150) — i.e. a one-time threat DROP, not a taunt-yank. Reclassify the E_TAUNT effect to our
    // E_REDUCE_THREAT (the handler reduces the caster's CURRENT threat by |base_points|). Fires on the
    // real E_TAUNT effect, before the residue guard. DISTINCT from Fade (a COMBAT_THREAT percent).
    if kind == E_TAUNT && name == "Feint" {
        return E_REDUCE_THREAT;
    }
    if kind != E_SCRIPTED && kind != A_FLAG {
        return kind;
    }
    match name {
        "Holy Light" | "Flash of Light" => E_HEAL, // script effect IS a heal (base/die carry the amount);
        // Flash of Light (paladin fast heal, L20) is the exact Holy-Light case — a SCRIPT_EFFECT dummy
        // carrying the heal in base_points — and fell to E_SCRIPTED (a hollow no-op "heal") without this.
        "Life Tap" => E_CONVERT_RESOURCE,  // health -> mana, 1:1
        "Charge" => E_CHARGE, // the rush effect (vanilla effect 96) -> teleport-to-target
        "Blink" => E_BLINK, // Mage Blink (116): the dead SCRIPT teleport effect (raw 29) -> teleport the caster ~20yd FORWARD along its facing. Name-rescued like Charge (raw 29 is a generic teleport with per-spell destination rules; Blink's is "forward"). eff2 (root/snare A_IMMUNITY) already maps natively.
        "Seal of Righteousness" => A_SEAL, // the inert A_FLAG marker IS a proc-on-swing holy seal
        "Stealth" => A_STEALTH, // the inert A_FLAG marker IS the stealth presence (creatures skip it; broken on action)
        "Retaliation" => A_RETALIATE, // the inert A_FLAG marker IS the free-counter-swing self-buff (any melee attacker gets swung back at)
        // The stance/form-switch spells encode their stance as APPLY_AURA + ModShapeshift(form) → the
        // inert A_FLAG marker (the form id in p0). Reclassify to E_SET_STANCE; the effect loop then
        // remaps p0 from the form id to our 0-based stance id via `form_to_stance`. The cast handler
        // writes WorldEntity.stance. This is the ONE place allowed to name the stance spells. Work-item
        // 156 added the Druid combat forms (Bear 5487 → stance 3, Cat 768 → 4, Dire Bear 9634 → 5) —
        // rank-less spells, so exact-name matching covers them; the non-combat forms (Aquatic/Travel/
        // Moonkin/Tree of Life) stay un-rescued (their marker stays the inert A_FLAG, the pre-156 shape).
        "Battle Stance" | "Defensive Stance" | "Berserker Stance" | "Bear Form"
        | "Dire Bear Form" | "Cat Form" => E_SET_STANCE,
        "Judgement" => E_JUDGEMENT, // unleash the active seal
        // Rogue GENERATORS: the inert combo-point Dummy effect (-> E_SCRIPTED) builds a combo point.
        // Garrote's eff2 is the same combo Dummy (its eff1 is the A_PERIODIC_DAMAGE bleed, unchanged).
        // (work-item 101: `instant_effect_to_kind` now maps the raw AddComboPoints effect id (80) to
        // E_ADD_COMBO natively too, but the in-kit generators carry the generic Dummy effect, not raw
        // 80, so this name rescue is still the ONLY path that reaches them — kept, not redundant here.)
        "Sinister Strike" | "Backstab" | "Gouge" | "Garrote" => E_ADD_COMBO,
        // Every class's combat-rez shares the same raw Resurrect effect (18/113) that falls to
        // E_SCRIPTED without this rescue — and E_SCRIPTED is a graceful-success no-op, so a missed
        // name here makes the cast "succeed" while reviving nobody (the 176 review caught
        // Redemption doing exactly that: the healer bot would tunnel a broken rez forever).
        // (work-item 101: `instant_effect_to_kind` now maps raw Resurrect/ResurrectNew (18/113) to
        // E_RESURRECT natively; per THIS comment's own prior claim that these spells "share the same
        // raw Resurrect effect (18/113)", the raw-id arm likely already resolves Resurrection/Redemption
        // before this rescue ever runs (the `kind != E_SCRIPTED` guard above short-circuits), making this
        // arm belt-and-braces for those two — still load-bearing for Rebirth/Ancestral Spirit, which
        // aren't Human-class spells and never ship through this curated kit. Harmless overlap either way;
        // left in place. [V] — unconfirmed without a client DBC dump.)
        "Resurrection" | "Redemption" | "Rebirth" | "Ancestral Spirit" => E_RESURRECT,
        "Pick Pocket" => E_PICKPOCKET, // the script effect grants creature copper without engaging
        // (Feint is handled above — its native Threat effect maps to E_TAUNT, not the E_SCRIPTED residue.)
        // Create Healthstone (all ranks): vanilla encodes these as SCRIPT_EFFECT (raw effect 77) with the
        // created item hardcoded in the mangos script (effect_item_type is 0 in the DBC), so the native
        // CreateItem path can't reach them — they fall to E_SCRIPTED. Reclassify by name to E_CREATE_ITEM
        // (count=1 rides base_points); the item entry is injected by the kind+name p0 fixup at the call site
        // (mirrors the E_SET_STANCE p0-remap), since there is no effect_item_type to read. Conjure Water/Food
        // need NO arm here — they use raw effect 24 and already map natively to E_CREATE_ITEM.
        "Create Healthstone"
        | "Create Healthstone (Minor)"
        | "Create Healthstone (Lesser)"
        | "Create Healthstone (Greater)"
        | "Create Healthstone (Major)" => E_CREATE_ITEM,
        _ => kind,
    }
}

/// Translate the vanilla Spell.dbc `Stances` (ShapeshiftMask) bitmask → our 0-based stance usability bits.
/// Vanilla stores bit `1 << (formId-1)` for each allowed form; every form `form_to_stance` maps (the
/// Warrior trio AND, since work-item 156, the Druid combat forms Bear/Cat/Dire Bear) folds onto our bit
/// `1 << stance` — so a druid ability's Bear|DireBear requirement (vanilla 0x90) survives import as our
/// 0x28 instead of being dropped. Bits for UNMAPPED forms (Aquatic/Travel/Tree/Ghoul/…) are still
/// dropped, and a 0 mask stays 0 ("usable in any stance"). The result is a u8 (our six stance bits fit
/// bits 0..5). Pure. [import]
fn translate_stance_mask(vanilla_mask: u32) -> u8 {
    let mut out: u8 = 0;
    for bit in 0u32..32 {
        if vanilla_mask & (1u32 << bit) == 0 {
            continue;
        }
        if let Some(stance) = form_to_stance(bit as i32 + 1) {
            out |= 1u8 << stance; // stance ids are 0..5 — always within the u8
        }
    }
    out
}

/// Remap a resolved E_SET_STANCE effect's p0 from the vanilla form id (carried by the reclassified
/// ModShapeshift→A_FLAG marker) to our 0-based stance id via `form_to_stance` (Battle 17→0 /
/// Defensive 18→1 / Berserker 19→2 / Bear 5→3 / Cat 1→4 / Dire Bear 8→5), so the cast handler writes
/// the right WorldEntity.stance. Non-stance kinds pass p0 through unchanged. An E_SET_STANCE effect
/// whose form id is unmapped passes p0 through too — unreachable by construction (the name rescue in
/// `correct_script_effect_kind` is the ONLY E_SET_STANCE producer and only names mapped forms), and a
/// raw form id ≥ 8 is inert in the module anyway (`stance_allows` disallows stances ≥ 8 under any
/// non-zero mask; the stance folds ignore it). Pure. [import]
fn stance_p0(kind: u8, p0: i32) -> i32 {
    if kind == E_SET_STANCE {
        form_to_stance(p0).unwrap_or(p0)
    } else {
        p0
    }
}

/// Resolve an `A_MOUNTED` effect's `p0` to a creature DISPLAY id. `resolve_aura_params` already default
/// the reading to "misc value IS the display id" — the shape of every real classic mount spell (vanilla
/// stores a `CreatureDisplayInfo` id directly in `EffectMiscValue`). The rarer shape, where the DBC
/// names a mount CREATURE TEMPLATE instead, is resolved here through `creature_displays` (entry →
/// display id, sourced from the imported creature-template presentation data); an entry absent from the
/// map falls through to the direct-display reading unchanged. Non-A_MOUNTED kinds pass p0 through. Pure.
/// [import]
fn mount_display_p0(kind: u8, p0: i32, creature_displays: &BTreeMap<u32, u32>) -> i32 {
    if kind != A_MOUNTED {
        return p0;
    }
    creature_displays
        .get(&(p0 as u32))
        .map(|&display| display as i32)
        .unwrap_or(p0)
}

/// An `A_PROC_DAMAGE` effect's `p0` is the PROC SPELL'S OWN school mask (the header's `school_mask`)
/// — aura 43 carries no per-effect misc value to resolve it from, unlike every other school-typed
/// aura (`A_MOD_RESISTANCE`, `A_ABSORB`). Same kind-keyed p0 override shape as `mount_display_p0` /
/// `stance_p0`; a non-A_PROC_DAMAGE kind's (p0, p0_kind) passes through untouched. Pure. [import]
fn proc_damage_school_p0(kind: u8, p0: i32, p0_kind: u8, school_mask: u8) -> (i32, u8) {
    if kind == A_PROC_DAMAGE {
        (school_mask as i32, P_SCHOOL_MASK)
    } else {
        (p0, p0_kind)
    }
}

/// Map a wow_dbc `AuraMod` variant → our KIND (aura-map design unit). Unmapped → `E_SCRIPTED`. The
/// per-effect p0/p0_kind are resolved separately by `resolve_aura_params` (which re-reads the variant
/// for the combat-field / mechanic / speed-kind it implies).
fn aura_mod_to_kind(aura: AuraMod) -> u8 {
    use AuraMod::*;
    match aura {
        // periodic
        PeriodicDamage | PeriodicLeech | PeriodicDamagePercent => A_PERIODIC_DAMAGE,
        PeriodicHeal => A_PERIODIC_HEAL,
        PeriodicEnergize | PeriodicManaLeech | PowerBurnMana => A_PERIODIC_ENERGIZE,
        // A channel's per-tick trigger (PeriodicTriggerSpell) fires at cast time -> E_TRIGGER (the
        // channel reclassify below turns it into A_PERIODIC_TRIGGER when the header is channeled).
        // A Proc's trigger (aura 42 ProcTriggerSpell) and its damage twin (aura 43 ProcTriggerDamage)
        // map natively onto the two proc kinds — no name rescue.
        PeriodicTriggerSpell => E_TRIGGER,
        ProcTriggerSpell => A_PROC_TRIGGER,
        ProcTriggerDamage => A_PROC_DAMAGE,

        // stat — FLAT (ModStat) vs PERCENT (ModPercentStat / ModTotalStatPercentage). The percent ones fold
        // as a multiplier in recompute_vitals (The Human Spirit = +5% Spirit), so they need a distinct kind.
        // Spell modifiers (264): the talent-passive class (Improved Fireball's cast-time cut,
        // fire-damage-% talents). Op rides EffectMiscValue -> p0 (P_SPELLMOD_OP); the affected-spell
        // family mask rides EffectItemType -> p1. The engine folds them at the cast-time/damage seams.
        AddFlatModifier => A_SPELLMOD_FLAT,
        AddPctModifier => A_SPELLMOD_PCT,

        ModStat => A_MOD_STAT,
        ModPercentStat | ModTotalStatPercentage => A_MOD_STAT_PCT,

        // resistance / armor
        ModResistance
        | ModBaseResistance
        | ModResistancePct
        | ModBaseResistancePct
        | ModResistanceOfStatPercent => A_MOD_RESISTANCE,

        // absorb
        SchoolAbsorb | DamageShield | ManaShield => A_ABSORB,

        // combat fields (AP / crit / hit / dmg-done / heal-power)
        ModAttackPower
        | ModRangedAttackPower
        | ModMeleeAttackPowerVersus
        | ModRangedAttackPowerVersus
        | ModAttackPowerPct
        | ModRangedAttackPowerPct => A_MOD_COMBAT,
        ModCritPercent | ModSpellCritChance | ModSpellCritChanceSchool | ModCritPercentVersus => {
            A_MOD_COMBAT
        }
        ModHitChance | ModSpellHitChance => A_MOD_COMBAT,
        ModDamageDone | ModDamagePercentDone | ModDamageDoneCreature | ModDamageDoneVersus => {
            A_MOD_COMBAT
        }
        ModHealing | ModHealingDone => A_MOD_COMBAT,
        // defender avoidance + threat: Evasion (+dodge) and Fade (-threat) reuse the EXISTING
        // COMBAT_DODGE / COMBAT_THREAT folds (effective_dodge_bp / threat::add_threat) — pull model,
        // no engine code. The field is resolved in resolve_aura_params below.
        ModDodgePercent | ModTotalThreat | ModThreat => A_MOD_COMBAT,

        // speed (move / swing / cast)
        ModIncreaseSpeed | ModDecreaseSpeed | ModSpeedAlways | ModSpeedNotStack => A_MOD_SPEED,
        ModIncreaseMountedSpeed | ModMountedSpeedAlways | ModMountedSpeedNotStack => A_MOD_SPEED,
        ModIncreaseSwimSpeed => A_MOD_SPEED,
        ModAttackspeed
        | ModMeleeHaste
        | ModRangedHaste
        | ModRangedAmmoHaste
        | ModCastingSpeedNotStack => A_MOD_SPEED,

        // incoming-damage modifier (defensive cooldowns like Shield Wall, or vulnerability debuffs) —
        // PERCENT only; the FLAT `ModDamageTaken` stays E_SCRIPTED until a flat handler exists.
        ModDamagePercentTaken => A_MOD_DAMAGE_TAKEN,

        // health/power pool
        ModIncreaseHealth | ModIncreaseEnergy | ObsModHealth | ObsModMana => A_MOD_HEALTH_POWER,

        // crowd control
        ModStun => A_CONTROL,
        ModRoot => A_CONTROL,
        ModFear => A_CONTROL,
        ModConfuse | ModCharm | ModPossess | ModPacify | ModPacifySilence | AoeCharm => A_CONTROL,

        // immunity
        SchoolImmunity | DamageImmunity | EffectImmunity | StateImmunity | MechanicImmunity
        | MechanicImmunityMask => A_IMMUNITY,

        // in-combat health regen percent: X% of normal health regen continues during combat.
        // `base_points` carries the percent (10 for Troll Regeneration racial). Reclassified from
        // the inert A_FLAG so the regen gate can read the magnitude without a spell-id or race check.
        ModRegenDuringCombat | ModHealthRegenInCombat => A_COMBAT_HEALTH_REGEN_PCT,

        // Demon Skin/Armor's health-per-5 (work-item 024): aura 84 SPELL_AURA_MOD_REGEN
        // is a COMBAT-INDEPENDENT periodic heal tick (it heals a living target on a fixed period
        // whether or not it is in combat) — the same primitive already
        // wired for Renew/bandages/food. Reclassified from the inert A_FLAG marker onto A_PERIODIC_HEAL;
        // `period_ms` is force-set to 5000 below — the observed vanilla cadence, which overrides
        // whatever EffectAmplitude the DBC carries.
        ModRegen => A_PERIODIC_HEAL,

        // Warrior Disarm (67): strips the enemy's main-hand weapon — the swing-range seam drops a disarmed
        // PLAYER to unarmed / scales a CREATURE's swing. A generic aura (natural expiry), no p0.
        ModDisarm => A_DISARM,
        // LAND MOUNT (78): the state of record for a player's mounted state — reclassified OUT of the
        // inert A_FLAG marker list below so `p0` carries a real resolved display id
        // instead of the raw misc value sitting dormant. See `resolve_aura_params`/`mount_display_p0`.
        Mounted => A_MOUNTED,
        // Priest Mind Soothe (91): reduces a creature's aggro/detection radius by `amount` yards while active.
        // The aggro pass subtracts the summed amount from the creature's aggro radius. A generic aura, no p0.
        ModDetectRange => A_MOD_DETECT_RANGE,

        // passive marker flags (no tick, no dispatch)
        Dummy
        | BindSight
        | ModTaunt
        | ModStealth
        | ModStealthDetect
        | ModInvisibility
        | ModInvisibilityDetection
        | ModShapeshift
        | DispelImmunity
        | TrackCreatures
        | TrackResources
        | Transform
        | ModScale
        | ModStalked
        | ModLanguage
        | FarSight
        | WaterBreathing
        | ModPowerRegen
        | ModHealthRegenPercent
        | ModPowerRegenPercent
        | PreventsFleeing
        | ModUnattackable
        | InterruptRegen
        | Ghost
        | AurasVisible
        | WaterWalk
        | FeatherFall
        | Hover
        | SharePetTracking
        | Untrackable
        | SafeFall
        | Persuaded
        | RetainComboPoints
        | ResistPushback
        | TrackStealthed
        | ModWaterBreathing
        | NoPvpCredit
        | ModAoeAvoidance
        | FeignDeath
        | ModSilence
        | SpiritOfRedemption
        | AllowChampionSpells
        | UseNormalMovementSpeed => A_FLAG,

        // everything else (no wired handler yet) → graceful no-op
        _ => E_SCRIPTED,
    }
}

/// Whether a per-effect `trigger_spell` names a castable RANK worth recording in `wrapper_to_rank`
/// (a genuine LearnSpell wrapper), as opposed to a payload that merely rides the same DBC column: a
/// channel's per-tick missile (A_PERIODIC_TRIGGER), either Proc kind's own effect data
/// (A_PROC_TRIGGER / A_PROC_DAMAGE), an inert marker (A_FLAG), or a plain instant trigger (E_TRIGGER,
/// e.g. Bloodrage's rage trickle). Mirrors `resolve_learn_target`'s own exclusion
/// (module/src/trainer.rs) — the two lists MUST stay in lockstep, so a rename or a new proc kind on
/// either side fails this test loud. Pure. [import]
fn is_wrapper_rank_trigger(kind: u8) -> bool {
    !matches!(
        kind,
        A_PERIODIC_TRIGGER | A_FLAG | A_PROC_TRIGGER | A_PROC_DAMAGE | E_TRIGGER
    )
}

/// Resolve the typed (p0, p0_kind) for an AURA effect from its KIND + AuraMod variant + the raw
/// `effect_misc_value`. The combat-field / mechanic / speed-kind / stat-id semantics come from the
/// VARIANT (not the raw misc value); school masks / power types / immunity ids come from misc value.
fn resolve_aura_params(kind: u8, aura: AuraMod, misc: u32) -> (i32, u8) {
    use AuraMod::*;
    match kind {
        // Flat AND percent stat mods resolve the stat the SAME way — from the effect's misc value. -1
        // (u32::MAX) = all stats (0xFF, e.g. Blessing of Kings/Mark of the Wild); a specific value = that
        // single stat (The Human Spirit's percent effect names Spirit, NOT all — the old blanket
        // ModTotalStatPercentage→STAT_ALL force was the bug that made it +5% to every stat). [104]
        A_MOD_STAT | A_MOD_STAT_PCT => {
            let p0 = if misc == u32::MAX {
                STAT_ALL
            } else {
                (misc & 0xFF) as i32
            };
            (p0, P_STAT_ID)
        }
        A_SPELLMOD_FLAT | A_SPELLMOD_PCT => (misc as i32, P_SPELLMOD_OP), // the SpellModOp (0=damage, 10=cast time, ...)
        A_MOD_RESISTANCE => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
        // damage-taken: capture the school mask (the runtime is all-school in v1, but keep the data).
        A_MOD_DAMAGE_TAKEN => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
        A_ABSORB => match aura {
            ManaShield => (0, P_NONE), // absorbs from the power pool, no school
            _ => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
        },
        A_MOD_COMBAT => {
            let field = match aura {
                ModDamageDone
                | ModDamagePercentDone
                | ModDamageDoneCreature
                | ModDamageDoneVersus => COMBAT_DMG_DONE,
                ModCritPercent
                | ModSpellCritChance
                | ModSpellCritChanceSchool
                | ModCritPercentVersus => COMBAT_CRIT,
                ModHitChance | ModSpellHitChance => COMBAT_HIT,
                ModHealing | ModHealingDone => COMBAT_SPELL_POWER,
                ModDodgePercent => COMBAT_DODGE, // Evasion — defender +dodge (scaled to bp below)
                // Fade (ModTotalThreat, signed % on ALL threat) + Righteous Fury (ModThreat, +60% →
                // 1.6× threat, the paladin tank aura) both fold onto the COMBAT_THREAT field the
                // threat::add_threat multiplier already reads — zero engine code.
                ModTotalThreat | ModThreat => COMBAT_THREAT,
                // remaining A_MOD_COMBAT auras are the attack-power family
                _ => COMBAT_ATTACK_POWER,
            };
            (field, P_COMBAT_FIELD)
        }
        A_MOD_SPEED => {
            // speed kind: 0=move, 1=swing, 2=cast, 3=mounted (derived from the variant). Mounted-speed
            // stays A_MOD_SPEED/SPEED_MOUNTED (not a second concept) — the DBC's own EffectBasePoints
            // for these auras is already vanilla's flat-amount −1 encoding (59/99), so the generic
            // `dbc_flat_amount` conversion applied to every effect's base_points normalizes them to the
            // nominal 60/100 with no extra code here.
            let kind_v = match aura {
                ModAttackspeed | ModMeleeHaste | ModRangedHaste | ModRangedAmmoHaste => 1,
                ModCastingSpeedNotStack => 2,
                ModIncreaseSwimSpeed => 2,
                ModIncreaseMountedSpeed | ModMountedSpeedAlways | ModMountedSpeedNotStack => {
                    SPEED_MOUNTED
                }
                _ => SPEED_MOVE,
            };
            (kind_v, P_SPEED_KIND)
        }
        // LAND MOUNT (vanilla AuraMod Mounted, 78): p0 defaults to the raw misc value AS the display
        // id — the shape of every real classic mount spell (EffectMiscValue names a CreatureDisplayInfo
        // id directly). `mount_display_p0` (below) additionally resolves the rarer case where the DBC
        // names a mount CREATURE TEMPLATE instead, through the imported creature-template presentation
        // data, so the frozen p0 is a display id in every case (the runtime never resolves anything).
        A_MOUNTED => (misc as i32, P_DISPLAY_ID),
        A_PERIODIC_ENERGIZE => ((misc & 0xFF) as i32, P_POWER_TYPE),
        A_CONTROL => {
            let mech = match aura {
                ModStun => M_STUN,
                ModRoot => M_ROOT,
                ModFear => M_FEAR,
                _ => M_POLY, // confuse / charm / possess / pacify(+silence) / aoe-charm
            };
            (mech, P_MECHANIC)
        }
        A_IMMUNITY => match aura {
            SchoolImmunity | DamageImmunity => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
            MechanicImmunity | MechanicImmunityMask => (misc as i32, P_MECHANIC),
            _ => (misc as i32, P_RAW), // effect/state immunity: opaque id
        },
        A_FLAG => ((misc & 0xFF) as i32, P_FLAG),
        E_TRIGGER => (0, P_RAW), // periodic/proc trigger: spell id lives in trigger_spell
        A_PROC_TRIGGER => (0, P_RAW), // same convention as E_TRIGGER: the trigger spell lives in trigger_spell
        // A_PROC_DAMAGE's real p0 is the header's own school_mask, not an effect misc value — the call
        // site overrides this placeholder once school_mask is in scope (mirrors the mount_display_p0 /
        // stance_p0 kind-keyed p0 fix-ups).
        A_PROC_DAMAGE => (0, P_NONE),
        E_SCRIPTED => (0, P_RAW),
        _ => (0, P_NONE),
    }
}

/// Polarity tripwire: an aura-placing effect that REDUCES a stat / resistance / combat field (negative
/// `base_points`) is a DEBUFF, so the spell must resolve onto an ENEMY, not an ally. The AuraMod variant
/// alone is ambiguous — the SAME `ModResistance`/`ModAttackPower`/`ModStat` places both buffs and debuffs;
/// only the sign of the magnitude distinguishes them. Reuses `aura_mod_to_kind` so it tracks the taxonomy
/// rather than duplicating the variant list. (Sunder Armor / Demoralizing Shout depend on this.)
fn is_reducing_modifier_aura(effect_id: i32, aura: AuraMod, base_points: i32) -> bool {
    is_aura_effect(effect_id)
        && base_points < 0
        && matches!(
            aura_mod_to_kind(aura),
            A_MOD_STAT | A_MOD_RESISTANCE | A_MOD_COMBAT
        )
}

/// Resolve the typed (p0, p0_kind) for an INSTANT effect from its KIND + raw `effect_misc_value`.
fn resolve_instant_params(kind: u8, misc: u32, item_type: i32) -> (i32, u8) {
    match kind {
        E_ENERGIZE => ((misc & 0xFF) as i32, P_POWER_TYPE),
        E_DISPEL => ((misc & 0x7F) as i32, P_SCHOOL_MASK), // dispel category (misnomer kept by design)
        E_CREATE_ITEM => (item_type, P_ITEM_ENTRY),
        // Summon names a creature template in EffectMiscValue.
        E_SUMMON_PET => (misc as i32, P_ENTRY),
        E_DUEL => (misc as i32, P_GAMEOBJECT_ENTRY),
        _ => (0, P_NONE), // damage/heal/trigger/taunt school is on the header; no p0
    }
}

/// Map unit selections from the pinned vanilla target definitions to Module targets.
/// Location targets use their companion selection in `resolve_effect_target`.
fn resolve_target(target: i32, is_negative: bool) -> u8 {
    match target {
        0 => T_SCRIPTED,
        1 => T_SELF,
        6 => T_TARGET_ENEMY,
        21 | 35 | 37 | 45 | 57 | 61 => T_TARGET_ALLY,
        25 => {
            if is_negative {
                T_TARGET_ENEMY
            } else {
                T_TARGET_ALLY
            }
        }
        15 | 16 | 28 => T_AREA_ENEMY,
        20 | 29 | 30 | 31 | 33 | 34 | 56 => T_AREA_ALLY,
        7 | 8 => {
            if is_negative {
                T_AREA_ENEMY
            } else {
                T_AREA_ALLY
            }
        }
        _ => T_SCRIPTED,
    }
}

// Pinned SpellTargetInfoTable entries whose first target supplies a location. If the companion
// target names a unit selection, that selection decides the Module's unit routing.
const LOCATION_TARGET_CODES: [i32; 19] = [
    9, 10, 17, 18, 22, 32, 39, 41, 42, 43, 44, 46, 47, 48, 49, 50, 53, 55, 63,
];

fn unit_selection_from_pair(target_a: i32, target_b: i32) -> i32 {
    if (target_a == 0 || LOCATION_TARGET_CODES.contains(&target_a)) && target_b != 0 {
        target_b
    } else {
        target_a
    }
}

const TARGET_FLAG_UNIT: u32 = 0x0002;
const TARGET_FLAG_UNIT_ENEMY: u32 = 0x0080;
const TARGET_FLAG_UNIT_ALLY: u32 = 0x0100;
const TARGET_FLAG_CORPSE_ENEMY: u32 = 0x0200;
const TARGET_FLAG_UNIT_DEAD: u32 = 0x0400;
const TARGET_FLAG_CORPSE_ALLY: u32 = 0x8000;

fn polarity_target(is_negative: bool) -> u8 {
    if is_negative {
        T_TARGET_ENEMY
    } else {
        T_TARGET_ALLY
    }
}

/// Resolve an explicit target shape from the Spell header when the effect has no implicit target.
/// Only flags accepted by the source effect type participate.
fn compatible_header_target(effect_id: i32, kind: u8, flags: u32, is_negative: bool) -> Option<u8> {
    if kind == E_SCRIPTED {
        return None;
    }
    let accepted = match effect_id {
        // Source effects whose required target type is Unit or UnitDest.
        1 | 2 | 5 | 6 | 8 | 9 | 10 | 11 | 16 | 17 | 19 | 24 | 29 | 30 | 31 | 34 | 36 | 38 | 40
        | 43 | 45 | 46 | 47 | 55 | 57 | 58 | 61 | 62 | 63 | 67 | 68 | 69 | 70 | 71 | 75 | 79
        | 80 | 82 | 83 | 85 | 92 | 95 | 96 | 98 | 100 | 102 | 103 | 108 | 111 | 114 | 115 | 117
        | 120 | 121 | 123 | 124 | 125 | 126 => {
            flags
                & (TARGET_FLAG_UNIT
                    | TARGET_FLAG_UNIT_ENEMY
                    | TARGET_FLAG_UNIT_ALLY
                    | TARGET_FLAG_UNIT_DEAD)
        }
        // Resurrection effects accept corpse flags rather than unit flags.
        18 | 113 | 116 => flags & (TARGET_FLAG_CORPSE_ENEMY | TARGET_FLAG_CORPSE_ALLY),
        _ => 0,
    };
    let enemy = accepted & (TARGET_FLAG_UNIT_ENEMY | TARGET_FLAG_CORPSE_ENEMY) != 0;
    let ally = accepted & (TARGET_FLAG_UNIT_ALLY | TARGET_FLAG_CORPSE_ALLY) != 0;
    match (enemy, ally) {
        (true, false) => Some(T_TARGET_ENEMY),
        (false, true) => Some(T_TARGET_ALLY),
        (true, true) => Some(T_TARGET_ANY),
        (false, false) if accepted != 0 => Some(polarity_target(is_negative)),
        (false, false) => None,
    }
}

/// Apply the pinned source's effect default after a `(TARGET_NONE, TARGET_NONE)` pair. Unsupported
/// effects retain the runtime fallback instead of acquiring a unit target from a foreign effect.
fn supported_effect_default(effect_id: i32, kind: u8, is_negative: bool) -> u8 {
    if kind == E_SCRIPTED {
        return T_SCRIPTED;
    }
    let default = match effect_id {
        2 => 6,                                              // enemy
        6 | 34 | 46 | 47 | 61 | 79 => 1,                     // caster
        11 | 18 | 24 | 36 | 44 | 85 | 116 | 118 | 123 => 25, // selected unit
        28 | 50 => 18,                                       // caster destination
        57 => 5,                                             // caster pet
        // Source area auras use the caster when neither implicit target names a single unit.
        35 | 119 => return T_SELF,
        _ => 0,
    };
    resolve_target(default, is_negative)
}

/// Coverage accounting accumulated across the whole import — drives the COVERAGE REPORT.
#[derive(Default)]
struct Coverage {
    spells: usize,
    effects: usize,
    real: usize,                  // effects mapped to a real (non-E_SCRIPTED) kind
    scripted: usize,              // effects that fell back to E_SCRIPTED
    by_kind: BTreeMap<u8, usize>, // histogram of emitted kinds
    unmapped_aura: BTreeMap<String, usize>, // top unmapped AuraMod variants
    unmapped_effect: BTreeMap<i32, usize>, // top unmapped instant SpellEffect ids
}

/// Derive the `game_spell` + `game_spell_effect` rows from the client DBC chain, plus human-readable
/// diagnostics for the dry-run. Returns (rows, coverage, diagnostics). IN-MEMORY only.
///
/// The rows are typed rather than pre-spelled SQL because two consumers need them: the reload
/// statements ([`assemble_spell_sql`]) and the Base Snapshot a Datascript reads
/// (`spell_snapshot::write_snapshot`). Deriving both from one row is what keeps them agreeing.
///
/// `only`: the additive allowlist. When EMPTY, the full wholesale path runs (clear ALL rows + reload
/// every spell). When NON-EMPTY, ONLY those spell ids are emitted and the DELETEs are SURGICAL (per-id),
/// so the import is additive + idempotent — it never touches the curated seed or the synthetic test
/// fixtures. The diagnostics then list EVERY allowlisted spell's full mapping (header + each effect,
/// including E_SCRIPTED no-ops) so the operator can see exactly how each ability resolved.
fn build_spell_rows(
    data_dir: &Path,
    only: &[u32],
    trainers: &[(u32, Vec<u32>)],
) -> Result<(SpellRows, Coverage, Vec<String>)> {
    derive_spell_rows(&open_spell_tables(data_dir)?, only, trainers)
}

fn derive_spell_rows(
    dbc: &SpellDbc,
    only: &[u32],
    trainers: &[(u32, Vec<u32>)],
) -> Result<(SpellRows, Coverage, Vec<String>)> {
    let allow: std::collections::HashSet<u32> = only.iter().copied().collect();
    let mut acc = SpellAccumulator::default();
    // Mount creature-template display resolution (entry → display id): the `--dbc --spells` CLI path
    // is DBC-only (no cmangos creature_template dump is loaded alongside it), so this is empty today —
    // every real classic mount spell resolves correctly regardless (see `mount_display_p0`), since its
    // EffectMiscValue already names a display id directly. Threading a real creature-entry source into
    // this map later is then a one-line change here, not a rewrite of the resolution logic itself.
    let creature_displays: BTreeMap<u32, u32> = BTreeMap::new();

    for s in &dbc.spells {
        let spell_id = s.id;
        if spell_id == 0 {
            continue; // the 0 placeholder
        }
        // Additive allowlist: when --only was given, emit ONLY those ids (every other spell is skipped
        // so the coverage report + emitted rows describe exactly the requested set).
        if !allow.is_empty() && !allow.contains(&spell_id) {
            continue;
        }

        // --- reagents (game_spell_reagent, work-item 282) ---
        // Spell.dbc carries up to 8 (Reagent, ReagentCount) pairs. Every real recipe's true mats
        // live here (and any reagent-consuming buff); the craft gate (module cast.rs) resolves by
        // this data instead of a hardcoded id list. Deterministic id (spell_id<<3)|slot →
        // idempotent clear+reload. Skip empty slots (reagent 0 or count 0).
        for (slot, (&item, &count)) in s.reagent.iter().zip(s.reagent_count.iter()).enumerate() {
            if item <= 0 || count == 0 {
                continue;
            }
            let id = ((spell_id as u64) << 3) | slot as u64;
            acc.reagent_rows
                .push(format!("({id},{spell_id},{item},{count})"));
        }

        let header = push_spell_header_row(s, dbc, &allow, &mut acc);
        push_spell_effect_rows(s, &header, dbc, &creature_displays, &allow, &mut acc);
    }

    let trainer_rows = trainer_offering_rows(trainers, &acc.spell_levels, &acc.wrapper_to_rank);

    Ok((
        SpellRows {
            headers: acc.spell_rows,
            effects: acc.effect_rows,
            fishing: fishing_marker_rows(),
            reagents: acc.reagent_rows,
            eventai_metadata: acc.eventai_metadata_rows,
            trainers: trainer_rows,
        },
        acc.cov,
        acc.samples,
    ))
}

/// The client tables the spell rows derive from, read once per import.
struct SpellDbc {
    spells: Vec<Spell>,
    cast_times: SpellCastTimes,
    ranges: SpellRange,
    durations: SpellDuration,
    radii: SpellRadius,
}

/// Open the operator's MPQ chain and read every table the spell import needs.
fn open_spell_tables(data_dir: &Path) -> Result<SpellDbc> {
    let mut chain = open_chain(data_dir)?;
    eprintln!("spells: opened MPQ chain from {}", data_dir.display());

    let spells = read_spells(&chain.read_file("DBFilesClient\\Spell.dbc")?)?;
    let cast_times: SpellCastTimes = read_table(&mut chain)?;
    let ranges: SpellRange = read_table(&mut chain)?;
    let durations: SpellDuration = read_table(&mut chain)?;
    let radii: SpellRadius = read_table(&mut chain)?;
    eprintln!(
        "spells: parsed Spell({}) + SpellCastTimes({}) + SpellRange({}) + SpellDuration({}) + SpellRadius({})",
        spells.len(),
        cast_times.rows().len(),
        ranges.rows().len(),
        durations.rows().len(),
        radii.rows().len(),
    );

    Ok(SpellDbc {
        spells,
        cast_times,
        ranges,
        durations,
        radii,
    })
}

/// The row sets and diagnostics one DBC sweep accumulates.
#[derive(Default)]
struct SpellAccumulator {
    cov: Coverage,
    spell_rows: Vec<SpellHeaderRow>,
    effect_rows: Vec<SpellEffectRow>,
    reagent_rows: Vec<String>,
    eventai_metadata_rows: Vec<String>,
    samples: Vec<String>,
    /// spell_id → its DBC-derived spell_level, for the trainer offerings' required_level.
    spell_levels: BTreeMap<u32, u8>,
    /// wrapper spell_id → the trigger RANK it teaches; the rank carries the real level and cost.
    wrapper_to_rank: BTreeMap<u32, u32>,
}

/// The header values the effect mapping below reads back off the spell it belongs to.
struct SpellHeader {
    name: String,
    school_mask: u8,
    is_negative: bool,
    channeled: bool,
}

/// One effect's resolved kind and magnitudes, after every curated reclassify and p0 fix-up.
struct ResolvedEffect {
    kind: u8,
    p0: i32,
    p0_kind: u8,
    base_points: i32,
    die_sides: i32,
    per_level: f32,
    period_ms: u32,
}

/// Derive one spell's `game_spell` header row and push it, returning the values the effect mapping
/// reads back.
fn push_spell_header_row(
    s: &Spell,
    dbc: &SpellDbc,
    allow: &std::collections::HashSet<u32>,
    acc: &mut SpellAccumulator,
) -> SpellHeader {
    let SpellAccumulator {
        cov,
        spell_rows,
        eventai_metadata_rows,
        samples,
        spell_levels,
        ..
    } = acc;
    let spell_id = s.id;
    let name = s.name.clone();
    let power_type = s.power_type as u8;
    let cost = s.mana_cost;
    let cast_time_ms = dbc
        .cast_times
        .get(SpellCastTimesKey::new(s.casting_time_index))
        .map(|r| r.base.max(0) as u32)
        .unwrap_or(0);
    // RecoveryTime is the spell-specific cooldown; CategoryRecoveryTime is the CATEGORY cooldown
    // (e.g. Hammer of Justice 853 = 60s, Divine Protection 498 = 5min). Both may carry the real
    // per-spell cooldown depending on how the DBC authored the spell, so take the max of both so
    // neither path is silently dropped.  gcd_ms is computed below after `attributes` is known.
    let cooldown_ms = s.recovery_time.max(s.category_recovery_time);
    let range_yd = dbc
        .ranges
        .get(SpellRangeKey::new(s.range))
        .map(|r| r.range_max.max(0.0) as u32)
        .unwrap_or(0);
    // DBC duration -1 means INFINITE (toggle auras like Devotion Aura 465, Battle Stance, etc.).
    // Collapse to u32::MAX as a sentinel; cast.rs converts that to Timestamp(i64::MAX) so the
    // reaper's `expires_at <= now` filter never matches (i64::MAX ≈ 9.2e18 µs >> 2026 epoch).
    // Any non-negative duration value is taken as-is (milliseconds). A missing DurationIndex
    // entry falls back to 0, which expires immediately (fine — those spells have no aura effect).
    let duration_ms = dbc
        .durations
        .get(SpellDurationKey::new(s.duration))
        .map(|r| {
            if r.duration == -1 {
                u32::MAX
            } else {
                r.duration.max(0) as u32
            }
        })
        .unwrap_or(0);
    // Spell.dbc `school` is a Resistances.dbc INDEX (0=phys,1=holy,2=fire,3=nature,4=frost,
    // 5=shadow,6=arcane), NOT a bitmask — our `school_mask` is the bitmask (1<<index, so phys=1,
    // fire=4, frost=16…). Convert; clamp the index to the 7 real schools so the shift can't overflow.
    let school_mask = 1u8 << (s.school.min(6) as u8);
    let dispel_type = s.dispel_type as u8;
    let mechanic = s.mechanic as u8;
    let real_stack = s.stack_amount;
    let max_stacks = if real_stack <= 1 {
        0u8
    } else {
        real_stack.min(255) as u8
    };
    let aura_interrupt = aura_interrupt_bits(s.aura_interrupt_flags);
    let attributes = s.attributes;
    // Passive and queued-swing spells do not start the global cooldown.
    let gcd_ms: u32 = if (attributes & 0x40) != 0 || (attributes & 0x4) != 0 {
        0
    } else {
        1500
    };
    let channeled = is_channeled(s.attributes_ex1, &name);
    if excludes_eventai_caster(s.attributes_ex1) {
        eventai_metadata_rows.push(format!("({spell_id},true)"));
    }
    // OUR OWN cast-gate flags (REQ_BEHIND / REQ_STEALTH / STEALTH_SAFE / CHANNELED …), set BY NAME or from
    // a dedicated DBC bit — emitted into the DEDICATED `cast_flags` column (NOT folded into `attributes`,
    // whose vanilla bits would collide).
    let mut cast_flags = spell_flag_attributes(&name);
    if channeled {
        cast_flags |= SPELL_ATTR_CHANNELED;
    }
    if is_ranged_auto_repeat(s.attributes_ex2, &name) {
        cast_flags |= SPELL_ATTR_RANGED_AUTO_REPEAT;
    }
    let stances = translate_stance_mask(s.shapeshift_mask);
    let spell_level = s.spell_level.min(255) as u8;
    spell_levels.insert(spell_id, spell_level); // for the trainer-offering required_level (firewall-clean)
    let max_level = s.max_level.min(255) as u8;
    let is_negative = spell_is_negative(s);

    let family_name = s.family_name as u8;
    let family_flags = s.family_flags;
    let (proc_flags, proc_chance, proc_charges) =
        proc_header_fields(s.proc_flags, s.proc_chance, s.proc_charges);
    spell_rows.push(SpellHeaderRow {
        spell_id,
        name: name.clone(),
        power_type,
        cost,
        cast_time_ms,
        gcd_ms,
        cooldown_ms,
        range_yd,
        duration_ms,
        school_mask,
        dispel_type,
        mechanic,
        max_stacks,
        aura_interrupt,
        attributes,
        spell_level,
        max_level,
        is_negative,
        cast_flags,
        stances,
        family_name,
        family_flags,
        proc_flags,
        proc_chance,
        proc_charges,
    });
    if binding_summon_entry(
        spell_id,
        s.effect[0],
        s.effect_misc_value[0],
        s.requires_spell_focus,
    )
    .is_some()
    {
        let row = spell_rows
            .last_mut()
            .expect("the spell header was just inserted");
        row.duration_ms = 360_000;
        row.cooldown_ms = 180_000;
    }
    cov.spells += 1;

    // Allowlist diagnostics: one header line per requested spell so the operator sees the full
    // resolved header (the per-effect lines follow in the effect loop below).
    if !allow.is_empty() {
        samples.push(format!(
                "spell {spell_id} '{title}': school_mask={school_mask} power_type={power_type} cost={cost} cast_ms={cast_time_ms} gcd_ms={gcd_ms} cd_ms={cooldown_ms} range={range_yd}yd dur_ms={duration_ms} spell_level={spell_level} max_stacks={max_stacks} negative={is_negative} attributes=0x{attributes:X} cast_flags=0x{cast_flags:X} stances=0x{stances:X} aura_interrupt=0x{aura_interrupt:X} proc_flags=0x{proc_flags:X} proc_chance={proc_chance} proc_charges={proc_charges}",
                title = name.chars().take(40).collect::<String>(),
            ));
    }

    SpellHeader {
        name,
        school_mask,
        is_negative,
        channeled,
    }
}

/// Whether a spell reads as a debuff. Vanilla has no buff/debuff flag, so derive it from the
/// effects: damage, a DoT, CC, or a stat-reducing modifier aura on a non-self target.
fn spell_is_negative(s: &Spell) -> bool {
    // A self-only penalty belongs to its buff, such as Stealth's movement penalty.
    let mut is_negative = false;
    for i in 0..3 {
        let eff = s.effect[i];
        if eff == 0 || (s.implicit_target_a[i] == 1 && s.implicit_target_b[i] == 0) {
            continue;
        }
        let aura = AuraMod::from_int(s.effect_aura[i]).unwrap_or_default();
        // damage / DoT / CC / a debuff-shaped aura on a non-self target ⇒ negative
        if matches!(eff, 2 | 17 | 58 | 121 | 31) {
            is_negative = true;
        }
        if matches!(
            aura,
            AuraMod::PeriodicDamage
                | AuraMod::PeriodicLeech
                | AuraMod::ModStun
                | AuraMod::ModRoot
                | AuraMod::ModFear
                | AuraMod::ModConfuse
                | AuraMod::ModDecreaseSpeed
                | AuraMod::ModSilence
                | AuraMod::ModDamageTaken
        ) {
            is_negative = true;
        }
        // A modifier aura that REDUCES a stat / resistance / combat field (negative base points) is a
        // DEBUFF — e.g. Sunder Armor (−armor via ModResistance), Demoralizing Shout (−AP via
        // ModAttackPower). The variant alone can't tell buff from debuff (the SAME AuraMod does both;
        // the sign of the magnitude decides), so the earlier variant-only list misses these and they'd
        // wrongly resolve onto an ALLY (see `is_reducing_modifier_aura`).
        if is_reducing_modifier_aura(eff, aura, s.effect_base_points[i]) {
            is_negative = true;
        }
    }
    is_negative
}

/// Derive effect rows and the curated seal marker from one normalized spell.
fn push_spell_effect_rows(
    s: &Spell,
    header: &SpellHeader,
    dbc: &SpellDbc,
    creature_displays: &BTreeMap<u32, u32>,
    allow: &std::collections::HashSet<u32>,
    acc: &mut SpellAccumulator,
) {
    let SpellAccumulator {
        cov,
        effect_rows,
        samples,
        wrapper_to_rank,
        ..
    } = acc;
    let spell_id = s.id;
    let name = header.name.as_str();
    // --- effects (game_spell_effect) ---
    // The curated seal marker takes the first unused slot within the packed 2-bit index.
    let mut used_slots = [false; 4];
    // `i` is the DBC EFFECT INDEX, not merely a position in `used_slots`: it addresses four
    // parallel DBC arrays (`effect`, `effect_aura`, `effect_mechanic`, `effect_item_type`) AND
    // is packed into the deterministic row id `(spell_id << 2) | index`. Iterating one of those
    // arrays instead (what `clippy::needless_range_loop` asks for) would hide that.
    #[allow(clippy::needless_range_loop)]
    for i in 0..3 {
        let effect_id = s.effect[i];
        if effect_id == 0 {
            continue; // sparse effect slots are common
        }
        used_slots[i] = true;
        let effect_index = i as u8;
        let ResolvedEffect {
            kind,
            p0,
            p0_kind,
            base_points,
            die_sides,
            per_level,
            period_ms,
        } = resolve_effect_kind(s, i, header, creature_displays, cov);
        let binding_entry = binding_summon_entry(
            spell_id,
            effect_id,
            s.effect_misc_value[i],
            s.requires_spell_focus,
        );
        let (kind, p0, p0_kind) = match binding_entry {
            Some(entry) => (E_SUMMON_HOSTILE, entry, P_ENTRY),
            None => (kind, p0, p0_kind),
        };
        let target = resolve_effect_target(s, i, kind, header);
        // Evocation's regen-percent aura needs the Module's periodic max-mana effect.
        let (kind, period_ms, base_points, target, p0, p0_kind) =
            if name == "Evocation" && effect_index == 0 {
                (
                    A_PERIODIC_ENERGIZE,
                    2000u32,
                    15i32,
                    T_SELF,
                    0i32,
                    P_PCT_MAX_POWER,
                )
            } else {
                (kind, period_ms, base_points, target, p0, p0_kind)
            };
        let radius_yd = if s.effect_radius[i] > 0 {
            dbc.radii
                .get(SpellRadiusKey::new(s.effect_radius[i]))
                .map(|r| r.radius)
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let chain_targets = s.effect_chain_target[i].clamp(0, 255) as u8;
        let trigger_spell = s.effect_trigger_spell[i];
        // Record wrapper -> rank: a LearnSpell wrapper's first nonzero trigger is the castable rank it
        // teaches (resolves the wrapper's trainer-offering level/cost to the rank, below).
        // `is_wrapper_rank_trigger` excludes every OTHER kind that rides the same `trigger_spell`
        // column for a non-rank payload (a channel's missile, a Proc's trigger, a plain instant
        // trigger) — see its own doc comment. This list must stay in lockstep with
        // `resolve_learn_target` (module/src/trainer.rs).
        if trigger_spell != 0 && is_wrapper_rank_trigger(kind) {
            wrapper_to_rank.entry(spell_id).or_insert(trigger_spell);
        }
        let effect_mechanic = s.effect_mechanic[i] as u8;
        let p1 = if kind == E_SUMMON_HOSTILE {
            s.requires_spell_focus as i32
        } else if kind == E_POWER_BURN {
            power_burn_ratio_bp(s.effect_multiple_values[i])
        } else if kind == A_SPELLMOD_FLAT || kind == A_SPELLMOD_PCT {
            // 264: the affected-spell FAMILY MASK (DBC EffectItemType) — matched at fold time
            // against the cast header's family_flags.
            s.effect_item_type[i]
        } else {
            power_word_shield_p1_override(spell_id, name, kind, 0i32)
        };
        let script_id = 0u32;
        // [093] data-driven "this energize enters/holds combat": set on Bloodrage (cast 2687 + trickle
        // 29131, BOTH named "Bloodrage") so the E_ENERGIZE / A_PERIODIC_ENERGIZE arms read the flag, not
        // a spell id. Any energize can opt in by adding the name.
        let enters_combat = name == "Bloodrage";

        effect_rows.push(SpellEffectRow {
            spell_id,
            effect_index,
            kind,
            base_points,
            die_sides,
            per_level,
            period_ms,
            target,
            radius_yd,
            chain_targets,
            trigger_spell,
            effect_mechanic,
            p0,
            p0_kind,
            p1,
            script_id,
            enters_combat,
        });

        cov.effects += 1;
        *cov.by_kind.entry(kind).or_default() += 1;
        if kind == E_SCRIPTED {
            cov.scripted += 1;
        } else {
            cov.real += 1;
        }

        if !allow.is_empty() {
            // Allowlist mode: log EVERY effect (incl. E_SCRIPTED no-ops) so an unmapped ability is
            // visible, not silently dropped — the operator decides whether it's usable as-is.
            samples.push(format!(
                    "    eff{effect_index}: {kn} (0x{kind:02X}) base={base_points} die={die_sides} period_ms={period_ms} target={target} p0={p0} p0_kind={p0_kind} trigger={trigger_spell}",
                    kn = kind_name(kind),
                ));
        } else if kind != E_SCRIPTED && samples.len() < 8 {
            // Full-import mode: keep a few human-readable samples for the dry-run (first 8 REAL effects).
            samples.push(format!(
                    "  spell {spell_id} '{}' eff{effect_index}: kind=0x{kind:02X} base={base_points} period_ms={period_ms} target={target} p0={p0} p0_kind={p0_kind}",
                    name.chars().take(28).collect::<String>(),
                ));
        }
    }

    // The curated seal marker uses the first slot after the client's populated effects.
    let synth_slot = used_slots.iter().position(|&u| !u).unwrap_or(3) as u8;
    if let Some(synth) = synthetic_seal_effect(spell_id, name, synth_slot) {
        cov.effects += 1;
        cov.real += 1;
        *cov.by_kind.entry(synth.kind).or_default() += 1;
        if !allow.is_empty() {
            samples.push(format!("    [synthetic+curated] {}", synth.sql_values()));
        }
        effect_rows.push(synth);
    }
}

/// Resolve one DBC effect slot to our kind + magnitudes: the aura/instant mapping, then every
/// curated reclassify and p0 fix-up that rides on it.
fn resolve_effect_kind(
    s: &Spell,
    i: usize,
    header: &SpellHeader,
    creature_displays: &BTreeMap<u32, u32>,
    cov: &mut Coverage,
) -> ResolvedEffect {
    let effect_id = s.effect[i];
    let aura = AuraMod::from_int(s.effect_aura[i]).unwrap_or_default();
    let name = header.name.as_str();
    let school_mask = header.school_mask;
    let channeled = header.channeled;
    let (kind, (p0, p0_kind)) = if is_aura_effect(effect_id) || s.effect_aura[i] != 0 {
        let k = aura_mod_to_kind(aura);
        if k == E_SCRIPTED {
            *cov.unmapped_aura
                .entry(format!("{} ({aura:?})", s.effect_aura[i]))
                .or_default() += 1;
        }
        (k, resolve_aura_params(k, aura, s.effect_misc_value[i]))
    } else {
        let k = instant_effect_to_kind(effect_id);
        if k == E_SCRIPTED {
            *cov.unmapped_effect.entry(effect_id).or_default() += 1;
        }
        (
            k,
            resolve_instant_params(k, s.effect_misc_value[i], s.effect_item_type[i]),
        )
    };
    // Curated correction (Spell.sql analog): reclassify the known script-effect-as-generic spells.
    let kind = if name == "Stealth" && aura != AuraMod::ModStealth {
        kind
    } else {
        correct_script_effect_kind(name, kind)
    };
    // Every build 5875 Gouge rank uses the incapacitate family, although its client aura is ModStun.
    let p0 = if GOUGE_RANK_IDS.contains(&s.id) && name == "Gouge" && kind == A_CONTROL {
        M_POLY
    } else {
        p0
    };

    // DISMOUNT reclassify (data-driven, never by name/id): a raw DISPEL_MECHANIC effect whose
    // misc value names the mount mechanic becomes E_DISMOUNT. Runs on the RAW effect id (not
    // the resolved kind), so it fires regardless of what instant_effect_to_kind mapped 108 to.
    let kind = dismount_effect_kind(effect_id, s.effect_misc_value[i]).unwrap_or(kind);

    // STANCE p0 remap: a reclassified E_SET_STANCE effect arrives with p0 = the vanilla form id
    // (from the ModShapeshift misc value). Remap it to our 0-based stance id via `form_to_stance`
    // (Battle 17→0 / Defensive 18→1 / Berserker 19→2 / Bear 5→3 / Cat 1→4 / Dire Bear 8→5) so the
    // cast handler writes the right WorldEntity.stance. Mirrors the COMBAT_DODGE ×100 base-point
    // rescale below — a kind-keyed p0 fix-up after the reclassify, never a spell id. Non-stance
    // effects keep their resolved p0.
    let p0 = stance_p0(kind, p0);

    // MOUNT DISPLAY resolution: an A_MOUNTED effect's p0 already defaults to the raw misc value
    // (resolve_aura_params) — resolve the rarer creature-template indirection here, mirroring
    // the stance/CREATE_ITEM kind-keyed p0 fix-ups above.
    let p0 = mount_display_p0(kind, p0, creature_displays);

    // PROC_DAMAGE school: an A_PROC_DAMAGE effect deals its frozen amount as damage of the
    // proc spell's own school — mirrors the mount/stance p0 fix-ups above.
    let (p0, p0_kind) = proc_damage_school_p0(kind, p0, p0_kind, school_mask);

    // CREATE_ITEM p0 injection (Healthstone): the by-name reclassify above made these E_CREATE_ITEM,
    // but their DBC effect_item_type is 0 (the mangos script hardcodes the item), so the resolved p0
    // is 0 (and p0_kind is P_NONE, since the kind was E_SCRIPTED when resolve_instant_params ran).
    // Inject the per-rank Healthstone item template entry + P_ITEM_ENTRY here — the same kind-keyed,
    // name-keyed p0 fix-up as the E_SET_STANCE remap above, never disturbing a real effect_item_type
    // p0 (the arm only fires for these names, whose native p0 is already 0). count=1 rides base_points.
    // The module's E_CREATE_ITEM handler reads only p0; the p0_kind set is for data parity with
    // natively-mapped Conjure Water/Food (which carry P_ITEM_ENTRY).
    let (p0, p0_kind) = if kind == E_CREATE_ITEM {
        let item = match name {
            "Create Healthstone (Minor)" => Some(5512),
            "Create Healthstone (Lesser)" => Some(5511),
            "Create Healthstone" => Some(5509),
            "Create Healthstone (Greater)" => Some(5510),
            "Create Healthstone (Major)" => Some(9421),
            _ => None, // a native CreateItem (Conjure Water/Food) keeps its effect_item_type (p0, p0_kind)
        };
        match item {
            Some(entry) => (entry, P_ITEM_ENTRY),
            None => (p0, p0_kind),
        }
    } else {
        (p0, p0_kind)
    };

    let base_points = dbc_flat_amount(s.effect_base_points[i]); // DBC +1 convention (mounted-speed 59/99 → 60/100 falls out of this for free)
                                                                // Avoidance-chance combat fields are basis-points in our engine but PERCENT in the DBC, so
                                                                // scale ×100 (Evasion's +50% dodge → +5000 bp). COMBAT_THREAT is a signed percent in both
                                                                // (the threat fold divides by 100), so it is NOT scaled.
    let base_points = if kind == A_MOD_COMBAT && p0 == COMBAT_DODGE {
        base_points * 100
    } else {
        base_points
    };
    let die_sides = s.effect_die_sides[i];
    let per_level = s.effect_real_points_per_level[i];
    let period_ms = s.effect_amplitude[i];
    // Vanilla ModRegen ticks every five seconds even when its amplitude is zero.
    let period_ms = if kind == A_PERIODIC_HEAL && aura == AuraMod::ModRegen {
        5000
    } else {
        period_ms
    };
    // CHANNEL reclassify: on a channeled spell, the periodic-trigger effect (a PeriodicTriggerSpell /
    // raw TriggerSpell mapped to E_TRIGGER, with a tick period) IS the channel tick — reclassify it to
    // A_PERIODIC_TRIGGER so `tick_auras` fires its `trigger_spell` (the missile) each period at the
    // channel target. Gated on the channeled header bit + a real period, so a non-channel periodic
    // trigger (a proc) is untouched. Keyed on the kind + period, never a spell id; the channel target
    // + the trigger spell id are frozen onto the aura at cast (aura_apply). [import]
    let kind = if channeled && kind == E_TRIGGER && period_ms > 0 {
        A_PERIODIC_TRIGGER
    } else {
        kind
    };
    // GROUND-AoE (118): a ground-persistent A_PERIODIC_DAMAGE is a FIXED-POSITION area, not a unit
    // DoT. The DBC encodes it as A_PERIODIC_DAMAGE with a dynobj/self target that resolves WRONG —
    // Consecration → T_SELF would DoT the paladin himself. Reclassify BY NAME to E_PERSISTENT_AREA
    // (the Charge/Blink name-rescue precedent) so it spawns a game_ground_area whose own
    // tick_ground_areas damages hostiles inside. Consecration is caster-anchored; Flamestrike (262)
    // is the first CLICKED-GROUND one — the 118 phase-2 dest plumbing (6067df1) anchors the area at
    // the click when the cast carries a DEST_LOCATION block, so the same kind serves both.
    // Blizzard/Rain of Fire remain un-rescued (channeled patches — their channel/tick interplay is
    // its own follow-up; leaving them A_PERIODIC_DAMAGE keeps them out of the curated kit).
    let kind = if kind == A_PERIODIC_DAMAGE && matches!(name, "Consecration" | "Flamestrike") {
        E_PERSISTENT_AREA
    } else {
        kind
    };
    ResolvedEffect {
        kind,
        p0,
        p0_kind,
        base_points,
        die_sides,
        per_level,
        period_ms,
    }
}

/// Resolve the DBC target pair, then apply the existing caster and ground-handler conventions.
fn resolve_effect_target(s: &Spell, i: usize, kind: u8, header: &SpellHeader) -> u8 {
    let name = header.name.as_str();
    let is_negative = header.is_negative;
    let channeled = header.channeled;
    let target_a = s.implicit_target_a[i];
    let target_b = s.implicit_target_b[i];
    let target = if target_a == 0 && target_b == 0 {
        compatible_header_target(s.effect[i], kind, s.targets, is_negative)
            .unwrap_or_else(|| supported_effect_default(s.effect[i], kind, is_negative))
    } else {
        let selection = unit_selection_from_pair(target_a, target_b);
        resolve_target(selection, is_negative)
    };
    // These handlers operate on the caster or a ground location, regardless of unit selection.
    let target = match kind {
        E_CHARGE | E_JUDGEMENT | E_PICKPOCKET | E_INTERRUPT | E_NEXT_SWING | E_TAUNT
        | E_TAME_CREATURE => T_TARGET_ENEMY,
        E_RESURRECT => T_TARGET_ALLY,
        E_REDUCE_THREAT | E_BLINK | E_PERSISTENT_AREA | E_SUMMON_PET | E_SUMMON_HOSTILE => T_SELF,
        E_DUEL => T_TARGET_ANY,
        _ => target,
    };
    // Slice and Dice's selected-unit marker permits spending combo points on the enemy.
    let target = if name == "Slice and Dice" && target == T_TARGET_ALLY {
        T_TARGET_ENEMY
    } else {
        target
    };
    // Flamestrike's impact uses the clicked ground location supplied with the cast.
    let target = if name == "Flamestrike" && kind == E_DAMAGE {
        T_AREA_ENEMY
    } else {
        target
    };
    // CHANNEL self-marker: a channeled spell carries an inert A_FLAG "you are channeling" marker
    // (Arcane Missiles eff2) that the DBC reads as ALLY-targeted — but the channel is cast AT an
    // ENEMY, so an ally-typed effect would make the faction gate REJECT the enemy cast (the same
    // trap fixed for Slice and Dice / Thunder Clap). Force the marker to T_SELF: it's a caster-side
    // flag, so self-targeting it both lands it correctly AND removes it from the faction gate's
    // hits_ally/hits_enemy scan (a self-cast effect imposes no faction constraint). Generic over the
    // channeled bit + the A_FLAG kind, never a spell id; the channel's A_PERIODIC_TRIGGER tick effect
    // is already T_SELF, so only the stray marker is corrected.
    if channeled && kind == A_FLAG {
        T_SELF
    } else {
        target
    }
}

/// The `game_trainer_spell` rows for each `--trainer <entry>=<ids>` binding.
fn trainer_offering_rows(
    trainers: &[(u32, Vec<u32>)],
    spell_levels: &BTreeMap<u32, u8>,
    wrapper_to_rank: &BTreeMap<u32, u32>,
) -> Vec<String> {
    // --- trainer offerings (game_trainer_spell) — FIREWALL-CLEAN ---
    // For each `--trainer <entry>=<ids>` binding, emit one row per offered spell. required_level comes
    // straight from the DBC `spell_level` captured above; cost from our own level-keyed formula
    // (trainer_cost) — NO cmangos npc_trainer value is touched. A wrapper whose header we just imported
    // (so its trigger rank is also imported) resolves to a real castable rank at buy time. The `#[auto_inc]`
    // PK lets us emit 0 for the id (SpacetimeDB assigns it); the logical key is (trainer_entry, spell_id).
    let mut trainer_rows: Vec<String> = Vec::new();
    let mut trainer_missing: Vec<u32> = Vec::new();
    for (entry, ids) in trainers {
        // The IDS_* lists carry BOTH a LearnSpell wrapper AND its rank; offer each spell ONCE. Drop any
        // offered id that is the trigger-rank of another offered wrapper (it is taught via that wrapper) —
        // so the trainer list shows one row per learnable spell, not the wrapper + rank duplicate.
        let offered_triggers: std::collections::HashSet<u32> = ids
            .iter()
            .filter_map(|id| wrapper_to_rank.get(id).copied())
            .collect();
        for &spell_id in ids {
            if offered_triggers.contains(&spell_id) {
                continue; // taught via its wrapper (also offered) — skip the bare rank
            }
            // required_level + cost come from the RANK the player actually learns — a LearnSpell wrapper's
            // own spell_level is 0 (the level lives on its trigger rank), so without this the trainer
            // level-gate would pass at level 1 for a level-N spell. Resolve wrapper -> rank first; the
            // OFFERING id stays the wrapper (the buy path resolves it to the rank), only level/cost use the rank.
            let rank = wrapper_to_rank.get(&spell_id).copied().unwrap_or(spell_id);
            match spell_levels.get(&rank) {
                Some(&lvl) => {
                    let cost = trainer_cost(lvl);
                    // id=0 → the table's #[auto_inc] PK assigns the real id on insert. END-append
                    // learn_skill_line=0, learn_skill_cap=75 (professions slices 3 + rank/cap): every
                    // class-spell offering is a normal spell row (the unchanged spell path); profession
                    // offerings are parent-SQL-seeded. The cap is unused on a line-0 row but must be NAMED
                    // on the INSERT; 75 matches the module's #[default(75u32)].
                    // 259: EXPLICIT reserved ids (not the auto_inc 0 sentinel). Two reasons: the
                    // --dump ETL writes trainer rows with explicit ids WITHOUT advancing the
                    // sequence (an id-0 insert could collide — the errno-12 class, danger-zones §2),
                    // and fixed ids let the reload below delete ONLY this pass's own rows instead of
                    // wiping the dump-imported class trees wholesale.
                    let id = CURATED_TRAINER_ID_BASE + trainer_rows.len() as u64;
                    trainer_rows.push(format!("({id},{entry},{spell_id},{cost},{lvl},0,75)"));
                }
                // An offered id with no imported header would emit required_level 0 / charge-but-uncastable,
                // so SKIP it and surface it — the operator either adds it to the import or drops the offering.
                None => trainer_missing.push(spell_id),
            }
        }
    }
    if !trainer_missing.is_empty() {
        trainer_missing.sort_unstable();
        trainer_missing.dedup();
        eprintln!(
            "spells: WARNING — {} --trainer offering id(s) skipped (not offered): the RANK each resolves to has no imported header — either the offering itself is unimported, or its wrapper resolution points at an unimported trigger (add the missing id to --only, or drop the offering): {:?}",
            trainer_missing.len(),
            trainer_missing,
        );
    }

    trainer_rows
}

/// Fishing (060): the E_FISH marker effect rows for the three tier ids (skill 356 — 7620/7731/7732,
/// PROFESSION_LEARN's fishing entry). The gateway routes CMSG_CAST_SPELL to the `fish` reducer by
/// this KIND (the enchant-route pattern — never a spell-id list). These are OUR OWN taxonomy rows
/// (not client data, firewall-clean), written by key AFTER the bulk insert so they replace whatever
/// the DBC mapped at the same effect slot.
fn fishing_marker_rows() -> Vec<SpellEffectRow> {
    const E_FISH: u8 = 0x1C; // lockstep with module taxonomy
    [7620u32, 7731, 7732]
        .into_iter()
        .map(|spell_id| SpellEffectRow {
            kind: E_FISH,
            p0_kind: 255,
            ..blank_effect(spell_id, 0)
        })
        .collect()
}

/// The (clear+)reload statements for one derived pass (both tables have NO Timestamp column → plain
/// SQL). Pure: the same rows always assemble the same statements, so this is testable without a
/// client.
///
/// `curated_trainers_requested` is the operator's `--trainer` intent, not the row count: an
/// offering list that resolved to nothing still has to clear the reserved span, or a shrunken list
/// leaves stale rows behind.
fn assemble_spell_sql(
    rows: &SpellRows,
    only: &[u32],
    curated_trainers_requested: bool,
) -> Vec<String> {
    let mut stmts: Vec<String> = spell_delete_statements(only);
    push_insert(
        &mut stmts,
        "game_spell",
        "spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges",
        &rows.headers.iter().map(SpellHeaderRow::sql_values).collect::<Vec<_>>(),
    );
    push_insert(
        &mut stmts,
        "game_spell_effect",
        "id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat",
        &rows.effects.iter().map(SpellEffectRow::sql_values).collect::<Vec<_>>(),
    );
    // Reagents (282): same clear-guard as the spell/effect tables (spell_delete_statements only
    // wipes < SYNTHETIC_SPELL_ID_FLOOR, so hand/test-fixture reagents survive a curated reload).
    push_insert(
        &mut stmts,
        "game_spell_reagent",
        "id,spell_id,item_entry,count",
        &rows.reagents,
    );
    push_insert(
        &mut stmts,
        "game_creature_ai_spell_metadata",
        "spell_id,exclude_caster",
        &rows.eventai_metadata,
    );

    // Deterministic-id delete+insert = idempotent on every curated re-run.
    for row in &rows.fishing {
        stmts.push(format!(
            "DELETE FROM game_spell_effect WHERE id = {}",
            row.row_id()
        ));
        stmts.push(format!(
            "INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES {}",
            row.sql_values()
        ));
    }

    // Trainer offerings (259 INVERSION): the --dump ETL's npc_trainer import is now the PRIMARY
    // source of class offerings (full per-class trees, the dump's real costs/reqlevels — an
    // operator-local .import read like every other --dump field); this curated pass is the
    // OVERRIDE layer for the "specials" cmangos delivers outside npc_trainer (Consecration,
    // Flamestrike, Kick, stances, …). It therefore reloads ONLY ITS OWN reserved-id rows
    // (delete-by-id over the reserved span) and never touches the dump rows or the profession
    // learn-rows. The old wholesale `DELETE WHERE learn_skill_line = 0` wiped the dump's ~4200
    // class rows and replaced them with the curated handful — exactly what 259 removes.
    if curated_trainers_requested {
        for n in 0..CURATED_TRAINER_ID_SPAN {
            stmts.push(format!(
                "DELETE FROM game_trainer_spell WHERE id = {}",
                CURATED_TRAINER_ID_BASE + n
            ));
        }
        push_insert(
            &mut stmts,
            "game_trainer_spell",
            "id,trainer_entry,spell_id,cost,required_level,learn_skill_line,learn_skill_cap",
            &rows.trainers,
        );
    }

    stmts
}

/// The (clear+)reload SQL plus the dry-run diagnostics: the shape every existing caller expects.
fn build_spell_sql(
    data_dir: &Path,
    only: &[u32],
    trainers: &[(u32, Vec<u32>)],
) -> Result<(Vec<String>, Coverage, Vec<String>)> {
    let (rows, cov, samples) = build_spell_rows(data_dir, only, trainers)?;
    let sql = assemble_spell_sql(&rows, only, !trainers.is_empty());
    Ok((sql, cov, samples))
}

/// Reserved `game_trainer_spell` id range for the CURATED override rows (259): far above the --dump
/// ETL's explicit ids (~4300) and disjoint from every other reserved range. The reload deletes this
/// whole span by id, so a shrunken override list leaves no stale rows behind.
const CURATED_TRAINER_ID_BASE: u64 = 5_200_000;
const CURATED_TRAINER_ID_SPAN: u64 = 500;

/// Floor of the synthetic/test fixture spell-id range seeded by `module/src/seed.rs` (Combat Insight
/// 50000, Minor Healing 50110) and `module/src/seed/fixtures.rs` (Test PW:Shield 50072, Test
/// Regeneration 50137) — NONE of these are re-created by any import script, so a wholesale
/// `game_spell` / `game_spell_effect` clear must never touch `spell_id >= SYNTHETIC_SPELL_ID_FLOOR`.
/// The ONE reducer-seeded spell BELOW the floor is Weakened Soul (6788, real vanilla id) — the
/// wholesale clear DOES wipe the seeded row, and the full import re-creates 6788 from its OWN real DBC
/// entry, which carries a genuine `A_IMMUNITY` (mechanic-shield) aura effect (confirmed for work-item
/// 122), so the reload is self-healing with no synthetic needed. The module side has no equivalent named constant (each fixture
/// hardcodes its own literal id); if one is ever added there, mirror the name
/// `SYNTHETIC_SPELL_ID_FLOOR` and keep both floors equal — a one-way hand-sync until then.
const SYNTHETIC_SPELL_ID_FLOOR: u32 = 50_000;

/// The DELETE half of the (clear+)reload, split out so the surgical-vs-wholesale choice is testable.
/// EMPTY `only` → the WHOLESALE clear (full import, replaces every row). NON-EMPTY → a SURGICAL per-id
/// DELETE so the import is ADDITIVE + idempotent and never touches the curated seed / test fixtures.
/// Per-id `WHERE x = N` (sorted, deterministic) avoids relying on SQL `IN(..)` support and keeps each
/// statement planner-friendly; the allowlist is always a small curated kit.
///
/// The wholesale branch guards `spell_id < SYNTHETIC_SPELL_ID_FLOOR` on BOTH tables so the synthetic
/// fixture spells (50000/50072/50110/50137, none re-created by any import script) survive a full-DBC
/// reload. `game_spell_effect` is filtered on its own `spell_id` COLUMN (btree `by_spell`,
/// `module/src/spell/tables.rs:65`) — NOT the packed `id` (`spell_id<<2|effect_index`) — a range filter
/// on the packed id would not track the fixture floor at all. Per danger-zones.md §2, a single-column
/// range filter on `spacetime sql` can wrongly return 0 rows; this guard NEEDS LIVE VERIFICATION (the
/// work-item's runbook) — it has NOT been run against a real node from this sandbox. Bounded blast
/// radius if it no-ops: `spell_id` is the PK, so the reload's INSERTs collide loudly instead of
/// corrupting silently.
fn spell_delete_statements(only: &[u32]) -> Vec<String> {
    if only.is_empty() {
        return vec![
            format!("DELETE FROM game_spell WHERE spell_id < {SYNTHETIC_SPELL_ID_FLOOR}"),
            format!("DELETE FROM game_spell_effect WHERE spell_id < {SYNTHETIC_SPELL_ID_FLOOR}"),
            // Reagents (282) share the fixture-floor guard so hand/test reagents survive a reload.
            format!("DELETE FROM game_spell_reagent WHERE spell_id < {SYNTHETIC_SPELL_ID_FLOOR}"),
            format!("DELETE FROM game_creature_ai_spell_metadata WHERE spell_id < {SYNTHETIC_SPELL_ID_FLOOR}"),
        ];
    }
    let mut ids: Vec<u32> = only.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut dels = Vec::with_capacity(ids.len() * 4);
    for id in ids {
        dels.push(format!("DELETE FROM game_spell WHERE spell_id = {id}"));
        dels.push(format!(
            "DELETE FROM game_spell_effect WHERE spell_id = {id}"
        ));
        dels.push(format!(
            "DELETE FROM game_spell_reagent WHERE spell_id = {id}"
        ));
        dels.push(format!(
            "DELETE FROM game_creature_ai_spell_metadata WHERE spell_id = {id}"
        ));
    }
    dels
}

/// Trainer-offering COST in copper, derived PURELY from the spell's DBC `required_level` — NO cmangos
/// npc_trainer value is read (the firewall-clean replacement for the cmangos cost column). 50 copper per
/// required level keeps the early Elwynn 1–10 abilities near-free (a level-1 ability costs 50c = half a
/// silver; a level-10 rank costs 5 silver), monotone-rising and the right rough shape for vanilla. One
/// line to tune. `required_level` 0 (the DBC "trainable at the trainer's floor" ranks) clamps to 1 so
/// nothing is free.
fn trainer_cost(required_level: u8) -> u32 {
    (required_level.max(1) as u32) * 50
}

/// Human-readable name of a KIND tag (for the coverage histogram).
fn kind_name(kind: u8) -> &'static str {
    match kind {
        E_DAMAGE => "E_DAMAGE",
        E_HEAL => "E_HEAL",
        E_ENERGIZE => "E_ENERGIZE",
        E_DISPEL => "E_DISPEL",
        E_TRIGGER => "E_TRIGGER",
        E_TAUNT => "E_TAUNT",
        E_CREATE_ITEM => "E_CREATE_ITEM",
        E_CHARGE => "E_CHARGE",
        E_BLINK => "E_BLINK",
        E_PERSISTENT_AREA => "E_PERSISTENT_AREA",
        E_OPEN_LOCK => "E_OPEN_LOCK",
        E_CONVERT_RESOURCE => "E_CONVERT_RESOURCE",
        E_JUDGEMENT => "E_JUDGEMENT",
        E_ADD_COMBO => "E_ADD_COMBO",
        E_FINISHER_DAMAGE => "E_FINISHER_DAMAGE",
        E_RESURRECT => "E_RESURRECT", // was missing (work-item 101) — the histogram printed "?" for this kind even though it's been a real, dispatched kind since the by-name rescue landed
        E_PICKPOCKET => "E_PICKPOCKET",
        E_INTERRUPT => "E_INTERRUPT",
        E_REDUCE_THREAT => "E_REDUCE_THREAT",
        E_NEXT_SWING => "E_NEXT_SWING",
        E_SET_STANCE => "E_SET_STANCE",
        E_SUMMON_PET => "E_SUMMON_PET",
        E_SUMMON_HOSTILE => "E_SUMMON_HOSTILE",
        E_HEAL_MAX_HEALTH => "E_HEAL_MAX_HEALTH",
        E_TAME_CREATURE => "E_TAME_CREATURE",
        E_FEED_PET => "E_FEED_PET",
        E_POWER_BURN => "E_POWER_BURN",
        E_DUEL => "E_DUEL",
        E_SCRIPTED => "E_SCRIPTED",
        A_PERIODIC_DAMAGE => "A_PERIODIC_DAMAGE",
        A_PERIODIC_HEAL => "A_PERIODIC_HEAL",
        A_PERIODIC_ENERGIZE => "A_PERIODIC_ENERGIZE",
        A_PERIODIC_TRIGGER => "A_PERIODIC_TRIGGER",
        A_MOD_STAT => "A_MOD_STAT",
        A_MOD_RESISTANCE => "A_MOD_RESISTANCE",
        A_ABSORB => "A_ABSORB",
        A_MOD_COMBAT => "A_MOD_COMBAT",
        A_MOD_SPEED => "A_MOD_SPEED",
        A_MOD_HEALTH_POWER => "A_MOD_HEALTH_POWER",
        A_MOD_DAMAGE_TAKEN => "A_MOD_DAMAGE_TAKEN",
        A_CONTROL => "A_CONTROL",
        A_IMMUNITY => "A_IMMUNITY",
        A_FLAG => "A_FLAG",
        A_PROC_TRIGGER => "A_PROC_TRIGGER",
        A_PROC_DAMAGE => "A_PROC_DAMAGE",
        _ => "?",
    }
}

/// Print the COVERAGE REPORT — the key deliverable: how many effect rows mapped to a REAL kind vs the
/// `E_SCRIPTED` no-op, the % real coverage, the kind histogram, and the top unmapped aura/effect types.
fn print_coverage(cov: &Coverage) {
    let pct = |n: usize, d: usize| {
        if d == 0 {
            0.0
        } else {
            100.0 * n as f64 / d as f64
        }
    };
    println!("\n=== SPELL TAXONOMY COVERAGE REPORT ===");
    // BY DESIGN (work-item 100): a full (non --only) import pulls in the raid/other-class/PvP long
    // tail, which has no Rust kind mapping yet (that's work-item 101) — so E_SCRIPTED% balloons on a
    // full run vs. the curated kit. That's the intended end state, NOT a regression to "fix" here.
    println!("Total spells imported: {}", cov.spells);
    println!("Total effect rows:     {}", cov.effects);
    println!(
        "  Real kinds (mapped): {} ({:.1}%)",
        cov.real,
        pct(cov.real, cov.effects)
    );
    println!(
        "  E_SCRIPTED (no-op):  {} ({:.1}%)",
        cov.scripted,
        pct(cov.scripted, cov.effects)
    );
    // Procs woke up: every effect that landed natively on the two Proc kinds this ticket added (aura
    // 42/43, no name rescue). An Operator reviews this count after an import to see what changed.
    let proc_trigger = cov.by_kind.get(&A_PROC_TRIGGER).copied().unwrap_or(0);
    let proc_damage = cov.by_kind.get(&A_PROC_DAMAGE).copied().unwrap_or(0);
    println!(
        "  proc auras: {} (42: {proc_trigger}, 43: {proc_damage})",
        proc_trigger + proc_damage
    );

    println!("\nBreakdown by kind (descending):");
    let mut kinds: Vec<(u8, usize)> = cov.by_kind.iter().map(|(&k, &n)| (k, n)).collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, n) in kinds {
        println!(
            "  0x{k:02X} {:<20} {:>6} ({:.1}%)",
            kind_name(k),
            n,
            pct(n, cov.effects)
        );
    }

    println!("\nTop 15 unmapped AuraMod variants (→ E_SCRIPTED):");
    let mut auras: Vec<(&String, usize)> = cov.unmapped_aura.iter().map(|(k, &n)| (k, n)).collect();
    auras.sort_by(|a, b| b.1.cmp(&a.1));
    if auras.is_empty() {
        println!("  (none)");
    }
    for (name, n) in auras.into_iter().take(15) {
        println!("  {name:<32} {n:>6}");
    }

    println!("\nTop 15 unmapped instant SpellEffect ids (→ E_SCRIPTED):");
    let mut effects: Vec<(i32, usize)> =
        cov.unmapped_effect.iter().map(|(&k, &n)| (k, n)).collect();
    effects.sort_by(|a, b| b.1.cmp(&a.1));
    if effects.is_empty() {
        println!("  (none)");
    }
    for (id, n) in effects.into_iter().take(15) {
        println!("  effect_id {id:<5} {n:>6}");
    }
    println!("======================================\n");
}

/// `--dbc <Data dir> --spell-snapshot <path>` mode: write the Base Snapshot a Datascript reads, and
/// touch no database.
///
/// The snapshot is the SAME derived rows the import would load, so what an author sees is what a
/// shard gets. It holds no client bytes — only `game_*` values the taxonomy mapping produced — and
/// it goes exactly where the operator points it. The author-facing target,
/// `datascripts/generated/base-snapshot.json`, is git-ignored.
pub fn run_spell_snapshot(data_dir: &str, path: &str) -> Result<()> {
    let (rows, cov, _) = build_spell_rows(Path::new(data_dir), &[], &[])?;
    let json = write_snapshot(&rows);

    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
    }
    std::fs::write(path, json.as_bytes()).with_context(|| format!("write {path}"))?;

    eprintln!(
        "spells: wrote base snapshot to {path} — {} spells, {} effects, {} bytes. No database was touched.",
        cov.spells,
        cov.effects,
        json.len(),
    );
    Ok(())
}

/// `--dbc <Data dir> --spells` mode: import Spell.dbc → game_spell/game_spell_effect. Dry-run by
/// default (parse + map + print coverage + sample rows, write NOTHING); `--apply` runs the SQL.
pub fn run_spells(data_dir: &str, args: &Args) -> Result<()> {
    let (stmts, cov, samples) = build_spell_sql(Path::new(data_dir), &args.only, &args.trainers)?;

    // The additive allowlist must resolve every id it was asked for — a typo'd / non-vanilla id would
    // silently import nothing. Fail loudly so the operator fixes the kit rather than shipping a gap.
    if !args.only.is_empty() && cov.spells != args.only.len() {
        eprintln!(
            "spells: WARNING — --only requested {} ids but matched {} in Spell.dbc (some id not found).",
            args.only.len(),
            cov.spells,
        );
    }

    if args.apply {
        run_sql_statements(args, &stmts, "spells")?;
        eprintln!(
            "spells: imported {} spells, {} effects ({:.1}% real kinds, {:.1}% E_SCRIPTED){}.",
            cov.spells,
            cov.effects,
            if cov.effects == 0 {
                0.0
            } else {
                100.0 * cov.real as f64 / cov.effects as f64
            },
            if cov.effects == 0 {
                0.0
            } else {
                100.0 * cov.scripted as f64 / cov.effects as f64
            },
            if args.only.is_empty() {
                ""
            } else {
                " [additive allowlist]"
            },
        );
        // Provenance stamp (work-item 216 convention, mirrored from the --dump loop's per-family
        // stamp_family calls in main.rs::run_dump). file_hash is "" here: unlike the --dump path
        // (which hashes the whole SQL dump's bytes in one shot), the DBC chain is read row-by-row
        // through `read_table`'s parsed-struct API (dbc.rs) with no single raw byte buffer to hash
        // cheaply — threading a chain-wide hash would mean re-reading every source MPQ a second time
        // just for provenance. Left "" rather than adding that cost; row_count (cov.spells) still
        // gives a verifiable count against this run.
        stamp_family(args, "spell", &args.source_sha, "", cov.spells as u64)
            .context("stamp_import_meta(spell)")?;
    } else if !args.only.is_empty() {
        // Allowlist dry-run: the diagnostics ARE the deliverable — print every requested spell's full
        // resolved mapping (header + per-effect lines) so the kit can be vetted before --apply.
        println!(
            "-- DRY RUN (additive allowlist, {} ids): {} surgical SQL statements, write NOTHING.\n",
            args.only.len(),
            stmts.len(),
        );
        for line in &samples {
            println!("{line}");
        }
        eprintln!("\nspells: dry-run — re-run with --apply to load this kit additively.");
    } else {
        println!(
            "-- DRY RUN: {} SQL statements (clear+reload game_spell + game_spell_effect), write NOTHING.",
            stmts.len()
        );
        // Show the two DELETEs + a truncated head of the first INSERT of each table (the full rows
        // would flood the terminal — the coverage report is the real signal).
        for s in stmts.iter().take(2) {
            println!("{s};");
        }
        for s in stmts.iter().skip(2) {
            if s.starts_with("INSERT INTO game_spell ") {
                println!("{}… ;", &s[..s.len().min(120)]);
                break;
            }
        }
        for s in stmts.iter().skip(2) {
            if s.starts_with("INSERT INTO game_spell_effect ") {
                println!("{}… ;", &s[..s.len().min(120)]);
                break;
            }
        }
        if !samples.is_empty() {
            println!("\nSample mapped effects:");
            for line in &samples {
                println!("{line}");
            }
        }
        eprintln!("spells: dry-run — re-run with --apply to load.");
    }

    // The Package Delta stage, last: the base rows are back and stamped, so every enabled Package's
    // claims go on top of them. It runs on the dry-run path too — that IS the read-only check, and
    // it must print the same plan an --apply would send.
    match &args.packages {
        Some(root) => package_delta::reapply(args, "spell", root)?,
        None => package_delta::warn_not_reapplied("spell"),
    }

    // Trainer-offering summary (per --trainer entry): how many game_trainer_spell rows this run emits for
    // each trainer. Printed on BOTH apply + dry-run so the operator confirms every class got offerings.
    if !args.trainers.is_empty() {
        println!("\n=== TRAINER OFFERINGS (game_trainer_spell) ===");
        let mut total = 0usize;
        for (entry, _ids) in &args.trainers {
            // Count the row tuples `(0,{entry},…)` across the emitted INSERT statements for this entry.
            let needle = format!("(0,{entry},");
            let n: usize = stmts
                .iter()
                .filter(|s| s.starts_with("INSERT INTO game_trainer_spell"))
                .map(|s| s.matches(&needle).count())
                .sum();
            println!("  trainer_entry {entry:<6} → {n:>3} offerings");
            total += n;
        }
        println!(
            "  TOTAL: {total} offerings across {} trainer(s)",
            args.trainers.len()
        );
        println!("===============================================\n");
    }

    print_coverage(&cov);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_spell_fixture() -> Vec<u8> {
        let mut bytes = b"WDBC".to_vec();
        for value in [1u32, 173, 692, 9] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.resize(712, 0);
        bytes.extend_from_slice(b"\0Fixture\0");
        // Authored build 5875 columns, independent of the dependency's named fields.
        for (column, value) in [
            (0, 509_0003),
            (1, 4),
            (4, 1),
            (5, 9),
            (6, 64),
            (7, 0x0008_0000),
            (8, 32),
            (11, 1 << 16),
            (13, TARGET_FLAG_UNIT_ALLY),
            (18, 1),
            (19, 1234),
            (20, 2345),
            (21, 99),
            (22, 2),
            (23, 88),
            (24, 40),
            (25, 75),
            (26, 3),
            (27, 60),
            (28, 4),
            (29, 7),
            (30, 1),
            (31, 0),
            (32, 33),
            (36, 1),
            (39, 5),
            (41, 987),
            (42, 111),
            (49, 888),
            (50, 2),
            (57, 9),
            (58, u32::MAX),
            (59, 31),
            (60, 1_048_608),
            (61, 6),
            (62, 6),
            (63, 64),
            (64, 1),
            (65, 2),
            (66, 3),
            (67, 1),
            (68, 2),
            (69, 3),
            (70, 0.25f32.to_bits()),
            (71, 0.5f32.to_bits()),
            (72, 0.75f32.to_bits()),
            (73, 1.5f32.to_bits()),
            (74, 2.5f32.to_bits()),
            (75, 3.5f32.to_bits()),
            (76, 6),
            (77, 14),
            (78, 20),
            (79, 1),
            (80, 2),
            (81, 3),
            (82, 6),
            (83, 1),
            (84, 6),
            (85, 21),
            (86, 22),
            (87, 23),
            (88, 1),
            (89, 2),
            (90, 0),
            (91, 29),
            (92, 8),
            (93, 0),
            (94, 1000),
            (95, 2000),
            (96, 3000),
            (97, 0.5f32.to_bits()),
            (98, 1.5f32.to_bits()),
            (99, 2.5f32.to_bits()),
            (100, 4),
            (101, 5),
            (102, 6),
            (103, 123),
            (104, 456),
            (105, 789),
            (106, 2),
            (107, 3),
            (108, 4),
            (109, 1011),
            (110, 1012),
            (111, 1013),
            (112, 4.5f32.to_bits()),
            (113, 5.5f32.to_bits()),
            (114, 6.5f32.to_bits()),
            (119, 999),
            (120, 1),
            (160, 3),
            (161, 0x8000_0001),
            (162, 0x1234_5678),
        ] {
            bytes[20 + column * 4..24 + column * 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn authored_table<T: DbcTable>(fields: u32, rows: &[&[u32]]) -> T {
        let mut bytes = b"WDBC".to_vec();
        for value in [rows.len() as u32, fields, fields * 4, 1] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for row in rows {
            for column in 0..fields as usize {
                bytes.extend_from_slice(&row.get(column).copied().unwrap_or(0).to_le_bytes());
            }
        }
        bytes.push(0);
        T::read(&mut std::io::Cursor::new(bytes)).unwrap()
    }

    fn fixture_tables() -> SpellDbc {
        SpellDbc {
            spells: read_spells(&source_spell_fixture()).unwrap(),
            cast_times: authored_table(4, &[&[1, 1500, 0, 1500]]),
            ranges: authored_table(22, &[&[1, 0, 35.0f32.to_bits()]]),
            durations: authored_table(4, &[&[1, 12000, 0, 12000]]),
            radii: authored_table(4, &[&[1, 7.0f32.to_bits()], &[2, 9.0f32.to_bits()]]),
        }
    }

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC with the owned build 5875 client"]
    fn actual_spell_import_keeps_restored_effects_and_curated_rules() {
        let dir = std::env::var("LYRACORE_TEST_DBC").expect("set LYRACORE_TEST_DBC");
        let source = open_spell_tables(Path::new(&dir)).unwrap();
        let new_location_companions = source
            .spells
            .iter()
            .flat_map(|spell| {
                (0..3).filter_map(move |slot| {
                    let target_a = spell.implicit_target_a[slot];
                    let target_b = spell.implicit_target_b[slot];
                    (spell.effect[slot] != 0
                        && LOCATION_TARGET_CODES.contains(&target_a)
                        && target_b != 0
                        && !matches!(target_a, 18 | 22))
                    .then_some((spell.id, slot, target_a, target_b))
                })
            })
            .collect::<Vec<_>>();
        let mut location_codes = new_location_companions
            .iter()
            .map(|(_, _, target_a, _)| *target_a)
            .collect::<Vec<_>>();
        location_codes.sort_unstable();
        location_codes.dedup();
        assert_eq!(new_location_companions.len(), 170);
        assert_eq!(location_codes, [17, 46, 53]);
        for spell_id in [1776, 1777, 8629, 11285, 11286] {
            let gouge = source
                .spells
                .iter()
                .find(|spell| spell.id == spell_id)
                .unwrap();
            assert_eq!(gouge.name, "Gouge");
            assert_eq!(gouge.effect[2], EFFECT_APPLY_AURA);
            assert_eq!(
                AuraMod::from_int(gouge.effect_aura[2]),
                Ok(AuraMod::ModStun)
            );
        }
        let (rows, _, _) = build_spell_rows(
            Path::new(&dir),
            &[
                17, 100, 139, 1459, 1776, 1777, 1784, 7728, 7747, 7748, 8629, 8674, 11285, 11286,
                12051, 17086, 17616, 18392, 20598, 21082,
            ],
            &[],
        )
        .unwrap();
        let effects = |id| {
            rows.effects
                .iter()
                .filter(|e| e.spell_id == id)
                .collect::<Vec<_>>()
        };
        for (id, entry) in [(7728, 5676), (8674, 5677)] {
            let summon = effects(id)
                .into_iter()
                .find(|e| e.kind == E_SUMMON_HOSTILE)
                .unwrap();
            assert_eq!(
                (summon.effect_index, summon.p0, summon.p1, summon.target),
                (0, entry, 83, T_SELF)
            );
            let header = rows.headers.iter().find(|s| s.spell_id == id).unwrap();
            assert_eq!((header.duration_ms, header.cooldown_ms), (360_000, 180_000));
        }
        let charge = effects(100);
        assert_eq!(charge.len(), 3);
        assert_eq!(
            (
                charge[2].effect_index,
                charge[2].kind,
                charge[2].trigger_spell,
                charge[2].target
            ),
            (2, E_TRIGGER, 7922, T_TARGET_ENEMY)
        );
        assert_eq!(effects(7748).len(), 1);
        assert_eq!(
            (effects(7748)[0].effect_index, effects(7748)[0].kind),
            (0, E_SCRIPTED)
        );
        assert_eq!(
            (
                effects(7747)[0].effect_index,
                effects(7747)[0].kind,
                effects(7747)[0].base_points
            ),
            (0, A_MOD_HEALTH_POWER, 15)
        );
        for spell_id in [1776, 1777, 8629, 11285, 11286] {
            let gouge = effects(spell_id);
            assert_eq!(gouge.len(), 3);
            assert_eq!((gouge[2].kind, gouge[2].p0), (A_CONTROL, M_POLY));
        }
        for spell_id in [17086, 17616, 18392] {
            assert_eq!(effects(spell_id)[0].target, T_AREA_ENEMY);
        }
        let stealth = effects(1784);
        assert!(
            !rows
                .headers
                .iter()
                .find(|s| s.spell_id == 1784)
                .unwrap()
                .is_negative
        );
        assert_eq!(stealth.len(), 3);
        assert_eq!(stealth.iter().filter(|e| e.kind == A_STEALTH).count(), 1);
        assert_eq!(
            (stealth[2].kind, stealth[2].base_points),
            (A_MOD_SPEED, -50)
        );
        assert_eq!(
            (
                effects(12051)[0].kind,
                effects(12051)[0].base_points,
                effects(12051)[0].period_ms
            ),
            (A_PERIODIC_ENERGIZE, 15, 2000)
        );
        assert_eq!(
            (
                effects(20598)[0].kind,
                effects(20598)[0].p0,
                effects(20598)[0].base_points
            ),
            (A_MOD_STAT_PCT, 4, 5)
        );
        assert_eq!(
            (effects(17)[0].target, effects(17)[0].p1),
            (T_TARGET_ALLY, 6788)
        );
        assert_eq!(
            (
                effects(139)[0].target,
                effects(139)[0].base_points,
                effects(139)[0].period_ms
            ),
            (T_TARGET_ALLY, 9, 3000)
        );
        assert_eq!(
            (effects(1459)[0].target, effects(1459)[0].p0),
            (T_TARGET_ALLY, 3)
        );
        let seal = effects(21082);
        assert_eq!(seal.len(), 4);
        assert_eq!((seal[3].kind, seal[3].base_points), (A_SEAL, 20));
        assert!(rows.reagents.contains(&"(61984,7748,10940,2)".to_owned()));
        assert!(rows.reagents.contains(&"(61985,7748,10938,2)".to_owned()));
    }

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC with the owned build 5875 client"]
    fn actual_target_none_uses_flags_defaults_and_scripted_fallback() {
        let dir = std::env::var("LYRACORE_TEST_DBC").expect("set LYRACORE_TEST_DBC");
        let (rows, _, _) =
            build_spell_rows(Path::new(&dir), &[16456, 18435, 21403, 28374], &[]).unwrap();
        let effects = |spell_id| {
            rows.effects
                .iter()
                .filter(|effect| effect.spell_id == spell_id)
                .collect::<Vec<_>>()
        };

        assert!(effects(16456)
            .iter()
            .all(|effect| effect.target == T_TARGET_ENEMY));
        assert!(effects(21403)
            .iter()
            .all(|effect| effect.target == T_TARGET_ALLY));
        assert_eq!(effects(18435)[1].target, T_SCRIPTED);
        assert_eq!(effects(28374)[2].target, T_SCRIPTED);
    }

    fn imported_spell_fixture(
        ids: &[u32],
    ) -> (crate::standalone_support::Standalone, String, String) {
        let dir = std::env::var("LYRACORE_TEST_DBC").expect("set LYRACORE_TEST_DBC");
        let (rows, _, _) = build_spell_rows(Path::new(&dir), ids, &[]).unwrap();
        let mut shard = crate::standalone_support::Standalone::start("imported-spell-columns");
        shard.publish_module();
        for sql in assemble_spell_sql(&rows, ids, false) {
            shard.assert_sql(&sql);
        }
        shard.assert_sql("DELETE FROM game_creature_move_schedule");
        shard.assert_sql("DELETE FROM game_melee_schedule");
        shard.assert_call("debug_spawn_player_entity", &["1"]);
        shard.assert_call("debug_set_level", &["1", "10"]);
        // The friendly trainer and hostile wolf are this disposable Module's seeded fixtures.
        let ally = ((0xf130u64 << 48) | (51001u64 << 24) | 1).to_string();
        let wolf = ((0xf130u64 << 48) | (51000u64 << 24) | 1).to_string();
        for guid in ["1", &ally, &wolf] {
            shard.assert_sql(&format!("UPDATE game_world_entity SET x = -8949.95, y = -132.493, z = 83.5312, health = 100, max_health = 1000, power = 1000, max_power = 1000 WHERE guid = {guid}"));
        }
        (shard, ally, wolf)
    }

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC, SpacetimeDB 2.7.1 and the Wasm toolchain"]
    fn actual_imported_friendly_spells_affect_the_selected_ally() {
        use crate::standalone_support::poll_until;
        use std::time::Duration;
        let (shard, ally, _) = imported_spell_fixture(&[139, 1459]);
        shard.assert_call("debug_cast_at", &["1", "139", &ally]);
        let renew = shard.query_rows(
            "SELECT target_guid, effect_id, amount, period_ms FROM game_aura WHERE spell_id = 139",
        );
        assert_eq!(renew.len(), 1);
        assert_eq!(renew[0]["target_guid"], ally);
        assert_eq!(renew[0]["amount"], "9");
        assert_eq!(renew[0]["period_ms"], "3000");
        assert!(poll_until(Duration::from_secs(5), || shard.query_rows(
            &format!("SELECT health FROM game_world_entity WHERE guid = {ally}")
        )[0]["health"]
            .parse::<u32>()
            .unwrap()
            >= 109));
        shard.assert_sql("DELETE FROM game_spell_cooldown");
        shard.assert_call("debug_cast_at", &["1", "1459", &ally]);
        let intellect = shard
            .query_rows("SELECT target_guid, amount, eff_p0 FROM game_aura WHERE spell_id = 1459");
        assert_eq!(intellect[0]["target_guid"], ally);
        assert_eq!(intellect[0]["amount"], "2");
        assert_eq!(intellect[0]["eff_p0"], "3");
    }

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC, SpacetimeDB 2.7.1 and the Wasm toolchain"]
    fn actual_imported_gouge_control_ends_on_damage() {
        let (shard, _, wolf) = imported_spell_fixture(&[1776]);
        shard.assert_sql("DELETE FROM game_aura_schedule");
        shard.assert_call("debug_cast_at", &["1", "1776", &wolf]);
        let control =
            shard.query_rows("SELECT effect_id, eff_p0 FROM game_aura WHERE spell_id = 1776");
        assert_eq!(control.len(), 1);
        assert_eq!(control[0]["effect_id"], "7106");
        assert_eq!(control[0]["eff_p0"], "4");
        let health = || {
            shard.query_rows(&format!(
                "SELECT health FROM game_world_entity WHERE guid = {wolf}"
            ))[0]["health"]
                .parse::<u32>()
                .unwrap()
        };
        let before = health();
        shard.assert_call("debug_apply_damage", &[&wolf, "10", "1"]);
        assert_eq!(health(), before - 10);
        assert!(shard
            .query_rows("SELECT id FROM game_aura WHERE spell_id = 1776")
            .is_empty());
    }

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC, SpacetimeDB 2.7.1 and the Wasm toolchain"]
    fn actual_imported_stealth_uses_its_third_source_effect() {
        let (shard, _, _) = imported_spell_fixture(&[1784]);
        shard.assert_call("debug_cast_at", &["1", "1784", "1"]);
        let slow = shard.query_rows(
            "SELECT effect_id, amount FROM game_aura WHERE spell_id = 1784 AND eff_kind = 164",
        );
        assert_eq!(slow.len(), 1);
        assert_eq!(slow[0]["effect_id"], "7138");
        assert_eq!(slow[0]["amount"], "-50");
    }

    #[test]
    fn source_columns_reach_all_three_derived_effect_rows() {
        let (rows, _, _) = derive_spell_rows(&fixture_tables(), &[], &[]).unwrap();
        assert_eq!(rows.effects.len(), 3);
        assert_eq!(
            rows.effects
                .iter()
                .map(|e| (
                    e.effect_index,
                    e.kind,
                    e.base_points,
                    e.die_sides,
                    e.per_level,
                    e.period_ms,
                    e.trigger_spell,
                    e.effect_mechanic
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, A_MOD_STAT, 7, 1, 1.5, 1000, 1011, 1),
                (1, A_PERIODIC_HEAL, 15, 2, 2.5, 2000, 1012, 2),
                (2, E_TRIGGER, 21, 3, 3.5, 3000, 1013, 3),
            ]
        );
    }

    #[test]
    fn every_gouge_rank_imports_incapacitate_while_other_mod_stuns_remain_stuns() {
        let gouge = |id| Spell {
            id,
            name: "Gouge".into(),
            effect: [0, 0, EFFECT_APPLY_AURA],
            effect_aura: [0, 0, AuraMod::ModStun as u32],
            implicit_target_a: [0, 0, 6],
            ..Default::default()
        };
        let mut dbc = fixture_tables();
        dbc.spells = [1776, 1777, 8629, 11285, 11286, 999_999]
            .into_iter()
            .map(gouge)
            .collect();

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        for spell_id in [1776, 1777, 8629, 11285, 11286] {
            let control = rows
                .effects
                .iter()
                .find(|effect| effect.spell_id == spell_id)
                .unwrap();
            assert_eq!((control.kind, control.p0), (A_CONTROL, M_POLY));
        }
        let unrelated = rows
            .effects
            .iter()
            .find(|effect| effect.spell_id == 999_999)
            .unwrap();
        assert_eq!((unrelated.kind, unrelated.p0), (A_CONTROL, M_STUN));
    }

    #[test]
    fn source_header_reagents_and_references_reach_the_derived_rows() {
        let dbc = fixture_tables();
        let source = &dbc.spells[0];
        assert_eq!(source.targets, TARGET_FLAG_UNIT_ALLY);
        assert_eq!(source.equipment, [u32::MAX, 31, 1_048_608]);
        assert_eq!(source.implicit_target_b, [21, 22, 23]);
        assert_eq!(source.effect_base_dice, [1, 2, 3]);
        assert_eq!(source.effect_dice_per_level, [0.25, 0.5, 0.75]);
        assert_eq!(source.effect_points_per_combo, [4.5, 5.5, 6.5]);
        assert_eq!(source.effect_multiple_values, [0.5, 1.5, 2.5]);
        assert_eq!(source.effect_item_type, [123, 456, 789]);
        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        let header = &rows.headers[0];
        assert_eq!(header.name, "Fixture");
        assert_eq!(
            (header.school_mask, header.dispel_type, header.mechanic),
            (16, 1, 9)
        );
        assert_eq!(
            (header.attributes, header.gcd_ms, header.stances),
            (64, 0, 1)
        );
        assert_eq!(header.cast_flags, SPELL_ATTR_RANGED_AUTO_REPEAT);
        assert_eq!(rows.eventai_metadata, ["(5090003,true)"]);
        assert_eq!(
            (header.power_type, header.cost, header.cast_time_ms),
            (0, 33, 1500)
        );
        assert_eq!(
            (header.cooldown_ms, header.range_yd, header.duration_ms),
            (2345, 35, 12000)
        );
        assert_eq!(
            (header.spell_level, header.max_level, header.max_stacks),
            (7, 60, 5)
        );
        assert_eq!(
            (header.proc_flags, header.proc_chance, header.proc_charges),
            (40, 75, 3)
        );
        assert_eq!(header.aura_interrupt, 1);
        assert_eq!(
            (header.family_name, header.family_flags),
            (3, 0x1234_5678_8000_0001)
        );
        assert_eq!(
            rows.reagents,
            ["(40720024,5090003,111,2)", "(40720031,5090003,888,9)"]
        );
        assert_eq!(
            rows.effects
                .iter()
                .map(|e| (e.target, e.radius_yd, e.chain_targets))
                .collect::<Vec<_>>(),
            [
                (T_TARGET_ENEMY, 7.0, 4),
                (T_SELF, 9.0, 5),
                (T_TARGET_ENEMY, 0.0, 6)
            ]
        );
        assert_eq!(
            (rows.effects[0].p0, rows.effects[0].p0_kind),
            (2, P_STAT_ID)
        );
    }

    #[test]
    fn source_references_and_fractional_magnitudes_reach_effect_parameters() {
        let mut bytes = source_spell_fixture();
        for (column, value) in [(61, 62u32), (62, 24), (63, 6), (91, 0), (92, 0), (93, 107)] {
            bytes[20 + column * 4..24 + column * 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut dbc = fixture_tables();
        dbc.spells = read_spells(&bytes).unwrap();
        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        assert_eq!(
            (rows.effects[0].kind, rows.effects[0].p1),
            (E_POWER_BURN, 50)
        );
        assert_eq!(
            (
                rows.effects[1].kind,
                rows.effects[1].p0,
                rows.effects[1].p0_kind
            ),
            (E_CREATE_ITEM, 456, P_ITEM_ENTRY)
        );
        assert_eq!(
            (
                rows.effects[2].kind,
                rows.effects[2].p0,
                rows.effects[2].p0_kind,
                rows.effects[2].p1
            ),
            (A_SPELLMOD_FLAT, 4, P_SPELLMOD_OP, 789)
        );
    }

    #[test]
    fn source_target_pairs_select_units_at_the_source_location() {
        let mut dbc = fixture_tables();
        dbc.spells[0].implicit_target_a = [21, 22, 57];
        dbc.spells[0].implicit_target_b = [0, 15, 0];
        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        assert_eq!(
            rows.effects.iter().map(|e| e.target).collect::<Vec<_>>(),
            [T_TARGET_ALLY, T_AREA_ENEMY, T_TARGET_ALLY]
        );
    }

    #[test]
    fn every_pinned_location_target_uses_its_companion_unit_selection() {
        for target_a in LOCATION_TARGET_CODES {
            assert_eq!(
                resolve_target(unit_selection_from_pair(target_a, 6), true),
                T_TARGET_ENEMY,
                "location target {target_a} did not use its enemy companion"
            );
        }
        assert_eq!(unit_selection_from_pair(0, 6), 6);
        assert_eq!(unit_selection_from_pair(45, 6), 45);
        assert_eq!(resolve_target(45, false), T_TARGET_ALLY);
        assert_eq!(unit_selection_from_pair(26, 6), 26);
    }

    #[test]
    fn source_target_none_uses_compatible_flags_then_effect_defaults() {
        let mut dbc = fixture_tables();
        dbc.spells[0].implicit_target_a = [0; 3];
        dbc.spells[0].implicit_target_b = [0; 3];
        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        assert_eq!(
            rows.effects
                .iter()
                .map(|effect| effect.target)
                .collect::<Vec<_>>(),
            [T_TARGET_ALLY, T_TARGET_ALLY, T_SCRIPTED]
        );

        dbc.spells[0].targets = 0;
        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        assert_eq!(
            rows.effects
                .iter()
                .map(|effect| effect.target)
                .collect::<Vec<_>>(),
            [T_SELF, T_SELF, T_SCRIPTED]
        );
    }

    #[test]
    fn evasion_and_fade_map_to_combat_fields() {
        use AuraMod::*;
        // Pull-model reuse: dodge/threat ride the EXISTING A_MOD_COMBAT(COMBAT_DODGE / COMBAT_THREAT) folds.
        assert_eq!(aura_mod_to_kind(ModDodgePercent), A_MOD_COMBAT);
        assert_eq!(aura_mod_to_kind(ModTotalThreat), A_MOD_COMBAT);
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModDodgePercent, 0).0,
            COMBAT_DODGE
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModTotalThreat, 0).0,
            COMBAT_THREAT
        );
    }

    #[test]
    fn curated_correction_reclassifies_script_effects_by_name() {
        // The Spell.sql-analog correction only touches effects that fell to E_SCRIPTED, keyed by name.
        assert_eq!(correct_script_effect_kind("Holy Light", E_SCRIPTED), E_HEAL);
        assert_eq!(
            correct_script_effect_kind("Life Tap", E_SCRIPTED),
            E_CONVERT_RESOURCE
        );
        assert_eq!(correct_script_effect_kind("Charge", E_SCRIPTED), E_CHARGE);
        // Feint (Rogue Slice 3): vanilla encodes it as the native Threat effect (→ E_TAUNT) with a
        // negative base; reclassify that E_TAUNT to E_REDUCE_THREAT (a one-time current-threat drop).
        assert_eq!(
            correct_script_effect_kind("Feint", E_TAUNT),
            E_REDUCE_THREAT
        );
        // A non-Feint E_TAUNT (a real taunt) is NOT disturbed.
        assert_eq!(correct_script_effect_kind("Growl", E_TAUNT), E_TAUNT);
        // Garrote's eff2 combo Dummy reclassifies to the combo GENERATOR (its eff1 bleed is unchanged).
        assert_eq!(
            correct_script_effect_kind("Garrote", E_SCRIPTED),
            E_ADD_COMBO
        );
        // The other rogue combo GENERATORS reclassify the same way (Sinister Strike/Backstab/Gouge).
        assert_eq!(
            correct_script_effect_kind("Sinister Strike", E_SCRIPTED),
            E_ADD_COMBO
        );
        assert_eq!(
            correct_script_effect_kind("Backstab", E_SCRIPTED),
            E_ADD_COMBO
        );
        assert_eq!(correct_script_effect_kind("Gouge", E_SCRIPTED), E_ADD_COMBO);
        // Pick Pocket's script effect grants creature copper without engaging combat.
        assert_eq!(
            correct_script_effect_kind("Pick Pocket", E_SCRIPTED),
            E_PICKPOCKET
        );
        // A correctly-mapped effect is never disturbed (even for a corrected name).
        assert_eq!(correct_script_effect_kind("Holy Light", E_HEAL), E_HEAL);
        assert_eq!(correct_script_effect_kind("Frostbolt", E_DAMAGE), E_DAMAGE);
        // An unknown script effect stays the graceful no-op.
        assert_eq!(
            correct_script_effect_kind("Some Other Spell", E_SCRIPTED),
            E_SCRIPTED
        );
        // The three Warrior stance switches reclassify the inert ModShapeshift→A_FLAG marker to E_SET_STANCE.
        assert_eq!(
            correct_script_effect_kind("Battle Stance", A_FLAG),
            E_SET_STANCE
        );
        assert_eq!(
            correct_script_effect_kind("Defensive Stance", A_FLAG),
            E_SET_STANCE
        );
        assert_eq!(
            correct_script_effect_kind("Berserker Stance", A_FLAG),
            E_SET_STANCE
        );
        // Create Healthstone (all ranks): SCRIPT_EFFECT (E_SCRIPTED) reclassifies to E_CREATE_ITEM by name
        // (the item entry is injected at the call site from effect_item_type-less data).
        assert_eq!(
            correct_script_effect_kind("Create Healthstone (Minor)", E_SCRIPTED),
            E_CREATE_ITEM
        );
        assert_eq!(
            correct_script_effect_kind("Create Healthstone", E_SCRIPTED),
            E_CREATE_ITEM
        );
        assert_eq!(
            correct_script_effect_kind("Create Healthstone (Major)", E_SCRIPTED),
            E_CREATE_ITEM
        );
    }

    #[test]
    fn eviscerate_damage_reclassifies_to_finisher_damage() {
        // Eviscerate's real E_DAMAGE effect scales with combo points — reclassify it to the FINISHER
        // kind so the handler reads combo points at cast time instead of a flat weapon-damage effect.
        assert_eq!(
            correct_script_effect_kind("Eviscerate", E_DAMAGE),
            E_FINISHER_DAMAGE
        );
        // A different spell's E_DAMAGE effect is never disturbed (the arm is keyed on the exact name).
        assert_eq!(correct_script_effect_kind("Frostbolt", E_DAMAGE), E_DAMAGE);
    }

    #[test]
    fn rez_name_rescue_covers_every_class_resurrect_and_guards_others() {
        // Every class's combat-rez shares the raw Resurrect effect, which falls to the graceful-success
        // E_SCRIPTED no-op without this by-name rescue — reviving nobody while reporting cast success.
        assert_eq!(
            correct_script_effect_kind("Resurrection", E_SCRIPTED),
            E_RESURRECT
        ); // Priest
        assert_eq!(
            correct_script_effect_kind("Redemption", E_SCRIPTED),
            E_RESURRECT
        ); // Paladin
        assert_eq!(
            correct_script_effect_kind("Rebirth", E_SCRIPTED),
            E_RESURRECT
        ); // Druid
        assert_eq!(
            correct_script_effect_kind("Ancestral Spirit", E_SCRIPTED),
            E_RESURRECT
        ); // Shaman
           // A same-shaped SCRIPT_EFFECT under any OTHER name stays the graceful no-op (the guard the
           // 176 review flagged: a missed name here silently makes the cast succeed while reviving nobody).
        assert_eq!(
            correct_script_effect_kind("Reincarnation", E_SCRIPTED),
            E_SCRIPTED
        );
    }

    #[test]
    fn stance_p0_remaps_form_id_to_zero_based_stance() {
        // The effect-loop remap turns the vanilla form id (carried in p0 by the A_FLAG marker) into our
        // 0-based stance id via form_to_stance. The handler reads this p0.
        assert_eq!(stance_p0(E_SET_STANCE, 17), 0); // Battle Stance → stance 0
        assert_eq!(stance_p0(E_SET_STANCE, 18), 1); // Defensive Stance → stance 1
        assert_eq!(stance_p0(E_SET_STANCE, 19), 2); // Berserker Stance → stance 2
                                                    // Druid combat forms (work-item 156): Bear Form 5487 carries ModShapeshift(5), Cat Form 768
                                                    // carries form 1, Dire Bear Form 9634 carries form 8.
        assert_eq!(stance_p0(E_SET_STANCE, 5), 3); // Bear Form → stance 3
        assert_eq!(stance_p0(E_SET_STANCE, 1), 4); // Cat Form → stance 4
        assert_eq!(stance_p0(E_SET_STANCE, 8), 5); // Dire Bear Form → stance 5
                                                   // A non-stance kind's p0 passes through untouched (the remap only fires for E_SET_STANCE).
        assert_eq!(stance_p0(E_DAMAGE, 17), 17);
        // An unmapped form under E_SET_STANCE passes through (unreachable by construction — the name
        // rescue only names mapped forms; a raw form id like Aquatic 4 is inert module-side).
        assert_eq!(stance_p0(E_SET_STANCE, 4), 4);
    }

    #[test]
    fn druid_form_switches_name_rescue_to_set_stance() {
        // The Druid combat-form switches carry the same inert ModShapeshift→A_FLAG marker as the Warrior
        // stances and reclassify to E_SET_STANCE by name (work-item 156).
        assert_eq!(
            correct_script_effect_kind("Bear Form", A_FLAG),
            E_SET_STANCE
        );
        assert_eq!(
            correct_script_effect_kind("Dire Bear Form", A_FLAG),
            E_SET_STANCE
        );
        assert_eq!(correct_script_effect_kind("Cat Form", A_FLAG), E_SET_STANCE);
        // Non-combat forms are NOT rescued — their marker stays the inert A_FLAG (the pre-156 shape).
        assert_eq!(correct_script_effect_kind("Aquatic Form", A_FLAG), A_FLAG);
        assert_eq!(correct_script_effect_kind("Travel Form", A_FLAG), A_FLAG);
        assert_eq!(correct_script_effect_kind("Moonkin Form", A_FLAG), A_FLAG);
    }

    #[test]
    fn translate_stance_mask_folds_vanilla_form_bits_to_zero_based() {
        // 0 mask (the stance switches + every unrestricted spell) → 0 (usable in any stance).
        assert_eq!(translate_stance_mask(0), 0);
        // Charge/Thunder Clap/Overpower: vanilla bit16 (Battle) → our bit0 (0x01).
        assert_eq!(translate_stance_mask(0x10000), 0x01);
        // Rend: vanilla bits16,17 (Battle/Defensive) → our bits0,1 (0x03).
        assert_eq!(translate_stance_mask(0x30000), 0x03);
        // Hamstring: vanilla bits16,18 (Battle/Berserker) → our bits0,2 (0x05).
        assert_eq!(translate_stance_mask(0x50000), 0x05);
        // Berserker-only would be vanilla bit18 → our bit2 (0x04).
        assert_eq!(translate_stance_mask(0x40000), 0x04);
        // Druid combat forms survive translation (work-item 156 — previously dropped):
        // Maul/Growl-shaped Bear|DireBear masks: vanilla bits4,7 (0x90) → our bits3,5 (0x28).
        assert_eq!(translate_stance_mask(0x90), 0x28);
        // Bear-only (vanilla bit4) → our bit3 (0x08); Cat-only (vanilla bit0) → our bit4 (0x10).
        assert_eq!(translate_stance_mask(0x10), 0x08);
        assert_eq!(translate_stance_mask(0x1), 0x10);
        // A warrior spell's mask is UNCHANGED by the widening (bit16-18 fold exactly as before), and a
        // mixed warrior+druid mask folds both sides.
        assert_eq!(translate_stance_mask(0x10090), 0x01 | 0x28);
        // Unmapped form bits (Travel form 3 → bit2, Ghoul form 7 → bit6, form 16 → bit15) still drop.
        assert_eq!(translate_stance_mask(0x4), 0);
        assert_eq!(translate_stance_mask(0x40), 0);
        assert_eq!(translate_stance_mask(0x8000), 0);
    }

    #[test]
    fn rogue_cast_gate_flags_set_by_name() {
        // REQUIRES_BEHIND on Backstab (the must) + the other vanilla behind-only openers.
        assert_eq!(
            spell_flag_attributes("Backstab") & SPELL_ATTR_REQ_BEHIND,
            SPELL_ATTR_REQ_BEHIND
        );
        assert_eq!(
            spell_flag_attributes("Garrote") & SPELL_ATTR_REQ_BEHIND,
            SPELL_ATTR_REQ_BEHIND
        );
        assert_eq!(
            spell_flag_attributes("Ambush") & SPELL_ATTR_REQ_BEHIND,
            SPELL_ATTR_REQ_BEHIND
        );
        // REQ_DAGGER: Backstab ONLY — not the other behind-only openers (Garrote/Ambush are not dagger-gated).
        assert_eq!(
            spell_flag_attributes("Backstab") & SPELL_ATTR_REQ_DAGGER,
            SPELL_ATTR_REQ_DAGGER
        );
        assert_eq!(spell_flag_attributes("Garrote") & SPELL_ATTR_REQ_DAGGER, 0);
        assert_eq!(spell_flag_attributes("Ambush") & SPELL_ATTR_REQ_DAGGER, 0);
        // Sap: REQ_STEALTH + STEALTH_SAFE + INCAP_OPENER (the out-of-combat/humanoid constraints); NOT REQ_BEHIND.
        assert_eq!(
            spell_flag_attributes("Sap") & SPELL_ATTR_REQ_STEALTH,
            SPELL_ATTR_REQ_STEALTH
        );
        assert_eq!(
            spell_flag_attributes("Sap") & SPELL_ATTR_STEALTH_SAFE,
            SPELL_ATTR_STEALTH_SAFE
        );
        assert_eq!(
            spell_flag_attributes("Sap") & SPELL_ATTR_INCAP_OPENER,
            SPELL_ATTR_INCAP_OPENER
        );
        assert_eq!(spell_flag_attributes("Sap") & SPELL_ATTR_REQ_BEHIND, 0);
        // Garrote (Rogue Slice 3): REQ_BEHIND + REQ_STEALTH, but NOT STEALTH_SAFE (it breaks stealth) and
        // NOT INCAP_OPENER (works on any type, in or out of combat — must not inherit Sap's constraints).
        assert_eq!(
            spell_flag_attributes("Garrote") & SPELL_ATTR_REQ_STEALTH,
            SPELL_ATTR_REQ_STEALTH
        );
        assert_eq!(
            spell_flag_attributes("Garrote") & SPELL_ATTR_STEALTH_SAFE,
            0
        );
        assert_eq!(
            spell_flag_attributes("Garrote") & SPELL_ATTR_INCAP_OPENER,
            0
        );
        assert_eq!(
            spell_flag_attributes("Garrote"),
            SPELL_ATTR_REQ_BEHIND | SPELL_ATTR_REQ_STEALTH
        );
        // Pick Pocket: STEALTH_SAFE only (fixes the Phase-6 stealth-break) — no REQ flags.
        assert_eq!(
            spell_flag_attributes("Pick Pocket"),
            SPELL_ATTR_STEALTH_SAFE
        );
        // Slice and Dice: the combo-FINISHER duration-scaling bit (Rogue Slice 2) — and ONLY that bit.
        assert_eq!(
            spell_flag_attributes("Slice and Dice"),
            SPELL_ATTR_FINISHER_DURATION
        );
        // An unrelated spell gets no flags (baseline-safe).
        assert_eq!(spell_flag_attributes("Fireball"), 0);
    }

    #[test]
    fn ranged_auto_repeat_flagged_by_dbc_bit_or_name() {
        // DBC AUTOREPEAT bit set (any name) → flagged.
        assert!(is_ranged_auto_repeat(ATTR_EX2_AUTOREPEAT, "Whatever"));
        // By-NAME fallback for the two player abilities when the bit isn't set.
        assert!(is_ranged_auto_repeat(0, "Auto Shot"));
        assert!(is_ranged_auto_repeat(0, "Shoot"));
        // A normal cast with neither → not flagged (baseline-safe).
        assert!(!is_ranged_auto_repeat(0, "Fireball"));
        assert!(!is_ranged_auto_repeat(0x1, "Frostbolt")); // an unrelated ex2 bit doesn't trip it
    }

    #[test]
    fn on_next_swing_reclassifies_heroic_strike_and_cleave() {
        // Tier 2b on-next-swing QUEUE: Heroic Strike / Cleave encode a weapon-damage effect (→ E_DAMAGE) in
        // the DBC, but vanilla QUEUES them onto the next swing — reclassify that E_DAMAGE to E_NEXT_SWING.
        assert_eq!(
            correct_script_effect_kind("Heroic Strike", E_DAMAGE),
            E_NEXT_SWING
        );
        assert_eq!(correct_script_effect_kind("Cleave", E_DAMAGE), E_NEXT_SWING);
        // Only the weapon-damage E_DAMAGE effect is reclassified — a non-damage effect on the same spell
        // (e.g. an aura) is untouched, and an unrelated damage spell stays E_DAMAGE (baseline-safe).
        assert_eq!(correct_script_effect_kind("Heroic Strike", A_FLAG), A_FLAG);
        assert_eq!(correct_script_effect_kind("Frostbolt", E_DAMAGE), E_DAMAGE);
    }

    #[test]
    fn react_window_flags_set_by_name() {
        // Tier 2b react windows: Overpower carries REQ_OVERPOWER, Revenge carries REQ_REVENGE — the cast gate
        // refuses each unless the matching window (armed by a dodge / an avoided incoming swing) is open.
        assert_eq!(
            spell_flag_attributes("Overpower") & SPELL_ATTR_REQ_OVERPOWER,
            SPELL_ATTR_REQ_OVERPOWER
        );
        assert_eq!(
            spell_flag_attributes("Overpower") & SPELL_ATTR_REQ_REVENGE,
            0
        );
        assert_eq!(
            spell_flag_attributes("Revenge") & SPELL_ATTR_REQ_REVENGE,
            SPELL_ATTR_REQ_REVENGE
        );
        assert_eq!(
            spell_flag_attributes("Revenge") & SPELL_ATTR_REQ_OVERPOWER,
            0
        );
        // Each is the ONLY bit on its spell (no accidental flag bleed).
        assert_eq!(spell_flag_attributes("Overpower"), SPELL_ATTR_REQ_OVERPOWER);
        assert_eq!(spell_flag_attributes("Revenge"), SPELL_ATTR_REQ_REVENGE);
    }

    #[test]
    fn kick_interrupt_effect_maps_to_e_interrupt() {
        // Kick's eff2 is the raw vanilla InterruptCast effect (68) — reclassified to E_INTERRUPT by id
        // (cleaner than a name match; auto-covers any future interrupt). Its eff1 (SchoolDamage=2) is the
        // strike, unchanged. A name-keyed correction is unnecessary — the residue guard never sees it.
        assert_eq!(instant_effect_to_kind(68), E_INTERRUPT);
        assert_eq!(instant_effect_to_kind(2), E_DAMAGE);
        // A residue-keyed correction does NOT touch E_INTERRUPT (it's already a real kind, not E_SCRIPTED).
        assert_eq!(correct_script_effect_kind("Kick", E_INTERRUPT), E_INTERRUPT);
    }

    #[test]
    fn summon_effect_maps_to_e_summon_pet_with_creature_entry_in_p0() {
        // The raw vanilla Summon effect (56) → E_SUMMON_PET (Summon Imp et al.).
        assert_eq!(instant_effect_to_kind(56), E_SUMMON_PET);
        // The summoned creature entry rides in effect_misc_value → p0, tagged P_ENTRY (a creature template
        // entry). Imp = 416 (item_type is unused for this kind). Mirrors E_CREATE_ITEM's item-entry routing.
        assert_eq!(resolve_instant_params(E_SUMMON_PET, 416, 0), (416, P_ENTRY));
        // A residue-keyed correction never disturbs it (it's already a real kind, not E_SCRIPTED).
        assert_eq!(
            correct_script_effect_kind("Summon Imp", E_SUMMON_PET),
            E_SUMMON_PET
        );
    }

    #[test]
    fn tame_creature_effect_maps_to_enemy_target_without_params() {
        assert_eq!(instant_effect_to_kind(55), E_TAME_CREATURE);
        assert_eq!(
            resolve_instant_params(E_TAME_CREATURE, 123, 456),
            (0, P_NONE)
        );
    }

    #[test]
    fn feed_pet_effect_maps_without_a_spell_id_branch() {
        assert_eq!(instant_effect_to_kind(101), E_FEED_PET);
        assert_eq!(resolve_instant_params(E_FEED_PET, 123, 456), (0, P_NONE));
    }

    #[test]
    fn heal_max_health_effect_split_out_of_e_heal() {
        // Lay on Hands is HealMaxHealth (effect 67) — it must map to E_HEAL_MAX_HEALTH (fill-to-max), NOT
        // E_HEAL (which heals base_points ~= 0 → heals nothing). Plain Heal (10) stays E_HEAL.
        assert_eq!(instant_effect_to_kind(67), E_HEAL_MAX_HEALTH);
        assert_eq!(instant_effect_to_kind(10), E_HEAL);
        // It's already a real kind, so the residue-keyed correction leaves it alone.
        assert_eq!(
            correct_script_effect_kind("Lay on Hands", E_HEAL_MAX_HEALTH),
            E_HEAL_MAX_HEALTH
        );
    }

    #[test]
    fn proc_auras_map_natively_no_name_involved() {
        // Aura 42 (ProcTriggerSpell) and aura 43 (ProcTriggerDamage) map onto the two proc kinds for
        // ANY spell — no name rescue. `name` is deliberately absent from these two calls.
        assert_eq!(aura_mod_to_kind(AuraMod::ProcTriggerSpell), A_PROC_TRIGGER);
        assert_eq!(aura_mod_to_kind(AuraMod::ProcTriggerDamage), A_PROC_DAMAGE);
        // A channel's per-tick trigger (PeriodicTriggerSpell) is unaffected — it still lands on
        // E_TRIGGER (the channel reclassify further downstream turns it into A_PERIODIC_TRIGGER).
        assert_eq!(aura_mod_to_kind(AuraMod::PeriodicTriggerSpell), E_TRIGGER);
        // Both proc kinds are already real (non-E_SCRIPTED, non-A_FLAG) kinds, so the curated
        // name correction passes them through untouched for any name, including one carrying no
        // rescue entry at all.
        assert_eq!(
            correct_script_effect_kind("Some Other Spell", A_PROC_TRIGGER),
            A_PROC_TRIGGER
        );
        assert_eq!(
            correct_script_effect_kind("Some Other Spell", A_PROC_DAMAGE),
            A_PROC_DAMAGE
        );
    }

    #[test]
    fn proc_damage_school_p0_reads_the_header_school_not_a_misc_value() {
        // Aura 43 carries no per-effect misc value for its school — p0 is the PROC SPELL'S OWN
        // school_mask (frost = bit 4 = 0x10), regardless of whatever misc value resolve_aura_params
        // produced upstream.
        assert_eq!(
            proc_damage_school_p0(A_PROC_DAMAGE, 0, P_NONE, 0x10),
            (0x10, P_SCHOOL_MASK)
        );
        // A non-A_PROC_DAMAGE kind's (p0, p0_kind) passes through untouched.
        assert_eq!(
            proc_damage_school_p0(A_MOD_RESISTANCE, 5, P_SCHOOL_MASK, 0x10),
            (5, P_SCHOOL_MASK)
        );
    }

    #[test]
    fn proc_header_fields_preserve_masks_and_bound_byte_columns() {
        assert_eq!(proc_header_fields(40, 100, 0), (40, 100, 0));
        assert_eq!(proc_header_fields(139944, 100, 3), (139944, 100, 3));
        // A percent/charge count is clamped into its u8 column rather than silently truncating.
        assert_eq!(proc_header_fields(1, 999, 999), (1, 255, 255));
        assert_eq!(proc_header_fields(1, 0, 0), (1, 0, 0));
    }

    #[test]
    fn wrapper_rank_trigger_excludes_every_non_rank_payload() {
        // Mirrors module/src/trainer.rs `resolve_learn_target`'s exclusion list — the two MUST stay
        // in lockstep, so this test pins the importer's half by kind, not by name.
        assert!(!is_wrapper_rank_trigger(A_PERIODIC_TRIGGER));
        assert!(!is_wrapper_rank_trigger(A_FLAG));
        assert!(!is_wrapper_rank_trigger(A_PROC_TRIGGER));
        assert!(!is_wrapper_rank_trigger(A_PROC_DAMAGE));
        assert!(!is_wrapper_rank_trigger(E_TRIGGER));
        // A genuine LearnSpell wrapper's effect maps to E_SCRIPTED — its trigger_spell IS a rank.
        assert!(is_wrapper_rank_trigger(E_SCRIPTED));
        assert!(is_wrapper_rank_trigger(E_DAMAGE));
    }

    #[test]
    fn synthetic_seal_added_to_seal_of_the_crusader_only() {
        // Seal of the Crusader (21082) gains a THIRD, SELF-targeted A_SEAL effect at the given slot —
        // The client fills three slots, so the curated marker uses the fourth.
        let row = synthetic_seal_effect(21082, "Seal of the Crusader", 3)
            .expect("Seal of the Crusader gains a seal effect");
        assert_eq!(row.row_id(), (21082u64 << 2) | 3);
        assert_eq!(row.kind, A_SEAL);
        assert_eq!(row.base_points, 20);
        assert_eq!(row.target, T_SELF); // self aura, like Seal of Righteousness
        assert!(synthetic_seal_effect(21082, "Seal of Command", 2).is_none()); // wrong name guard
        assert!(synthetic_seal_effect(21084, "Seal of the Crusader", 2).is_none()); // wrong id guard (a hypothetical other rank)
                                                                                    // Seal of Righteousness reclassifies its OWN inert marker by name — it needs no synthetic add.
        assert!(synthetic_seal_effect(21084, "Seal of Righteousness", 2).is_none());
    }

    #[test]
    fn power_word_shield_p1_links_weakened_soul() {
        // The real live Power Word: Shield (17) A_ABSORB effect gets its p1 overridden to link Weakened
        // Soul (6788) — the generic linked-debuff mechanic (work-item 013).
        assert_eq!(
            power_word_shield_p1_override(17, "Power Word: Shield", A_ABSORB, 0),
            6788
        );
        // Wrong id guard (a hypothetical other rank/spell reusing the name).
        assert_eq!(
            power_word_shield_p1_override(18, "Power Word: Shield", A_ABSORB, 0),
            0
        );
        // Wrong name guard.
        assert_eq!(
            power_word_shield_p1_override(17, "Power Word: Fortitude", A_ABSORB, 0),
            0
        );
        // Wrong kind guard — only the real A_ABSORB effect is overridden.
        assert_eq!(
            power_word_shield_p1_override(17, "Power Word: Shield", A_FLAG, 0),
            0
        );
    }

    #[test]
    fn instant_effects_map_to_real_kinds() {
        assert_eq!(instant_effect_to_kind(2), E_DAMAGE); // SchoolDamage
        assert_eq!(instant_effect_to_kind(58), E_DAMAGE); // WeaponDamage
        assert_eq!(instant_effect_to_kind(121), E_DAMAGE); // NormalizedWeaponDmg
        assert_eq!(instant_effect_to_kind(10), E_HEAL); // Heal (flat)
        assert_eq!(instant_effect_to_kind(67), E_HEAL_MAX_HEALTH); // HealMaxHealth (Lay on Hands — fill to max, not E_HEAL)
        assert_eq!(instant_effect_to_kind(30), E_ENERGIZE); // Energize
        assert_eq!(instant_effect_to_kind(38), E_DISPEL); // Dispel
        assert_eq!(instant_effect_to_kind(64), E_TRIGGER); // TriggerSpell
        assert_eq!(instant_effect_to_kind(63), E_TAUNT); // Threat
        assert_eq!(instant_effect_to_kind(91), E_TAUNT); // ThreatAll
        assert_eq!(instant_effect_to_kind(24), E_CREATE_ITEM); // CreateItem (conjure / quest item)
        assert_eq!(instant_effect_to_kind(62), E_POWER_BURN); // PowerBurn (Mana Burn)
        assert_eq!(instant_effect_to_kind(33), E_OPEN_LOCK); // OpenLock (Pick Lock 1804, work-item 119)
        assert_eq!(instant_effect_to_kind(59), E_OPEN_LOCK); // OpenLockItem (the item-lock variant)
                                                             // LearnSpell / Summon are still out of taxonomy → graceful no-op.
        assert_eq!(instant_effect_to_kind(36), E_SCRIPTED);
    }

    #[test]
    fn instant_effect_raw_id_arms_land_work_item_101() {
        // Exact-arm pins for the four raw ids work-item 101 lands (verified against
        // `wow_world_base-0.3.0`'s vanilla/tbc/wrath `SpellEffect::from_int`, all three eras agree):
        // AttackMe=114, AddComboPoints=80, Resurrect=18, ResurrectNew=113.
        assert_eq!(instant_effect_to_kind(114), E_TAUNT); // AttackMe
        assert_eq!(instant_effect_to_kind(80), E_ADD_COMBO); // AddComboPoints
        assert_eq!(instant_effect_to_kind(18), E_RESURRECT); // Resurrect
        assert_eq!(instant_effect_to_kind(113), E_RESURRECT); // ResurrectNew
                                                              // Negative pins: ids adjacent to / resembling the above stay the graceful E_SCRIPTED no-op —
                                                              // proves the match arms above are exact, not off-by-one or a wildcard.
        assert_eq!(instant_effect_to_kind(3), E_SCRIPTED); // Dummy — the Rogue combo generators' ACTUAL
                                                           // raw effect in-kit (rescued by name in correct_script_effect_kind, not by this raw-id arm)
        assert_eq!(instant_effect_to_kind(115), E_SCRIPTED); // one past AttackMe(114) — still unmapped
        assert_eq!(instant_effect_to_kind(19), E_SCRIPTED); // one past Resurrect(18) — still unmapped
        assert_eq!(instant_effect_to_kind(112), E_SCRIPTED); // one before ResurrectNew(113) — still unmapped
    }

    #[test]
    fn power_burn_ratio_bp_converts_the_dbc_fraction_to_basis_points() {
        assert_eq!(power_burn_ratio_bp(0.5), 50); // vanilla Mana Burn: EffectMultipleValue 0.5
        assert_eq!(power_burn_ratio_bp(1.0), 100);
        assert_eq!(power_burn_ratio_bp(0.0), 0); // the module treats <=0 as 100 (1:1) — the raw DBC value round-trips as-is
    }

    #[test]
    fn apply_aura_effect_id_routes_to_aura_branch() {
        assert!(is_aura_effect(6)); // ApplyAura
        assert!(is_aura_effect(27)); // PersistentAreaAura
        assert!(is_aura_effect(35)); // ApplyAreaAuraParty
        assert!(is_aura_effect(119)); // ApplyAreaAuraPet
        assert!(!is_aura_effect(2)); // SchoolDamage is instant
    }

    #[test]
    fn auras_map_to_real_kinds() {
        use AuraMod::*;
        assert_eq!(aura_mod_to_kind(PeriodicDamage), A_PERIODIC_DAMAGE);
        assert_eq!(aura_mod_to_kind(PeriodicHeal), A_PERIODIC_HEAL);
        // Demon Skin/Armor's health-per-5 (work-item 024): ModRegen is a combat-independent periodic
        // heal, reclassified onto the SAME A_PERIODIC_HEAL kind as PeriodicHeal (Renew/bandages/food) —
        // NOT left as the inert A_FLAG marker.
        assert_eq!(aura_mod_to_kind(ModRegen), A_PERIODIC_HEAL);
        assert_eq!(aura_mod_to_kind(PeriodicEnergize), A_PERIODIC_ENERGIZE);
        assert_eq!(aura_mod_to_kind(ModStat), A_MOD_STAT);
        assert_eq!(aura_mod_to_kind(ModResistance), A_MOD_RESISTANCE);
        assert_eq!(aura_mod_to_kind(SchoolAbsorb), A_ABSORB);
        assert_eq!(aura_mod_to_kind(ModAttackPower), A_MOD_COMBAT);
        assert_eq!(aura_mod_to_kind(ModStun), A_CONTROL);
        assert_eq!(aura_mod_to_kind(ModRoot), A_CONTROL);
        assert_eq!(aura_mod_to_kind(ModFear), A_CONTROL);
        assert_eq!(aura_mod_to_kind(SchoolImmunity), A_IMMUNITY);
        assert_eq!(aura_mod_to_kind(Dummy), A_FLAG);
        // incoming-damage % modifier (Shield Wall / vulnerability) — wired; the FLAT variant stays no-op.
        assert_eq!(aura_mod_to_kind(ModDamagePercentTaken), A_MOD_DAMAGE_TAKEN);
        assert_eq!(aura_mod_to_kind(ModDamageTaken), E_SCRIPTED); // flat damage-taken not handled yet
                                                                  // Spell modifiers (work-item 264): AddFlatModifier/AddPctModifier were the "unmapped → no-op"
                                                                  // example here until the passive-modifier engine landed (2000292) and gave them real kinds —
                                                                  // this assertion is the regression guard for that reclassification, not a stale count.
        assert_eq!(aura_mod_to_kind(AddFlatModifier), A_SPELLMOD_FLAT);
        assert_eq!(aura_mod_to_kind(AddPctModifier), A_SPELLMOD_PCT);
        // still-genuinely-unmapped → graceful no-op (ModRating has no handler anywhere in
        // aura_mod_to_kind; if this starts failing, either it grew a real kind — update this pin to a
        // different unmapped variant — or the catch-all arm broke).
        assert_eq!(aura_mod_to_kind(ModRating), E_SCRIPTED);
    }

    #[test]
    fn control_params_pick_the_right_mechanic() {
        use AuraMod::*;
        assert_eq!(
            resolve_aura_params(A_CONTROL, ModStun, 0),
            (M_STUN, P_MECHANIC)
        );
        assert_eq!(
            resolve_aura_params(A_CONTROL, ModRoot, 0),
            (M_ROOT, P_MECHANIC)
        );
        assert_eq!(
            resolve_aura_params(A_CONTROL, ModFear, 0),
            (M_FEAR, P_MECHANIC)
        );
        assert_eq!(
            resolve_aura_params(A_CONTROL, ModConfuse, 0),
            (M_POLY, P_MECHANIC)
        );
        assert_eq!(
            resolve_aura_params(A_CONTROL, ModCharm, 0),
            (M_POLY, P_MECHANIC)
        );
    }

    #[test]
    fn combat_field_resolves_from_variant_not_misc() {
        use AuraMod::*;
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModAttackPower, 99),
            (COMBAT_ATTACK_POWER, P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModCritPercent, 0),
            (COMBAT_CRIT, P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModHitChance, 0),
            (COMBAT_HIT, P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModDamageDone, 0),
            (COMBAT_DMG_DONE, P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModHealing, 0),
            (COMBAT_SPELL_POWER, P_COMBAT_FIELD)
        );
    }

    #[test]
    fn speed_kind_resolves_all_four_arms_from_variant() {
        use AuraMod::*;
        // move (default/generic — e.g. ModIncreaseSpeed/ModDecreaseSpeed).
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModIncreaseSpeed, 0),
            (0, P_SPEED_KIND)
        );
        // swing (melee/ranged attack speed).
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModMeleeHaste, 0),
            (1, P_SPEED_KIND)
        );
        // cast (casting speed).
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModCastingSpeedNotStack, 0),
            (2, P_SPEED_KIND)
        );
        // mounted.
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModIncreaseMountedSpeed, 0),
            (3, P_SPEED_KIND)
        );
    }

    #[test]
    fn absorb_params_split_mana_shield_from_school_absorb() {
        use AuraMod::*;
        // ManaShield absorbs from the power pool — no school, P_NONE.
        assert_eq!(resolve_aura_params(A_ABSORB, ManaShield, 0x7F), (0, P_NONE));
        // A real SchoolAbsorb (Power Word: Shield) carries the 7-bit school mask from misc.
        assert_eq!(
            resolve_aura_params(A_ABSORB, SchoolAbsorb, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
    }

    #[test]
    fn immunity_params_resolve_all_three_arms() {
        use AuraMod::*;
        // school/damage immunity: 7-bit school mask.
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, SchoolImmunity, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, DamageImmunity, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
        // mechanic immunity: the raw mechanic id/mask, untouched.
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, MechanicImmunity, 5),
            (5, P_MECHANIC)
        );
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, MechanicImmunityMask, 5),
            (5, P_MECHANIC)
        );
        // anything else (effect/state immunity): an opaque raw id.
        assert_eq!(resolve_aura_params(A_IMMUNITY, ModStun, 9), (9, P_RAW));
    }

    #[test]
    fn is_channeled_reads_the_attributes_ex1_bit_and_the_arcane_missiles_fallback() {
        // AttributesEx1 CHANNELED bit 0x44 set → channeled, for any name.
        assert!(is_channeled(0x44, "Some Spell"));
        // The bit alone (0x04) or (0x40) individually still trips the mask (bitwise AND != 0).
        assert!(is_channeled(0x04, "Some Spell"));
        assert!(is_channeled(0x40, "Some Spell"));
        // No bit set, no name match → not channeled.
        assert!(!is_channeled(0, "Some Spell"));
        // Arcane Missiles is channeled by NAME even if the DBC carries no CHANNELED bit at all (the
        // cmangos-classic fallback for the one channel the vanilla DBC under-flags).
        assert!(is_channeled(0, "Arcane Missiles"));
    }

    #[test]
    fn stat_all_sentinel_and_school_mask() {
        use AuraMod::*;
        // misc -1 (u32::MAX) on a ModStat ⇒ all-stats sentinel 0xFF.
        assert_eq!(
            resolve_aura_params(A_MOD_STAT, ModStat, u32::MAX),
            (STAT_ALL, P_STAT_ID)
        );
        assert_eq!(resolve_aura_params(A_MOD_STAT, ModStat, 3), (3, P_STAT_ID)); // INT
                                                                                 // ModStat is FLAT; ModTotalStatPercentage / ModPercentStat are PERCENT (distinct kind). BOTH resolve
                                                                                 // the stat from misc — a SPECIFIC stat (4=Spirit, The Human Spirit), not a blanket all-stats (104).
        assert_eq!(aura_mod_to_kind(ModStat), A_MOD_STAT);
        assert_eq!(aura_mod_to_kind(ModTotalStatPercentage), A_MOD_STAT_PCT);
        assert_eq!(
            resolve_aura_params(A_MOD_STAT_PCT, ModTotalStatPercentage, 4),
            (4, P_STAT_ID)
        ); // Spirit
        assert_eq!(
            resolve_aura_params(A_MOD_STAT_PCT, ModTotalStatPercentage, u32::MAX),
            (STAT_ALL, P_STAT_ID)
        );
        // resistance keeps the 7-bit school mask.
        assert_eq!(
            resolve_aura_params(A_MOD_RESISTANCE, ModResistance, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
    }

    #[test]
    fn periodic_energize_decodes_power_type_from_misc() {
        // p0 = the power type id, read straight from the low byte of misc (rage=1, mana=0).
        assert_eq!(
            resolve_aura_params(A_PERIODIC_ENERGIZE, AuraMod::PeriodicEnergize, 1),
            (1, P_POWER_TYPE)
        );
        assert_eq!(
            resolve_aura_params(A_PERIODIC_ENERGIZE, AuraMod::PeriodicEnergize, 0),
            (0, P_POWER_TYPE)
        );
    }

    #[test]
    fn instant_param_decodes_power_and_dispel() {
        assert_eq!(resolve_instant_params(E_ENERGIZE, 1, 0), (1, P_POWER_TYPE)); // rage
        assert_eq!(resolve_instant_params(E_DISPEL, 1, 0), (1, P_SCHOOL_MASK)); // magic category
        assert_eq!(resolve_instant_params(E_DAMAGE, 4, 0), (0, P_NONE)); // school on header
        assert_eq!(instant_effect_to_kind(83), E_DUEL);
        assert_eq!(
            resolve_instant_params(E_DUEL, 21680, 0),
            (21680, P_GAMEOBJECT_ENTRY)
        );
        // CreateItem: p0 = the item entry (from effect_item_type), tagged P_ITEM_ENTRY; misc ignored.
        assert_eq!(
            resolve_instant_params(E_CREATE_ITEM, 99, 2070),
            (2070, P_ITEM_ENTRY)
        );
    }

    #[test]
    fn target_definitions_distinguish_enemy_ally_area_and_unsupported_selections() {
        // Target 6 is explicitly hostile, independent of magnitude or aura polarity.
        assert_eq!(resolve_target(6, true), T_TARGET_ENEMY);
        assert_eq!(resolve_target(6, false), T_TARGET_ENEMY);
        assert_eq!(resolve_target(1, false), T_SELF);
        assert_eq!(resolve_target(0, false), T_SCRIPTED);
        // Target 20 selects the party within caster range.
        assert_eq!(resolve_target(20, false), T_AREA_ALLY);
        // area-of-effect code 8: polarity still decides enemy-area vs ally-area.
        assert_eq!(resolve_target(8, true), T_AREA_ENEMY);
        assert_eq!(resolve_target(8, false), T_AREA_ALLY);
        // Unrecognized target codes retain the scripted fallback.
        assert_eq!(resolve_target(-5, true), T_SCRIPTED);
        // an unrecognized positive code defers to runtime resolution.
        assert_eq!(resolve_target(999, false), T_SCRIPTED);
    }

    #[test]
    fn reducing_modifier_aura_is_a_debuff() {
        use AuraMod::*;
        // Sunder Armor shape: ApplyAura(6) + ModResistance + negative base → debuff (→ enemy target).
        assert!(is_reducing_modifier_aura(6, ModResistance, -90));
        // Demoralizing Shout shape: ApplyAura(6) + ModAttackPower + negative base → debuff.
        assert!(is_reducing_modifier_aura(6, ModAttackPower, -35));
        assert!(is_reducing_modifier_aura(6, ModStat, -10));
        // A POSITIVE modifier (a buff — e.g. Battle Shout +AP, Inner Fire +armor) is NOT flagged.
        assert!(!is_reducing_modifier_aura(6, ModAttackPower, 15));
        assert!(!is_reducing_modifier_aura(6, ModResistance, 2000));
        // A negative magnitude on a NON-modifier aura is not this rule's business (handled elsewhere).
        assert!(!is_reducing_modifier_aura(6, PeriodicDamage, -5));
        // An INSTANT effect (not aura-placing) is never a "reducing modifier aura".
        assert!(!is_reducing_modifier_aura(2, ModResistance, -90));
    }

    #[test]
    fn eventai_exclude_caster_uses_the_spell_attribute_bit() {
        assert!(excludes_eventai_caster(0x0008_0000));
        assert!(excludes_eventai_caster(0x0008_0044));
        assert!(!excludes_eventai_caster(0x0000_0044));
    }

    #[test]
    fn trainer_cost_is_level_keyed_and_never_free() {
        // DBC-derived: 50 copper per required level, monotone, clamped so a level-0 rank still costs 50c.
        assert_eq!(trainer_cost(0), 50); // the DBC "floor" ranks clamp to level 1 → 50c (not free)
        assert_eq!(trainer_cost(1), 50);
        assert_eq!(trainer_cost(4), 200);
        assert_eq!(trainer_cost(10), 500);
        assert!(trainer_cost(10) > trainer_cost(4)); // monotone-rising
                                                     // No cmangos value is consulted — the cost is purely a function of the level.
    }

    #[test]
    fn delete_statements_are_wholesale_or_surgical() {
        // Full import (no allowlist): the wholesale clear of both tables, GUARDED at the synthetic
        // fixture floor (module/src/seed.rs 50000/50110, module/src/seed/fixtures.rs 50072/50137) so a
        // full-DBC reload never deletes rows no import script re-creates. Weakened Soul (6788) sits
        // BELOW the floor by design — the full import re-creates it from its own real DBC entry.
        // `game_spell_effect` MUST be filtered on its own `spell_id` column (not the packed
        // `(spell_id<<2)|effect_index` `id`) — asserting the exact column name here catches a
        // regression back to the packed-id form.
        let full = spell_delete_statements(&[]);
        assert_eq!(full.len(), 4);
        assert_eq!(full[0], "DELETE FROM game_spell WHERE spell_id < 50000");
        assert_eq!(
            full[1],
            "DELETE FROM game_spell_effect WHERE spell_id < 50000"
        );
        assert_eq!(
            full[2],
            "DELETE FROM game_spell_reagent WHERE spell_id < 50000"
        );
        assert_eq!(
            full[3],
            "DELETE FROM game_creature_ai_spell_metadata WHERE spell_id < 50000"
        );
        // Per danger-zones.md §2, a single-column range filter via `spacetime sql` can silently return
        // 0 rows in some conditions — this guard's live behavior (fixtures 50000/50072/50110/50137
        // survive; all non-zero DBC rows below the floor ARE replaced) still NEEDS verification on a
        // real node per the work-item's runbook; this test only pins the SQL string.

        // Additive allowlist: one surgical delete per table and id, sorted and deduped.
        let surgical = spell_delete_statements(&[7386, 78, 78]);
        assert_eq!(
            surgical.len(),
            8,
            "two ids across four tables, with the duplicate removed"
        );
        assert_eq!(surgical[0], "DELETE FROM game_spell WHERE spell_id = 78");
        assert_eq!(
            surgical[1],
            "DELETE FROM game_spell_effect WHERE spell_id = 78"
        );
        assert_eq!(
            surgical[2],
            "DELETE FROM game_spell_reagent WHERE spell_id = 78"
        );
        assert_eq!(
            surgical[3],
            "DELETE FROM game_creature_ai_spell_metadata WHERE spell_id = 78"
        );
        assert_eq!(surgical[4], "DELETE FROM game_spell WHERE spell_id = 7386");
        assert_eq!(
            surgical[5],
            "DELETE FROM game_spell_effect WHERE spell_id = 7386"
        );
        assert_eq!(
            surgical[6],
            "DELETE FROM game_spell_reagent WHERE spell_id = 7386"
        );
        assert_eq!(
            surgical[7],
            "DELETE FROM game_creature_ai_spell_metadata WHERE spell_id = 7386"
        );
        // The surgical path NEVER emits an unbounded clear (that would clobber the seed/fixtures).
        assert!(surgical.iter().all(|s| !s.contains(">= 0")));
    }

    #[test]
    fn mounted_aura_maps_to_a_mounted_with_display_id_p0() {
        // The vanilla Mounted aura (78) reclassifies out of the inert A_FLAG marker list onto A_MOUNTED
        // — no longer a dormant no-op.
        assert_eq!(aura_mod_to_kind(AuraMod::Mounted), A_MOUNTED);
        // p0 defaults to the raw misc value AS the display id (the shape of every real classic mount:
        // EffectMiscValue names a CreatureDisplayInfo id directly), tagged P_DISPLAY_ID.
        assert_eq!(
            resolve_aura_params(A_MOUNTED, AuraMod::Mounted, 723),
            (723, P_DISPLAY_ID)
        );
    }

    #[test]
    fn mount_display_p0_resolves_creature_template_indirection_and_falls_through() {
        let mut displays: BTreeMap<u32, u32> = BTreeMap::new();
        displays.insert(1234, 5678); // a rare mount that names a creature TEMPLATE, not a display
                                     // Acceptance criterion: "A mount spell whose DBC names a creature template resolves to the
                                     // template's display."
        assert_eq!(mount_display_p0(A_MOUNTED, 1234, &displays), 5678);
        // The common case: the misc value isn't in the map (it's already a real display id) — pass
        // through unchanged.
        assert_eq!(mount_display_p0(A_MOUNTED, 723, &displays), 723);
        // Non-A_MOUNTED kinds are never touched.
        assert_eq!(mount_display_p0(E_DAMAGE, 1234, &displays), 1234);
    }

    #[test]
    fn dispel_mechanic_reclassifies_the_mount_parameter_only() {
        // The ONE data-driven reclassify the design allows: raw DISPEL_MECHANIC (108) whose misc value
        // names the mount mechanic (21) becomes E_DISMOUNT — Dazed's shape (A_MOD_SPEED slow +
        // this effect), but not keyed on Dazed's spell id or name.
        assert_eq!(dismount_effect_kind(108, 21), Some(E_DISMOUNT));
        // Acceptance criterion: any OTHER mechanic parameter does NOT reclassify — it stays unmapped
        // (falls to E_SCRIPTED via instant_effect_to_kind, counted in cov.unmapped_effect).
        assert_eq!(dismount_effect_kind(108, 1), None); // e.g. mechanic 1 (a different CC)
        assert_eq!(dismount_effect_kind(108, 0), None);
        // A different raw effect id carrying misc==21 is not a dispel-mechanic effect at all — untouched.
        assert_eq!(dismount_effect_kind(2, 21), None);
        // 108 isn't in instant_effect_to_kind's raw-id table — it falls to the graceful no-op until the
        // mechanic-21 reclassify above fires.
        assert_eq!(instant_effect_to_kind(108), E_SCRIPTED);
    }

    #[test]
    fn mounted_speed_normalizes_the_vanilla_stored_flat_amount() {
        // Mounted speed stays A_MOD_SPEED / SPEED_MOUNTED, never a second concept.
        use AuraMod::*;
        for mounted in [
            ModIncreaseMountedSpeed,
            ModMountedSpeedAlways,
            ModMountedSpeedNotStack,
        ] {
            assert_eq!(
                resolve_aura_params(A_MOD_SPEED, mounted, 0),
                (SPEED_MOUNTED, P_SPEED_KIND)
            );
        }
        // Vanilla stores the flat amount MINUS one; 59/99 normalize to the nominal 60%/100%.
        assert_eq!(dbc_flat_amount(59), 60);
        assert_eq!(dbc_flat_amount(99), 100);
    }

    #[test]
    fn aura_interrupt_translates_damage_and_movement_without_other_flags() {
        assert_eq!(aura_interrupt_bits(0x2), 1);
        assert_eq!(aura_interrupt_bits(0x8), 2);
        assert_eq!(aura_interrupt_bits(0xa), 3);
        assert_eq!(aura_interrupt_bits(0x80), 0);
        assert_eq!(aura_interrupt_bits(0x1), 0);
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    use wow_dbc::vanilla_tables::spell_cast_times::{SpellCastTimesKey, SpellCastTimesRow};
    use wow_dbc::vanilla_tables::spell_range::SpellRangeRow;

    fn source_spell(spell_id: u32, event: u32) -> Spell {
        let mut bytes = b"WDBC".to_vec();
        for value in [1u32, 173, 692, 1] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.resize(20 + 692 + 1, 0);
        for (column, value) in [
            (0, spell_id),
            (15, 83),
            (18, 7),
            (36, 7),
            (61, 61),
            (62, 86),
            (82, 0),
            (83, 40),
            (106, event),
            (107, 1),
        ] {
            bytes[20 + column * 4..24 + column * 4].copy_from_slice(&value.to_le_bytes());
        }
        read_spells(&bytes).unwrap().remove(0)
    }

    #[test]
    fn binding_source_events_import_one_hostile_summon_and_keep_the_focus() {
        // Facts from classic-db cd0c426a3b2ff56dd518bf009025299468e60fdb,
        // spell_template and dbscripts_on_event. This fixture contains no client archive bytes.
        for (spell_id, event, entry) in [(7728, 1131, 5676), (8674, 1134, 5677)] {
            let row = source_spell(spell_id, event);
            let dbc = SpellDbc {
                spells: vec![],
                cast_times: SpellCastTimes {
                    rows: vec![SpellCastTimesRow {
                        id: SpellCastTimesKey::new(7),
                        base: 10_000,
                        per_level_increase: 0,
                        minimum: 0,
                    }],
                },
                ranges: SpellRange {
                    rows: vec![SpellRangeRow {
                        id: SpellRangeKey::new(7),
                        range_min: 0.0,
                        range_max: 10.0,
                        flags: 0,
                        display_name: Default::default(),
                        display_name_short: Default::default(),
                    }],
                },
                durations: SpellDuration { rows: vec![] },
                radii: SpellRadius { rows: vec![] },
            };
            let mut acc = SpellAccumulator::default();
            let allow = std::collections::HashSet::new();
            let header = push_spell_header_row(&row, &dbc, &allow, &mut acc);
            push_spell_effect_rows(&row, &header, &dbc, &BTreeMap::new(), &allow, &mut acc);
            let summons: Vec<_> = acc
                .effect_rows
                .iter()
                .filter(|e| e.kind == E_SUMMON_HOSTILE)
                .collect();
            assert_eq!(summons.len(), 1);
            assert_eq!(
                (
                    summons[0].p0,
                    summons[0].p0_kind,
                    summons[0].p1,
                    summons[0].target
                ),
                (entry, P_ENTRY, 83, T_SELF)
            );
            assert_eq!(acc.spell_rows[0].cast_time_ms, 10_000);
            assert_eq!(acc.spell_rows[0].range_yd, 10);
            assert_eq!(acc.spell_rows[0].duration_ms, 360_000);
            assert_eq!(acc.spell_rows[0].cooldown_ms, 180_000);
            assert_eq!(acc.effect_rows[1].kind, E_SCRIPTED);
        }
    }

    #[test]
    fn unrelated_events_and_owned_pet_effects_are_not_binding_summons() {
        for (spell, effect, event, focus) in [
            (688, 56, 416, 0),
            (7728, 61, 1134, 83),
            (8674, 61, 1134, 43),
            (999, 61, 1131, 83),
            (7728, 86, 1131, 83),
        ] {
            assert_eq!(binding_summon_entry(spell, effect, event, focus), None);
        }
        assert_eq!(instant_effect_to_kind(56), E_SUMMON_PET);
        assert_eq!(resolve_instant_params(E_SUMMON_PET, 416, 0), (416, P_ENTRY));
    }
}
