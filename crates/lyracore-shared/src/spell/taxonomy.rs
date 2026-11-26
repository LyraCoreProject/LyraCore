macro_rules! effect_kinds {
    ($($code:ident => $variant:ident = $value:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum EffectKind {
            $($variant = $value),+
        }

        impl EffectKind {
            pub const ALL: &[Self] = &[$(Self::$variant),+];
        }

        impl TryFrom<u8> for EffectKind {
            type Error = u8;

            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    other => Err(other),
                }
            }
        }

        // Keep byte names for authored rows and Package API compatibility.
        $(pub const $code: u8 = EffectKind::$variant as u8;)+
    };
}

effect_kinds! {
    E_DAMAGE => Damage = 0x01,
    E_HEAL => Heal = 0x02,
    E_ENERGIZE => Energize = 0x03,
    E_DISPEL => Dispel = 0x04,
    E_TRIGGER => Trigger = 0x05,
    E_TAUNT => Taunt = 0x06,
    E_CREATE_ITEM => CreateItem = 0x07,
    // WeaponStrike p0 is the weapon damage percent; p1 is 1 when the weapon speed is normalized.
    E_WEAPON_STRIKE => WeaponStrike = 0x08,
    E_CHARGE => Charge = 0x09,
    E_CONVERT_RESOURCE => ConvertResource = 0x0A,
    E_JUDGEMENT => Judgement = 0x0B,
    E_ADD_COMBO => AddCombo = 0x0C,
    E_FINISHER_DAMAGE => FinisherDamage = 0x0D,
    E_RESURRECT => Resurrect = 0x0E,
    E_SCRIPTED => Scripted = 0x0F,
    E_PICKPOCKET => Pickpocket = 0x10,
    E_INTERRUPT => Interrupt = 0x11,
    E_REDUCE_THREAT => ReduceThreat = 0x12,
    E_NEXT_SWING => NextSwing = 0x13,
    // Reserved: no import writes it and the Module ignores it; Forms are `A_SHAPESHIFT` Auras.
    E_SET_STANCE => SetStance = 0x14,
    E_SUMMON_PET => SummonPet = 0x15,
    E_HEAL_MAX_HEALTH => HealMaxHealth = 0x16,
    E_ENCHANT_ITEM => EnchantItem = 0x17,
    E_DISENCHANT => Disenchant = 0x18,
    E_POWER_BURN => PowerBurn = 0x19,
    E_BLINK => Blink = 0x1A,
    E_PERSISTENT_AREA => PersistentArea = 0x1B,
    E_FISH => Fish = 0x1C,
    E_OPEN_LOCK => OpenLock = 0x1D,
    E_RECALL_HOME => RecallHome = 0x1F,
    E_TAME_CREATURE => TameCreature = 0x20,
    E_FEED_PET => FeedPet = 0x21,
    E_DUEL => Duel = 0x22,
    E_DISMOUNT => Dismount = 0x23,
    E_SUMMON_HOSTILE => SummonHostile = 0x24,
    E_DISTRACT => Distract = 0x25,
    E_SELF_RESURRECT => SelfResurrect = 0x26,
    A_PERIODIC_DAMAGE => AuraPeriodicDamage = 0x90,
    A_PERIODIC_HEAL => AuraPeriodicHeal = 0x91,
    A_PERIODIC_ENERGIZE => AuraPeriodicEnergize = 0x92,
    A_PERIODIC_TRIGGER => AuraPeriodicTrigger = 0x93,
    A_MOD_STAT => AuraModStat = 0xA0,
    A_MOD_RESISTANCE => AuraModResistance = 0xA1,
    A_ABSORB => AuraAbsorb = 0xA2,
    A_MOD_COMBAT => AuraModCombat = 0xA3,
    A_MOD_SPEED => AuraModSpeed = 0xA4,
    A_MOD_HEALTH_POWER => AuraModHealthPower = 0xA5,
    A_MOD_DAMAGE_TAKEN => AuraModDamageTaken = 0xA6,
    A_SEAL => AuraSeal = 0xA7,
    A_STEALTH => AuraStealth = 0xA8,
    A_COMBAT_HEALTH_REGEN_PCT => AuraCombatHealthRegenPct = 0xA9,
    A_MOD_STAT_PCT => AuraModStatPct = 0xAA,
    A_PROC_TRIGGER => AuraProcTrigger = 0xAB,
    A_SPELLMOD_FLAT => AuraSpellmodFlat = 0xAC,
    A_SPELLMOD_PCT => AuraSpellmodPct = 0xAD,
    A_DISARM => AuraDisarm = 0xAE,
    A_RETALIATE => AuraRetaliate = 0xAF,
    A_CONTROL => AuraControl = 0xB0,
    A_IMMUNITY => AuraImmunity = 0xB1,
    A_MOD_DETECT_RANGE => AuraModDetectRange = 0xB2,
    A_MOUNTED => AuraMounted = 0xB3,
    A_PROC_DAMAGE => AuraProcDamage = 0xB4,
    A_SELF_RESURRECT => AuraSelfResurrect = 0xB5,
    A_TRANSFORM => AuraTransform = 0xB6,
    A_MOD_POWER_REGEN_PCT => AuraModPowerRegenPct = 0xB7,
    A_MOD_HEALING => AuraModHealing = 0xB8,
    A_INERT => AuraInert = 0xB9,
    // p0 is the skill line; p1 selects temporary (0) or permanent (1) bonus points.
    A_MOD_SKILL => AuraModSkill = 0xBD,
    A_FLAG => AuraFlag = 0xBE,
    A_DAMAGE_SHIELD => AuraDamageShield = 0xC0,
    A_MOD_POWER_REGEN => AuraModPowerRegen = 0xC1,
    A_MOD_HEALTH_REGEN => AuraModHealthRegen = 0xC2,
    A_MOD_REGEN => AuraModRegen = 0xC3,
    A_MOD_HEALTH_REGEN_PCT => AuraModHealthRegenPct = 0xC4,
    A_MANA_REGEN_INTERRUPT => AuraManaRegenInterrupt = 0xC5,
    A_MOD_DAMAGE_DONE_FLAT => AuraModDamageDoneFlat = 0xD0,
    A_MOD_DAMAGE_TAKEN_FLAT => AuraModDamageTakenFlat = 0xD1,
    A_MOD_SCHOOL_CRIT => AuraModSchoolCrit = 0xD2,
    A_MOD_DAMAGE_DONE_PCT => AuraModDamageDonePct = 0xD3,
    A_MOD_HEALING_TAKEN => AuraModHealingTaken = 0xD4,
    // p0 is the Form id; the Aura row is the unit's only Form state.
    A_SHAPESHIFT => AuraShapeshift = 0xD5,
    // p0 is the power type; the amount is what a power rule that caps on Form entry keeps.
    A_FORM_POWER_RETAINED => AuraFormPowerRetained = 0xD6,
    // Amount: percent chance that entering a Form in the spell's Form Set casts its entry spell.
    A_FORM_ENTRY_CHANCE => AuraFormEntryChance = 0xD7,
    E_SUMMON_GUARDIANS => SummonGuardians = 0x27,
    E_ADD_EXTRA_ATTACKS => AddExtraAttacks = 0x28,
    // Effect points are the duration in seconds; p0 is the enchantment id.
    E_ENCHANT_ITEM_TEMPORARY => EnchantItemTemporary = 0x29,
    // p0 is the GameObject entry the caster places and owns, as Spell.dbc effect 50 names it.
    E_SUMMON_OBJECT => SummonObject = 0x2A,
    // Offers the target a teleport to the caster, or to the Summoning Ritual that cast it.
    E_SUMMON_PLAYER => SummonPlayer = 0x2B,
    // Effect points times 256 add to the target Character's drunk value.
    E_INEBRIATE => Inebriate = 0x2C,
    // The target Character learns `trigger_spell` and the spells it teaches in turn.
    E_LEARN_SPELL => LearnSpell = 0x2D,
    // Effect points are the percent of maximum health a dead Hunter pet returns with.
    E_REVIVE_PET => RevivePet = 0x2E,
}

