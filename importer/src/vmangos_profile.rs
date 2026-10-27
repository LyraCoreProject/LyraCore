use std::cell::RefCell;
use std::collections::BTreeMap;

use anyhow::{bail, ensure, Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

pub(crate) const NAME: &str = "vmangos-4641790";
const PROFILE: &str = include_str!("../manifests/vmangos-4641790.json");

pub(crate) struct SourceProfile {
    definition: Value,
    counts: RefCell<BTreeMap<String, Value>>,
}

impl SourceProfile {
    pub(crate) fn load() -> Result<Self> {
        let definition: Value = serde_json::from_str(PROFILE)?;
        ensure!(
            definition["name"] == NAME && definition["format_version"] == 1,
            "invalid vmangos Source Profile"
        );
        Ok(Self {
            definition,
            counts: RefCell::default(),
        })
    }

    pub(crate) fn check_table(&self, connection: &Connection, table: &str) -> Result<()> {
        if self.counts.borrow().contains_key(table) {
            return Ok(());
        }
        let contract = &self.definition["tables"][table];
        let columns = contract["columns"]
            .as_object()
            .with_context(|| format!("{NAME}: unapproved source table {table}"))?;
        let mut statement = connection.prepare("SELECT name, type FROM pragma_table_info(?1)")?;
        let actual = statement
            .query_map([table], |row| {
                Ok((row.get::<_, String>("name")?, row.get::<_, String>("type")?))
            })?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        for (expected, names) in columns {
            for column in names.as_array().context("invalid profile column list")? {
                let column = column.as_str().context("invalid profile column name")?;
                let found = actual
                    .get(column)
                    .with_context(|| format!("{NAME}: {table}.{column}: missing column"))?;
                ensure!(
                    affinity(found) == expected,
                    "{NAME}: {table}.{column}: expected {expected}, got {found}"
                );
            }
        }
        if let Some(values) = contract["values"].as_object() {
            for (column, approved) in values {
                check_values(connection, table, column, None, |value| {
                    approved
                        .as_array()
                        .is_some_and(|allowed| allowed.contains(&json!(value)))
                })?;
            }
        }
        if let Some(bits) = contract["bits"].as_object() {
            for (column, allowed) in bits {
                let allowed = allowed.as_u64().context("invalid profile flag bits")?;
                check_values(connection, table, column, None, |value| {
                    u64::try_from(value).is_ok_and(|value| value & !allowed == 0)
                })?;
            }
        }
        if let Some(bits) = contract["command_bits"].as_array() {
            for rule in bits {
                let command = rule["command"]
                    .as_u64()
                    .context("invalid profile command")?;
                let column = rule["column"]
                    .as_str()
                    .context("invalid profile command column")?;
                let allowed = rule["allowed"]
                    .as_u64()
                    .context("invalid profile command bits")?;
                check_values(connection, table, column, Some(command), |value| {
                    u64::try_from(value).is_ok_and(|value| value & !allowed == 0)
                })?;
            }
        }
        if let Some(values) = contract["command_values"].as_array() {
            for rule in values {
                let command = rule["command"]
                    .as_u64()
                    .context("invalid profile command")?;
                let column = rule["column"]
                    .as_str()
                    .context("invalid profile command column")?;
                let allowed = rule["allowed"]
                    .as_array()
                    .context("invalid profile command values")?;
                check_values(connection, table, column, Some(command), |value| {
                    allowed.contains(&json!(value))
                })?;
            }
        }
        let count = connection.query_row(
            &format!("SELECT count(*) AS total FROM {}", quote(table)),
            [],
            |row| row.get::<_, i64>("total"),
        )?;
        self.counts.borrow_mut().insert(
            table.to_owned(),
            json!({"locked":contract["locked_rows"],"observed":count}),
        );
        Ok(())
    }

    pub(crate) fn report(&self) -> Value {
        json!({"name": NAME, "accepted_commit": self.definition["commit"], "counts": *self.counts.borrow(), "counts_gate": false})
    }
}

fn check_values(
    connection: &Connection,
    table: &str,
    column: &str,
    command: Option<u64>,
    accepted: impl Fn(i64) -> bool,
) -> Result<()> {
    let filter = command.map_or_else(String::new, |command| format!(" WHERE command = {command}"));
    let mut statement = connection.prepare(&format!(
        "SELECT DISTINCT {} AS value FROM {}{filter} ORDER BY value",
        quote(column),
        quote(table)
    ))?;
    let values = statement
        .query_map([], |row| row.get::<_, i64>("value"))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let unapproved = values
        .into_iter()
        .filter(|value| !accepted(*value))
        .collect::<Vec<_>>();
    if !unapproved.is_empty() {
        bail!(
            "{}",
            json!({"source_profile":NAME,"classification":"unapproved","table":table,"column":column,"command":command,"values":unapproved})
        );
    }
    Ok(())
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn affinity(declaration: &str) -> &'static str {
    let name = declaration.to_ascii_uppercase();
    if name.contains("INT") {
        "integer"
    } else if ["CHAR", "CLOB", "TEXT"]
        .iter()
        .any(|part| name.contains(part))
    {
        "text"
    } else if ["REAL", "FLOA", "DOUB"]
        .iter()
        .any(|part| name.contains(part))
    {
        "real"
    } else if name.is_empty() || name.contains("BLOB") {
        "blob"
    } else {
        "numeric"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> SourceProfile {
        SourceProfile {
            definition: json!({"tables": {
                "events": {"columns":{"integer":["kind","flags"]},"values":{"kind":[1,2]},"bits":{"flags":3},"locked_rows":1},
                "other": {"columns":{"text":["name"]},"locked_rows":0}
            }}),
            counts: RefCell::default(),
        }
    }

    #[test]
    fn changed_counts_and_combinations_of_approved_bits_pass() {
        let database = Connection::open_in_memory().unwrap();
        database.execute_batch("CREATE TABLE events (kind INTEGER, flags INTEGER, new_column TEXT); INSERT INTO events VALUES (1,1,''),(2,3,'');").unwrap();
        let profile = profile();
        profile.check_table(&database, "events").unwrap();
        assert_eq!(
            profile.report()["counts"]["events"],
            json!({"locked":1,"observed":2})
        );
        assert_eq!(profile.report()["counts_gate"], false);
    }

    #[test]
    fn unknown_values_and_bits_are_refusals() {
        for (kind, flags, column) in [(9, 1, "kind"), (1, 4, "flags")] {
            let database = Connection::open_in_memory().unwrap();
            database.execute_batch(&format!("CREATE TABLE events (kind INTEGER, flags INTEGER); INSERT INTO events VALUES ({kind},{flags});")).unwrap();
            let error = profile()
                .check_table(&database, "events")
                .unwrap_err()
                .to_string();
            let finding: Value = serde_json::from_str(&error).unwrap();
            assert_eq!(finding["classification"], "unapproved");
            assert_eq!(finding["column"], column);
        }
    }

    #[test]
    fn empty_tables_still_need_the_column_contract_and_other_tables_remain_readable() {
        for declaration in ["kind INTEGER", "kind INTEGER, flags TEXT"] {
            let database = Connection::open_in_memory().unwrap();
            database
                .execute_batch(&format!(
                    "CREATE TABLE events ({declaration}); CREATE TABLE other (name TEXT);"
                ))
                .unwrap();
            let profile = profile();
            assert!(profile
                .check_table(&database, "events")
                .unwrap_err()
                .to_string()
                .contains("events.flags"));
            profile.check_table(&database, "other").unwrap();
        }
    }

    #[test]
    fn command_flag_checks_apply_to_their_command_only() {
        let database = Connection::open_in_memory().unwrap();
        database.execute_batch("CREATE TABLE scripts (command INTEGER, datalong2 INTEGER); INSERT INTO scripts VALUES (15,2),(28,999);").unwrap();
        check_values(&database, "scripts", "datalong2", Some(15), |value| {
            value & !3 == 0
        })
        .unwrap();
        database
            .execute("INSERT INTO scripts VALUES (15,4)", [])
            .unwrap();
        assert!(
            check_values(&database, "scripts", "datalong2", Some(15), |value| value
                & !3
                == 0)
            .is_err()
        );
    }
}
