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

/// The Forms a spell requires or excludes. It holds Form ids, so a Form outside the 1.12 client's
/// 32-bit `Stances` mask still fits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormSet(Vec<u32>);

impl FormSet {
    /// Spell.dbc `Stances` and `StancesNot` set bit `form_id - 1` for each Form.
    pub fn from_dbc_mask(mask: u32) -> Self {
        Self(
            (0..32)
                .filter(|bit| mask & (1 << bit) != 0)
                .map(|bit| bit + 1)
                .collect(),
        )
    }

    pub fn contains(&self, form_id: u32) -> bool {
        self.0.contains(&form_id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dbc_mask_names_each_form_by_its_id() {
        let stances = FormSet::from_dbc_mask(0x0007_0000);
        assert!(stances.contains(17) && stances.contains(18) && stances.contains(19));
        assert!(!stances.contains(16) && !stances.contains(20));
        let bear = FormSet::from_dbc_mask(0x90);
        assert!(bear.contains(5) && bear.contains(8) && !bear.contains(1));
        assert!(FormSet::from_dbc_mask(0).is_empty());
        assert!(FormSet::from_dbc_mask(0x8000_0000).contains(32));
        assert!(!FormSet::from_dbc_mask(u32::MAX).contains(33));
    }

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
