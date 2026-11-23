//! SpacetimeDB SQL INSERT needs every column, and assigns values by column position, not by name.

/// Writes each named INSERT with every published column in published order; see
/// docs/danger-zones.md §1 item 2. Errs when an INSERT is unreadable, names a table or column the
/// Module lacks or a column twice, has a row of the wrong length, or leaves a published column with
/// no value and no default.
pub fn in_table_order(
    sql: &str,
    published_columns: impl Fn(&str) -> Option<Vec<String>>,
) -> Result<String, String> {
    let statements = statements(sql)
        .into_iter()
        .map(|statement| ordered(statement, &published_columns))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(statements.join(";"))
}

enum Cell<'a> {
    Named(usize),
    Default(&'a str),
}

fn ordered(
    statement: &str,
    published_columns: &impl Fn(&str) -> Option<Vec<String>>,
) -> Result<String, String> {
    let Some(insert) = NamedInsert::parse(statement)? else {
        return Ok(statement.to_owned());
    };
    let table = insert.table;
    let published = published_columns(table)
        .ok_or_else(|| format!("the published Module has no table `{table}`"))?;
    let named: Vec<String> = insert.columns.iter().map(|name| canonical(name)).collect();
    for (at, name) in insert.columns.iter().enumerate() {
        if !published.contains(&named[at]) {
            return Err(format!("{table} has no column `{name}`"));
        }
        if named[..at].contains(&named[at]) {
            return Err(format!("{table}.{name} is named twice"));
        }
    }
    let cells = published
        .iter()
        .map(
            |column| match named.iter().position(|name| name == column) {
                Some(at) => Ok(Cell::Named(at)),
                None => migration_defaults(table)
                    .find(|(name, _)| canonical(name) == *column)
                    .map(|(_, value)| Cell::Default(value))
                    .ok_or_else(|| format!("{table}.{column} has no value and no default")),
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    let rows = value_rows(insert.values)
        .ok_or_else(|| format!("{table}: malformed VALUES"))?
        .iter()
        .map(|row| {
            if row.len() != insert.columns.len() {
                return Err(format!(
                    "{table}: {} values for {} columns",
                    row.len(),
                    insert.columns.len()
                ));
            }
            let values: Vec<&str> = cells
                .iter()
                .map(|cell| match cell {
                    Cell::Named(at) => row[*at],
                    Cell::Default(value) => *value,
                })
                .collect();
            Ok(format!("({})", values.join(",")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!(
        "{}INSERT INTO {table} ({}) VALUES {}",
        insert.indent,
        published.join(","),
        rows.join(",")
    ))
}

struct NamedInsert<'a> {
    indent: &'a str,
    table: &'a str,
    columns: Vec<&'a str>,
    values: &'a str,
}

impl<'a> NamedInsert<'a> {
    /// `None` for a statement that is not an INSERT, or one that names no columns.
    fn parse(statement: &'a str) -> Result<Option<Self>, String> {
        let Some(rest) = keyword(statement, "INSERT") else {
            return Ok(None);
        };
        let unreadable = || {
            let start: String = statement.trim().chars().take(80).collect();
            format!("cannot read the INSERT `{start}`")
        };
        let rest = keyword(rest, "INTO").ok_or_else(unreadable)?.trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let (table, rest) = rest.split_at(end);
        if table.is_empty() {
            return Err(unreadable());
        }
        if keyword(rest, "VALUES").is_some() {
            return Ok(None);
        }
        let rest = rest.trim_start().strip_prefix('(').ok_or_else(unreadable)?;
        let (columns, rest) = rest.split_once(')').ok_or_else(unreadable)?;
        Ok(Some(Self {
            indent: &statement[..statement.len() - statement.trim_start().len()],
            table,
            columns: columns.split(',').map(str::trim).collect(),
            values: keyword(rest, "VALUES").ok_or_else(unreadable)?,
        }))
    }
}

// SpacetimeDB 2.14 publishes `convert_case` 0.6 `Case::Snake` column names (`value1` as `value_1`);
// SQL also accepts the source name. This ports its default boundaries for ASCII identifiers.
fn canonical(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut words: Vec<String> = vec![String::new()];
    for (at, &c) in chars.iter().enumerate() {
        if matches!(c, '_' | '-' | ' ') {
            words.push(String::new());
            continue;
        }
        if let Some(&before) = at.checked_sub(1).and_then(|before| chars.get(before)) {
            let after = chars.get(at + 1);
            let boundary = (before.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (before.is_ascii_uppercase() && c.is_ascii_digit())
                || (before.is_ascii_digit() && c.is_ascii_alphabetic())
                || (before.is_ascii_lowercase() && c.is_ascii_digit())
                || (before.is_ascii_uppercase()
                    && c.is_ascii_uppercase()
                    && after.is_some_and(char::is_ascii_lowercase));
            if boundary {
                words.push(String::new());
            }
        }
        words
            .last_mut()
            .expect("words starts non-empty")
            .push(c.to_ascii_lowercase());
    }
    words.retain(|word| !word.is_empty());
    words.join("_")
}

fn keyword<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let text = text.trim_start();
    let rest = text.get(word.len()..)?;
    let continues = rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
    (text[..word.len()].eq_ignore_ascii_case(word) && !continues).then_some(rest)
}

fn migration_defaults(table: &str) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    include_str!("import_sql_defaults.txt")
        .lines()
        .filter_map(move |line| {
            let mut fields = line.split_whitespace();
            let source_table = fields.next()?;
            let column = fields.next()?;
            let value = fields.next()?;
            (source_table == table).then_some((column, value))
        })
}

fn value_rows(text: &str) -> Option<Vec<Vec<&str>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut depth = 0u32;
    let mut quoted = false;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        if c == '\'' {
            if depth == 0 {
                return None;
            }
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match c {
            '(' | '[' => {
                if depth == 0 && c == '[' {
                    return None;
                }
                depth += 1;
                if depth == 1 {
                    start = at + 1;
                }
            }
            ')' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    row.push(text[start..at].trim());
                    rows.push(std::mem::take(&mut row));
                }
            }
            ',' if depth == 1 => {
                row.push(text[start..at].trim());
                start = at + 1;
            }
            _ if depth == 0 && c != ',' && !c.is_whitespace() => return None,
            _ => {}
        }
    }
    (depth == 0 && !quoted && !rows.is_empty()).then_some(rows)
}

