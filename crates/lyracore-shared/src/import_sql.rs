//! SQL INSERT needs explicit values even for columns with migration defaults.

#[must_use]
pub fn spell_defaults(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut quoted = false;
    let mut start = 0;
    for (at, c) in sql.char_indices() {
        if c == '\'' {
            quoted = !quoted;
        }
        if c == ';' && !quoted {
            append_statement(&mut output, &sql[start..at]);
            output.push(';');
            start = at + 1;
        }
    }
    append_statement(&mut output, &sql[start..]);
    output
}

fn append_statement(output: &mut String, sql: &str) {
    let trimmed = sql.trim_start();
    let Some(rest) = trimmed.strip_prefix("INSERT INTO ") else {
        output.push_str(sql);
        return;
    };
    let Some(open) = rest.find('(') else {
        output.push_str(sql);
        return;
    };
    let table = rest[..open].trim();
    let defaults: Vec<_> = include_str!("import_sql_defaults.txt")
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let source_table = fields.next()?;
            let column = fields.next()?;
            let value = fields.next()?;
            (source_table == table).then_some((column, value))
        })
        .collect();
    let Some(close) = rest.find(')') else {
        output.push_str(sql);
        return;
    };
    let columns: Vec<_> = rest[open + 1..close].split(',').map(str::trim).collect();
    let missing: Vec<_> = defaults
        .iter()
        .filter(|(name, _)| !columns.contains(name))
        .collect();
    if missing.is_empty() || !rest[close + 1..].trim_start().starts_with("VALUES") {
        output.push_str(sql);
        return;
    }
    let close = sql.len() - rest.len() + close;
    output.push_str(&sql[..close]);
    for (name, _) in &missing {
        output.push(',');
        output.push_str(name);
    }
    output.push(')');
    let mut quoted = false;
    let mut depth = 0u32;
    for c in sql[close + 1..].chars() {
        if c == '\'' {
            quoted = !quoted;
        }
        if !quoted {
            if c == '(' {
                depth += 1;
            }
            if c == ')' {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    for (_, value) in &missing {
                        output.push(',');
                        output.push_str(value);
                    }
                }
            }
        }
        output.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_batches_preserve_quoted_delimiters_and_explicit_new_values() {
        let sql = "INSERT INTO game_spell (spell_id,name,custom_flags) VALUES (1,'a); ''b',8),(2,'c',16);\nINSERT INTO game_spell_effect (id,kind) VALUES (4,2);SELECT * FROM game_spell";
        assert_eq!(spell_defaults(sql), "INSERT INTO game_spell (spell_id,name,custom_flags,min_target_level,single_target_key) VALUES (1,'a); ''b',8,0,0),(2,'c',16,0,0);\nINSERT INTO game_spell_effect (id,kind,bonus_coefficient) VALUES (4,2,-1.0);SELECT * FROM game_spell");
        assert_eq!(spell_defaults(&spell_defaults(sql)), spell_defaults(sql));
    }

    #[test]
    fn unrelated_statements_are_unchanged() {
        for sql in [
            "DELETE FROM game_spell",
            "INSERT INTO game_spell_chain (spell_id) VALUES (1)",
            "SELECT ';' FROM game_spell",
        ] {
            assert_eq!(spell_defaults(sql), sql);
        }
    }
}
