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
use crate::dump::push_insert;
use crate::package_delta;
use crate::shard::Shard;
use crate::spell_snapshot::{write_snapshot, SpellEffectRow, SpellHeaderRow, SpellRows};
use crate::world_import_scope::WorldImportScope;
use std::path::Path;

use lyracore_shared::spell::taxonomy::{
    A_ABSORB, A_COMBAT_HEALTH_REGEN_PCT, A_CONTROL, A_DISARM, A_FLAG, A_IMMUNITY, A_MOD_COMBAT,
    A_MOD_DAMAGE_TAKEN, A_MOD_DETECT_RANGE, A_MOD_HEALTH_POWER, A_MOD_POWER_REGEN_PCT,
    A_MOD_RESISTANCE, A_MOD_SPEED, A_MOD_STAT, A_MOD_STAT_PCT, A_MOUNTED, A_PERIODIC_DAMAGE,
    A_PERIODIC_ENERGIZE, A_PERIODIC_HEAL, A_PERIODIC_TRIGGER, A_PROC_DAMAGE, A_PROC_TRIGGER,
    A_RETALIATE, A_SEAL, A_SELF_RESURRECT, A_SPELLMOD_FLAT, A_SPELLMOD_PCT, A_STEALTH, A_TRANSFORM,
    COMBAT_ATTACK_POWER, COMBAT_CRIT, COMBAT_DMG_DONE, COMBAT_DODGE, COMBAT_HIT,
    COMBAT_SPELL_POWER, COMBAT_THREAT, E_ADD_COMBO, E_BLINK, E_CHARGE, E_CONVERT_RESOURCE,
    E_CREATE_ITEM, E_DAMAGE, E_DISENCHANT, E_DISMOUNT, E_DISPEL, E_DISTRACT, E_DUEL, E_ENERGIZE,
    E_FEED_PET, E_FINISHER_DAMAGE, E_FISH, E_HEAL, E_HEAL_MAX_HEALTH, E_INTERRUPT, E_JUDGEMENT,
    E_NEXT_SWING, E_OPEN_LOCK, E_PERSISTENT_AREA, E_PICKPOCKET, E_POWER_BURN, E_RECALL_HOME,
    E_REDUCE_THREAT, E_RESURRECT, E_SCRIPTED, E_SELF_RESURRECT, E_SET_STANCE, E_SUMMON_HOSTILE,
    E_SUMMON_PET, E_TAME_CREATURE, E_TAUNT, E_TRIGGER, E_WEAPON_STRIKE, M_FEAR, M_POLY, M_ROOT,
    M_STUN, P_COMBAT_FIELD, P_DISPLAY_ID, P_ENTRY, P_FLAG, P_FLAT_MANA, P_GAMEOBJECT_ENTRY,
    P_ITEM_ENTRY, P_MECHANIC, P_NONE, P_PCT_MAX_POWER, P_POWER_TYPE, P_RAW, P_SCHOOL_MASK,
    P_SPEED_KIND, P_SPELLMOD_OP, P_SPELL_ID, P_STAT_ID, SPEED_MOUNTED, SPEED_MOVE,
    SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK, SPEED_SWIM, SPELL_ATTR_CHANNELED,
    SPELL_ATTR_FINISHER_DURATION, SPELL_ATTR_INCAP_OPENER, SPELL_ATTR_RANGED_AUTO_REPEAT,
    SPELL_ATTR_REQ_BEHIND, SPELL_ATTR_REQ_DAGGER, SPELL_ATTR_REQ_OVERPOWER, SPELL_ATTR_REQ_REVENGE,
    SPELL_ATTR_REQ_STEALTH, SPELL_ATTR_STEALTH_SAFE, SPELL_ATTR_USES_RANGED_SLOT, STANCE_BATTLE,
    STANCE_BEAR, STANCE_BERSERKER, STANCE_CAT, STANCE_DEFENSIVE, STANCE_DIRE_BEAR, STAT_ALL,
    T_AREA_ALLY, T_AREA_ENEMY, T_SCRIPTED, T_SELF, T_TARGET_ALLY, T_TARGET_ANY, T_TARGET_ENEMY,
};

fn binding_summon_entry(spell_id: u32, effect: i32, event: u32, focus: u32) -> Option<i32> {
    match (spell_id, effect, event, focus) {
        (7728, 61, 1131, 83) => Some(5676),
        (8674, 61, 1134, 83) => Some(5677),
        _ => None,
    }
}

const GOUGE_RANK_IDS: [u32; 5] = [1776, 1777, 8629, 11285, 11286];

// Vanilla form ids differ from durable stance ids.
const FORM_CAT: i32 = 1;
const FORM_BEAR: i32 = 5;
const FORM_DIRE_BEAR: i32 = 8;
const FORM_BATTLE: i32 = 17;
const FORM_DEFENSIVE: i32 = 18;
const FORM_BERSERKER: i32 = 19;

fn form_to_stance(form_id: i32) -> Option<i32> {
    match form_id {
        FORM_BATTLE => Some(i32::from(STANCE_BATTLE)),
        FORM_DEFENSIVE => Some(i32::from(STANCE_DEFENSIVE)),
        FORM_BERSERKER => Some(i32::from(STANCE_BERSERKER)),
        FORM_BEAR => Some(i32::from(STANCE_BEAR)),
        FORM_CAT => Some(i32::from(STANCE_CAT)),
        FORM_DIRE_BEAR => Some(i32::from(STANCE_DIRE_BEAR)),
        _ => None,
    }
}

// AttributesEx1 bits 0x04 and 0x40 both mark a channeled spell.
const ATTR_EX1_CHANNELED_MASK: u32 = 0x0000_0044;
const ATTR_EX1_EXCLUDE_CASTER: u32 = 0x0008_0000;
const ATTR_EX2_AUTOREPEAT: u32 = 0x0000_0020;
// Attributes 0x04 and 0x400 both mark an on-next-swing spell.
const ATTR_ON_NEXT_SWING_MASK: u32 = 0x0000_0404;

const SCHOOL_MASK_PHYSICAL: u8 = 1;
const EFFECT_WEAPON_DAMAGE_NOSCHOOL: i32 = 17;
const EFFECT_WEAPON_DAMAGE: i32 = 58;
const EFFECT_NORMALIZED_WEAPON_DAMAGE: i32 = 121;
const EFFECT_WEAPON_PERCENT_DAMAGE: i32 = 31;
const WEAPON_DAMAGE_FULL_PCT: i32 = 100;
// vmangos adds this to the cast time of every ranged-slot spell that does not auto-repeat.
const RANGED_SLOT_CAST_MS: u32 = 500;

// A queued strike is one physical weapon swing plus flat damage; any other spell stays instant.
fn queues_on_next_swing(attributes: u32, school_mask: u8, kinds: &[u8]) -> bool {
    attributes & ATTR_ON_NEXT_SWING_MASK != 0
        && school_mask == SCHOOL_MASK_PHYSICAL
        && kinds.contains(&E_WEAPON_STRIKE)
        && kinds
            .iter()
            .all(|&kind| kind == E_WEAPON_STRIKE || kind == E_SCRIPTED)
}

struct WeaponStrikeRow {
    effect_index: usize,
    base_points: i32,
    die_sides: i32,
    per_level: f32,
    percent: i32,
    normalized: bool,
}

// vmangos EffectWeaponDmg folds every weapon effect into one strike at the last of them.
fn weapon_strike_row(s: &Spell, resolved: &[(usize, ResolvedEffect)]) -> Option<WeaponStrikeRow> {
    let mut row: Option<WeaponStrikeRow> = None;
    for (i, effect) in resolved {
        let effect_id = s.effect[*i];
        if !is_weapon_damage_effect(effect_id, effect.kind) {
            continue;
        }
        let strike = row.get_or_insert(WeaponStrikeRow {
            effect_index: *i,
            base_points: 0,
            die_sides: 0,
            per_level: 0.0,
            percent: WEAPON_DAMAGE_FULL_PCT,
            normalized: false,
        });
        strike.effect_index = *i;
        if effect_id == EFFECT_WEAPON_PERCENT_DAMAGE {
            strike.percent = effect.base_points;
        } else {
            strike.base_points += effect.base_points;
            strike.die_sides = strike.die_sides.max(effect.die_sides);
            strike.per_level += effect.per_level;
            strike.normalized |= effect_id == EFFECT_NORMALIZED_WEAPON_DAMAGE;
        }
    }
    row
}

fn is_weapon_damage_effect(effect_id: i32, kind: u8) -> bool {
    kind == E_WEAPON_STRIKE || (kind == E_DAMAGE && effect_id == EFFECT_WEAPON_PERCENT_DAMAGE)
}

fn ranged_slot_cast_time(cast_time_ms: u32, attributes: u32, auto_repeat: bool) -> u32 {
    if attributes & SPELL_ATTR_USES_RANGED_SLOT != 0 && !auto_repeat {
        cast_time_ms + RANGED_SLOT_CAST_MS
    } else {
        cast_time_ms
    }
}

const EFFECT_APPLY_AURA: i32 = 6;
const EFFECT_PERSISTENT_AREA_AURA: i32 = 27;
const EFFECT_APPLY_AREA_AURA_PARTY: i32 = 35;
const EFFECT_APPLY_AREA_AURA_PET: i32 = 119;

fn is_aura_effect(effect_id: i32) -> bool {
    matches!(
        effect_id,
        EFFECT_APPLY_AURA
            | EFFECT_PERSISTENT_AREA_AURA
            | EFFECT_APPLY_AREA_AURA_PARTY
            | EFFECT_APPLY_AREA_AURA_PET
    )
}

const EFFECT_DISPEL_MECHANIC: i32 = 108;
const MECHANIC_MOUNT: u32 = 21;
const MECHANIC_BANDAGE: u32 = 16;
const RECENTLY_BANDAGED: u32 = 11196;

fn dismount_effect_kind(effect_id: i32, misc: u32) -> Option<u8> {
    if effect_id == EFFECT_DISPEL_MECHANIC && misc == MECHANIC_MOUNT {
        Some(E_DISMOUNT)
    } else {
        None
    }
}

// EffectBasePoints stores the nominal integer amount minus one.
fn dbc_flat_amount(raw: i32) -> i32 {
    raw + 1
}

fn proc_header_fields(flags: u32, chance: u32, charges: u32) -> (u32, u8, u8) {
    (flags, chance.min(255) as u8, charges.min(255) as u8)
}

