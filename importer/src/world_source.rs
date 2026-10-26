use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};

use crate::dump::TableRows;

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
    Null,
    Number(Box<str>),
    Text(Box<str>),
}

impl TryFrom<Value> for Cell {
    type Error = anyhow::Error;

    fn try_from(value: Value) -> Result<Self> {
        Ok(match value {
            Value::Null => Self::Null,
            Value::Integer(value) => Self::Number(value.to_string().into()),
            Value::Real(value) if value.is_finite() => Self::Number(value.to_string().into()),
            Value::Text(value) => Self::Text(value.into()),
            _ => bail!("unsupported World Source value type"),
        })
    }
}

#[derive(Debug)]
struct TableStorage {
    table: String,
    indices: HashMap<String, usize>,
    text: String,
}

#[derive(Clone, Debug)]
enum StoredCell {
    Null,
    Zero,
    Number(std::ops::Range<u32>),
    Text(std::ops::Range<u32>),
}

pub(crate) struct TableBuilder {
    columns: TableStorage,
    rows: Vec<Box<[StoredCell]>>,
}

impl TableBuilder {
    pub(crate) fn new(table: &str, names: impl IntoIterator<Item = String>) -> Self {
        Self {
            columns: TableStorage {
                table: table.into(),
                indices: names
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| (name, index))
                    .collect(),
                text: String::new(),
            },
            rows: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, cells: Vec<Cell>) -> Result<()> {
        if cells.len() != self.columns.indices.len() {
            bail!("{}: row does not match its column list", self.columns.table);
        }
        let cells = cells
            .into_iter()
            .map(|cell| {
                let (number, value) = match cell {
                    Cell::Null => return Ok(StoredCell::Null),
                    Cell::Number(value) => (true, value),
                    Cell::Text(value) => (false, value),
                };
                let start = u32::try_from(self.columns.text.len())?;
                self.columns.text.push_str(&value);
                let end = u32::try_from(self.columns.text.len())
                    .context("source table text exceeds 4 GiB")?;
                Ok(if number {
                    StoredCell::Number(start..end)
                } else {
                    StoredCell::Text(start..end)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.rows.push(cells.into_boxed_slice());
        Ok(())
    }

    pub(crate) fn finish(self) -> Vec<Row> {
        let columns = Arc::new(self.columns);
        self.rows
            .into_iter()
            .map(|cells| Row {
                columns: Arc::clone(&columns),
                cells,
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    columns: Arc<TableStorage>,
    cells: Box<[StoredCell]>,
}

impl Row {
    fn cell(&self, column: &str) -> Result<&StoredCell> {
        self.columns
            .indices
            .get(column)
            .and_then(|&index| self.cells.get(index))
            .with_context(|| format!("{}.{}: missing column", self.columns.table, column))
    }

    fn value(&self, range: &std::ops::Range<u32>) -> &str {
        &self.columns.text[range.start as usize..range.end as usize]
    }

    pub(crate) fn number<T: std::str::FromStr>(&self, column: &str) -> Result<T> {
        self.numeric_text(column)?.parse().map_err(|_| {
            anyhow::anyhow!(
                "{}.{}: invalid {}",
                self.columns.table,
                column,
                std::any::type_name::<T>()
            )
        })
    }

    pub(crate) fn numeric_text(&self, column: &str) -> Result<&str> {
        match self.cell(column)? {
            StoredCell::Number(value) => Ok(self.value(value).trim()),
            StoredCell::Zero => Ok("0"),
            _ => bail!("{}.{}: expected a number", self.columns.table, column),
        }
    }

    pub(crate) fn text(&self, column: &str) -> Result<&str> {
        match self.cell(column)? {
            StoredCell::Text(value) => Ok(self.value(value)),
            _ => bail!("{}.{}: expected text", self.columns.table, column),
        }
    }

    pub(crate) fn optional_text(&self, column: &str) -> Result<Option<&str>> {
        match self.cell(column)? {
            StoredCell::Null => Ok(None),
            StoredCell::Text(value) => Ok(Some(self.value(value))),
            _ => bail!("{}.{}: expected text or NULL", self.columns.table, column),
        }
    }

    pub(crate) fn raw(&self, column: &str) -> Result<&str> {
        Ok(match self.cell(column)? {
            StoredCell::Null => "NULL",
            StoredCell::Zero => "0",
            StoredCell::Number(value) | StoredCell::Text(value) => self.value(value).trim(),
        })
    }
}

pub(crate) enum WorldSource {
    ClassicDb(crate::dump::ParsedDump),
    Vmangos(Connection),
}

impl WorldSource {
    pub(crate) fn open(path: &Path, kind: SourceKind) -> Result<Self> {
        match kind {
            SourceKind::ClassicDb => Ok(Self::ClassicDb(crate::dump::read_dump(
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
            Self::ClassicDb(dump) => Ok(dump.rows(table)?.len()),
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
}

impl TableRows for WorldSource {
    fn kind(&self) -> SourceKind {
        match self {
            Self::ClassicDb(_) => SourceKind::ClassicDb,
            Self::Vmangos(_) => SourceKind::Vmangos,
        }
    }

    fn rows(&self, table: &str) -> Result<Cow<'_, [Row]>> {
        match self {
            Self::ClassicDb(dump) => dump.rows(table),
            Self::Vmangos(connection) => read_rows(connection, table, None).map(Cow::Owned),
        }
    }
    fn columns(&self, table: &str, columns: &[&str]) -> Result<Cow<'_, [Row]>> {
        match self {
            Self::ClassicDb(dump) => dump.rows(table),
            Self::Vmangos(connection) => {
                read_rows(connection, table, Some(columns)).map(Cow::Owned)
            }
        }
    }
    fn source_hash(&self) -> Result<String> {
        match self {
            Self::ClassicDb(dump) => dump.source_hash(),
            Self::Vmangos(connection) => {
                let path = connection
                    .path()
                    .filter(|path| !path.is_empty())
                    .context("World Source has no file to hash")?;
                crate::dump::sha256_hex(&std::fs::read(path)?)
            }
        }
    }
}

fn read_rows(connection: &Connection, table: &str, requested: Option<&[&str]>) -> Result<Vec<Row>> {
    check_table(connection, table)?;
    let mut metadata =
        connection.prepare("SELECT name, pk FROM pragma_table_info(?1) ORDER BY pk")?;
    let metadata = metadata
        .query_map([table], |row| {
            Ok((row.get::<_, String>("name")?, row.get::<_, u32>("pk")?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let primary_key = metadata
        .iter()
        .filter(|(_, ordinal)| *ordinal > 0)
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let available = metadata
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    let columns = if let Some(requested) = requested {
        let mut columns = requested
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>();
        columns.extend(primary_key.iter().cloned());
        columns.extend(
            crate::source_selection::required_columns(table)
                .iter()
                .map(|name| (*name).to_owned()),
        );
        for name in [
            "patch",
            "build",
            "patch_min",
            "patch_max",
            "build_min",
            "build_max",
            "condition_id",
        ] {
            if available.contains(name) {
                columns.insert(name.to_owned());
            }
        }
        for name in &columns {
            anyhow::ensure!(available.contains(name), "{table}.{name}: missing column");
        }
        columns
    } else {
        available
    };
    let projection = columns
        .iter()
        .map(|name| quote_identifier(name))
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = connection.prepare(&format!(
        "SELECT {projection} FROM {}",
        quote_identifier(table)
    ))?;
    let mut output = TableBuilder::new(table, columns.iter().cloned());
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let cells = columns
            .iter()
            .map(|name| Cell::try_from(row.get::<_, Value>(name.as_str())?))
            .collect::<Result<Vec<_>>>()?;
        output.push(cells)?;
    }
    let selected = crate::source_selection::select(table, &columns, &primary_key, output.finish())?;
    if !crate::source_selection::condition_is_static(table) || selected.is_empty() {
        return Ok(selected);
    }
    let ids = selected
        .iter()
        .map(|row| row.number::<u32>("condition_id"))
        .collect::<Result<Vec<_>>>()?;
    if ids.iter().all(|&id| id == 0) {
        return Ok(selected);
    }
    let conditions =
        crate::source_selection::static_conditions(&read_rows(connection, "conditions", None)?)?;
    Ok(selected
        .into_iter()
        .zip(ids)
        .filter_map(
            |(mut row, id)| match conditions.get(&id).copied().flatten() {
                Some(false) => None,
                Some(true) => {
                    row.cells[row.columns.indices["condition_id"]] = StoredCell::Zero;
                    Some(row)
                }
                None => Some(row),
            },
        )
        .collect())
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
        let classic = crate::dump::named_table("CREATE TABLE `example` (\n`optional` int,\n`entry` int,\n`name` text\n); INSERT INTO `example` VALUES (NULL,42,'O\\'Brien');", "example").unwrap();
        let sqlite_rows = sqlite.rows("example").unwrap();
        for rows in [sqlite_rows.as_ref(), classic.as_slice()] {
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
        let rows = source.rows("item_template").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text("name").unwrap(), "current");
    }

    #[test]
    fn projection_keeps_selection_keys_and_refuses_missing_columns() {
        let source = sqlite_fixture(&[FixtureTable {
            name: "item_template",
            columns: &[("entry", "INTEGER"), ("patch", "INTEGER"), ("name", "TEXT")],
            key: &["entry", "patch"],
            rows: &[
                &[
                    ("entry", Value::Integer(4)),
                    ("patch", Value::Integer(9)),
                    ("name", Value::Text("old".into())),
                ],
                &[
                    ("entry", Value::Integer(4)),
                    ("patch", Value::Integer(10)),
                    ("name", Value::Text("selected".into())),
                ],
            ],
        }])
        .unwrap();
        let rows = source.columns("item_template", &["name"]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text("name").unwrap(), "selected");
        assert_eq!(rows[0].number::<u32>("entry").unwrap(), 4);
        assert!(source.columns("item_template", &["missing"]).is_err());
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
            source.rows("example").unwrap()[0]
                .number::<u32>("entry")
                .unwrap(),
            42
        );
        assert!(source.rows("indirect").is_err());
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
        assert!(source.rows("example").unwrap()[0]
            .number::<u32>("number")
            .is_err());
        let source = sqlite_fixture(&[FixtureTable {
            name: "example",
            columns: &[("number", "INTEGER")],
            key: &[],
            rows: &[&[("number", Value::Integer(256))]],
        }])
        .unwrap();
        assert!(source.rows("example").unwrap()[0]
            .number::<u8>("number")
            .is_err());
        assert!(Cell::try_from(Value::Blob(vec![1])).is_err());
    }
}