pub const KIND_AURA_BIT: u8 = 0x80;
// Spell.dbc `attributes` bits are SPELL_ATTR_*; LyraCore `cast_flags` bits are CAST_FLAG_*.
pub const SPELL_ATTR_PASSIVE: u32 = 0x40;
pub const SPELL_ATTR_USES_RANGED_SLOT: u32 = 0x02;
pub const CAST_FLAG_REQ_BEHIND: u32 = 0x0001;
pub const CAST_FLAG_REQ_STEALTH: u32 = 0x0002;
pub const CAST_FLAG_STEALTH_SAFE: u32 = 0x0004;
// A finishing move needs and spends combo points.
pub const CAST_FLAG_COMBO_FINISHER: u32 = 0x0008;
pub const CAST_FLAG_INCAP_OPENER: u32 = 0x0010;
pub const CAST_FLAG_REQ_OVERPOWER: u32 = 0x0020;
pub const CAST_FLAG_REQ_REVENGE: u32 = 0x0040;
pub const CAST_FLAG_REQ_DAGGER: u32 = 0x0100;
pub const SPELL_FAMILY_DRUID: u8 = 7;
pub const SPELL_FAMILY_ROGUE: u8 = 8;
// Spell.dbc SpellFamilyFlags bits, as vmangos `SpellClassMask.h` names them.
pub const FAMILY_FLAG_ROGUE_EVISCERATE: u64 = 0x0002_0000;
pub const FAMILY_FLAG_ROGUE_RUPTURE: u64 = 0x0010_0000;
pub const FAMILY_FLAG_DRUID_RIP_BITE: u64 = 0x0080_0000;
pub const HUMANOID_TYPE: u8 = 7;
pub const BEAST_TYPE: u8 = 1;
pub const CRITTER_TYPE: u8 = 8;
pub const TOTEM_TYPE: u8 = 11;
// A_CONTROL control classes, not Spell.dbc mechanics: docs/schema.md#mechanic-numbering.
pub const M_STUN: i32 = 1;
pub const M_ROOT: i32 = 2;
pub const M_FEAR: i32 = 3;
pub const M_POLY: i32 = 4;
pub const P_NONE: u8 = 0;
pub const P_STAT_ID: u8 = 1;
pub const P_SCHOOL_MASK: u8 = 2;
pub const P_MECHANIC: u8 = 3;
pub const P_POWER_TYPE: u8 = 4;
pub const P_COMBAT_FIELD: u8 = 5;
pub const P_SPEED_KIND: u8 = 6;
pub const P_FLAG: u8 = 7;
pub const P_ITEM_ENTRY: u8 = 8;
pub const P_ENTRY: u8 = 9;
pub const P_ENCHANT_ID: u8 = 10;
pub const SPELLMOD_OP_DAMAGE: i32 = 0;
pub const SPELLMOD_OP_CASTING_TIME: i32 = 10;
pub const P_SPELLMOD_OP: u8 = 11;
pub const P_PCT_MAX_POWER: u8 = 12;
pub const P_GAMEOBJECT_ENTRY: u8 = 13;
pub const P_DISPLAY_ID: u8 = 14;
pub const P_SPELL_ID: u8 = 15;
pub const P_FLAT_MANA: u8 = 16;
// Source combat percentages convert to basis points after rolling the effect amount.
pub const P_COMBAT_PERCENT: u8 = 17;
pub const P_SKILL_LINE: u8 = 18;
pub const P_FORM: u8 = 19;
pub const P_RAW: u8 = 255;
pub const T_SELF: u8 = 0;
pub const T_TARGET_ENEMY: u8 = 1;
pub const T_TARGET_ALLY: u8 = 2;
pub const T_TARGET_ANY: u8 = 3;
pub const T_AREA_ENEMY: u8 = 4;
pub const T_AREA_ALLY: u8 = 5;
pub const T_CHAIN_ENEMY: u8 = 6;
pub const T_SCRIPTED: u8 = 7;
pub const T_CASTER_PET: u8 = 8;
// vmangos FillRaidOrPartyTargets: the caster's subgroup, or its whole raid, and their pets.
pub const T_PARTY_AREA: u8 = 9;
pub const T_RAID_AREA: u8 = 10;
// Every unit not hostile to the caster: helpful script areas (7, 8) and target 30.
pub const T_AREA_NOT_HOSTILE: u8 = 11;
// vmangos SetTargetMap targets 37, 45 and 61, each measured from the selected unit.
pub const T_TARGET_SUBGROUP: u8 = 12;
pub const T_CHAIN_ALLY: u8 = 13;
pub const T_TARGET_GROUP_CLASS: u8 = 14;
pub const COMBAT_ATTACK_POWER: u8 = 0;
pub const COMBAT_CRIT: u8 = 1;
pub const COMBAT_HIT: u8 = 2;
pub const COMBAT_DMG_DONE: u8 = 3;
pub const COMBAT_SPELL_POWER: u8 = 5;
pub const COMBAT_PARRY: u8 = 6;
pub const COMBAT_DODGE: u8 = 7;
pub const COMBAT_BLOCK: u8 = 8;
pub const COMBAT_DEFENSE: u8 = 9;
pub const COMBAT_THREAT: u8 = 10;
pub const COMBAT_RANGED_ATTACK_POWER: u8 = 11;
pub const COMBAT_SPELL_CRIT: u8 = 13;
pub const COMBAT_ATTACK_POWER_PCT: u8 = 14;
pub const COMBAT_RANGED_ATTACK_POWER_PCT: u8 = 15;
pub const SPEED_MOVE: u8 = 0;
pub const SPEED_SWING: u8 = 1;
pub const SPEED_CAST: u8 = 2;
pub const SPEED_MOUNTED: u8 = 3;
pub const SPEED_SWIM: u8 = 4;
pub const SPEED_MOVE_ALWAYS: u8 = 5;
pub const SPEED_MOVE_NOT_STACK: u8 = 6;
pub const STAT_STR: u8 = 0;
pub const STAT_AGI: u8 = 1;
pub const STAT_STA: u8 = 2;
pub const STAT_INT: u8 = 3;
pub const STAT_SPI: u8 = 4;
pub const STAT_ALL: u8 = 0xFF;
pub const RESIST_ARMOR: u8 = 0x01;
pub const CAST_FLAG_CHANNELED: u32 = 0x0080;
pub const CAST_FLAG_RANGED_AUTO_REPEAT: u32 = 0x0200;
// vmangos SpellEntry::CanTriggerWeaponProcs: the hit rolls the main-hand weapon's Chance on Hit.
pub const CAST_FLAG_WEAPON_PROCS: u32 = 0x0400;
pub const CAST_FLAG_BINARY: u32 = 0x0800;
pub const CAST_FLAG_CANNOT_CRIT: u32 = 0x1000;
pub const CAST_FLAG_IGNORE_RESISTANCES: u32 = 0x2000;
pub const CAST_FLAG_ALWAYS_HIT: u32 = 0x4000;
pub const CAST_FLAG_MAGIC_HIT: u32 = 0x8000;
pub const CAST_FLAG_AREA_AURA: u32 = 0x0001_0000;
pub const CAST_FLAG_ITEM_TARGET: u32 = 0x0002_0000;
pub const CAST_FLAG_RIGHTEOUSNESS: u32 = 0x0004_0000;
// vmangos refuses this spell when its caster selects itself as the unit target.
pub const CAST_FLAG_REFUSES_SELF_CAST: u32 = 0x0008_0000;
// vmangos HandleCancelAuraOpcode refuses an Aura its client shows no icon for.
pub const CAST_FLAG_NO_AURA_CANCEL: u32 = 0x0010_0000;
// While a unit holds this spell's Aura, it cannot cast a shapeshift (vmangos Spell::CheckCast).
pub const CAST_FLAG_LOCKS_FORM: u32 = 0x0020_0000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_round_trip_and_unknown_codes_are_returned() {
        for kind in EffectKind::ALL {
            assert_eq!(EffectKind::try_from(*kind as u8), Ok(*kind));
        }
        assert_eq!(EffectKind::try_from(0), Err(0));
        assert_eq!(EffectKind::try_from(0x1E), Err(0x1E));
        assert_eq!(EffectKind::try_from(0xFF), Err(0xFF));
    }

    // Imported `game_spell_effect.kind` rows store these codes.
    #[test]
    fn kind_wire_values() {
        assert_eq!(KIND_AURA_BIT, 0x80);
        assert_eq!(E_DAMAGE, 0x01);
        assert_eq!(E_HEAL, 0x02);
        assert_eq!(E_DISPEL, 0x04);
        assert_eq!(E_SUMMON_GUARDIANS, 0x27);
        assert_eq!(E_ADD_EXTRA_ATTACKS, 0x28);
        assert_eq!(E_ENCHANT_ITEM_TEMPORARY, 0x29);
        assert_eq!(E_SUMMON_OBJECT, 0x2A);
        assert_eq!(E_SUMMON_PLAYER, 0x2B);
        assert_eq!(E_INEBRIATE, 0x2C);
        assert_eq!(E_LEARN_SPELL, 0x2D);
        assert_eq!(E_REVIVE_PET, 0x2E);
        assert_eq!(A_PERIODIC_DAMAGE, 0x90);
        assert_eq!(A_PERIODIC_HEAL, 0x91);
        assert_eq!(A_ABSORB, 0xA2);
        assert_eq!(A_MOD_COMBAT, 0xA3);
        assert_eq!(A_MOD_POWER_REGEN_PCT, 0xB7);
        assert_eq!(A_MOD_POWER_REGEN, 0xC1);
        assert_eq!(A_MOD_HEALTH_REGEN, 0xC2);
        assert_eq!(A_MOD_REGEN, 0xC3);
        assert_eq!(A_MOD_HEALTH_REGEN_PCT, 0xC4);
        assert_eq!(A_MANA_REGEN_INTERRUPT, 0xC5);
        assert_eq!(A_FLAG, 0xBE);
        assert_eq!(A_DAMAGE_SHIELD, 0xC0);
        assert_eq!(A_MOD_DAMAGE_DONE_FLAT, 0xD0);
        assert_eq!(A_MOD_DAMAGE_TAKEN_FLAT, 0xD1);
        assert_eq!(A_MOD_SCHOOL_CRIT, 0xD2);
        assert_eq!(A_MOD_DAMAGE_DONE_PCT, 0xD3);
        assert_eq!(A_MOD_HEALING_TAKEN, 0xD4);
        assert_eq!(A_SHAPESHIFT, 0xD5);
        assert_eq!(A_FORM_POWER_RETAINED, 0xD6);
        assert_eq!(A_FORM_ENTRY_CHANCE, 0xD7);
        for k in [
            E_DAMAGE, E_HEAL, E_ENERGIZE, E_DISPEL, E_TRIGGER, E_SCRIPTED,
        ] {
            assert_eq!(
                k & KIND_AURA_BIT,
                0,
                "instant kind 0x{k:02x} must not set the aura bit"
            );
        }
        for k in [
            A_PERIODIC_DAMAGE,
            A_PERIODIC_HEAL,
            A_MOD_COMBAT,
            A_FLAG,
            A_CONTROL,
        ] {
            assert_ne!(
                k & KIND_AURA_BIT,
                0,
                "aura kind 0x{k:02x} must set the aura bit"
            );
        }
    }

    // Imported `game_spell_effect` rows store these tags in `p0_kind`.
    #[test]
    fn param_tag_wire_values_exhaustive() {
        assert_eq!(P_NONE, 0);
        assert_eq!(P_STAT_ID, 1);
        assert_eq!(P_SCHOOL_MASK, 2);
        assert_eq!(P_MECHANIC, 3);
        assert_eq!(P_POWER_TYPE, 4);
        assert_eq!(P_COMBAT_FIELD, 5);
        assert_eq!(P_SPEED_KIND, 6);
        assert_eq!(P_FLAG, 7);
        assert_eq!(P_ITEM_ENTRY, 8);
        assert_eq!(P_ENTRY, 9);
        assert_eq!(P_ENCHANT_ID, 10);
        assert_eq!(P_PCT_MAX_POWER, 12);
        assert_eq!(P_GAMEOBJECT_ENTRY, 13);
        assert_eq!(P_DISPLAY_ID, 14);
        assert_eq!(P_SPELL_ID, 15);
        assert_eq!(P_FLAT_MANA, 16);
        assert_eq!(P_COMBAT_PERCENT, 17);
        assert_eq!(P_SKILL_LINE, 18);
        assert_eq!(A_MOD_SKILL, 0xBD);
        assert_eq!(P_RAW, 255);
        let all = [
            P_NONE,
            P_STAT_ID,
            P_SCHOOL_MASK,
            P_MECHANIC,
            P_POWER_TYPE,
            P_COMBAT_FIELD,
            P_SPEED_KIND,
            P_FLAG,
            P_ITEM_ENTRY,
            P_ENTRY,
            P_ENCHANT_ID,
            P_PCT_MAX_POWER,
            P_GAMEOBJECT_ENTRY,
            P_DISPLAY_ID,
            P_SPELL_ID,
            P_FLAT_MANA,
            P_COMBAT_PERCENT,
            P_SKILL_LINE,
            P_RAW,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "param tags must be distinct: {a} vs {b}");
            }
        }
    }

    // Imported `game_spell_effect` rows store these mechanics in `p0`.
    #[test]
    fn mechanic_wire_values() {
        assert_eq!(M_STUN, 1);
        assert_eq!(M_ROOT, 2);
        assert_eq!(M_FEAR, 3);
        assert_eq!(M_POLY, 4);
        let ms = [M_STUN, M_ROOT, M_FEAR, M_POLY];
        for (i, a) in ms.iter().enumerate() {
            assert_ne!(*a, 0, "mechanic must be non-zero (0 = inert sentinel)");
            for b in &ms[i + 1..] {
                assert_ne!(a, b, "mechanics must be distinct");
            }
        }
        assert_eq!(A_CONTROL & KIND_AURA_BIT, KIND_AURA_BIT);
    }
}
