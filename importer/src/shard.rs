use std::collections::BTreeMap;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

#[derive(Clone)]
pub(crate) struct Shard {
    pub(crate) server: String,
    pub(crate) db: String,
}

impl Shard {
    // Each spacetime CLI argument is Sats JSON; encode the whole payload once.
    pub(crate) fn call_reducer(&self, reducer: &str, payload: &str) -> Result<()> {
        let payload = serde_json::to_string(payload).context("encode reducer String argument")?;
        self.call_reducer_args(reducer, &[&payload])
    }

    pub(crate) fn call_reducer_args(&self, reducer: &str, values: &[&str]) -> Result<()> {
        let mut command = spacetime();
        command.args(["call", "-s", &self.server, &self.db, reducer]);
        command.args(values);
        let out = command
            .output()
            .with_context(|| format!("run spacetime call {reducer}"))?;
        if !out.status.success() {
            bail!("{reducer} failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        Ok(())
    }

    pub(crate) fn stamp_family(
        &self,
        family: &str,
        source_sha: &str,
        file_hash: &str,
        row_count: u64,
    ) -> Result<()> {
        let row_count_s = row_count.to_string();
        let out = spacetime()
            .args([
                "call",
                "-s",
                &self.server,
                &self.db,
                "stamp_import_meta",
                family,
                source_sha,
                file_hash,
                &row_count_s,
            ])
            .output()
            .with_context(|| format!("stamp_import_meta({family})"))?;
        if !out.status.success() {
            bail!(
                "stamp_import_meta({family}) failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    // Each SQL statement commits separately; a failed sequence requires an idempotent rerun.
    pub(crate) fn run_sql_statements(&self, stmts: &[String], label: &str) -> Result<()> {
        let stmts = in_published_order(stmts, &self.published_columns()?)
            .with_context(|| format!("{label}: no import statements applied"))?;
        for (n, s) in stmts.iter().enumerate() {
            eprintln!(
                "{label} sql [{}/{}]: {}…",
                n + 1,
                stmts.len(),
                s.char_indices()
                    .nth(60)
                    .map_or(s.as_str(), |(at, _)| &s[..at])
            );
            let out = spacetime()
                .args(["sql", "-s", &self.server, &self.db, s])
                .output()
                .context("run spacetime sql")?;
            if !out.status.success() {
                bail!(
                    "statement {} failed: {}",
                    n + 1,
                    String::from_utf8_lossy(&out.stderr)
                );
            }
        }
        Ok(())
    }

    fn published_columns(&self) -> Result<BTreeMap<String, Vec<String>>> {
        let out = spacetime()
            .args(["describe", "-s", &self.server, "--json", &self.db])
            .output()
            .context("run spacetime describe")?;
        if !out.status.success() {
            bail!(
                "spacetime describe failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        table_columns(&serde_json::from_slice(&out.stdout).context("read the Module definition")?)
    }

    pub(crate) fn query_json(&self, label: &str, query: &str) -> Result<String> {
        let out = spacetime()
            .args([
                "sql",
                "--format",
                "json",
                "-s",
                &self.server,
                &self.db,
                query,
            ])
            .output()
            .with_context(|| format!("run {label} SQL query: {query}"))?;
        if !out.status.success() {
            bail!(
                "{label} SQL query failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

fn in_published_order(
    stmts: &[String],
    columns: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>> {
    stmts
        .iter()
        .map(|sql| {
            lyracore_shared::import_sql::in_table_order(sql, |table| columns.get(table).cloned())
                .map_err(anyhow::Error::msg)
        })
        .collect()
}

// `spacetime describe --json` returns a V10 Module definition: sections keyed by kind.
fn table_columns(definition: &Value) -> Result<BTreeMap<String, Vec<String>>> {
    let section = |kind: &str| {
        definition["sections"]
            .as_array()
            .and_then(|sections| sections.iter().find_map(|section| section.get(kind)))
            .with_context(|| format!("the Module definition has no {kind} section"))
    };
    let types = section("Typespace")?["types"]
        .as_array()
        .context("the Module definition has no types")?;
    section("Tables")?
        .as_array()
        .context("the Module definition tables are not a list")?
        .iter()
        .map(|table| {
            let name = table["source_name"]
                .as_str()
                .context("a table has no name")?;
            let row = table["product_type_ref"]
                .as_u64()
                .and_then(|at| types.get(usize::try_from(at).ok()?))
                .with_context(|| format!("{name} has no row type"))?;
            let columns = row["Product"]["elements"]
                .as_array()
                .with_context(|| format!("{name} rows are not a product"))?
                .iter()
                .map(|element| {
                    element["name"]["some"]
                        .as_str()
                        .map(str::to_owned)
                        .with_context(|| format!("{name} has an unnamed column"))
                })
                .collect::<Result<_>>()?;
            Ok((name.to_owned(), columns))
        })
        .collect()
}

fn spacetime() -> Command {
    let mut command = Command::new("spacetime");
    if let Some(path) = std::env::var_os("LYRACORE_SPACETIME_CONFIG_PATH") {
        command.arg("--config-path").arg(path);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_malformed_insert_refuses_the_whole_batch() {
        let columns = BTreeMap::from([(
            "game_area".to_owned(),
            vec!["id".to_owned(), "name".to_owned()],
        )]);
        let result = in_published_order(
            &[
                "DELETE FROM game_area".into(),
                "INSERT INTO game_area (id,name) VALUES (1,'a',3)".into(),
            ],
            &columns,
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            "game_area: 3 values for 2 columns"
        );
    }

    #[test]
    fn table_columns_keep_the_published_row_order() {
        let column = |name: &str| json!({"name": {"some": name}, "algebraic_type": {"U32": []}});
        let definition = json!({"sections": [
            {"Typespace": {"types": [
                {"Product": {"elements": [column("id")]}},
                {"Product": {"elements": [
                    column("faction_id"),
                    column("base_standing"),
                    column("base_race_mask_1"),
                ]}},
            ]}},
            {"Tables": [{"source_name": "game_faction", "product_type_ref": 1}]},
        ]});
        assert_eq!(
            table_columns(&definition).unwrap(),
            BTreeMap::from([(
                "game_faction".to_owned(),
                vec![
                    "faction_id".to_owned(),
                    "base_standing".to_owned(),
                    "base_race_mask_1".to_owned(),
                ],
            )])
        );
    }
}
