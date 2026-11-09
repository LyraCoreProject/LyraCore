use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::plan::Catalogue;

const CONTINENT_MAP_TYPE: u64 = 0;

pub(crate) struct Shard {
    pub(crate) server: String,
    pub(crate) database: String,
}

impl Shard {
    pub(crate) fn call(&self, reducer: &str, argument: &str) -> Result<Result<(), String>> {
        self.call_with(reducer, &[argument.to_string()])
    }

    // Returns the reducer's refusal text when the call fails.
    pub(crate) fn call_with(
        &self,
        reducer: &str,
        arguments: &[String],
    ) -> Result<Result<(), String>> {
        let output = spacetime()
            .args(["call", "-s", &self.server, &self.database, reducer])
            .args(arguments)
            .output();
        let output = match output {
            Ok(output) => output,
            Err(error) => return Ok(Err(format!("could not run spacetime call: {error}"))),
        };
        if output.status.success() {
            return Ok(Ok(()));
        }
        Ok(Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_string()))
    }

    fn rows(&self, query: &str, columns: &[&str]) -> Result<Vec<Vec<Value>>> {
        let output = spacetime()
            .args([
                "sql",
                "--format",
                "json",
                "-s",
                &self.server,
                &self.database,
                query,
            ])
            .output()
            .with_context(|| format!("run spacetime sql: {query}"))?;
        if !output.status.success() {
            bail!(
                "{query} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("parse the JSON result of {query}"))?;
        column_rows(&value, columns).with_context(|| format!("read the result of {query}"))
    }

    fn ids(&self, table: &str, column: &str) -> Result<HashSet<u32>> {
        self.rows(&format!("SELECT {column} FROM {table}"), &[column])?
            .iter()
            .map(|row| number(&row[0]))
            .collect()
    }

    pub(crate) fn catalogue(&self) -> Result<Catalogue> {
        let mut spells = self.ids("game_spell", "spell_id")?;
        let rank_columns = [
            "spell_id",
            "rank_spell_2",
            "rank_spell_3",
            "rank_spell_4",
            "rank_spell_5",
        ];
        for row in self.rows(
            &format!("SELECT {} FROM game_talent", rank_columns.join(", ")),
            &rank_columns,
        )? {
            for value in row {
                let spell = number(&value)?;
                if spell != 0 {
                    spells.insert(spell);
                }
            }
        }
        let mut reputation_factions = HashSet::new();
        for row in self.rows(
            "SELECT faction_id, reputation_index FROM game_faction",
            &["faction_id", "reputation_index"],
        )? {
            if row[1].as_i64().is_some_and(|index| index >= 0) {
                reputation_factions.insert(number(&row[0])?);
            }
        }
        let mut maps = HashSet::new();
        let mut continents = HashSet::new();
        for row in self.rows("SELECT id, map_type FROM game_map", &["id", "map_type"])? {
            let map = number(&row[0])?;
            maps.insert(map);
            if row[1].as_u64() == Some(CONTINENT_MAP_TYPE) {
                continents.insert(map);
            }
        }
        let mut beasts = HashSet::new();
        for row in self.rows(
            "SELECT entry, creature_family FROM game_creature_template",
            &["entry", "creature_family"],
        )? {
            if row[1].as_u64().is_some_and(|family| family != 0) {
                beasts.insert(number(&row[0])?);
            }
        }
        let area_bits = self
            .rows("SELECT area_bit FROM game_area", &["area_bit"])?
            .iter()
            .filter_map(|row| row[0].as_i64().and_then(|bit| u32::try_from(bit).ok()))
            .collect();
        let character_names = self
            .rows("SELECT name FROM game_character", &["name"])?
            .iter()
            .filter_map(|row| row[0].as_str().map(str::to_lowercase))
            .collect();
        let mut accounts = HashMap::new();
        for row in self.rows(
            "SELECT username, salt, verifier FROM game_account",
            &["username", "salt", "verifier"],
        )? {
            if let Some(username) = row[0].as_str() {
                accounts.insert(username.to_string(), (hex(&row[1])?, hex(&row[2])?));
            }
        }
        let migrated = self
            .rows(
                "SELECT source_key FROM game_character_migration",
                &["source_key"],
            )?
            .iter()
            .filter_map(|row| row[0].as_str().map(str::to_string))
            .collect();
        Ok(Catalogue {
            items: self.ids("game_item_template", "entry")?,
            spells,
            skills: self.ids("game_skill_line", "skill_line")?,
            reputation_factions,
            quests: self.ids("game_quest_template", "entry")?,
            area_bits,
            taxi_nodes: self.ids("game_taxi_node", "client_node_id")?,
            maps,
            continents,
            beasts,
            character_names,
            accounts,
            migrated,
        })
    }
}

fn spacetime() -> Command {
    let mut command = Command::new("spacetime");
    if let Some(path) = std::env::var_os("LYRACORE_SPACETIME_CONFIG_PATH") {
        command.arg("--config-path").arg(path);
    }
    command
}

fn number(value: &Value) -> Result<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .with_context(|| format!("{value} is not a 32-bit id"))
}

// SATS JSON writes a byte array as a hex string or as an array of numbers.
fn hex(value: &Value) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.to_ascii_lowercase()),
        Value::Array(bytes) => bytes
            .iter()
            .map(|byte| {
                byte.as_u64()
                    .filter(|byte| *byte <= 0xff)
                    .map(|byte| format!("{byte:02x}"))
                    .with_context(|| format!("{byte} is not a byte"))
            })
            .collect(),
        other => bail!("{other} is not a byte array"),
    }
}

// `spacetime sql --format json` returns statements, each with a schema and positional rows.
fn column_rows(value: &Value, columns: &[&str]) -> Result<Vec<Vec<Value>>> {
    let mut rows = Vec::new();
    for result in value.as_array().context("the result is not a list")? {
        let elements = result
            .pointer("/schema/elements")
            .and_then(Value::as_array)
            .context("the result has no schema")?;
        let indices = columns
            .iter()
            .map(|column| {
                elements
                    .iter()
                    .position(|element| {
                        element.pointer("/name/some").and_then(Value::as_str) == Some(*column)
                    })
                    .with_context(|| format!("the result has no column {column}"))
            })
            .collect::<Result<Vec<_>>>()?;
        for row in result
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let row = row.as_array().context("a result row is not a list")?;
            rows.push(
                indices
                    .iter()
                    .map(|&index| row.get(index).cloned().unwrap_or(Value::Null))
                    .collect(),
            );
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_json_rows_are_read_by_column_name() {
        let value = serde_json::json!([{
            "schema": {"elements": [
                {"name": {"some": "username"}, "algebraic_type": {}},
                {"name": {"some": "salt"}, "algebraic_type": {}}
            ]},
            "rows": [["MIGRANT", "0aff"], ["ALT", [10, 255]]]
        }]);
        let rows = column_rows(&value, &["salt", "username"]).unwrap();
        assert_eq!(rows[0][1], "MIGRANT");
        assert_eq!(hex(&rows[0][0]).unwrap(), "0aff");
        assert_eq!(hex(&rows[1][0]).unwrap(), "0aff");
    }
}
