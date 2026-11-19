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

/// The Forms Spell.dbc `Stances` and `StancesNot` can name: bit `form_id - 1` reaches Form 32.
pub const DBC_MASK_FORMS: std::ops::RangeInclusive<u32> = 1..=32;

const CLASS_WARRIOR: u8 = 1;
const RAGE: u8 = crate::packing::power_type::RAGE;

/// What vmangos knows about a Form that SpellShapeshiftForm.dbc does not carry.
pub struct FormFact {
    pub form_id: u32,
    /// The spell whose Aura enters the Form.
    pub spell_id: u32,
    pub power_type: u8,
    pub power_on_entry: PowerOnEntry,
    /// Zero is no class.
    pub default_for_class: u8,
    pub passives: &'static [u32],
}

// vmangos d9173eca: Aura::HandleAuraModShapeshift (power), Aura::HandleShapeshiftBoosts
// (passives) and Player::_LoadAuras (Battle Stance 2457 for a warrior without a form).
pub const FORM_FACTS: &[FormFact] = &[
    FormFact {
        form_id: 17,
        spell_id: 2457,
        power_type: RAGE,
        power_on_entry: PowerOnEntry::KeepUpToRetained,
        default_for_class: CLASS_WARRIOR,
        passives: &[21156],
    },
    FormFact {
        form_id: 18,
        spell_id: 71,
        power_type: RAGE,
        power_on_entry: PowerOnEntry::KeepUpToRetained,
        default_for_class: 0,
        passives: &[7376],
    },
    FormFact {
        form_id: 19,
        spell_id: 2458,
        power_type: RAGE,
        power_on_entry: PowerOnEntry::KeepUpToRetained,
        default_for_class: 0,
        passives: &[7381],
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
