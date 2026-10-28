use std::collections::BTreeMap;

use lyracore_shared::constants::gossip_condition;
use lyracore_shared::gossip::{
    evaluate_reference, CharacterFacts, Condition, ConditionKind, QuestState,
};

use crate::dump::{gmo, parse_table};

pub(super) type Conditions = BTreeMap<u32, Condition>;

fn source_kind(kind: i32) -> ConditionKind {
    match kind {
        -3 => ConditionKind::Not,
        -2 => ConditionKind::Any,
        -1 => ConditionKind::All,
        0 => ConditionKind::Always,
        6 => ConditionKind::Team,
        8 => ConditionKind::QuestRewarded,
        9 => ConditionKind::QuestTaken,
        14 => ConditionKind::RaceClass,
        15 => ConditionKind::Level,
        22 => ConditionKind::QuestAbsent,
        other => ConditionKind::Unsupported(other),
    }
}

pub(super) fn read(dump: &(impl crate::dump::TableRows + ?Sized)) -> anyhow::Result<Conditions> {
    let mut conditions = BTreeMap::new();
    for row in parse_table(dump, "conditions")?.iter() {
        let id = row.number::<u32>("condition_entry")?;
        if id == 0 {
            continue;
        }
        let source_type = row.number::<i32>("type")?;
        let kind = source_kind(source_type);
        let unsupported = matches!(kind, ConditionKind::Unsupported(_)) && source_type != 24;
        let mut values = [0; 4];
        for (slot, value) in values.iter_mut().enumerate() {
            let column = format!("value{}", slot + 1);
            if unsupported {
                row.number::<i64>(&column)?;
            } else {
                *value = row.number(&column)?;
            }
        }
        let mut condition = Condition {
            kind: if unsupported {
                ConditionKind::Unsupported(i32::MIN)
            } else {
                kind
            },
            values,
            flags: row.number("flags")?,
        };
        if source_type == 24 && condition.values[2..] == [0, 0] {
            let enabled =
                crate::source_selection::wow_patch(condition.values[0], condition.values[1])?;
            condition.kind = ConditionKind::Always;
            condition.values = [0; 4];
            condition.flags ^= u8::from(!enabled);
        }
        let unavailable = Condition {
            kind: ConditionKind::Unsupported(i32::MIN),
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
            format!("({id},{},{a},{b},{c},{d},{flags})", kind.code())
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
    let menu_id = row.numeric_text(gmo::MENU_ID)?;
    let option_id = row.numeric_text(gmo::ID)?;
    let diagnostic = |reason: &str| {
        eprintln!(
            "gossip menu {} option {}: {reason}; unavailable",
            menu_id, option_id,
        )
    };
    let Ok(id) = row.numeric_text(gmo::CONDITION_ID)?.parse::<u32>() else {
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
    use lyracore_shared::gossip::Unavailable;

    fn conditions(sql: &str) -> crate::world_source::WorldSource {
        crate::world_source::fixture_sql(&format!(
            "CREATE TABLE conditions (condition_entry INTEGER, type INTEGER, value1 INTEGER DEFAULT 0,
                value2 INTEGER DEFAULT 0, value3 INTEGER DEFAULT 0, value4 INTEGER DEFAULT 0,
                flags INTEGER DEFAULT 0); {sql}"
        ))
    }

    #[test]
    fn compound_operands_and_reverse_flags_survive_import() {
        let rows = read(&conditions("INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`, `value3`, `value4`, `flags`) VALUES (501,-2,502,503,504,505,1),(502,15,20,1,0,0,0),(503,6,469,0,0,0,0),(504,14,1,128,0,0,0),(505,8,777,0,0,0,0);")).unwrap();
        assert_eq!(
            rows[&501],
            Condition {
                kind: ConditionKind::Any,
                values: [502, 503, 504, 505],
                flags: 1
            }
        );
        assert_eq!(rows[&504].values, [1, 128, 0, 0]);
        assert_eq!(sql_rows(&rows)[0], "(501,-2,502,503,504,505,1)");
    }

    #[test]
    fn non_numeric_operands_fail_the_read() {
        let error = read(&conditions("INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`) VALUES (1,9,777,'bad');")).unwrap_err();
        assert!(error.to_string().contains("conditions.value2"));
    }

    #[test]
    fn duplicate_and_unresolvable_source_rows_stay_unavailable() {
        let rows = read(&conditions("INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`) VALUES (1,99,777,0),(2,0,0,0),(2,0,0,0),(3,-3,1,0),(4,-3,2,0),(5,-3,6,0),(6,-3,5,0);")).unwrap();
        for (id, unavailable) in [
            (1, Unavailable::Unsupported(i32::MIN)),
            (2, Unavailable::Unsupported(i32::MIN)),
            (3, Unavailable::Unsupported(i32::MIN)),
            (4, Unavailable::Unsupported(i32::MIN)),
            (5, Unavailable::Cycle(5)),
            (6, Unavailable::Cycle(6)),
        ] {
            assert_eq!(
                evaluate_reference(
                    id,
                    CharacterFacts::default(),
                    |_| QuestState::Absent,
                    |id| rows.get(&id).copied()
                ),
                Err(unavailable)
            );
        }
    }
}