fn statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (at, c) in sql.char_indices() {
        if c == '\'' {
            quoted = !quoted;
        }
        if c == ';' && !quoted {
            statements.push(&sql[start..at]);
            start = at + 1;
        }
    }
    statements.push(&sql[start..]);
    statements
}

#[must_use]
pub fn insert_defaults(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    for (index, statement) in statements(sql).into_iter().enumerate() {
        if index > 0 {
            output.push(';');
        }
        append_statement(&mut output, statement);
    }
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
    let defaults: Vec<_> = migration_defaults(table).collect();
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

    fn published(table: &str) -> Option<Vec<String>> {
        let columns: &[&str] = match table {
            "game_faction" => &[
                "faction_id",
                "reputation_index",
                "base_standing",
                "base_standing_2",
                "base_race_mask_1",
            ],
            "game_quest_template" => &[
                "entry",
                "required_skill",
                "required_skill_value",
                "required_min_rep_faction",
                "required_min_rep_value",
                "required_max_rep_faction",
                "required_max_rep_value",
            ],
            "game_area" => &["id", "name"],
            "game_gossip_condition" => &["condition_id", "value_1", "value_2"],
            _ => return None,
        };
        Some(columns.iter().map(|&column| column.to_owned()).collect())
    }

    #[test]
    fn named_columns_and_every_row_take_the_published_order() {
        let sql = "INSERT INTO game_faction (faction_id,reputation_index,base_standing,base_race_mask_1,base_standing_2) \
                   VALUES (5090901,62,-42000,2,1000),(7, 1 ,0,8,-5)";
        assert_eq!(
            in_table_order(sql, published).unwrap(),
            "INSERT INTO game_faction (faction_id,reputation_index,base_standing,base_standing_2,base_race_mask_1) \
             VALUES (5090901,62,-42000,1000,2),(7,1,0,-5,8)"
        );
    }

    #[test]
    fn unnamed_published_columns_take_their_default_and_unpublished_defaults_are_ignored() {
        let sql = "INSERT INTO game_quest_template (entry,required_skill) VALUES (1,129)";
        assert_eq!(
            in_table_order(sql, published).unwrap(),
            "INSERT INTO game_quest_template (entry,required_skill,required_skill_value,required_min_rep_faction,required_min_rep_value,required_max_rep_faction,required_max_rep_value) \
             VALUES (1,129,0,0,0,0,0)"
        );
    }

    #[test]
    fn quoted_values_and_other_statements_survive() {
        let sql = "DELETE FROM game_area WHERE id > 0;\n INSERT INTO game_area (name,id) VALUES ('O''Brien, (east);',3);SELECT ';' FROM game_area";
        assert_eq!(
            in_table_order(sql, published).unwrap(),
            "DELETE FROM game_area WHERE id > 0;\n INSERT INTO game_area (id,name) VALUES (3,'O''Brien, (east);');SELECT ';' FROM game_area"
        );
    }

    #[test]
    fn source_column_names_match_their_published_snake_case_names() {
        let sql = "INSERT INTO game_gossip_condition (value2,condition_id,value1) VALUES (2,7,1)";
        assert_eq!(
            in_table_order(sql, published).unwrap(),
            "INSERT INTO game_gossip_condition (condition_id,value_1,value_2) VALUES (7,1,2)"
        );
    }

    #[test]
    fn canonical_names_split_where_convert_case_snake_splits() {
        for (source, published) in [
            ("value1", "value_1"),
            ("p0_kind", "p_0_kind"),
            ("data10", "data_10"),
            ("x2y", "x_2_y"),
            ("base_standing_2", "base_standing_2"),
            ("value_1", "value_1"),
            ("castTimeIndex", "cast_time_index"),
            ("HTTPServer", "http_server"),
        ] {
            assert_eq!(canonical(source), published, "{source}");
        }
    }

    #[test]
    fn positional_inserts_pass_and_keywords_match_in_any_case() {
        let sql = "INSERT INTO game_area VALUES (1,'a');insert\n into game_area\n(name, id)\nvalues ('b',2)";
        assert_eq!(
            in_table_order(sql, published).unwrap(),
            "INSERT INTO game_area VALUES (1,'a');INSERT INTO game_area (id,name) VALUES (2,'b')"
        );
    }

    #[test]
    fn an_insert_the_published_module_cannot_hold_is_refused() {
        for (sql, error) in [
            (
                "INSERT INTO game_area (id,nmae) VALUES (1,'a')",
                "game_area has no column `nmae`",
            ),
            (
                "INSERT INTO game_area (id,id) VALUES (1,2)",
                "game_area.id is named twice",
            ),
            (
                "INSERT INTO game_area (id,name) VALUES (1,'a'),(2,'b',3)",
                "game_area: 3 values for 2 columns",
            ),
            (
                "INSERT INTO game_missing (id) VALUES (1)",
                "the published Module has no table `game_missing`",
            ),
            (
                "INSERT INTO game_area (id) VALUES (1)",
                "game_area.name has no value and no default",
            ),
            (
                "INSERT game_area (id) VALUES (1)",
                "cannot read the INSERT `INSERT game_area (id) VALUES (1)`",
            ),
        ] {
            assert_eq!(in_table_order(sql, published).unwrap_err(), error);
        }
    }

    #[test]
    fn every_row_of_a_batch_gets_the_defaults_and_quoted_delimiters_survive() {
        let sql = "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'a); ''b'),(2,'c');\nSELECT * FROM game_spell";
        let expected = "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'a); ''b',0),(2,'c',0);\nSELECT * FROM game_spell";
        assert_eq!(insert_defaults(sql), expected);
        assert_eq!(insert_defaults(expected), expected);
    }

    #[test]
    fn unrelated_statements_are_unchanged() {
        for sql in [
            "DELETE FROM game_spell",
            "INSERT INTO game_spell_chain (spell_id) VALUES (1)",
            "SELECT ';' FROM game_spell",
        ] {
            assert_eq!(insert_defaults(sql), sql);
        }
    }

    #[test]
    fn older_broadcast_rows_are_silent_unless_they_supply_a_sound() {
        let silent =
            "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'Greeting')";
        assert_eq!(insert_defaults(silent), "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'Greeting',0)");
        let audible = "INSERT INTO game_creature_ai_broadcast_text (id,sound_id) VALUES (1,42)";
        assert_eq!(insert_defaults(audible), audible);
    }

    #[test]
    fn legacy_creature_catalogues_get_silent_sound_and_male_gender_defaults() {
        let sql = "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'Greeting');INSERT INTO game_creature_display (id,display_id) VALUES (2,100)";
        assert_eq!(insert_defaults(sql), "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'Greeting',0);INSERT INTO game_creature_display (id,display_id,gender,bounding_radius) VALUES (2,100,0,0.389)");
    }
}
