//! Forms: client SpellShapeshiftForm.dbc rows composed with the vmangos facts the DBC lacks.

use lyracore_package_delta::packed_form_spell_id;
use lyracore_shared::form::{DBC_MASK_FORMS, FORM_FACTS};

use crate::dump::{push_insert, sql_str};

// SpellShapeshiftForm.dbc flags, vmangos SHAPESHIFT_FLAG_STANCE (spells treat the form as
// unshifted) and SHAPESHIFT_FLAG_AGILITY_ATTACK_BONUS (Cat Form).
const SHAPESHIFT_FLAG_STANCE: i32 = 0x1;
const SHAPESHIFT_FLAG_AGILITY_ATTACK_BONUS: i32 = 0x20;

// vmangos HandleAuraModShapeshift maps Tactical Mastery's class script ids to the rage it keeps.
const RETAINED_RAGE_SCRIPTS: [(u32, i32); 5] =
    [(831, 50), (832, 100), (833, 150), (834, 200), (835, 250)];

// vmangos HandleAuraModShapeshift casts Berserker Stance when Nefarian's warrior call ends.
const SHAPESHIFT_RETURN_SPELLS: [(u32, u32); 1] = [(23397, 2458)];

// vmangos HandleAuraModShapeshift takes Furor's chance from its dummy Aura (SpellIconID 238).
const FORM_ENTRY_CHANCE_SPELLS: [u32; 5] = [17056, 17058, 17059, 17060, 17061];

// vmangos Spell::CheckCast refuses every shapeshift while Nefarian's warrior call is held.
const FORM_LOCKING_SPELLS: [u32; 1] = [23397];

/// Whether this spell's Aura locks its holder in its Form.
pub(crate) fn locks_form(spell_id: u32) -> bool {
    FORM_LOCKING_SPELLS.contains(&spell_id)
}

/// Whether this spell's dummy Aura is the chance that entering a Form casts its entry spell.
pub(crate) fn grants_form_entry_chance(spell_id: u32) -> bool {
    FORM_ENTRY_CHANCE_SPELLS.contains(&spell_id)
}

/// The spell a unit gets when this spell's shapeshift Aura ends and leaves it in no Form; the
/// importer stores it as the `A_SHAPESHIFT` effect's trigger spell.
pub(crate) fn shapeshift_return_spell(spell_id: u32) -> Option<u32> {
    SHAPESHIFT_RETURN_SPELLS
        .iter()
        .find(|(spell, _)| *spell == spell_id)
        .map(|(_, next)| *next)
}

/// Whether a source shapeshift Aura for this form imports as `A_SHAPESHIFT`.
pub(crate) fn imports_shapeshift(form_id: u32) -> bool {
    FORM_FACTS.iter().any(|fact| fact.form_id == form_id)
}

/// The rage a Tactical Mastery class script keeps on Form entry.
pub(crate) fn retained_rage(script_id: u32) -> Option<i32> {
    RETAINED_RAGE_SCRIPTS
        .iter()
        .find(|(script, _)| *script == script_id)
        .map(|(_, rage)| *rage)
}

/// The Forms Spell.dbc `Stances` and `StancesNot` name, as `(form_id, excluded)`. Bit
/// `form_id - 1` names a Form. A Form in both masks is excluded, our rule: vmangos reads the masks
/// differently, and no 1.12 row sets both.
pub(crate) fn dbc_form_set(required: u32, excluded: u32) -> impl Iterator<Item = (u32, bool)> {
    DBC_MASK_FORMS
        .filter(move |form| (required | excluded) & (1 << (form - 1)) != 0)
        .map(move |form| (form, excluded & (1 << (form - 1)) != 0))
}

pub(crate) struct ShapeshiftFormEntry {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) flags: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FormRow {
    pub(crate) form_id: u32,
    pub(crate) name: String,
    pub(crate) spell_id: u32,
    pub(crate) acts_as_shapeshifted: bool,
    pub(crate) power_type: u8,
    pub(crate) power_on_entry: u8,
    pub(crate) default_for_class: u8,
    pub(crate) client_form: u8,
    pub(crate) natural_attack_ms: u32,
    pub(crate) attack_power_from_agility: bool,
    pub(crate) clears_snares: bool,
    pub(crate) entry_chance_spell: u32,
}

