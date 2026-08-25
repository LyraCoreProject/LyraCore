use super::super::presentation::NpcFlagsProjection;

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportVerifiedRajaxxProjection {
    _private: (),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlagOverride {
    Base,
    Set,
    Clear,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreaturePresentationMount {
    Clear,
    Raider,
    Kerr,
    Huntress,
    TwilightMarauder,
}

impl CreaturePresentationMount {
    pub(crate) fn display_id(self) -> u32 {
        match self {
            Self::Clear => 0,
            Self::Raider => 207,
            Self::Kerr => 2_328,
            Self::Huntress => 9_991,
            Self::TwilightMarauder => 14_337,
        }
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreaturePresentationInstruction {
    SetFaction { faction_template: u32 },
    ShowTemplateDisplay { template_entry: u32 },
    SetCreatureMount { mount: CreaturePresentationMount },
    SetNpcFlags { flags: NpcFlagsProjection },
    EmptyMana,
    ClearVirtualMainHand,
    SetNotAttackable,
    ClearNotAttackable,
    SetImmuneToPlayers,
    ClearImmuneToPlayers,
    SetImmuneToCreatures,
    ClearImmuneToCreatures,
    SetImmuneToPlayersAndCreatures,
    ClearImmuneToPlayersAndCreatures,
    SetNotSelectable,
    SetRajaxxSpawnProtection(ImportVerifiedRajaxxProjection),
}

pub(super) fn import_verified_rajaxx_spawn_protection() -> CreaturePresentationInstruction {
    CreaturePresentationInstruction::SetRajaxxSpawnProtection(ImportVerifiedRajaxxProjection {
        _private: (),
    })
}
