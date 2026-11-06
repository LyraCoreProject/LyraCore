use std::process::Command;

use anyhow::{bail, Context, Result};

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
        let stmts: Vec<_> = stmts
            .iter()
            .map(|sql| lyracore_shared::import_sql::insert_defaults(sql))
            .collect();
        for statement in &stmts {
            validate_insert_arity(statement)?;
        }
        for (n, s) in stmts.iter().enumerate() {
            eprintln!(
                "{label} sql [{}/{}]: {}…",
                n + 1,
                stmts.len(),
                &s[..s.len().min(60)]
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

fn validate_insert_arity(sql: &str) -> Result<()> {
    let Some(insert) = sql.trim_start().strip_prefix("INSERT INTO ") else {
        return Ok(());
    };
    let Some((columns, values)) = insert.split_once(" VALUES ") else {
        return Ok(());
    };
    let Some((table, columns)) = columns.split_once('(') else {
        return Ok(());
    };
    let expected = columns.split(',').count();
    let mut quoted = false;
    let mut depth = 0u32;
    let mut actual = 1;
    for c in values.chars() {
        if c == '\'' {
            quoted = !quoted;
        }
        if quoted {
            continue;
        }
        match c {
            '(' | '[' => {
                depth += 1;
                if depth == 1 {
                    actual = 1;
                }
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && actual != expected {
                    bail!(
                        "{}: {actual} values for {expected} columns; no import statements applied",
                        table.trim()
                    );
                }
            }
            ',' if depth == 1 => actual += 1,
            _ => {}
        }
    }
    Ok(())
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

    #[test]
    fn malformed_insert_is_refused_before_running_import_statements() {
        let shard = Shard {
            server: "unused".into(),
            db: "unused".into(),
        };
        let result = shard.run_sql_statements(
            &[
                "DELETE FROM game_spell_effect".into(),
                "INSERT INTO game_spell_effect (id,kind) VALUES (1,2,3)".into(),
            ],
            "spell",
        );
        assert!(result.unwrap_err().to_string().contains("values for"));
    }

    #[test]
    fn insert_arity_preserves_quoted_commas_and_nested_values() {
        validate_insert_arity(
            "INSERT INTO t (id,name,flags) VALUES (1,'a, ''b);',[1,2]),(2,'c',3)",
        )
        .unwrap();
        assert!(validate_insert_arity("INSERT INTO t (id,name) VALUES (1,'a'),(2,'b',3)").is_err());
    }
}