impl FormRow {
    fn sql_values(&self) -> String {
        format!(
            "({},{},{},{},{},{},{},{},{},{},{},{})",
            self.form_id,
            sql_str(&self.name),
            self.spell_id,
            self.acts_as_shapeshifted,
            self.power_type,
            self.power_on_entry,
            self.default_for_class,
            self.client_form,
            self.natural_attack_ms,
            self.attack_power_from_agility,
            self.clears_snares,
            self.entry_chance_spell
        )
    }
}

#[derive(Default)]
pub(crate) struct FormRows {
    pub(crate) forms: Vec<FormRow>,
    pub(crate) passives: Vec<(u32, u32)>,
}

/// Composes each declared Form the client's SpellShapeshiftForm.dbc defines.
pub(crate) fn form_rows(dbc: &[ShapeshiftFormEntry]) -> FormRows {
    let mut rows = FormRows::default();
    for fact in FORM_FACTS {
        let Some(entry) = dbc.iter().find(|entry| entry.id == fact.form_id) else {
            eprintln!(
                "forms: SpellShapeshiftForm.dbc has no row {}; Form skipped",
                fact.form_id
            );
            continue;
        };
        rows.forms.push(FormRow {
            form_id: fact.form_id,
            name: entry.name.clone(),
            spell_id: fact.spell_id,
            acts_as_shapeshifted: entry.flags & SHAPESHIFT_FLAG_STANCE == 0,
            power_type: fact.power_type,
            power_on_entry: fact.power_on_entry as u8,
            default_for_class: fact.default_for_class,
            // The 1.12 client knows every DBC form by its own id.
            client_form: fact.form_id as u8,
            natural_attack_ms: fact.natural_attack_ms,
            attack_power_from_agility: entry.flags & SHAPESHIFT_FLAG_AGILITY_ATTACK_BONUS != 0,
            clears_snares: fact.clears_snares,
            entry_chance_spell: fact.entry_chance_spell,
        });
        rows.passives
            .extend(fact.passives.iter().map(|&spell| (fact.form_id, spell)));
    }
    rows
}

