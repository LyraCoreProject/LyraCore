//! Spell corrections the importer applies over the World Source and the client Spell.dbc.
//! Name-keyed entries cover every rank of that spell; id-keyed entries cover one spell.

use lyracore_shared::spell::taxonomy::*;
use wow_world_base::vanilla::AuraMod;

pub struct NamedSpell {
    pub name: &'static str,
    pub cast_flags: u32,
    pub channeled: bool,
    pub ranged_auto_repeat: bool,
    pub enters_combat: bool,
    /// Effect Kind for an effect the generic mapping leaves scripted, flag-only or inert.
    pub script_kind: Option<u8>,
    /// The kind corrections apply only to effects carrying this aura.
    pub script_kind_aura: Option<AuraMod>,
    /// Replaces the first kind with the second wherever the generic mapping produced it.
    pub kind_swap: Option<(u8, u8)>,
    /// Item entry for a create-item script effect that omits effect_item_type.
    pub created_item: Option<i32>,
    pub periodic_damage_as_persistent_area: bool,
    pub ally_target_as_enemy: bool,
    pub direct_damage_target: Option<u8>,
    /// Effect 0 becomes a periodic energize: (percent of maximum power, period in ms).
    pub first_effect_energize: Option<(i32, u32)>,
}

const NAMED: NamedSpell = NamedSpell {
    name: "",
    cast_flags: 0,
    channeled: false,
    ranged_auto_repeat: false,
    enters_combat: false,
    script_kind: None,
    script_kind_aura: None,
    kind_swap: None,
    created_item: None,
    periodic_damage_as_persistent_area: false,
    ally_target_as_enemy: false,
    direct_damage_target: None,
    first_effect_energize: None,
};

const fn script(name: &'static str, kind: u8) -> NamedSpell {
    NamedSpell {
        name,
        script_kind: Some(kind),
        ..NAMED
    }
}

const fn healthstone(name: &'static str, item: i32) -> NamedSpell {
    NamedSpell {
        name,
        script_kind: Some(E_CREATE_ITEM),
        created_item: Some(item),
        ..NAMED
    }
}

pub const NAMED_SPELLS: &[NamedSpell] = &[
    NamedSpell {
        name: "Backstab",
        cast_flags: CAST_FLAG_REQ_BEHIND | CAST_FLAG_REQ_DAGGER,
        script_kind: Some(E_ADD_COMBO),
        ..NAMED
    },
    NamedSpell {
        name: "Garrote",
        cast_flags: CAST_FLAG_REQ_BEHIND | CAST_FLAG_REQ_STEALTH,
        script_kind: Some(E_ADD_COMBO),
        ..NAMED
    },
    NamedSpell {
        name: "Ambush",
        cast_flags: CAST_FLAG_REQ_BEHIND,
        ..NAMED
    },
    NamedSpell {
        name: "Sap",
        cast_flags: CAST_FLAG_REQ_STEALTH | CAST_FLAG_INCAP_OPENER | CAST_FLAG_STEALTH_SAFE,
        ..NAMED
    },
    NamedSpell {
        name: "Pick Pocket",
        cast_flags: CAST_FLAG_STEALTH_SAFE,
        script_kind: Some(E_PICKPOCKET),
        ..NAMED
    },
    NamedSpell {
        name: "Distract",
        cast_flags: CAST_FLAG_STEALTH_SAFE,
        ..NAMED
    },
    NamedSpell {
        name: "Overpower",
        cast_flags: CAST_FLAG_REQ_OVERPOWER,
        ..NAMED
    },
    NamedSpell {
        name: "Revenge",
        cast_flags: CAST_FLAG_REQ_REVENGE,
        ..NAMED
    },
    NamedSpell {
        name: "Arcane Missiles",
        channeled: true,
        ..NAMED
    },
    NamedSpell {
        name: "Evocation",
        channeled: true,
        first_effect_energize: Some((15, 2000)),
        ..NAMED
    },
    NamedSpell {
        name: "Auto Shot",
        ranged_auto_repeat: true,
        ..NAMED
    },
    NamedSpell {
        name: "Shoot",
        ranged_auto_repeat: true,
        ..NAMED
    },
    NamedSpell {
        name: "Bloodrage",
        enters_combat: true,
        ..NAMED
    },
    // vmangos scripts Reckoning: crits taken stack up to four swings that follow the next swing.
    NamedSpell {
        name: "Reckoning",
        kind_swap: Some((E_ADD_EXTRA_ATTACKS, E_SCRIPTED)),
        ..NAMED
    },
    NamedSpell {
        name: "Feint",
        kind_swap: Some((E_TAUNT, E_REDUCE_THREAT)),
        ..NAMED
    },
    NamedSpell {
        name: "Stealth",
        script_kind: Some(A_STEALTH),
        script_kind_aura: Some(AuraMod::ModStealth),
        ..NAMED
    },
    NamedSpell {
        name: "Consecration",
        periodic_damage_as_persistent_area: true,
        ..NAMED
    },
    NamedSpell {
        name: "Flamestrike",
        periodic_damage_as_persistent_area: true,
        direct_damage_target: Some(T_AREA_ENEMY),
        ..NAMED
    },
    NamedSpell {
        name: "Slice and Dice",
        ally_target_as_enemy: true,
        ..NAMED
    },
    script("Holy Light", E_HEAL),
    script("Flash of Light", E_HEAL),
    script("Life Tap", E_CONVERT_RESOURCE),
    script("Charge", E_CHARGE),
    script("Blink", E_BLINK),
    script("Seal of Righteousness", A_SEAL),
    script("Retaliation", A_RETALIATE),
    script("Battle Stance", E_SET_STANCE),
    script("Defensive Stance", E_SET_STANCE),
    script("Berserker Stance", E_SET_STANCE),
    script("Bear Form", E_SET_STANCE),
    script("Dire Bear Form", E_SET_STANCE),
    script("Cat Form", E_SET_STANCE),
    script("Judgement", E_JUDGEMENT),
    script("Sinister Strike", E_ADD_COMBO),
    script("Gouge", E_ADD_COMBO),
    script("Resurrection", E_RESURRECT),
    script("Redemption", E_RESURRECT),
    script("Rebirth", E_RESURRECT),
    script("Ancestral Spirit", E_RESURRECT),
    healthstone("Create Healthstone (Minor)", 5512),
    healthstone("Create Healthstone (Lesser)", 5511),
    healthstone("Create Healthstone", 5509),
    healthstone("Create Healthstone (Greater)", 5510),
    healthstone("Create Healthstone (Major)", 9421),
    script("Soulstone Resurrection", A_SELF_RESURRECT),
];

