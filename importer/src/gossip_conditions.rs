use std::collections::BTreeMap;

use lyracore_shared::constants::gossip_condition;
use lyracore_shared::gossip::{evaluate_reference, CharacterFacts, Condition, QuestState};

use crate::dump::{field, gmo, parse_table};

pub(super) type Conditions = BTreeMap<u32, Condition>;

pub(super) fn read(dump: &(impl crate::dump::TableRows + ?Sized)) -> anyhow::Result<Conditions> {
    let mut conditions = BTreeMap::new();
    for row in parse_table(dump, "conditions")?.iter() {
        let Ok(id) = field(row, "condition_entry")?.parse::<u32>() else {
            eprintln!(
                "gossip condition: invalid source id {}",
                field(row, "condition_entry")?
            );
            continue;
        };
        if id == 0 {
            continue;
        }
        let condition = Condition {
            kind: row.number("type")?,
            values: [
                row.number("value1")?,
                row.number("value2")?,
                row.number("value3")?,
                row.number("value4")?,
            ],
            flags: row.number("flags")?,
        };
        let unavailable = Condition {
            kind: i32::MIN,
            values: [0; 4],
            flags: 0,
        };
        if conditions.insert(id, condition).is_some() {
            eprintln!("gossip condition {id}: duplicate source id; unavailable");
            conditions.insert(id, unavailable);
        }
    }
    Ok(conditions)
}

pub(super) fn sql_rows(conditions: &Conditions) -> Vec<String> {
    conditions
        .iter()
        .map(|(id, condition)| {
            let Condition {
                kind,
                values: [a, b, c, d],
                flags,
            } = condition;
            format!("({id},{kind},{a},{b},{c},{d},{flags})")
        })
        .collect()
}

pub(super) fn resolve(
    row: &crate::world_source::Row,
    conditions: &Conditions,
) -> anyhow::Result<(u32, u32, u32)> {
    let facts = CharacterFacts {
        race: 1,
        class: 1,
        level: 1,
    };
    let menu_id = field(row, gmo::MENU_ID)?;
    let option_id = field(row, gmo::ID)?;
    let diagnostic = |reason: &str| {
        eprintln!(
            "gossip menu {} option {}: {reason}; unavailable",
            menu_id, option_id,
        )
    };
    let Ok(id) = field(row, gmo::CONDITION_ID)?.parse::<u32>() else {
        diagnostic("invalid condition reference");
        return Ok((gossip_condition::NEVER, 0, 0));
    };
    if id == 0 {
        return Ok((gossip_condition::NONE, 0, 0));
    }
    if let Err(reason) = evaluate_reference(
        id,
        facts,
        |_| QuestState::Absent,
        |id| conditions.get(&id).copied(),
    ) {
        diagnostic(&format!("condition {id}: {reason:?}"));
    }
    Ok((gossip_condition::REFERENCE, id, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_operands_and_reverse_flags_survive_import() {
        let rows = read("INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`, `value3`, `value4`, `flags`) VALUES (501,-2,502,503,504,505,1),(502,15,20,1,0,0,0),(503,6,469,0,0,0,0),(504,14,1,128,0,0,0),(505,8,777,0,0,0,0);").unwrap();
        assert_eq!(
            rows[&501],
            Condition {
                kind: -2,
                values: [502, 503, 504, 505],
                flags: 1
            }
        );
        assert_eq!(rows[&504].values, [1, 128, 0, 0]);
        assert_eq!(sql_rows(&rows)[0], "(501,-2,502,503,504,505,1)");
    }

    #[test]
    fn non_numeric_operands_fail_the_read() {
        let error = read("INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`) VALUES (1,9,777,'bad');").unwrap_err();
        assert!(error.to_string().contains("conditions.value2"));
    }

    #[test]
    fn duplicate_and_unresolvable_source_rows_stay_unavailable() {
        let rows = read("INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`) VALUES (1,99,777,0),(2,0,0,0),(2,0,0,0),(3,-3,1,0),(4,-3,2,0),(5,-3,6,0),(6,-3,5,0);").unwrap();
        for id in [1, 2, 3, 4, 5, 6] {
            assert!(evaluate_reference(
                id,
                CharacterFacts::default(),
                |_| QuestState::Absent,
                |id| rows.get(&id).copied()
            )
            .is_err());
        }
    }
}