pub(crate) fn push_form_sql(stmts: &mut Vec<String>, rows: &FormRows) {
    for form in &rows.forms {
        stmts.push(format!(
            "DELETE FROM game_form WHERE form_id = {}",
            form.form_id
        ));
        stmts.push(format!(
            "DELETE FROM game_form_spell WHERE form_id = {}",
            form.form_id
        ));
    }
    push_insert(
        stmts,
        "game_form",
        "form_id,name,spell_id,acts_as_shapeshifted,power_type,power_on_entry,default_for_class,client_form,natural_attack_ms,attack_power_from_agility,clears_snares,entry_chance_spell",
        &rows.forms.iter().map(FormRow::sql_values).collect::<Vec<_>>(),
    );
    push_insert(
        stmts,
        "game_form_spell",
        "id,form_id,spell_id",
        &rows
            .passives
            .iter()
            .map(|&(form, spell)| format!("({},{form},{spell})", packed_form_spell_id(form, spell)))
            .collect::<Vec<_>>(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::form::PowerOnEntry;
    use lyracore_shared::packing::power_type;

    fn build_5875_rows() -> Vec<ShapeshiftFormEntry> {
        [
            (1, "Cat Form", 0x70),
            (17, "Battle Stance", 0x7),
            (18, "Defensive Stance", 0x7),
            (19, "Berserker Stance", 0x7),
            (5, "Bear Form", 0x50),
            (8, "Dire Bear Form", 0x50),
        ]
        .into_iter()
        .map(|(id, name, flags)| ShapeshiftFormEntry {
            id,
            name: name.to_string(),
            flags,
        })
        .collect()
    }

    fn stance(form_id: u32, name: &str, spell_id: u32, default_for_class: u8) -> FormRow {
        FormRow {
            form_id,
            name: name.into(),
            spell_id,
            acts_as_shapeshifted: false,
            power_type: power_type::RAGE,
            power_on_entry: PowerOnEntry::KeepUpToRetained as u8,
            default_for_class,
            client_form: form_id as u8,
            natural_attack_ms: 0,
            attack_power_from_agility: false,
            clears_snares: false,
            entry_chance_spell: 0,
        }
    }

    fn rows_for(rows: &FormRows, forms: &[u32]) -> Vec<FormRow> {
        forms
            .iter()
            .filter_map(|&id| rows.forms.iter().find(|form| form.form_id == id))
            .cloned()
            .collect()
    }

    #[test]
    fn the_three_warrior_stances_become_forms_with_their_vmangos_passives() {
        let rows = form_rows(&build_5875_rows());
        assert_eq!(
            rows_for(&rows, &[17, 18, 19]),
            vec![
                stance(17, "Battle Stance", 2457, 1),
                stance(18, "Defensive Stance", 71, 0),
                stance(19, "Berserker Stance", 2458, 0),
            ]
        );
        for passive in [(17, 21156), (18, 7376), (19, 7381)] {
            assert!(rows.passives.contains(&passive));
        }
    }

    #[test]
    fn the_three_feral_forms_fight_with_natural_attacks_and_clear_snares() {
        let rows = form_rows(&build_5875_rows());
        let feral =
            |form_id: u32, name: &str, spell_id, power, rule: PowerOnEntry, attack_ms| FormRow {
                form_id,
                name: String::from(name),
                spell_id,
                acts_as_shapeshifted: true,
                power_type: power,
                power_on_entry: rule as u8,
                default_for_class: 0,
                client_form: form_id as u8,
                natural_attack_ms: attack_ms,
                attack_power_from_agility: form_id == 1,
                clears_snares: true,
                entry_chance_spell: if form_id == 1 { 17099 } else { 17057 },
            };
        assert_eq!(
            rows_for(&rows, &[1, 5, 8]),
            vec![
                feral(
                    1,
                    "Cat Form",
                    768,
                    power_type::ENERGY,
                    PowerOnEntry::Zero,
                    1000
                ),
                feral(
                    5,
                    "Bear Form",
                    5487,
                    power_type::RAGE,
                    PowerOnEntry::Keep,
                    2500
                ),
                feral(
                    8,
                    "Dire Bear Form",
                    9634,
                    power_type::RAGE,
                    PowerOnEntry::Keep,
                    2500
                ),
            ]
        );
        for passive in [(1, 3025), (5, 1178), (5, 21178), (8, 9635), (8, 21178)] {
            assert!(rows.passives.contains(&passive), "{passive:?}");
        }
    }

    #[test]
    fn furor_is_the_form_entry_chance() {
        for rank in [17056, 17058, 17059, 17060, 17061] {
            assert!(grants_form_entry_chance(rank));
        }
        assert!(!grants_form_entry_chance(17057));
    }

    #[test]
    fn only_declared_forms_import_as_shapeshift_auras() {
        for form in [1, 5, 8, 17, 18, 19] {
            assert!(imports_shapeshift(form));
        }
        // Travel, Aquatic, Ghost Wolf, Shadowform, Stealth, Moonkin and the creature forms keep their
        // old import.
        for form in [3, 4, 14, 15, 16, 28, 30, 31] {
            assert!(!imports_shapeshift(form));
        }
    }

    #[test]
    fn a_dbc_mask_names_each_form_by_its_id_and_an_exclusion_wins() {
        assert_eq!(
            dbc_form_set(0x0003_0000, 0x0002_0000).collect::<Vec<_>>(),
            vec![(17, false), (18, true)]
        );
        assert_eq!(
            dbc_form_set(0x8000_0090, 0).collect::<Vec<_>>(),
            vec![(5, false), (8, false), (32, false)]
        );
        assert_eq!(dbc_form_set(0, 0).count(), 0);
    }

    #[test]
    fn tactical_mastery_keeps_five_rage_per_rank() {
        assert_eq!(retained_rage(831), Some(50));
        assert_eq!(retained_rage(835), Some(250));
        assert_eq!(retained_rage(830), None);
        assert_eq!(retained_rage(836), None);
    }

    #[test]
    fn form_sql_replaces_only_the_forms_it_writes() {
        let mut stmts = Vec::new();
        push_form_sql(&mut stmts, &form_rows(&build_5875_rows()[1..2]));
        assert_eq!(
            stmts,
            vec![
                "DELETE FROM game_form WHERE form_id = 17".to_string(),
                "DELETE FROM game_form_spell WHERE form_id = 17".to_string(),
                "INSERT INTO game_form (form_id,name,spell_id,acts_as_shapeshifted,power_type,power_on_entry,default_for_class,client_form,natural_attack_ms,attack_power_from_agility,clears_snares,entry_chance_spell) VALUES (17,'Battle Stance',2457,false,1,2,1,17,0,false,false,0)".to_string(),
                format!("INSERT INTO game_form_spell (id,form_id,spell_id) VALUES ({},17,21156)", (17u64 << 32) | 21156),
            ]
        );
        let mut empty = Vec::new();
        push_form_sql(&mut empty, &FormRows::default());
        assert!(empty.is_empty());
    }
}