pub fn named(name: &str) -> Option<&'static NamedSpell> {
    NAMED_SPELLS.iter().find(|spell| spell.name == name)
}

pub struct SpellIdOverride {
    pub ids: &'static [u32],
    pub name: &'static str,
    /// Mechanic for the control aura instead of the aura's own.
    pub control_mechanic: Option<i32>,
    /// Debuff the absorb effect applies, which the source effect omits.
    pub absorb_debuff: Option<i32>,
    /// Flat Seal damage on a synthetic effect in the first free slot.
    pub seal_damage: Option<i32>,
}

pub const SPELL_ID_OVERRIDES: &[SpellIdOverride] = &[
    SpellIdOverride {
        ids: &[1776, 1777, 8629, 11285, 11286],
        name: "Gouge",
        control_mechanic: Some(M_POLY),
        absorb_debuff: None,
        seal_damage: None,
    },
    SpellIdOverride {
        ids: &[17],
        name: "Power Word: Shield",
        control_mechanic: None,
        absorb_debuff: Some(6788),
        seal_damage: None,
    },
    // The client Dummy magnitude names a spell; the curated Seal marker needs flat damage.
    SpellIdOverride {
        ids: &[21082],
        name: "Seal of the Crusader",
        control_mechanic: None,
        absorb_debuff: None,
        seal_damage: Some(20),
    },
];

pub fn by_id(id: u32, name: &str) -> Option<&'static SpellIdOverride> {
    SPELL_ID_OVERRIDES
        .iter()
        .find(|spell| spell.ids.contains(&id) && spell.name == name)
}

pub struct BindingSummon {
    pub spell: u32,
    pub effect: i32,
    pub event: u32,
    pub focus: u32,
    pub creature: i32,
}

/// Binding summons whose source effect sends an event instead of summoning the creature.
pub const BINDING_SUMMONS: &[BindingSummon] = &[
    BindingSummon {
        spell: 7728,
        effect: 61,
        event: 1131,
        focus: 83,
        creature: 5676,
    },
    BindingSummon {
        spell: 8674,
        effect: 61,
        event: 1134,
        focus: 83,
        creature: 5677,
    },
];
pub const BINDING_SUMMON_DURATION_MS: u32 = 360_000;
pub const BINDING_SUMMON_COOLDOWN_MS: u32 = 180_000;

pub fn binding_summon(spell: u32, effect: i32, event: u32, focus: u32) -> Option<i32> {
    BINDING_SUMMONS
        .iter()
        .find(|binding| {
            (binding.spell, binding.effect, binding.event, binding.focus)
                == (spell, effect, event, focus)
        })
        .map(|binding| binding.creature)
}

/// Soulstone aura spell to the resurrection option spell it offers.
pub const SOULSTONE_OPTIONS: &[(u32, i32)] = &[
    (20707, 3026),
    (20762, 20758),
    (20763, 20759),
    (20764, 20760),
    (20765, 20761),
];

/// Spells whose effect 0 is the fishing marker.
pub const FISHING_MARKERS: &[u32] = &[7620, 7731, 7732];

/// Spells that roll as binary whatever their effects.
pub const BINARY_SPELLS: &[u32] = &[26143, 26478];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_spell_name_and_id_has_one_entry() {
        let mut names: Vec<_> = NAMED_SPELLS.iter().map(|spell| spell.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), NAMED_SPELLS.len());
        let mut ids: Vec<_> = SPELL_ID_OVERRIDES
            .iter()
            .flat_map(|spell| spell.ids.iter().copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            SPELL_ID_OVERRIDES
                .iter()
                .map(|spell| spell.ids.len())
                .sum::<usize>()
        );
    }
}
