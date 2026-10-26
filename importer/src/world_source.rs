use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SourceKind {
    #[default]
    ClassicDb,
    Vmangos,
}

impl std::str::FromStr for SourceKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "classic-db" => Ok(Self::ClassicDb),
            "vmangos" => Ok(Self::Vmangos),
            _ => bail!("unknown World Source {value:?}; expected classic-db or vmangos"),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Cell {
    #[cfg(test)]
    AbsentFixtureColumn,
    Null,
    Number(String),
    Text(String),
}

impl TryFrom<Value> for Cell {
    type Error = anyhow::Error;

    fn try_from(value: Value) -> Result<Self> {
        Ok(match value {
            Value::Null => Self::Null,
            Value::Integer(value) => Self::Number(value.to_string()),
            Value::Real(value) if value.is_finite() => Self::Number(value.to_string()),
            Value::Text(value) => Self::Text(value),
            _ => bail!("unsupported World Source value type"),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    table: String,
    cells: BTreeMap<String, Cell>,
}

impl Row {
    pub(crate) fn new(table: &str, cells: BTreeMap<String, Cell>) -> Self {
        Self {
            table: table.into(),
            cells,
        }
    }

    pub(crate) fn column_count(&self) -> usize {
        self.cells.keys().filter(|name| self.has(name)).count()
    }

    pub(crate) fn has(&self, column: &str) -> bool {
        #[cfg(test)]
        if matches!(self.cells.get(column), Some(Cell::AbsentFixtureColumn)) {
            return false;
        }
        self.cells.contains_key(column)
    }

    fn cell(&self, column: &str) -> Result<&Cell> {
        self.cells
            .get(column)
            .with_context(|| format!("{}.{}: missing column", self.table, column))
    }

    pub(crate) fn require_type(&self, column: &str, number: bool) -> Result<()> {
        match self.cell(column)? {
            Cell::Null => Ok(()),
            Cell::Number(_) if number => Ok(()),
            Cell::Text(_) if !number => Ok(()),
            _ => bail!("{}.{}: source column type changed", self.table, column),
        }
    }

    pub(crate) fn number<T: std::str::FromStr>(&self, column: &str) -> Result<T> {
        match self.cell(column)? {
            Cell::Number(value) => value.trim().parse().map_err(|_| {
                anyhow::anyhow!(
                    "{}.{}: invalid {}",
                    self.table,
                    column,
                    std::any::type_name::<T>()
                )
            }),
            _ => bail!("{}.{}: expected a number", self.table, column),
        }
    }

    pub(crate) fn text(&self, column: &str) -> Result<&str> {
        match self.cell(column)? {
            Cell::Text(value) => Ok(value),
            _ => bail!("{}.{}: expected text", self.table, column),
        }
    }

    pub(crate) fn raw(&self, column: &str) -> Result<&str> {
        Ok(match self.cell(column)? {
            #[cfg(test)]
            Cell::AbsentFixtureColumn => "",
            Cell::Null => "NULL",
            Cell::Number(value) | Cell::Text(value) => value.trim(),
        })
    }
}

pub(crate) enum WorldSource {
    ClassicDb(String),
    Vmangos(Connection),
}

impl WorldSource {
    pub(crate) fn open(path: &Path, kind: SourceKind) -> Result<Self> {
        match kind {
            SourceKind::ClassicDb => Ok(Self::ClassicDb(crate::dump::read_dump_text(
                path.to_str().context("World Source path is not UTF-8")?,
            )?)),
            SourceKind::Vmangos => {
                let connection =
                    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .with_context(|| format!("open World Source {}", path.display()))?;
                connection.pragma_update(None, "trusted_schema", false)?;
                connection.pragma_update(None, "query_only", true)?;
                Ok(Self::Vmangos(connection))
            }
        }
    }

    pub(crate) fn row_count(&self, table: &str) -> Result<usize> {
        match self {
            Self::ClassicDb(sql) => Ok(crate::dump::named_table(sql, table)?.len()),
            Self::Vmangos(connection) => {
                check_table(connection, table)?;
                connection
                    .query_row(
                        &format!("SELECT count(*) AS total FROM {}", quote_identifier(table)),
                        [],
                        |row| row.get::<_, i64>("total"),
                    )
                    .map_err(anyhow::Error::from)
                    .and_then(|count| usize::try_from(count).map_err(Into::into))
            }
        }
    }

    pub(crate) fn table(&self, table: &str) -> Result<Vec<Row>> {
        match self {
            Self::ClassicDb(sql) => crate::dump::named_table(sql, table),
            Self::Vmangos(connection) => {
                check_table(connection, table)?;
                let mut statement =
                    connection.prepare(&format!("SELECT * FROM {}", quote_identifier(table)))?;
                let columns: Vec<String> = statement.column_names().map(str::to_owned).collect();
                let mut rows = statement.query([])?;
                let mut output = Vec::new();
                while let Some(row) = rows.next()? {
                    let cells = columns
                        .iter()
                        .map(|name| {
                            Ok((
                                name.clone(),
                                Cell::try_from(row.get::<_, Value>(name.as_str())?)?,
                            ))
                        })
                        .collect::<Result<BTreeMap<_, _>>>()?;
                    output.push(Row::new(table, cells));
                }
                let mut metadata =
                    connection.prepare("SELECT name, pk FROM pragma_table_info(?1) ORDER BY pk")?;
                let primary_key = metadata
                    .query_map([table], |row| {
                        Ok((row.get::<_, String>("name")?, row.get::<_, u32>("pk")?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                    .into_iter()
                    .filter_map(|(name, ordinal)| (ordinal > 0).then_some(name))
                    .collect::<Vec<_>>();
                let columns = columns.into_iter().collect::<BTreeSet<_>>();
                crate::source_selection::select(table, &columns, &primary_key, output)
            }
        }
    }
}

fn check_table(connection: &Connection, table: &str) -> Result<()> {
    let kind: String = connection
        .query_row(
            "SELECT type FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
            [table],
            |row| row.get("type"),
        )
        .with_context(|| format!("missing source table {table}"))?;
    if kind != "table" {
        bail!("{table}: expected a source table, got {kind}");
    }
    Ok(())
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
pub(crate) struct FixtureTable<'a> {
    pub name: &'a str,
    pub columns: &'a [(&'a str, &'a str)],
    pub key: &'a [&'a str],
    pub rows: &'a [&'a [(&'a str, Value)]],
}

#[cfg(test)]
pub(crate) fn sqlite_fixture(tables: &[FixtureTable<'_>]) -> Result<WorldSource> {
    let connection = Connection::open_in_memory()?;
    for table in tables {
        let mut definitions = table
            .columns
            .iter()
            .map(|(name, kind)| format!("{} {kind}", quote_identifier(name)))
            .collect::<Vec<_>>();
        if !table.key.is_empty() {
            definitions.push(format!(
                "PRIMARY KEY ({})",
                table
                    .key
                    .iter()
                    .map(|name| quote_identifier(name))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        connection.execute_batch(&format!(
            "CREATE TABLE {} ({})",
            quote_identifier(table.name),
            definitions.join(",")
        ))?;
        for row in table.rows {
            let names = row
                .iter()
                .map(|(name, _)| quote_identifier(name))
                .collect::<Vec<_>>()
                .join(",");
            let parameters = vec!["?"; row.len()].join(",");
            connection.execute(
                &format!(
                    "INSERT INTO {} ({names}) VALUES ({parameters})",
                    quote_identifier(table.name)
                ),
                rusqlite::params_from_iter(row.iter().map(|(_, value)| value)),
            )?;
        }
    }
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.pragma_update(None, "query_only", true)?;
    Ok(WorldSource::Vmangos(connection))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sources_read_reordered_columns_and_check_requested_types() {
        let sqlite = sqlite_fixture(&[FixtureTable {
            name: "example",
            columns: &[
                ("name", "TEXT"),
                ("entry", "INTEGER"),
                ("optional", "INTEGER"),
            ],
            key: &[],
            rows: &[&[
                ("entry", Value::Integer(42)),
                ("name", Value::Text("O'Brien".into())),
            ]],
        }])
        .unwrap();
        let classic = WorldSource::ClassicDb("CREATE TABLE `example` (\n`optional` int,\n`entry` int,\n`name` text\n); INSERT INTO `example` VALUES (NULL,42,'O\\'Brien');".into());
        for source in [sqlite, classic] {
            let rows = source.table("example").unwrap();
            let row = &rows[0];
            assert_eq!(row.number::<u32>("entry").unwrap(), 42);
            assert_eq!(row.text("name").unwrap(), "O'Brien");
            assert!(row.number::<u32>("name").is_err());
            assert!(row.text("entry").is_err());
            assert!(row.number::<u32>("optional").is_err());
            assert!(row
                .number::<u32>("missing")
                .unwrap_err()
                .to_string()
                .contains("example.missing"));
            assert!(source.table("absent").is_err());
        }
    }

    #[test]
    fn sqlite_selection_uses_named_primary_key_columns() {
        let source = sqlite_fixture(&[FixtureTable {
            name: "item_template",
            columns: &[("name", "TEXT"), ("patch", "INTEGER"), ("entry", "INTEGER")],
            key: &["entry", "patch"],
            rows: &[
                &[
                    ("entry", Value::Integer(4)),
                    ("patch", Value::Integer(9)),
                    ("name", Value::Text("old".into())),
                ],
                &[
                    ("name", Value::Text("current".into())),
                    ("entry", Value::Integer(4)),
                    ("patch", Value::Integer(10)),
                ],
            ],
        }])
        .unwrap();
        let rows = source.table("item_template").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text("name").unwrap(), "current");
    }

    #[test]
    fn sqlite_file_is_read_only_and_does_not_create_a_missing_file() {
        let path = std::env::temp_dir().join(format!("world-source-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(WorldSource::open(&path, SourceKind::Vmangos).is_err());
        assert!(!path.exists());
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE example (entry INTEGER); INSERT INTO example VALUES (42); CREATE VIEW indirect AS SELECT * FROM example;").unwrap();
        drop(connection);
        let source = WorldSource::open(&path, SourceKind::Vmangos).unwrap();
        assert_eq!(
            source.table("example").unwrap()[0]
                .number::<u32>("entry")
                .unwrap(),
            42
        );
        assert!(source.table("indirect").is_err());
        if let WorldSource::Vmangos(connection) = source {
            assert!(connection.execute("DELETE FROM example", []).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn numeric_access_refuses_overflow_fractional_integers_and_blob_cells() {
        let source = sqlite_fixture(&[FixtureTable {
            name: "example",
            columns: &[("number", "REAL")],
            key: &[],
            rows: &[&[("number", Value::Real(1.5))]],
        }])
        .unwrap();
        assert!(source.table("example").unwrap()[0]
            .number::<u32>("number")
            .is_err());
        let source = sqlite_fixture(&[FixtureTable {
            name: "example",
            columns: &[("number", "INTEGER")],
            key: &[],
            rows: &[&[("number", Value::Integer(256))]],
        }])
        .unwrap();
        assert!(source.table("example").unwrap()[0]
            .number::<u8>("number")
            .is_err());
        assert!(Cell::try_from(Value::Blob(vec![1])).is_err());
    }
}