fn aura_interrupt_bits(raw: u32) -> u16 {
    u16::from(raw & 0x2 != 0) | (u16::from(raw & 0x8 != 0) << 1)
}

fn spell_flag_attributes(name: &str) -> u32 {
    let mut bits = 0u32;
    if matches!(name, "Backstab" | "Garrote" | "Ambush") {
        bits |= SPELL_ATTR_REQ_BEHIND;
    }
    if name == "Backstab" {
        bits |= SPELL_ATTR_REQ_DAGGER;
    }
    if matches!(name, "Sap" | "Garrote") {
        bits |= SPELL_ATTR_REQ_STEALTH;
    }
    if name == "Sap" {
        bits |= SPELL_ATTR_INCAP_OPENER;
    }
    if matches!(name, "Sap" | "Pick Pocket" | "Distract") {
        bits |= SPELL_ATTR_STEALTH_SAFE;
    }
    if name == "Slice and Dice" {
        bits |= SPELL_ATTR_FINISHER_DURATION;
    }
    if name == "Overpower" {
        bits |= SPELL_ATTR_REQ_OVERPOWER;
    }
    if name == "Revenge" {
        bits |= SPELL_ATTR_REQ_REVENGE;
    }
    bits
}

fn is_channeled(attributes_ex1: u32, name: &str) -> bool {
    attributes_ex1 & ATTR_EX1_CHANNELED_MASK != 0 || matches!(name, "Arcane Missiles" | "Evocation")
}

fn excludes_eventai_caster(attributes_ex1: u32) -> bool {
    attributes_ex1 & ATTR_EX1_EXCLUDE_CASTER != 0
}

fn is_ranged_auto_repeat(attributes_ex2: u32, name: &str) -> bool {
    attributes_ex2 & ATTR_EX2_AUTOREPEAT != 0 || matches!(name, "Auto Shot" | "Shoot")
}

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

// The client Dummy magnitude names a spell; the curated seal marker needs flat damage.
fn synthetic_seal_effect(spell_id: u32, name: &str, used_index: u8) -> Option<SpellEffectRow> {
    if !(spell_id == 21082 && name == "Seal of the Crusader") {
        return None;
    }
    Some(SpellEffectRow {
        kind: A_SEAL,
        base_points: 20,
        ..blank_effect(spell_id, used_index)
    })
}

// Bandage spells omit their Recently Bandaged debuff from the DBC row.
fn recently_bandaged_effect(s: &Spell, target: u8, used_index: u8) -> Option<SpellEffectRow> {
    (s.mechanic == MECHANIC_BANDAGE).then(|| SpellEffectRow {
        kind: E_TRIGGER,
        target,
        trigger_spell: RECENTLY_BANDAGED,
        ..blank_effect(s.id, used_index)
    })
}

// Power Word: Shield omits its linked Weakened Soul debuff from the DBC row.
fn power_word_shield_p1_override(spell_id: u32, name: &str, kind: u8, p1: i32) -> i32 {
    if spell_id == 17 && name == "Power Word: Shield" && kind == A_ABSORB {
        return 6788;
    }
    p1
}

// Store EffectMultipleValue in hundredths: 0.5 becomes 50.
fn power_burn_ratio_bp(multiple: f32) -> i32 {
    (multiple * 100.0).round() as i32
}

fn instant_effect_to_kind(effect_id: i32) -> u8 {
    match effect_id {
        2 | 31 => E_DAMAGE,
        EFFECT_WEAPON_DAMAGE_NOSCHOOL | EFFECT_WEAPON_DAMAGE | EFFECT_NORMALIZED_WEAPON_DAMAGE => {
            E_WEAPON_STRIKE
        }
        10 => E_HEAL,
        67 => E_HEAL_MAX_HEALTH,
        30 => E_ENERGIZE,
        38 => E_DISPEL,
        64 => E_TRIGGER,
        63 | 91 => E_TAUNT,
        114 => E_TAUNT,
        24 => E_CREATE_ITEM,
        68 => E_INTERRUPT,
        55 => E_TAME_CREATURE,
        101 => E_FEED_PET,
        56 => E_SUMMON_PET,
        62 => E_POWER_BURN,
        83 => E_DUEL,     // Duel p0 carries the duel-flag GameObject entry.
        69 => E_DISTRACT, // Distract effect amounts use seconds.
        33 | 59 => E_OPEN_LOCK,
        99 => E_DISENCHANT,
        80 => E_ADD_COMBO,
        94 => E_SELF_RESURRECT,
        18 | 113 => E_RESURRECT,
        _ => E_SCRIPTED,
    }
}

fn correct_script_effect_kind(name: &str, kind: u8) -> u8 {
    if kind == E_DAMAGE && name == "Eviscerate" {
        return E_FINISHER_DAMAGE;
    }
    if kind == E_TAUNT && name == "Feint" {
        return E_REDUCE_THREAT;
    }
    if kind != E_SCRIPTED && kind != A_FLAG {
        return kind;
    }
    match name {
        "Holy Light" | "Flash of Light" => E_HEAL,
        "Life Tap" => E_CONVERT_RESOURCE,
        "Charge" => E_CHARGE,
        "Blink" => E_BLINK,
        "Seal of Righteousness" => A_SEAL,
        "Stealth" => A_STEALTH,
        "Retaliation" => A_RETALIATE,

        "Battle Stance" | "Defensive Stance" | "Berserker Stance" | "Bear Form"
        | "Dire Bear Form" | "Cat Form" => E_SET_STANCE,
        "Judgement" => E_JUDGEMENT,
        "Sinister Strike" | "Backstab" | "Gouge" | "Garrote" => E_ADD_COMBO,

        "Resurrection" | "Redemption" | "Rebirth" | "Ancestral Spirit" => E_RESURRECT,
        "Pick Pocket" => E_PICKPOCKET,
        "Create Healthstone"
        | "Create Healthstone (Minor)"
        | "Create Healthstone (Lesser)"
        | "Create Healthstone (Greater)"
        | "Create Healthstone (Major)" => E_CREATE_ITEM,
        "Soulstone Resurrection" => A_SELF_RESURRECT,
        _ => kind,
    }
}

fn soulstone_option_spell(aura_spell_id: u32) -> Option<i32> {
    match aura_spell_id {
        20707 => Some(3026),
        20762 => Some(20758),
        20763 => Some(20759),
        20764 => Some(20760),
        20765 => Some(20761),
        _ => None,
    }
}

// Vanilla Stances stores 1 << (form_id - 1).
fn translate_stance_mask(vanilla_mask: u32) -> u8 {
    let mut out: u8 = 0;
    for bit in 0u32..32 {
        if vanilla_mask & (1u32 << bit) == 0 {
            continue;
        }
        if let Some(stance) = form_to_stance(bit as i32 + 1) {
            out |= 1u8 << stance;
        }
    }
    out
}

fn stance_p0(kind: u8, p0: i32) -> i32 {
    if kind == E_SET_STANCE {
        form_to_stance(p0).unwrap_or(p0)
    } else {
        p0
    }
}

fn mount_display_p0(kind: u8, p0: i32, creature_displays: &BTreeMap<u32, u32>) -> i32 {
    if kind != A_MOUNTED {
        return p0;
    }
    creature_displays
        .get(&(p0 as u32))
        .map(|&display| display as i32)
        .unwrap_or(p0)
}

// Aura 43 takes its school from the proc spell header, not effect misc.
fn proc_damage_school_p0(kind: u8, p0: i32, p0_kind: u8, school_mask: u8) -> (i32, u8) {
    if kind == A_PROC_DAMAGE {
        (school_mask as i32, P_SCHOOL_MASK)
    } else {
        (p0, p0_kind)
    }
}

fn aura_mod_to_kind(aura: AuraMod) -> u8 {
    use AuraMod::*;
    match aura {
        PeriodicDamage | PeriodicLeech | PeriodicDamagePercent => A_PERIODIC_DAMAGE,
        PeriodicHeal => A_PERIODIC_HEAL,
        PeriodicEnergize | PeriodicManaLeech | PowerBurnMana => A_PERIODIC_ENERGIZE,
        PeriodicTriggerSpell => E_TRIGGER,
        ProcTriggerSpell => A_PROC_TRIGGER,
        ProcTriggerDamage => A_PROC_DAMAGE,

        AddFlatModifier => A_SPELLMOD_FLAT,
        AddPctModifier => A_SPELLMOD_PCT,

        ModStat => A_MOD_STAT,
        ModPercentStat | ModTotalStatPercentage => A_MOD_STAT_PCT,

        ModResistance
        | ModBaseResistance
        | ModResistancePct
        | ModBaseResistancePct
        | ModResistanceOfStatPercent => A_MOD_RESISTANCE,

        SchoolAbsorb | DamageShield | ManaShield => A_ABSORB,

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
        ModDodgePercent | ModTotalThreat | ModThreat => A_MOD_COMBAT,

        ModIncreaseSpeed | ModDecreaseSpeed | ModSpeedAlways | ModSpeedNotStack => A_MOD_SPEED,
        ModIncreaseMountedSpeed | ModMountedSpeedAlways | ModMountedSpeedNotStack => A_MOD_SPEED,
        ModIncreaseSwimSpeed => A_MOD_SPEED,
        ModAttackspeed
        | ModMeleeHaste
        | ModRangedHaste
        | ModRangedAmmoHaste
        | ModCastingSpeedNotStack => A_MOD_SPEED,

        ModDamagePercentTaken => A_MOD_DAMAGE_TAKEN,

        ModIncreaseHealth | ModIncreaseEnergy | ObsModHealth | ObsModMana => A_MOD_HEALTH_POWER,

        ModStun => A_CONTROL,
        ModRoot => A_CONTROL,
        ModFear => A_CONTROL,
        ModConfuse | ModCharm | ModPossess | ModPacify | ModPacifySilence | AoeCharm => A_CONTROL,

        SchoolImmunity | DamageImmunity | EffectImmunity | StateImmunity | MechanicImmunity
        | MechanicImmunityMask => A_IMMUNITY,

        ModRegenDuringCombat | ModHealthRegenInCombat => A_COMBAT_HEALTH_REGEN_PCT,

        ModRegen => A_PERIODIC_HEAL,
        ModPowerRegen => A_PERIODIC_ENERGIZE,
        ModPowerRegenPercent => A_MOD_POWER_REGEN_PCT,

        ModDisarm => A_DISARM,
        Mounted => A_MOUNTED,
        Transform => A_TRANSFORM,
        ModDetectRange => A_MOD_DETECT_RANGE,

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
        | ModScale
        | ModStalked
        | ModLanguage
        | FarSight
        | WaterBreathing
        | ModHealthRegenPercent
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

        _ => E_SCRIPTED,
    }
}

