//! Forms: client SpellShapeshiftForm.dbc rows composed with the vmangos facts the DBC lacks.

use lyracore_package_delta::packed_form_spell_id;
use lyracore_shared::form::FORM_FACTS;

use crate::dump::{push_insert, sql_str};

// SpellShapeshiftForm.dbc flags bit 1 (vmangos SHAPESHIFT_FLAG_STANCE): spells treat the form as
// unshifted.
const SHAPESHIFT_FLAG_STANCE: i32 = 0x1;

// vmangos HandleAuraModShapeshift maps Tactical Mastery's class script ids to the rage it keeps.
const RETAINED_RAGE_SCRIPTS: [(u32, i32); 5] =
    [(831, 50), (832, 100), (833, 150), (834, 200), (835, 250)];

// vmangos HandleAuraModShapeshift casts Berserker Stance when Nefarian's warrior call ends.
const SHAPESHIFT_RETURN_SPELLS: [(u32, u32); 1] = [(23397, 2458)];

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
/// `form_id - 1` names a Form; an excluded Form wins over a required one, as in vmangos.
pub(crate) fn dbc_form_set(required: u32, excluded: u32) -> impl Iterator<Item = (u32, bool)> {
    (1..=32u32)
        .filter(move |form| (required | excluded) & (1 << (form - 1)) != 0)
        .map(move |form| (form, excluded & (1 << (form - 1)) != 0))
}

pub(crate) struct ShapeshiftFormEntry {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) flags: i32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FormRow {
    pub(crate) form_id: u32,
    pub(crate) name: String,
    pub(crate) spell_id: u32,
    pub(crate) acts_as_shapeshifted: bool,
    pub(crate) power_type: u8,
    pub(crate) power_on_entry: u8,
    pub(crate) default_for_class: u8,
    pub(crate) client_form: u8,
}

impl FormRow {
    fn sql_values(&self) -> String {
        format!(
            "({},{},{},{},{},{},{},{})",
            self.form_id,
            sql_str(&self.name),
            self.spell_id,
            self.acts_as_shapeshifted,
            self.power_type,
            self.power_on_entry,
            self.default_for_class,
            self.client_form
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
        "form_id,name,spell_id,acts_as_shapeshifted,power_type,power_on_entry,default_for_class,client_form",
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
        ]
        .into_iter()
        .map(|(id, name, flags)| ShapeshiftFormEntry {
            id,
            name: name.to_string(),
            flags,
        })
        .collect()
    }

    #[test]
    fn the_three_warrior_stances_become_forms_with_their_vmangos_passives() {
        let rows = form_rows(&build_5875_rows());
        let rage = power_type::RAGE;
        let rule = PowerOnEntry::KeepUpToRetained as u8;
        assert_eq!(
            rows.forms,
            vec![
                FormRow {
                    form_id: 17,
                    name: "Battle Stance".into(),
                    spell_id: 2457,
                    acts_as_shapeshifted: false,
                    power_type: rage,
                    power_on_entry: rule,
                    default_for_class: 1,
                    client_form: 17,
                },
                FormRow {
                    form_id: 18,
                    name: "Defensive Stance".into(),
                    spell_id: 71,
                    acts_as_shapeshifted: false,
                    power_type: rage,
                    power_on_entry: rule,
                    default_for_class: 0,
                    client_form: 18,
                },
                FormRow {
                    form_id: 19,
                    name: "Berserker Stance".into(),
                    spell_id: 2458,
                    acts_as_shapeshifted: false,
                    power_type: rage,
                    power_on_entry: rule,
                    default_for_class: 0,
                    client_form: 19,
                },
            ]
        );
        assert_eq!(rows.passives, vec![(17, 21156), (18, 7376), (19, 7381)]);
    }

    #[test]
    fn only_declared_forms_import_as_shapeshift_auras() {
        for form in [17, 18, 19] {
            assert!(imports_shapeshift(form));
        }
        // Cat, Bear, Ghost Wolf, Shadowform, Stealth and the creature forms keep their old import.
        for form in [1, 5, 8, 14, 15, 16, 28, 30] {
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
                "INSERT INTO game_form (form_id,name,spell_id,acts_as_shapeshifted,power_type,power_on_entry,default_for_class,client_form) VALUES (17,'Battle Stance',2457,false,1,2,1,17)".to_string(),
                format!("INSERT INTO game_form_spell (id,form_id,spell_id) VALUES ({},17,21156)", (17u64 << 32) | 21156),
            ]
        );
        let mut empty = Vec::new();
        push_form_sql(&mut empty, &FormRows::default());
        assert!(empty.is_empty());
    }
}
