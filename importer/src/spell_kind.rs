use lyracore_shared::spell::taxonomy::*;
use wow_world_base::vanilla::AuraMod;

pub fn instant_effect_to_kind(effect_id: i32) -> u8 {
    match effect_id {
        2 | 31 => E_DAMAGE,
        17 | 58 | 121 => E_WEAPON_STRIKE,
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
        19 => E_ADD_EXTRA_ATTACKS,
        _ => E_SCRIPTED,
    }
}

pub fn correct_script_effect_kind(name: &str, kind: u8) -> u8 {
    // vmangos scripts Reckoning: crits taken stack up to four swings that follow the next swing.
    if kind == E_ADD_EXTRA_ATTACKS && name == "Reckoning" {
        return E_SCRIPTED;
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

pub fn aura_mod_to_kind(aura: AuraMod) -> u8 {
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

pub fn dismount_effect_kind(effect_id: i32, misc: u32) -> Option<u8> {
    if effect_id == 108 && misc == 21 {
        Some(E_DISMOUNT)
    } else {
        None
    }
}

#[derive(serde::Deserialize)]
pub struct EffectSlot {
    pub effect: i32,
    pub aura: u32,
    pub target_a: i32,
    pub target_b: i32,
    pub misc: i32,
}

pub fn supported_effect(name: &str, slot: &EffectSlot) -> bool {
    let aura = AuraMod::from_int(slot.aura).unwrap_or_default();
    let kind = if matches!(slot.effect, 6 | 27 | 35 | 119) || slot.aura != 0 {
        aura_mod_to_kind(aura)
    } else if slot.effect == 5 && (slot.target_a == 9 || slot.target_b == 9) {
        E_RECALL_HOME
    } else {
        instant_effect_to_kind(slot.effect)
    };
    let kind = if name == "Stealth" && aura != AuraMod::ModStealth {
        kind
    } else {
        correct_script_effect_kind(name, kind)
    };
    dismount_effect_kind(slot.effect, slot.misc as u32).unwrap_or(kind) != E_SCRIPTED
}

#[cfg(test)]
mod tests {
    use super::{supported_effect, EffectSlot};

    fn slot(effect: i32, aura: u32, target_b: i32, misc: i32) -> EffectSlot {
        EffectSlot {
            effect,
            aura,
            target_a: 0,
            target_b,
            misc,
        }
    }

    #[test]
    fn support_uses_importer_overrides_and_leaves_unknown_kinds_open() {
        assert!(supported_effect("Other", &slot(17, 0, 0, 0)));
        assert!(supported_effect("Other", &slot(5, 0, 9, 0)));
        assert!(supported_effect("Other", &slot(108, 0, 0, 21)));
        assert!(supported_effect("Stealth", &slot(6, 16, 0, 0)));
        assert!(!supported_effect("Stealth", &slot(3, 0, 0, 0)));
        assert!(!supported_effect("Other", &slot(6, 999, 0, 0)));
        assert!(!supported_effect("Other", &slot(108, 0, 0, 1)));
    }
}