fn resolve_aura_params(kind: u8, aura: AuraMod, misc: u32) -> (i32, u8) {
    use AuraMod::*;
    match kind {
        A_MOD_STAT | A_MOD_STAT_PCT => {
            let p0 = if misc == u32::MAX {
                i32::from(STAT_ALL)
            } else {
                (misc & 0xFF) as i32
            };
            (p0, P_STAT_ID)
        }
        // SpellModOp, including 0 for damage and 10 for cast time.
        A_SPELLMOD_FLAT | A_SPELLMOD_PCT => (misc as i32, P_SPELLMOD_OP),
        A_MOD_RESISTANCE => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
        A_MOD_DAMAGE_TAKEN => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
        A_ABSORB => match aura {
            ManaShield => (0, P_NONE),
            _ => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
        },
        A_MOD_COMBAT => {
            let field = match aura {
                ModDamageDone
                | ModDamagePercentDone
                | ModDamageDoneCreature
                | ModDamageDoneVersus => i32::from(COMBAT_DMG_DONE),
                ModCritPercent
                | ModSpellCritChance
                | ModSpellCritChanceSchool
                | ModCritPercentVersus => i32::from(COMBAT_CRIT),
                ModHitChance | ModSpellHitChance => i32::from(COMBAT_HIT),
                ModHealing | ModHealingDone => i32::from(COMBAT_SPELL_POWER),
                ModDodgePercent => i32::from(COMBAT_DODGE),
                ModTotalThreat | ModThreat => i32::from(COMBAT_THREAT),
                _ => i32::from(COMBAT_ATTACK_POWER),
            };
            (field, P_COMBAT_FIELD)
        }
        A_MOD_SPEED => {
            let kind_v = match aura {
                ModAttackspeed | ModMeleeHaste | ModRangedHaste | ModRangedAmmoHaste => 1,
                ModCastingSpeedNotStack => 2,
                ModIncreaseSwimSpeed => i32::from(SPEED_SWIM),
                ModSpeedAlways => i32::from(SPEED_MOVE_ALWAYS),
                ModSpeedNotStack => i32::from(SPEED_MOVE_NOT_STACK),
                ModIncreaseMountedSpeed | ModMountedSpeedAlways | ModMountedSpeedNotStack => {
                    i32::from(SPEED_MOUNTED)
                }
                _ => i32::from(SPEED_MOVE),
            };
            (kind_v, P_SPEED_KIND)
        }
        A_MOUNTED => (misc as i32, P_DISPLAY_ID),
        A_TRANSFORM => (misc as i32, P_ENTRY),
        A_PERIODIC_ENERGIZE | A_MOD_POWER_REGEN_PCT => ((misc & 0xFF) as i32, P_POWER_TYPE),
        A_CONTROL => {
            let mech = match aura {
                ModStun => M_STUN,
                ModRoot => M_ROOT,
                ModFear => M_FEAR,
                _ => M_POLY,
            };
            (mech, P_MECHANIC)
        }
        A_IMMUNITY => match aura {
            SchoolImmunity | DamageImmunity => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
            MechanicImmunity | MechanicImmunityMask => (misc as i32, P_MECHANIC),
            _ => (misc as i32, P_RAW),
        },
        A_FLAG => ((misc & 0xFF) as i32, P_FLAG),
        E_TRIGGER => (0, P_RAW), // Trigger spell ids live in trigger_spell, outside p0.
        A_PROC_TRIGGER => (0, P_RAW),
        A_PROC_DAMAGE => (0, P_NONE),
        E_SCRIPTED => (0, P_RAW),
        _ => (0, P_NONE),
    }
}

fn is_reducing_modifier_aura(effect_id: i32, aura: AuraMod, base_points: i32) -> bool {
    is_aura_effect(effect_id)
        && base_points < 0
        && matches!(
            aura_mod_to_kind(aura),
            A_MOD_STAT | A_MOD_RESISTANCE | A_MOD_COMBAT
        )
}

fn resolve_instant_params(kind: u8, misc: u32, item_type: i32) -> (i32, u8) {
    match kind {
        E_ENERGIZE => ((misc & 0xFF) as i32, P_POWER_TYPE),
        E_DISPEL => ((misc & 0x7F) as i32, P_SCHOOL_MASK),
        E_CREATE_ITEM => (item_type, P_ITEM_ENTRY),
        E_SUMMON_PET => (misc as i32, P_ENTRY),
        E_DUEL => (misc as i32, P_GAMEOBJECT_ENTRY),
        E_SELF_RESURRECT => (misc as i32, P_FLAT_MANA),
        _ => (0, P_NONE),
    }
}

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

