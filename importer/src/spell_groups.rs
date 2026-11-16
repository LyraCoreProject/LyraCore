use std::collections::BTreeSet;

use anyhow::Result;

use super::*;

const MAGIC_TAKEN_GROUP: u32 = 0x4000_0003;

pub(super) fn append(
    source: &impl crate::dump::TableRows,
    spells: &[Spell],
    rows: &SpellRows,
    statements: &mut Vec<String>,
) -> Result<()> {
    let selected: BTreeSet<_> = rows
        .effects
        .iter()
        .filter(|effect| {
            matches!(
                effect.kind,
                A_MOD_DAMAGE_DONE_FLAT
                    | A_MOD_DAMAGE_TAKEN_FLAT
                    | A_MOD_SCHOOL_CRIT
                    | A_MOD_DAMAGE_DONE_PCT
                    | A_MOD_HEALING_TAKEN
            )
        })
        .map(|effect| effect.spell_id)
        .collect();
    if selected.is_empty() {
        return Ok(());
    }
    let memberships = source.rows("spell_group")?;
    let mut groups = BTreeSet::new();
    for row in memberships.iter() {
        if selected.contains(&row.number::<u32>("spell_id")?) {
            groups.insert(row.number::<u32>("group_id")?);
        }
    }
    let rules = source.rows("spell_group_stack_rules")?;
    for group in groups {
        let mut members = Vec::new();
        for row in memberships.iter() {
            if row.number::<u32>("group_id")? == group {
                members.push((
                    row.number::<u32>("spell_id")?,
                    row.number::<u32>("group_spell_id")?.saturating_add(1),
                ));
            }
        }
        let mut rule = 0;
        for row in rules.iter() {
            if row.number::<u32>("group_id")? == group {
                rule = row.number::<u8>("stack_rule")?;
            }
        }
        write_group(statements, group, rule, &members);
    }
    let magic_taken: Vec<_> = spells
        .iter()
        .filter(|spell| {
            spell.family_name == 3
                && spell.effect == [6, 6, 0]
                && spell.effect_aura == [14, 115, 0]
                && spell.effect_misc_value == [126, 126, 0]
                && spell.effect_item_type == [0, 0, 0]
        })
        .map(|spell| (spell.id, 0))
        .collect();
    if magic_taken
        .iter()
        .any(|(spell, _)| selected.contains(spell))
    {
        write_group(statements, MAGIC_TAKEN_GROUP, 1, &magic_taken);
    }
    Ok(())
}

fn write_group(statements: &mut Vec<String>, group: u32, rule: u8, members: &[(u32, u32)]) {
    statements.push(format!(
        "DELETE FROM game_spell_group WHERE group_id = {group}"
    ));
    statements.push(format!(
        "DELETE FROM game_spell_group_rule WHERE group_id = {group}"
    ));
    for (spell, power_rank) in members {
        let id = (u64::from(group) << 32) | u64::from(*spell);
        statements.push(format!(
            "INSERT INTO game_spell_group (id,group_id,spell_id,power_rank) VALUES ({id},{group},{spell},{power_rank})"
        ));
    }
    statements.push(format!("INSERT INTO game_spell_group_rule (group_id,rule,rank_is_comparable) VALUES ({group},{rule},false)"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_import_rewrites_only_the_source_groups_it_names() {
        let source = crate::world_source::fixture_sql(
            "CREATE TABLE spell_group (group_id INTEGER, group_spell_id INTEGER, spell_id INTEGER,
                 build_min INTEGER, build_max INTEGER,
                 PRIMARY KEY (group_id, group_spell_id, spell_id));
             INSERT INTO spell_group VALUES (1007, 0, 20911, 0, 5875), (1007, 1, 25899, 0, 5875);
             CREATE TABLE spell_group_stack_rules (group_id INTEGER, build INTEGER, stack_rule INTEGER,
                 PRIMARY KEY (group_id, build));
             INSERT INTO spell_group_stack_rules VALUES (1007, 0, 1);",
        );
        let rows = SpellRows {
            effects: [20911, 25899]
                .map(|spell| SpellEffectRow {
                    kind: A_MOD_DAMAGE_TAKEN_FLAT,
                    ..blank_effect(spell, 0)
                })
                .into(),
            ..Default::default()
        };
        let mut statements = Vec::new();
        append(&source, &[], &rows, &mut statements).unwrap();
        let membership_deletes: Vec<_> = statements
            .iter()
            .filter(|sql| sql.starts_with("DELETE FROM game_spell_group "))
            .collect();
        assert_eq!(
            membership_deletes,
            ["DELETE FROM game_spell_group WHERE group_id = 1007"]
        );
    }
}
