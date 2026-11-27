/// What a Form's power rule does to the unit's power when the unit enters the Form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PowerOnEntry {
    Keep = 0,
    Zero = 1,
    /// Keeps power up to the unit's `A_FORM_POWER_RETAINED` amount, which is 0 without one.
    KeepUpToRetained = 2,
}

impl TryFrom<u8> for PowerOnEntry {
    type Error = u8;

    fn try_from(code: u8) -> Result<Self, Self::Error> {
        match code {
            0 => Ok(Self::Keep),
            1 => Ok(Self::Zero),
            2 => Ok(Self::KeepUpToRetained),
            other => Err(other),
        }
    }
}

/// The `power_type` of a Form that shows the unit's own pool, whatever its power type.
pub const OWN_POOL: u8 = u8::MAX;

/// The Forms Spell.dbc `Stances` and `StancesNot` can name: bit `form_id - 1` reaches Form 32.
pub const DBC_MASK_FORMS: std::ops::RangeInclusive<u32> = 1..=32;

const CLASS_WARRIOR: u8 = 1;
const RAGE: u8 = crate::packing::power_type::RAGE;
const ENERGY: u8 = crate::packing::power_type::ENERGY;

/// What vmangos knows about a Form that SpellShapeshiftForm.dbc does not carry.
pub struct FormFact {
    pub form_id: u32,
    /// The spell whose Aura enters the Form.
    pub spell_id: u32,
    /// A `packing::power_type`, or `OWN_POOL`.
    pub power_type: u8,
    pub power_on_entry: PowerOnEntry,
    /// Zero is no class.
    pub default_for_class: u8,
    pub passives: &'static [u32],
    /// The swing time of the Form's natural attacks; zero keeps the weapon combat profile.
    pub natural_attack_ms: u32,
    /// Entering and leaving the Form ends the unit's roots and snares.
    pub clears_snares: bool,
    /// The spell entering the Form casts at the unit's `A_FORM_ENTRY_CHANCE`; zero is none.
    pub entry_chance_spell: u32,
    /// The unit can neither mount nor take a taxi in the Form.
    pub blocks_mounting: bool,
    /// The model a unit of the Alliance and a creature shows in the Form; zero keeps its own.
    pub alliance_display_id: u32,
    /// The model a Horde unit shows in the Form; zero keeps its own.
    pub horde_display_id: u32,
    /// The scale the Form's model shows at.
    pub display_scale: f32,
}

const STANCE: FormFact = FormFact {
    form_id: 0,
    spell_id: 0,
    power_type: RAGE,
    power_on_entry: PowerOnEntry::KeepUpToRetained,
    default_for_class: 0,
    passives: &[],
    natural_attack_ms: 0,
    clears_snares: false,
    entry_chance_spell: 0,
    blocks_mounting: false,
    alliance_display_id: 0,
    horde_display_id: 0,
    display_scale: 1.0,
};

// vmangos Furor: 17099 gives 40 energy and 17057 gives 10 rage.
const FUROR_ENERGY: u32 = 17099;
const FUROR_RAGE: u32 = 17057;

const FERAL: FormFact = FormFact {
    power_on_entry: PowerOnEntry::Keep,
    clears_snares: true,
    entry_chance_spell: FUROR_RAGE,
    blocks_mounting: true,
    ..STANCE
};

const OWN_POOL_FORM: FormFact = FormFact {
    power_type: OWN_POOL,
    power_on_entry: PowerOnEntry::Keep,
    ..STANCE
};

const DRUID_OWN_POOL: FormFact = FormFact {
    clears_snares: true,
    blocks_mounting: true,
    ..OWN_POOL_FORM
};

// vmangos d9173eca: Aura::HandleAuraModShapeshift (power, Shapeshift Form Effect 9033 and Furor),
// Aura::HandleShapeshiftBoosts (passives), Player::InitDataForForm (attack times, and the class's
// own pool in every other Form), Unit::IsInDisallowedMountForm and Player::_LoadAuras (Battle
// Stance 2457 for a warrior without a form) and GetShapeshiftDisplayInfo (models and scales).
pub const FORM_FACTS: &[FormFact] = &[
    FormFact {
        form_id: 1,
        spell_id: 768,
        power_type: ENERGY,
        power_on_entry: PowerOnEntry::Zero,
        passives: &[3025],
        natural_attack_ms: 1000,
        entry_chance_spell: FUROR_ENERGY,
        alliance_display_id: 892,
        horde_display_id: 8571,
        display_scale: 0.8,
        ..FERAL
    },
    FormFact {
        form_id: 5,
        spell_id: 5487,
        passives: &[1178, 21178],
        natural_attack_ms: 2500,
        alliance_display_id: 2281,
        horde_display_id: 2289,
        ..FERAL
    },
    FormFact {
        form_id: 8,
        spell_id: 9634,
        passives: &[9635, 21178],
        natural_attack_ms: 2500,
        alliance_display_id: 2281,
        horde_display_id: 2289,
        ..FERAL
    },
    FormFact {
        form_id: 17,
        spell_id: 2457,
        default_for_class: CLASS_WARRIOR,
        passives: &[21156],
        ..STANCE
    },
    FormFact {
        form_id: 18,
        spell_id: 71,
        passives: &[7376],
        ..STANCE
    },
    FormFact {
        form_id: 19,
        spell_id: 2458,
        passives: &[7381],
        ..STANCE
    },
    FormFact {
        form_id: 3,
        spell_id: 783,
        passives: &[5419],
        alliance_display_id: 632,
        horde_display_id: 632,
        display_scale: 0.8,
        ..DRUID_OWN_POOL
    },
    FormFact {
        form_id: 4,
        spell_id: 1066,
        passives: &[5421],
        alliance_display_id: 2428,
        horde_display_id: 2428,
        display_scale: 0.8,
        ..DRUID_OWN_POOL
    },
    FormFact {
        form_id: 31,
        spell_id: 24858,
        passives: &[24905],
        alliance_display_id: 15374,
        horde_display_id: 15375,
        ..DRUID_OWN_POOL
    },
    FormFact {
        form_id: 16,
        spell_id: 2645,
        blocks_mounting: true,
        alliance_display_id: 4613,
        horde_display_id: 4613,
        display_scale: 0.8,
        ..OWN_POOL_FORM
    },
    FormFact {
        form_id: 28,
        spell_id: 15473,
        ..OWN_POOL_FORM
    },
    FormFact {
        form_id: 30,
        spell_id: 1784,
        ..OWN_POOL_FORM
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_rule_codes_round_trip_and_unknown_codes_are_returned() {
        for rule in [
            PowerOnEntry::Keep,
            PowerOnEntry::Zero,
            PowerOnEntry::KeepUpToRetained,
        ] {
            assert_eq!(PowerOnEntry::try_from(rule as u8), Ok(rule));
        }
        assert_eq!(PowerOnEntry::try_from(3), Err(3));
    }
}