fn compatible_header_target(effect_id: i32, kind: u8, flags: u32, is_negative: bool) -> Option<u8> {
    if kind == E_SCRIPTED {
        return None;
    }
    let accepted = match effect_id {
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

fn supported_effect_default(effect_id: i32, kind: u8, is_negative: bool) -> u8 {
    if kind == E_SCRIPTED {
        return T_SCRIPTED;
    }
    let default = match effect_id {
        2 => 6,
        6 | 34 | 46 | 47 | 61 | 79 => 1,
        11 | 18 | 24 | 36 | 44 | 85 | 116 | 118 | 123 => 25,
        28 | 50 => 18,
        57 => 5,
        35 | 119 => return T_SELF,
        _ => 0,
    };
    resolve_target(default, is_negative)
}

#[derive(Default)]
pub(crate) struct Coverage {
    spells: usize,
    effects: usize,
    real: usize,
    scripted: usize,
    by_kind: BTreeMap<u8, usize>,
    unmapped_aura: BTreeMap<String, usize>,
    unmapped_effect: BTreeMap<i32, usize>,
}

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
    let creature_displays: BTreeMap<u32, u32> = BTreeMap::new();

    for s in &dbc.spells {
        let spell_id = s.id;
        if spell_id == 0 {
            continue;
        }
        if !allow.is_empty() && !allow.contains(&spell_id) {
            continue;
        }

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

pub(crate) struct SpellCatalogue {
    pub(crate) ids: std::collections::HashSet<u64>,
    pub(crate) proc_effects: std::collections::HashMap<u32, Vec<usize>>,
}

pub(crate) fn read_catalogue(data_dir: &str) -> Result<SpellCatalogue> {
    let mut chain = open_chain(Path::new(data_dir))?;
    let spells = read_spells(&chain.read_file("DBFilesClient\\Spell.dbc")?)?;
    let ids = spells.iter().map(|spell| u64::from(spell.id)).collect();
    let proc_effects = spells
        .iter()
        .map(|spell| {
            let effects = (0..3)
                .filter(|&index| {
                    is_aura_effect(spell.effect[index])
                        && matches!(
                            AuraMod::from_int(spell.effect_aura[index]),
                            Ok(AuraMod::ProcTriggerSpell | AuraMod::ProcTriggerDamage)
                        )
                })
                .collect();
            (spell.id, effects)
        })
        .collect();
    Ok(SpellCatalogue { ids, proc_effects })
}

struct SpellDbc {
    spells: Vec<Spell>,
    cast_times: SpellCastTimes,
    ranges: SpellRange,
    durations: SpellDuration,
    radii: SpellRadius,
}

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

#[derive(Default)]
struct SpellAccumulator {
    cov: Coverage,
    spell_rows: Vec<SpellHeaderRow>,
    effect_rows: Vec<SpellEffectRow>,
    reagent_rows: Vec<String>,
    eventai_metadata_rows: Vec<String>,
    samples: Vec<String>,
    spell_levels: BTreeMap<u32, u8>,
    wrapper_to_rank: BTreeMap<u32, u32>,
}

struct SpellHeader {
    name: String,
    school_mask: u8,
    is_negative: bool,
    channeled: bool,
}

struct ResolvedEffect {
    kind: u8,
    p0: i32,
    p0_kind: u8,
    base_points: i32,
    die_sides: i32,
    per_level: f32,
    period_ms: u32,
}

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
    let auto_repeat = is_ranged_auto_repeat(s.attributes_ex2, &name);
    let cast_time_ms = dbc
        .cast_times
        .get(SpellCastTimesKey::new(s.casting_time_index))
        .map(|r| r.base.max(0) as u32)
        .unwrap_or(0);
    let cast_time_ms = ranged_slot_cast_time(cast_time_ms, s.attributes, auto_repeat);
    // Either spell or category recovery can carry the real cooldown.
    let cooldown_ms = s.recovery_time.max(s.category_recovery_time);
    let range_yd = dbc
        .ranges
        .get(SpellRangeKey::new(s.range))
        .map(|r| r.range_max.max(0.0) as u32)
        .unwrap_or(0);
    // DBC duration -1 means infinite; u32::MAX preserves that sentinel.
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
    // Spell.dbc school is a Resistances.dbc index, not a mask.
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
    let gcd_ms: u32 = if (attributes & 0x40) != 0 || (attributes & 0x4) != 0 {
        0
    } else {
        1500
    };
    let channeled = is_channeled(s.attributes_ex1, &name);
    if excludes_eventai_caster(s.attributes_ex1) {
        eventai_metadata_rows.push(format!("({spell_id},true)"));
    }
    let mut cast_flags = spell_flag_attributes(&name);
    if channeled {
        cast_flags |= SPELL_ATTR_CHANNELED;
    }
    if auto_repeat {
        cast_flags |= SPELL_ATTR_RANGED_AUTO_REPEAT;
    }
    let stances = translate_stance_mask(s.shapeshift_mask);
    let spell_level = s.spell_level.min(255) as u8;
    spell_levels.insert(spell_id, spell_level);
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

fn spell_is_negative(s: &Spell) -> bool {
    let mut is_negative = false;
    for i in 0..3 {
        let eff = s.effect[i];
        if eff == 0 || (s.implicit_target_a[i] == 1 && s.implicit_target_b[i] == 0) {
            continue;
        }
        let aura = AuraMod::from_int(s.effect_aura[i]).unwrap_or_default();
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
        if is_reducing_modifier_aura(eff, aura, s.effect_base_points[i]) {
            is_negative = true;
        }
    }
    is_negative
}

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
    let resolved: Vec<(usize, ResolvedEffect)> = (0..3)
        .filter(|&i| s.effect[i] != 0)
        .map(|i| (i, resolve_effect_kind(s, i, header, creature_displays, cov)))
        .collect();
    let effect_kinds: Vec<u8> = resolved.iter().map(|(_, effect)| effect.kind).collect();
    let queued_strike = queues_on_next_swing(s.attributes, header.school_mask, &effect_kinds);
    let strike = if queued_strike {
        None
    } else {
        weapon_strike_row(s, &resolved)
    };
    let mut used_slots = [false; 4];
    let mut healed_target = T_SELF;
    for (i, effect) in resolved {
        let effect_id = s.effect[i];
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
        } = effect;
        let strike_row = strike
            .as_ref()
            .filter(|_| is_weapon_damage_effect(effect_id, kind));
        if strike_row.is_some_and(|row| row.effect_index != i) {
            continue;
        }
        let (kind, base_points, die_sides, per_level, p0, p0_kind) = match strike_row {
            Some(strike) => (
                E_WEAPON_STRIKE,
                strike.base_points,
                strike.die_sides,
                strike.per_level,
                strike.percent,
                P_RAW,
            ),
            None if queued_strike && kind == E_WEAPON_STRIKE => {
                (E_NEXT_SWING, base_points, die_sides, per_level, p0, p0_kind)
            }
            None => (kind, base_points, die_sides, per_level, p0, p0_kind),
        };
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
        if kind == A_PERIODIC_HEAL {
            healed_target = target;
        }
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
        if lyracore_shared::trainer::learn_target_trigger(kind, trigger_spell).is_some() {
            wrapper_to_rank.entry(spell_id).or_insert(trigger_spell);
        }
        let effect_mechanic = s.effect_mechanic[i] as u8;
        let p1 = if kind == E_SUMMON_HOSTILE {
            s.requires_spell_focus as i32
        } else if kind == E_POWER_BURN {
            power_burn_ratio_bp(s.effect_multiple_values[i])
        } else if kind == A_SPELLMOD_FLAT || kind == A_SPELLMOD_PCT {
            s.effect_item_type[i]
        } else if let Some(row) = strike_row {
            i32::from(row.normalized)
        } else {
            power_word_shield_p1_override(spell_id, name, kind, 0i32)
        };
        let script_id = 0u32;
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
            samples.push(format!(
                    "    eff{effect_index}: {kn} (0x{kind:02X}) base={base_points} die={die_sides} period_ms={period_ms} target={target} p0={p0} p0_kind={p0_kind} trigger={trigger_spell}",
                    kn = kind_name(kind),
                ));
        } else if kind != E_SCRIPTED && samples.len() < 8 {
            samples.push(format!(
                    "  spell {spell_id} '{}' eff{effect_index}: kind=0x{kind:02X} base={base_points} period_ms={period_ms} target={target} p0={p0} p0_kind={p0_kind}",
                    name.chars().take(28).collect::<String>(),
                ));
        }
    }

    let synth_slot = used_slots.iter().position(|&u| !u).unwrap_or(3) as u8;
    let synth = synthetic_seal_effect(spell_id, name, synth_slot)
        .or_else(|| recently_bandaged_effect(s, healed_target, synth_slot));
    if let Some(synth) = synth {
        cov.effects += 1;
        cov.real += 1;
        *cov.by_kind.entry(synth.kind).or_default() += 1;
        if !allow.is_empty() {
            samples.push(format!("    [synthetic+curated] {}", synth.sql_values()));
        }
        effect_rows.push(synth);
    }
}

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
        let k = if effect_id == 5 && (s.implicit_target_a[i] == 9 || s.implicit_target_b[i] == 9) {
            E_RECALL_HOME
        } else {
            instant_effect_to_kind(effect_id)
        };
        if k == E_SCRIPTED {
            *cov.unmapped_effect.entry(effect_id).or_default() += 1;
        }
        (
            k,
            resolve_instant_params(k, s.effect_misc_value[i], s.effect_item_type[i]),
        )
    };
    let kind = if name == "Stealth" && aura != AuraMod::ModStealth {
        kind
    } else {
        correct_script_effect_kind(name, kind)
    };
    let p0 = if GOUGE_RANK_IDS.contains(&s.id) && name == "Gouge" && kind == A_CONTROL {
        M_POLY
    } else {
        p0
    };

    let kind = dismount_effect_kind(effect_id, s.effect_misc_value[i]).unwrap_or(kind);

    let p0 = stance_p0(kind, p0);

    let p0 = mount_display_p0(kind, p0, creature_displays);

    let (p0, p0_kind) = proc_damage_school_p0(kind, p0, p0_kind, school_mask);

    // Healthstone script effects omit effect_item_type; supply their item entries here.
    let (p0, p0_kind) = if kind == E_CREATE_ITEM {
        let item = match name {
            "Create Healthstone (Minor)" => Some(5512),
            "Create Healthstone (Lesser)" => Some(5511),
            "Create Healthstone" => Some(5509),
            "Create Healthstone (Greater)" => Some(5510),
            "Create Healthstone (Major)" => Some(9421),
            _ => None,
        };
        match item {
            Some(entry) => (entry, P_ITEM_ENTRY),
            None => (p0, p0_kind),
        }
    } else {
        (p0, p0_kind)
    };

    let (p0, p0_kind) = if kind == A_SELF_RESURRECT {
        (soulstone_option_spell(s.id).unwrap_or(0), P_SPELL_ID)
    } else {
        (p0, p0_kind)
    };

    let base_points = dbc_flat_amount(s.effect_base_points[i]);
    // DBC dodge modifiers use percent; durable combat values use basis points.
    let base_points = if kind == A_MOD_COMBAT && p0 == i32::from(COMBAT_DODGE) {
        base_points * 100
    } else {
        base_points
    };
    let die_sides = s.effect_die_sides[i];
    let per_level = s.effect_real_points_per_level[i];
    let period_ms = s.effect_amplitude[i];
    // Vanilla ModRegen and ModPowerRegen tick every five seconds even when amplitude is zero.
    let period_ms = if matches!(
        (kind, aura),
        (A_PERIODIC_HEAL, AuraMod::ModRegen) | (A_PERIODIC_ENERGIZE, AuraMod::ModPowerRegen)
    ) {
        5000
    } else {
        period_ms
    };
    let kind = if channeled && kind == E_TRIGGER && period_ms > 0 {
        A_PERIODIC_TRIGGER
    } else {
        kind
    };
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
    let target = match kind {
        E_CHARGE | E_JUDGEMENT | E_PICKPOCKET | E_INTERRUPT | E_NEXT_SWING | E_TAUNT
        | E_TAME_CREATURE => T_TARGET_ENEMY,
        E_RESURRECT => T_TARGET_ALLY,
        E_REDUCE_THREAT | E_BLINK | E_PERSISTENT_AREA | E_SUMMON_PET | E_SUMMON_HOSTILE
        | E_SELF_RESURRECT | E_RECALL_HOME => T_SELF,
        E_DUEL => T_TARGET_ANY,
        _ => target,
    };
    let target = if name == "Slice and Dice" && target == T_TARGET_ALLY {
        T_TARGET_ENEMY
    } else {
        target
    };
    let target = if name == "Flamestrike" && kind == E_DAMAGE {
        T_AREA_ENEMY
    } else {
        target
    };
    if channeled && kind == A_FLAG {
        T_SELF
    } else {
        target
    }
}

fn trainer_offering_rows(
    trainers: &[(u32, Vec<u32>)],
    spell_levels: &BTreeMap<u32, u8>,
    wrapper_to_rank: &BTreeMap<u32, u32>,
) -> Vec<String> {
    let mut trainer_rows: Vec<String> = Vec::new();
    let mut trainer_missing: Vec<u32> = Vec::new();
    for (entry, ids) in trainers {
        let offered_triggers: std::collections::HashSet<u32> = ids
            .iter()
            .filter_map(|id| wrapper_to_rank.get(id).copied())
            .collect();
        for &spell_id in ids {
            if offered_triggers.contains(&spell_id) {
                continue;
            }
            // LearnSpell wrappers have level zero; use the taught rank for price and level.
            let rank = wrapper_to_rank.get(&spell_id).copied().unwrap_or(spell_id);
            match spell_levels.get(&rank) {
                Some(&lvl) => {
                    let cost = trainer_cost(lvl);
                    let id = CURATED_TRAINER_ID_BASE + trainer_rows.len() as u64;
                    trainer_rows.push(format!("({id},{entry},{spell_id},{cost},{lvl},0,75)"));
                }
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

fn fishing_marker_rows() -> Vec<SpellEffectRow> {
    [7620u32, 7731, 7732]
        .into_iter()
        .map(|spell_id| SpellEffectRow {
            kind: E_FISH,
            p0_kind: 255,
            ..blank_effect(spell_id, 0)
        })
        .collect()
}

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

pub(crate) fn build_spell_sql(
    data_dir: &Path,
    only: &[u32],
    trainers: &[(u32, Vec<u32>)],
) -> Result<(Vec<String>, Coverage, Vec<String>)> {
    let (rows, cov, samples) = build_spell_rows(data_dir, only, trainers)?;
    let sql = assemble_spell_sql(&rows, only, !trainers.is_empty());
    Ok((sql, cov, samples))
}

const CURATED_TRAINER_ID_BASE: u64 = 5_200_000;
const CURATED_TRAINER_ID_SPAN: u64 = 500;

const SYNTHETIC_SPELL_ID_FLOOR: u32 = 50_000;

fn spell_delete_statements(only: &[u32]) -> Vec<String> {
    if only.is_empty() {
        return vec![
            format!("DELETE FROM game_spell WHERE spell_id < {SYNTHETIC_SPELL_ID_FLOOR}"),
            format!("DELETE FROM game_spell_effect WHERE spell_id < {SYNTHETIC_SPELL_ID_FLOOR}"),
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

fn trainer_cost(required_level: u8) -> u32 {
    (required_level.max(1) as u32) * 50
}

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
        E_RECALL_HOME => "E_RECALL_HOME",
        E_PERSISTENT_AREA => "E_PERSISTENT_AREA",
        E_OPEN_LOCK => "E_OPEN_LOCK",
        E_CONVERT_RESOURCE => "E_CONVERT_RESOURCE",
        E_JUDGEMENT => "E_JUDGEMENT",
        E_ADD_COMBO => "E_ADD_COMBO",
        E_FINISHER_DAMAGE => "E_FINISHER_DAMAGE",
        E_RESURRECT => "E_RESURRECT",
        E_PICKPOCKET => "E_PICKPOCKET",
        E_INTERRUPT => "E_INTERRUPT",
        E_REDUCE_THREAT => "E_REDUCE_THREAT",
        E_NEXT_SWING => "E_NEXT_SWING",
        E_WEAPON_STRIKE => "E_WEAPON_STRIKE",
        E_SET_STANCE => "E_SET_STANCE",
        E_SUMMON_PET => "E_SUMMON_PET",
        E_SUMMON_HOSTILE => "E_SUMMON_HOSTILE",
        E_DISTRACT => "E_DISTRACT",
        E_HEAL_MAX_HEALTH => "E_HEAL_MAX_HEALTH",
        E_TAME_CREATURE => "E_TAME_CREATURE",
        E_FEED_PET => "E_FEED_PET",
        E_POWER_BURN => "E_POWER_BURN",
        E_DUEL => "E_DUEL",
        E_SELF_RESURRECT => "E_SELF_RESURRECT",
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
        A_SELF_RESURRECT => "A_SELF_RESURRECT",
        A_MOD_POWER_REGEN_PCT => "A_MOD_POWER_REGEN_PCT",
        _ => "?",
    }
}

fn print_coverage(cov: &Coverage) {
    let pct = |n: usize, d: usize| {
        if d == 0 {
            0.0
        } else {
            100.0 * n as f64 / d as f64
        }
    };
    println!("\n=== SPELL TAXONOMY COVERAGE REPORT ===");

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

pub(crate) struct SpellArgs {
    pub(crate) only: Vec<u32>,
    pub(crate) trainers: Vec<(u32, Vec<u32>)>,
    pub(crate) source_sha: String,
    pub(crate) packages: Option<String>,
    pub(crate) scope: WorldImportScope,
    pub(crate) apply: bool,
    pub(crate) shard: Shard,
}

pub fn run_spells(data_dir: &str, args: &SpellArgs) -> Result<()> {
    let (stmts, cov, samples) = build_spell_sql(Path::new(data_dir), &args.only, &args.trainers)?;

    if !args.only.is_empty() && cov.spells != args.only.len() {
        eprintln!(
            "spells: WARNING — --only requested {} ids but matched {} in Spell.dbc (some id not found).",
            args.only.len(),
            cov.spells,
        );
    }

    if args.apply {
        args.shard.run_sql_statements(&stmts, "spells")?;
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
        args.shard
            .stamp_family("spell", &args.source_sha, "", cov.spells as u64)
            .context("stamp_import_meta(spell)")?;
    } else if !args.only.is_empty() {
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

    match &args.packages {
        Some(root) => package_delta::reapply(&args.scope, args.apply, &args.shard, "spell", root)?,
        None => package_delta::warn_not_reapplied("spell"),
    }

    if !args.trainers.is_empty() {
        println!("\n=== TRAINER OFFERINGS (game_trainer_spell) ===");
        let mut total = 0usize;
        for (entry, _ids) in &args.trainers {
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

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC with the owned build 5875 client"]
    fn actual_distract_imports_one_ground_area_effect_and_a_stealth_safe_header() {
        let dir = std::env::var("LYRACORE_TEST_DBC").expect("set LYRACORE_TEST_DBC");
        let (rows, _, _) = build_spell_rows(Path::new(&dir), &[1725], &[]).unwrap();
        let effects = rows
            .effects
            .iter()
            .filter(|effect| effect.spell_id == 1725)
            .map(|e| {
                (
                    e.effect_index,
                    e.kind,
                    e.target,
                    e.radius_yd,
                    e.base_points,
                    e.die_sides,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(effects, [(0, E_DISTRACT, T_AREA_ENEMY, 10.0, 10, 1)]);
        let header = rows.headers.iter().find(|s| s.spell_id == 1725).unwrap();
        assert_eq!(
            (
                header.range_yd,
                header.power_type,
                header.cost,
                header.cooldown_ms,
                header.cast_time_ms
            ),
            (30, 3, 30, 30_000, 0)
        );
        assert_ne!(header.cast_flags & SPELL_ATTR_STEALTH_SAFE, 0);
    }

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC with the owned build 5875 client"]
    fn actual_adrenaline_rush_doubles_energy_regeneration() {
        let dir = std::env::var("LYRACORE_TEST_DBC").expect("set LYRACORE_TEST_DBC");
        let (rows, _, _) = build_spell_rows(Path::new(&dir), &[13750], &[]).unwrap();
        let effects = rows
            .effects
            .iter()
            .filter(|effect| effect.spell_id == 13750)
            .map(|e| (e.kind, e.base_points, e.p0, e.p0_kind))
            .collect::<Vec<_>>();
        assert_eq!(effects, [(A_MOD_POWER_REGEN_PCT, 100, 3, P_POWER_TYPE)]);
        let header = rows.headers.iter().find(|s| s.spell_id == 13750).unwrap();
        assert_eq!((header.duration_ms, header.cooldown_ms), (15_000, 300_000));
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
        for target_a in [
            9, 10, 17, 18, 22, 32, 39, 41, 42, 43, 44, 46, 47, 48, 49, 50, 53, 55, 63,
        ] {
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
        assert_eq!(aura_mod_to_kind(ModDodgePercent), A_MOD_COMBAT);
        assert_eq!(aura_mod_to_kind(ModTotalThreat), A_MOD_COMBAT);
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModDodgePercent, 0).0,
            i32::from(COMBAT_DODGE)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModTotalThreat, 0).0,
            i32::from(COMBAT_THREAT)
        );
    }

    #[test]
    fn curated_correction_reclassifies_script_effects_by_name() {
        assert_eq!(correct_script_effect_kind("Holy Light", E_SCRIPTED), E_HEAL);
        assert_eq!(
            correct_script_effect_kind("Life Tap", E_SCRIPTED),
            E_CONVERT_RESOURCE
        );
        assert_eq!(correct_script_effect_kind("Charge", E_SCRIPTED), E_CHARGE);
        assert_eq!(
            correct_script_effect_kind("Feint", E_TAUNT),
            E_REDUCE_THREAT
        );
        assert_eq!(correct_script_effect_kind("Growl", E_TAUNT), E_TAUNT);
        assert_eq!(
            correct_script_effect_kind("Garrote", E_SCRIPTED),
            E_ADD_COMBO
        );
        assert_eq!(
            correct_script_effect_kind("Sinister Strike", E_SCRIPTED),
            E_ADD_COMBO
        );
        assert_eq!(
            correct_script_effect_kind("Backstab", E_SCRIPTED),
            E_ADD_COMBO
        );
        assert_eq!(correct_script_effect_kind("Gouge", E_SCRIPTED), E_ADD_COMBO);
        assert_eq!(
            correct_script_effect_kind("Pick Pocket", E_SCRIPTED),
            E_PICKPOCKET
        );
        assert_eq!(correct_script_effect_kind("Holy Light", E_HEAL), E_HEAL);
        assert_eq!(correct_script_effect_kind("Frostbolt", E_DAMAGE), E_DAMAGE);
        assert_eq!(
            correct_script_effect_kind("Some Other Spell", E_SCRIPTED),
            E_SCRIPTED
        );
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
        assert_eq!(
            correct_script_effect_kind("Eviscerate", E_DAMAGE),
            E_FINISHER_DAMAGE
        );
        assert_eq!(correct_script_effect_kind("Frostbolt", E_DAMAGE), E_DAMAGE);
    }

    #[test]
    fn rez_name_rescue_covers_every_class_resurrect_and_guards_others() {
        assert_eq!(
            correct_script_effect_kind("Resurrection", E_SCRIPTED),
            E_RESURRECT
        );
        assert_eq!(
            correct_script_effect_kind("Redemption", E_SCRIPTED),
            E_RESURRECT
        );
        assert_eq!(
            correct_script_effect_kind("Rebirth", E_SCRIPTED),
            E_RESURRECT
        );
        assert_eq!(
            correct_script_effect_kind("Ancestral Spirit", E_SCRIPTED),
            E_RESURRECT
        );

        assert_eq!(
            correct_script_effect_kind("Reincarnation", E_SCRIPTED),
            E_SCRIPTED
        );
    }

    const SOULSTONE_RANKS: [(u32, u32, i32, u32); 5] = [
        (20707, 3026, -401, 700),
        (20762, 20758, -751, 1200),
        (20763, 20759, -1101, 1700),
        (20764, 20760, -1601, 2200),
        (20765, 20761, -2201, 2800),
    ];

    fn soulstone_aura(id: u32) -> Spell {
        Spell {
            id,
            name: "Soulstone Resurrection".into(),
            effect: [EFFECT_APPLY_AURA, 0, 0],
            effect_aura: [AuraMod::Dummy as u32, 0, 0],
            implicit_target_a: [21, 0, 0],
            ..Default::default()
        }
    }

    fn self_resurrect_spell(id: u32, raw_base: i32, misc: u32) -> Spell {
        Spell {
            id,
            name: "Use Soulstone".into(),
            effect: [94, 0, 0],
            effect_base_points: [raw_base, 0, 0],
            effect_die_sides: [1, 0, 0],
            effect_misc_value: [misc, 0, 0],
            implicit_target_a: [1, 0, 0],
            ..Default::default()
        }
    }

    fn first_effect(rows: &SpellRows, spell_id: u32) -> &SpellEffectRow {
        rows.effects
            .iter()
            .find(|effect| effect.spell_id == spell_id)
            .unwrap()
    }

    #[test]
    fn drink_restores_mana_every_five_seconds() {
        let mut dbc = fixture_tables();
        dbc.spells = vec![Spell {
            id: 430,
            name: "Drink".into(),
            effect: [EFFECT_APPLY_AURA, 0, 0],
            effect_aura: [AuraMod::ModPowerRegen as u32, 0, 0],
            implicit_target_a: [1, 0, 0],
            ..Default::default()
        }];

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        let drink = first_effect(&rows, 430);
        assert_eq!(
            (drink.kind, drink.period_ms, drink.p0, drink.p0_kind),
            (A_PERIODIC_ENERGIZE, 5000, 0, P_POWER_TYPE)
        );
    }

    #[test]
    fn bandage_applies_recently_bandaged_to_the_unit_it_heals() {
        let mut dbc = fixture_tables();
        dbc.spells = vec![Spell {
            id: 746,
            name: "First Aid".into(),
            mechanic: 16,
            effect: [EFFECT_APPLY_AURA, 0, 0],
            effect_aura: [AuraMod::PeriodicHeal as u32, 0, 0],
            implicit_target_a: [21, 0, 0],
            ..Default::default()
        }];

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        let trigger = rows
            .effects
            .iter()
            .find(|effect| effect.spell_id == 746 && effect.effect_index == 1)
            .unwrap();
        assert_eq!(
            (trigger.kind, trigger.trigger_spell, trigger.target),
            (E_TRIGGER, 11196, T_TARGET_ALLY)
        );
    }

    #[test]
    fn every_soulstone_aura_rank_names_its_self_resurrect_spell() {
        let mut dbc = fixture_tables();
        dbc.spells = SOULSTONE_RANKS
            .iter()
            .map(|&(aura, ..)| soulstone_aura(aura))
            .chain([soulstone_aura(999_998)])
            .collect();

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        for (aura, option, ..) in SOULSTONE_RANKS {
            let effect = first_effect(&rows, aura);
            assert_eq!(
                (effect.kind, effect.p0, effect.p0_kind),
                (A_SELF_RESURRECT, option as i32, P_SPELL_ID),
                "aura rank {aura}"
            );
        }
        let unknown = first_effect(&rows, 999_998);
        assert_eq!((unknown.kind, unknown.p0), (A_SELF_RESURRECT, 0));
    }

    #[test]
    fn self_resurrect_effects_keep_the_vanilla_sign_rule() {
        let mut dbc = fixture_tables();
        dbc.spells = SOULSTONE_RANKS
            .iter()
            .map(|&(_, option, raw_base, mana)| self_resurrect_spell(option, raw_base, mana))
            .chain([self_resurrect_spell(21169, 19, 0)])
            .collect();

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        let expected = [
            (3026, -400, 700),
            (20758, -750, 1200),
            (20759, -1100, 1700),
            (20760, -1600, 2200),
            (20761, -2200, 2800),
            (21169, 20, 0),
        ];
        for (spell_id, base_points, mana) in expected {
            let effect = first_effect(&rows, spell_id);
            assert_eq!(
                (
                    effect.kind,
                    effect.base_points,
                    effect.p0,
                    effect.p0_kind,
                    effect.target
                ),
                (E_SELF_RESURRECT, base_points, mana, P_FLAT_MANA, T_SELF),
                "spell {spell_id}"
            );
        }
    }

    #[test]
    fn stance_p0_remaps_form_id_to_zero_based_stance() {
        assert_eq!(stance_p0(E_SET_STANCE, 17), 0);
        assert_eq!(stance_p0(E_SET_STANCE, 18), 1);
        assert_eq!(stance_p0(E_SET_STANCE, 19), 2);
        assert_eq!(stance_p0(E_SET_STANCE, 5), 3);
        assert_eq!(stance_p0(E_SET_STANCE, 1), 4);
        assert_eq!(stance_p0(E_SET_STANCE, 8), 5);
        assert_eq!(stance_p0(E_DAMAGE, 17), 17);
        assert_eq!(stance_p0(E_SET_STANCE, 4), 4);
    }

    #[test]
    fn druid_form_switches_name_rescue_to_set_stance() {
        assert_eq!(
            correct_script_effect_kind("Bear Form", A_FLAG),
            E_SET_STANCE
        );
        assert_eq!(
            correct_script_effect_kind("Dire Bear Form", A_FLAG),
            E_SET_STANCE
        );
        assert_eq!(correct_script_effect_kind("Cat Form", A_FLAG), E_SET_STANCE);
        assert_eq!(correct_script_effect_kind("Aquatic Form", A_FLAG), A_FLAG);
        assert_eq!(correct_script_effect_kind("Travel Form", A_FLAG), A_FLAG);
        assert_eq!(correct_script_effect_kind("Moonkin Form", A_FLAG), A_FLAG);
    }

    #[test]
    fn translate_stance_mask_folds_vanilla_form_bits_to_zero_based() {
        assert_eq!(translate_stance_mask(0), 0);
        assert_eq!(translate_stance_mask(0x10000), 0x01);
        assert_eq!(translate_stance_mask(0x30000), 0x03);
        assert_eq!(translate_stance_mask(0x50000), 0x05);
        assert_eq!(translate_stance_mask(0x40000), 0x04);
        assert_eq!(translate_stance_mask(0x90), 0x28);
        assert_eq!(translate_stance_mask(0x10), 0x08);
        assert_eq!(translate_stance_mask(0x1), 0x10);
        assert_eq!(translate_stance_mask(0x10090), 0x01 | 0x28);
        assert_eq!(translate_stance_mask(0x4), 0);
        assert_eq!(translate_stance_mask(0x40), 0);
        assert_eq!(translate_stance_mask(0x8000), 0);
    }

    #[test]
    fn rogue_cast_gate_flags_set_by_name() {
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
        assert_eq!(
            spell_flag_attributes("Backstab") & SPELL_ATTR_REQ_DAGGER,
            SPELL_ATTR_REQ_DAGGER
        );
        assert_eq!(spell_flag_attributes("Garrote") & SPELL_ATTR_REQ_DAGGER, 0);
        assert_eq!(spell_flag_attributes("Ambush") & SPELL_ATTR_REQ_DAGGER, 0);
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
        assert_eq!(
            spell_flag_attributes("Pick Pocket"),
            SPELL_ATTR_STEALTH_SAFE
        );
        assert_eq!(
            spell_flag_attributes("Slice and Dice"),
            SPELL_ATTR_FINISHER_DURATION
        );
        assert_eq!(spell_flag_attributes("Fireball"), 0);
    }

    #[test]
    fn ranged_auto_repeat_flagged_by_dbc_bit_or_name() {
        assert!(is_ranged_auto_repeat(ATTR_EX2_AUTOREPEAT, "Whatever"));
        assert!(is_ranged_auto_repeat(0, "Auto Shot"));
        assert!(is_ranged_auto_repeat(0, "Shoot"));
        assert!(!is_ranged_auto_repeat(0, "Fireball"));
        assert!(!is_ranged_auto_repeat(0x1, "Frostbolt"));
    }

    fn melee_ability(id: u32, name: &str, attributes: u32, effect: [i32; 3]) -> Spell {
        Spell {
            id,
            name: name.into(),
            attributes,
            effect,
            effect_aura: effect.map(|id| {
                if id == EFFECT_APPLY_AURA {
                    AuraMod::ModStun as u32
                } else {
                    0
                }
            }),
            implicit_target_a: [6, 6, 0],
            ..Default::default()
        }
    }

    #[test]
    fn the_on_next_swing_attribute_queues_every_weapon_strike() {
        let mut dbc = fixture_tables();
        dbc.spells = vec![
            melee_ability(78, "Heroic Strike", 0x0005_0014, [17, 0, 0]),
            melee_ability(2973, "Raptor Strike", 0x0005_0404, [58, 0, 0]),
            melee_ability(6807, "Maul", 0x0000_0414, [58, 0, 0]),
            melee_ability(20666, "Cleave", 0x0004_0014, [58, 98, 0]),
            melee_ability(14112, "Flaying Vine", 0x0004_0410, [58, 0, 0]),
            melee_ability(3242, "Ravage", 0x0000_0014, [58, 6, 0]),
            melee_ability(678, "zzOLDHoly Strike", 0x0000_0404, [2, 0, 0]),
            Spell {
                school: 1,
                ..melee_ability(13953, "Holy Strike", 0x0005_0404, [58, 0, 0])
            },
            melee_ability(1752, "Sinister Strike", 0x0005_0010, [17, 0, 0]),
        ];

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        let kind = |id| first_effect(&rows, id).kind;
        for queued in [78, 2973, 6807, 20666, 14112] {
            assert_eq!(kind(queued), E_NEXT_SWING, "spell {queued}");
            assert_eq!(first_effect(&rows, queued).target, T_TARGET_ENEMY);
        }
        assert_eq!(
            kind(3242),
            E_WEAPON_STRIKE,
            "a strike with a live aura stays instant"
        );
        assert_eq!(kind(678), E_DAMAGE, "school damage is not a weapon swing");
        assert_eq!(
            kind(13953),
            E_WEAPON_STRIKE,
            "a holy weapon strike is not a physical swing"
        );
        assert_eq!(kind(1752), E_WEAPON_STRIKE, "no on-next-swing attribute");
    }

    #[test]
    fn every_weapon_effect_of_an_instant_spell_folds_into_one_strike() {
        let mut dbc = fixture_tables();
        dbc.spells = vec![
            Spell {
                effect_base_points: [9, 149, 0],
                effect_die_sides: [1, 1, 1],
                ..melee_ability(53, "Backstab", 0x0005_0010, [121, 31, 80])
            },
            Spell {
                effect_base_points: [149, 0, 0],
                effect_die_sides: [1, 0, 0],
                ..melee_ability(14251, "Riposte", 0x0005_0010, [31, 6, 0])
            },
        ];

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        let strikes = |id| {
            rows.effects
                .iter()
                .filter(|effect| effect.spell_id == id && effect.kind == E_WEAPON_STRIKE)
                .map(|effect| {
                    (
                        effect.effect_index,
                        effect.base_points,
                        effect.die_sides,
                        effect.p0,
                        effect.p1,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(strikes(53), [(1, 10, 1, 150, 1)]);
        assert!(rows
            .effects
            .iter()
            .any(|effect| effect.spell_id == 53 && effect.kind == E_ADD_COMBO));
        assert_eq!(strikes(14251), [(0, 0, 0, 150, 0)]);
    }

    #[test]
    fn a_ranged_slot_spell_casts_for_half_a_second_more_unless_it_repeats() {
        let mut dbc = fixture_tables();
        dbc.cast_times = authored_table(4, &[&[1, 0, 0, 0]]);
        dbc.spells = vec![
            Spell {
                casting_time_index: 1,
                ..melee_ability(2764, "Throw", 0x0041_0012, [58, 0, 0])
            },
            Spell {
                casting_time_index: 1,
                attributes_ex2: ATTR_EX2_AUTOREPEAT,
                ..melee_ability(75, "Auto Shot", 0x0005_0012, [58, 0, 0])
            },
            Spell {
                casting_time_index: 1,
                ..melee_ability(1752, "Sinister Strike", 0x0005_0010, [121, 0, 0])
            },
        ];

        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        let cast_ms = |id| {
            rows.headers
                .iter()
                .find(|header| header.spell_id == id)
                .unwrap()
                .cast_time_ms
        };
        assert_eq!((cast_ms(2764), cast_ms(75), cast_ms(1752)), (500, 0, 0));
    }

    #[test]
    fn react_window_flags_set_by_name() {
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
        assert_eq!(spell_flag_attributes("Overpower"), SPELL_ATTR_REQ_OVERPOWER);
        assert_eq!(spell_flag_attributes("Revenge"), SPELL_ATTR_REQ_REVENGE);
    }

    #[test]
    fn kick_interrupt_effect_maps_to_e_interrupt() {
        assert_eq!(instant_effect_to_kind(68), E_INTERRUPT);
        assert_eq!(instant_effect_to_kind(2), E_DAMAGE);
        assert_eq!(correct_script_effect_kind("Kick", E_INTERRUPT), E_INTERRUPT);
    }

    #[test]
    fn summon_effect_maps_to_e_summon_pet_with_creature_entry_in_p0() {
        assert_eq!(instant_effect_to_kind(56), E_SUMMON_PET);
        assert_eq!(resolve_instant_params(E_SUMMON_PET, 416, 0), (416, P_ENTRY));
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
    fn distract_maps_natively_to_an_enemy_area_effect_that_keeps_stealth() {
        assert_eq!(instant_effect_to_kind(69), E_DISTRACT);
        assert_eq!(kind_name(E_DISTRACT), "E_DISTRACT");
        assert_eq!(
            resolve_target(unit_selection_from_pair(16, 0), true),
            T_AREA_ENEMY
        );
        assert_eq!(resolve_instant_params(E_DISTRACT, 0, 0), (0, P_NONE));
        assert_eq!(spell_flag_attributes("Distract"), SPELL_ATTR_STEALTH_SAFE);
    }

    #[test]
    fn home_bind_teleports_keep_the_cast_time_and_recall_once() {
        let mut dbc = fixture_tables();
        dbc.cast_times = authored_table(4, &[&[1, 10_000, 0, 10_000]]);
        dbc.spells = vec![Spell {
            id: 8690,
            name: "Hearthstone".into(),
            casting_time_index: 1,
            recovery_time: 3_600_000,
            effect: [5, 77, 0],
            implicit_target_a: [9, 1, 0],
            ..Default::default()
        }];
        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        assert_eq!(rows.headers[0].cast_time_ms, 10_000);
        assert_eq!(rows.headers[0].cooldown_ms, 3_600_000);
        assert_eq!(rows.effects[0].kind, E_RECALL_HOME);
        assert_eq!(rows.effects[0].target, T_SELF);
        assert_eq!(rows.effects[1].kind, E_SCRIPTED);

        dbc.spells[0].implicit_target_a[0] = 17;
        let (rows, _, _) = derive_spell_rows(&dbc, &[], &[]).unwrap();
        assert_eq!(rows.effects[0].kind, E_SCRIPTED);
    }

    #[test]
    fn feed_pet_effect_maps_without_a_spell_id_branch() {
        assert_eq!(instant_effect_to_kind(101), E_FEED_PET);
        assert_eq!(resolve_instant_params(E_FEED_PET, 123, 456), (0, P_NONE));
    }

    #[test]
    fn heal_max_health_effect_split_out_of_e_heal() {
        assert_eq!(instant_effect_to_kind(67), E_HEAL_MAX_HEALTH);
        assert_eq!(instant_effect_to_kind(10), E_HEAL);
        assert_eq!(
            correct_script_effect_kind("Lay on Hands", E_HEAL_MAX_HEALTH),
            E_HEAL_MAX_HEALTH
        );
    }

    #[test]
    fn proc_auras_map_natively_no_name_involved() {
        assert_eq!(aura_mod_to_kind(AuraMod::ProcTriggerSpell), A_PROC_TRIGGER);
        assert_eq!(aura_mod_to_kind(AuraMod::ProcTriggerDamage), A_PROC_DAMAGE);
        assert_eq!(aura_mod_to_kind(AuraMod::PeriodicTriggerSpell), E_TRIGGER);
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
        assert_eq!(
            proc_damage_school_p0(A_PROC_DAMAGE, 0, P_NONE, 0x10),
            (0x10, P_SCHOOL_MASK)
        );
        assert_eq!(
            proc_damage_school_p0(A_MOD_RESISTANCE, 5, P_SCHOOL_MASK, 0x10),
            (5, P_SCHOOL_MASK)
        );
    }

    #[test]
    fn proc_header_fields_preserve_masks_and_bound_byte_columns() {
        assert_eq!(proc_header_fields(40, 100, 0), (40, 100, 0));
        assert_eq!(proc_header_fields(139944, 100, 3), (139944, 100, 3));
        assert_eq!(proc_header_fields(1, 999, 999), (1, 255, 255));
        assert_eq!(proc_header_fields(1, 0, 0), (1, 0, 0));
    }

    #[test]
    fn synthetic_seal_added_to_seal_of_the_crusader_only() {
        let row = synthetic_seal_effect(21082, "Seal of the Crusader", 3)
            .expect("Seal of the Crusader gains a seal effect");
        assert_eq!(row.row_id(), (21082u64 << 2) | 3);
        assert_eq!(row.kind, A_SEAL);
        assert_eq!(row.base_points, 20);
        assert_eq!(row.target, T_SELF);
        assert!(synthetic_seal_effect(21082, "Seal of Command", 2).is_none());
        assert!(synthetic_seal_effect(21084, "Seal of the Crusader", 2).is_none());
        assert!(synthetic_seal_effect(21084, "Seal of Righteousness", 2).is_none());
    }

    #[test]
    fn power_word_shield_p1_links_weakened_soul() {
        assert_eq!(
            power_word_shield_p1_override(17, "Power Word: Shield", A_ABSORB, 0),
            6788
        );
        assert_eq!(
            power_word_shield_p1_override(18, "Power Word: Shield", A_ABSORB, 0),
            0
        );
        assert_eq!(
            power_word_shield_p1_override(17, "Power Word: Fortitude", A_ABSORB, 0),
            0
        );
        assert_eq!(
            power_word_shield_p1_override(17, "Power Word: Shield", A_FLAG, 0),
            0
        );
    }

    #[test]
    fn instant_effects_map_to_real_kinds() {
        assert_eq!(instant_effect_to_kind(2), E_DAMAGE);
        assert_eq!(instant_effect_to_kind(17), E_WEAPON_STRIKE);
        assert_eq!(instant_effect_to_kind(58), E_WEAPON_STRIKE);
        assert_eq!(instant_effect_to_kind(121), E_WEAPON_STRIKE);
        assert_eq!(instant_effect_to_kind(10), E_HEAL);
        assert_eq!(instant_effect_to_kind(67), E_HEAL_MAX_HEALTH);
        assert_eq!(instant_effect_to_kind(30), E_ENERGIZE);
        assert_eq!(instant_effect_to_kind(38), E_DISPEL);
        assert_eq!(instant_effect_to_kind(64), E_TRIGGER);
        assert_eq!(instant_effect_to_kind(63), E_TAUNT);
        assert_eq!(instant_effect_to_kind(91), E_TAUNT);
        assert_eq!(instant_effect_to_kind(24), E_CREATE_ITEM);
        assert_eq!(instant_effect_to_kind(62), E_POWER_BURN);
        assert_eq!(instant_effect_to_kind(33), E_OPEN_LOCK);
        assert_eq!(instant_effect_to_kind(59), E_OPEN_LOCK);
        assert_eq!(instant_effect_to_kind(36), E_SCRIPTED);
    }

    #[test]
    fn instant_effect_raw_id_arms_land_work_item_101() {
        assert_eq!(instant_effect_to_kind(114), E_TAUNT);
        assert_eq!(instant_effect_to_kind(80), E_ADD_COMBO);
        assert_eq!(instant_effect_to_kind(18), E_RESURRECT);
        assert_eq!(instant_effect_to_kind(113), E_RESURRECT);
        assert_eq!(instant_effect_to_kind(3), E_SCRIPTED);
        assert_eq!(instant_effect_to_kind(115), E_SCRIPTED);
        assert_eq!(instant_effect_to_kind(19), E_SCRIPTED);
        assert_eq!(instant_effect_to_kind(112), E_SCRIPTED);
    }

    #[test]
    fn power_burn_ratio_bp_converts_the_dbc_fraction_to_basis_points() {
        assert_eq!(power_burn_ratio_bp(0.5), 50);
        assert_eq!(power_burn_ratio_bp(1.0), 100);
        assert_eq!(power_burn_ratio_bp(0.0), 0);
    }

    #[test]
    fn apply_aura_effect_id_routes_to_aura_branch() {
        assert!(is_aura_effect(6));
        assert!(is_aura_effect(27));
        assert!(is_aura_effect(35));
        assert!(is_aura_effect(119));
        assert!(!is_aura_effect(2));
    }

    #[test]
    fn auras_map_to_real_kinds() {
        use AuraMod::*;
        assert_eq!(aura_mod_to_kind(PeriodicDamage), A_PERIODIC_DAMAGE);
        assert_eq!(aura_mod_to_kind(PeriodicHeal), A_PERIODIC_HEAL);
        assert_eq!(aura_mod_to_kind(ModRegen), A_PERIODIC_HEAL);
        assert_eq!(aura_mod_to_kind(ModPowerRegen), A_PERIODIC_ENERGIZE);
        assert_eq!(aura_mod_to_kind(PeriodicEnergize), A_PERIODIC_ENERGIZE);
        assert_eq!(
            aura_mod_to_kind(ModPowerRegenPercent),
            A_MOD_POWER_REGEN_PCT
        );
        assert_eq!(aura_mod_to_kind(ModStat), A_MOD_STAT);
        assert_eq!(aura_mod_to_kind(ModResistance), A_MOD_RESISTANCE);
        assert_eq!(aura_mod_to_kind(SchoolAbsorb), A_ABSORB);
        assert_eq!(aura_mod_to_kind(ModAttackPower), A_MOD_COMBAT);
        assert_eq!(aura_mod_to_kind(ModStun), A_CONTROL);
        assert_eq!(aura_mod_to_kind(ModRoot), A_CONTROL);
        assert_eq!(aura_mod_to_kind(ModFear), A_CONTROL);
        assert_eq!(aura_mod_to_kind(SchoolImmunity), A_IMMUNITY);
        assert_eq!(aura_mod_to_kind(Dummy), A_FLAG);
        assert_eq!(aura_mod_to_kind(ModDamagePercentTaken), A_MOD_DAMAGE_TAKEN);
        assert_eq!(aura_mod_to_kind(ModDamageTaken), E_SCRIPTED);
        assert_eq!(aura_mod_to_kind(AddFlatModifier), A_SPELLMOD_FLAT);
        assert_eq!(aura_mod_to_kind(AddPctModifier), A_SPELLMOD_PCT);
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
            (i32::from(COMBAT_ATTACK_POWER), P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModCritPercent, 0),
            (i32::from(COMBAT_CRIT), P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModHitChance, 0),
            (i32::from(COMBAT_HIT), P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModDamageDone, 0),
            (i32::from(COMBAT_DMG_DONE), P_COMBAT_FIELD)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_COMBAT, ModHealing, 0),
            (i32::from(COMBAT_SPELL_POWER), P_COMBAT_FIELD)
        );
    }

    #[test]
    fn speed_kind_resolves_run_swim_swing_cast_and_mount() {
        use AuraMod::*;
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModIncreaseSpeed, 0),
            (0, P_SPEED_KIND)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModSpeedAlways, 0),
            (i32::from(SPEED_MOVE_ALWAYS), P_SPEED_KIND)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModSpeedNotStack, 0),
            (i32::from(SPEED_MOVE_NOT_STACK), P_SPEED_KIND)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModMeleeHaste, 0),
            (1, P_SPEED_KIND)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModCastingSpeedNotStack, 0),
            (2, P_SPEED_KIND)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModIncreaseMountedSpeed, 0),
            (3, P_SPEED_KIND)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_SPEED, ModIncreaseSwimSpeed, 0),
            (i32::from(SPEED_SWIM), P_SPEED_KIND)
        );
    }

    #[test]
    fn transform_import_retains_the_creature_entry() {
        assert_eq!(aura_mod_to_kind(AuraMod::Transform), A_TRANSFORM);
        assert_eq!(
            resolve_aura_params(A_TRANSFORM, AuraMod::Transform, 12861),
            (12861, P_ENTRY)
        );
    }

    #[test]
    #[ignore = "requires LYRACORE_TEST_DBC with the owned build 5875 client"]
    fn actual_ghost_and_wisp_import_keep_vanilla_speed_and_transform() {
        let dir = std::env::var("LYRACORE_TEST_DBC").expect("set LYRACORE_TEST_DBC");
        let (rows, _, _) = build_spell_rows(Path::new(&dir), &[8326, 20584], &[]).unwrap();
        assert_eq!(rows.headers.len(), 2);
        assert!(rows
            .headers
            .iter()
            .all(|header| header.duration_ms == u32::MAX));
        assert!(rows.effects.iter().all(|effect| effect.target == T_SELF));
        let effects = |spell_id| {
            rows.effects
                .iter()
                .filter(|effect| effect.spell_id == spell_id)
                .map(|effect| (effect.kind, effect.p0, effect.p0_kind, effect.base_points))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            effects(8326),
            vec![
                (A_FLAG, 0, P_FLAG, -20),
                (A_MOD_SPEED, i32::from(SPEED_MOVE), P_SPEED_KIND, 25),
                (A_MOD_SPEED, i32::from(SPEED_SWIM), P_SPEED_KIND, 25),
            ]
        );
        assert_eq!(
            effects(20584),
            vec![
                (A_MOD_SPEED, i32::from(SPEED_MOVE), P_SPEED_KIND, 50),
                (A_MOD_SPEED, i32::from(SPEED_SWIM), P_SPEED_KIND, 50),
                (A_TRANSFORM, 12861, P_ENTRY, 0),
            ]
        );
    }

    #[test]
    fn absorb_params_split_mana_shield_from_school_absorb() {
        use AuraMod::*;
        assert_eq!(resolve_aura_params(A_ABSORB, ManaShield, 0x7F), (0, P_NONE));
        assert_eq!(
            resolve_aura_params(A_ABSORB, SchoolAbsorb, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
    }

    #[test]
    fn immunity_params_resolve_all_three_arms() {
        use AuraMod::*;
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, SchoolImmunity, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, DamageImmunity, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, MechanicImmunity, 5),
            (5, P_MECHANIC)
        );
        assert_eq!(
            resolve_aura_params(A_IMMUNITY, MechanicImmunityMask, 5),
            (5, P_MECHANIC)
        );
        assert_eq!(resolve_aura_params(A_IMMUNITY, ModStun, 9), (9, P_RAW));
    }

    #[test]
    fn is_channeled_reads_the_attributes_ex1_bit_and_the_arcane_missiles_fallback() {
        assert!(is_channeled(0x44, "Some Spell"));
        assert!(is_channeled(0x04, "Some Spell"));
        assert!(is_channeled(0x40, "Some Spell"));
        assert!(!is_channeled(0, "Some Spell"));
        assert!(is_channeled(0, "Arcane Missiles"));
    }

    #[test]
    fn stat_all_sentinel_and_school_mask() {
        use AuraMod::*;
        assert_eq!(
            resolve_aura_params(A_MOD_STAT, ModStat, u32::MAX),
            (i32::from(STAT_ALL), P_STAT_ID)
        );
        assert_eq!(resolve_aura_params(A_MOD_STAT, ModStat, 3), (3, P_STAT_ID));
        assert_eq!(aura_mod_to_kind(ModStat), A_MOD_STAT);
        assert_eq!(aura_mod_to_kind(ModTotalStatPercentage), A_MOD_STAT_PCT);
        assert_eq!(
            resolve_aura_params(A_MOD_STAT_PCT, ModTotalStatPercentage, 4),
            (4, P_STAT_ID)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_STAT_PCT, ModTotalStatPercentage, u32::MAX),
            (i32::from(STAT_ALL), P_STAT_ID)
        );
        assert_eq!(
            resolve_aura_params(A_MOD_RESISTANCE, ModResistance, 0x7F),
            (0x7F, P_SCHOOL_MASK)
        );
    }

    #[test]
    fn periodic_energize_decodes_power_type_from_misc() {
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
    fn power_regen_percent_decodes_power_type_from_misc() {
        assert_eq!(
            resolve_aura_params(A_MOD_POWER_REGEN_PCT, AuraMod::ModPowerRegenPercent, 3),
            (3, P_POWER_TYPE)
        );
    }

    #[test]
    fn instant_param_decodes_power_and_dispel() {
        assert_eq!(resolve_instant_params(E_ENERGIZE, 1, 0), (1, P_POWER_TYPE));
        assert_eq!(resolve_instant_params(E_DISPEL, 1, 0), (1, P_SCHOOL_MASK));
        assert_eq!(resolve_instant_params(E_DAMAGE, 4, 0), (0, P_NONE));
        assert_eq!(instant_effect_to_kind(83), E_DUEL);
        assert_eq!(
            resolve_instant_params(E_DUEL, 21680, 0),
            (21680, P_GAMEOBJECT_ENTRY)
        );
        assert_eq!(
            resolve_instant_params(E_CREATE_ITEM, 99, 2070),
            (2070, P_ITEM_ENTRY)
        );
    }

    #[test]
    fn target_definitions_distinguish_enemy_ally_area_and_unsupported_selections() {
        assert_eq!(resolve_target(6, true), T_TARGET_ENEMY);
        assert_eq!(resolve_target(6, false), T_TARGET_ENEMY);
        assert_eq!(resolve_target(1, false), T_SELF);
        assert_eq!(resolve_target(0, false), T_SCRIPTED);
        assert_eq!(resolve_target(20, false), T_AREA_ALLY);
        assert_eq!(resolve_target(8, true), T_AREA_ENEMY);
        assert_eq!(resolve_target(8, false), T_AREA_ALLY);
        assert_eq!(resolve_target(-5, true), T_SCRIPTED);
        assert_eq!(resolve_target(999, false), T_SCRIPTED);
    }

    #[test]
    fn reducing_modifier_aura_is_a_debuff() {
        use AuraMod::*;
        assert!(is_reducing_modifier_aura(6, ModResistance, -90));
        assert!(is_reducing_modifier_aura(6, ModAttackPower, -35));
        assert!(is_reducing_modifier_aura(6, ModStat, -10));
        assert!(!is_reducing_modifier_aura(6, ModAttackPower, 15));
        assert!(!is_reducing_modifier_aura(6, ModResistance, 2000));
        assert!(!is_reducing_modifier_aura(6, PeriodicDamage, -5));
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
        assert_eq!(trainer_cost(0), 50);
        assert_eq!(trainer_cost(1), 50);
        assert_eq!(trainer_cost(4), 200);
        assert_eq!(trainer_cost(10), 500);
        assert!(trainer_cost(10) > trainer_cost(4));
    }

    #[test]
    fn delete_statements_are_wholesale_or_surgical() {
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
        assert!(surgical.iter().all(|s| !s.contains(">= 0")));
    }

    #[test]
    fn mounted_aura_maps_to_a_mounted_with_display_id_p0() {
        assert_eq!(aura_mod_to_kind(AuraMod::Mounted), A_MOUNTED);
        assert_eq!(
            resolve_aura_params(A_MOUNTED, AuraMod::Mounted, 723),
            (723, P_DISPLAY_ID)
        );
    }

    #[test]
    fn mount_display_p0_resolves_creature_template_indirection_and_falls_through() {
        let mut displays: BTreeMap<u32, u32> = BTreeMap::new();
        displays.insert(1234, 5678);
        assert_eq!(mount_display_p0(A_MOUNTED, 1234, &displays), 5678);
        assert_eq!(mount_display_p0(A_MOUNTED, 723, &displays), 723);
        assert_eq!(mount_display_p0(E_DAMAGE, 1234, &displays), 1234);
    }

    #[test]
    fn dispel_mechanic_reclassifies_the_mount_parameter_only() {
        assert_eq!(dismount_effect_kind(108, 21), Some(E_DISMOUNT));
        assert_eq!(dismount_effect_kind(108, 1), None);
        assert_eq!(dismount_effect_kind(108, 0), None);
        assert_eq!(dismount_effect_kind(2, 21), None);
        assert_eq!(instant_effect_to_kind(108), E_SCRIPTED);
    }

    #[test]
    fn mounted_speed_normalizes_the_vanilla_stored_flat_amount() {
        use AuraMod::*;
        for mounted in [
            ModIncreaseMountedSpeed,
            ModMountedSpeedAlways,
            ModMountedSpeedNotStack,
        ] {
            assert_eq!(
                resolve_aura_params(A_MOD_SPEED, mounted, 0),
                (i32::from(SPEED_MOUNTED), P_SPEED_KIND)
            );
        }
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
