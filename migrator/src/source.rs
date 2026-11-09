use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceKind {
    Vmangos,
    Cmangos,
}

impl std::str::FromStr for SourceKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "vmangos" => Ok(Self::Vmangos),
            "cmangos" => Ok(Self::Cmangos),
            other => bail!("unknown Migration Source kind {other:?}; use vmangos or cmangos"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColumnType {
    Integer,
    Float,
    Text,
}

impl ColumnType {
    pub(crate) fn from_mysql(data_type: &str) -> Option<Self> {
        match data_type {
            "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => Some(Self::Integer),
            "float" | "double" | "decimal" => Some(Self::Float),
            "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" => {
                Some(Self::Text)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Row(pub(crate) HashMap<String, Value>);

impl Row {
    fn value(&self, column: &str) -> Result<&Value> {
        self.0
            .get(column)
            .ok_or_else(|| anyhow!("source row has no column {column}"))
    }

    pub(crate) fn int(&self, column: &str) -> Result<i64> {
        match self.value(column)? {
            Value::Int(value) => Ok(*value),
            Value::Null => Ok(0),
            other => bail!("column {column} holds {other:?}, not an integer"),
        }
    }

    pub(crate) fn number<T: TryFrom<i64>>(&self, column: &str) -> Result<T> {
        let value = self.int(column)?;
        T::try_from(value).map_err(|_| anyhow!("column {column} value {value} is out of range"))
    }

    pub(crate) fn float(&self, column: &str) -> Result<f64> {
        match self.value(column)? {
            Value::Float(value) => Ok(*value),
            Value::Int(value) => Ok(*value as f64),
            Value::Null => Ok(0.0),
            other => bail!("column {column} holds {other:?}, not a number"),
        }
    }

    pub(crate) fn text(&self, column: &str) -> Result<String> {
        match self.value(column)? {
            Value::Text(value) => Ok(value.clone()),
            Value::Null => Ok(String::new()),
            other => bail!("column {column} holds {other:?}, not text"),
        }
    }

    // MaNGOS stores descriptor arrays such as explored zones as space-separated decimal words.
    pub(crate) fn words(&self, column: &str) -> Result<Vec<u32>> {
        self.text(column)?
            .split_ascii_whitespace()
            .map(|word| {
                word.parse::<u32>()
                    .with_context(|| format!("column {column} holds the non-number word {word:?}"))
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Equals(&'static str, u64),
}

// The external edge of a Migration Source: one read-only database snapshot.
pub(crate) trait SourceDatabase {
    fn column_types(&mut self, table: &str) -> Result<HashMap<String, ColumnType>>;
    fn select(&mut self, table: &str, columns: &[&str], filter: Filter) -> Result<Vec<Row>>;
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    #[derive(Default)]
    pub(crate) struct FakeDatabase {
        tables: HashMap<String, (HashMap<String, ColumnType>, Vec<Row>)>,
    }

    impl FakeDatabase {
        pub(crate) fn table(&mut self, table: &str, columns: &[(&str, ColumnType)]) {
            self.tables.insert(
                table.to_string(),
                (
                    columns
                        .iter()
                        .map(|(name, kind)| ((*name).to_string(), *kind))
                        .collect(),
                    Vec::new(),
                ),
            );
        }

        pub(crate) fn row(&mut self, table: &str, values: &[(&str, Value)]) {
            let (columns, rows) = self
                .tables
                .get_mut(table)
                .unwrap_or_else(|| panic!("fake table {table} is not declared"));
            let mut row = HashMap::new();
            for (name, kind) in columns.iter() {
                let default = match kind {
                    ColumnType::Integer => Value::Int(0),
                    ColumnType::Float => Value::Float(0.0),
                    ColumnType::Text => Value::Text(String::new()),
                };
                row.insert(name.clone(), default);
            }
            for (name, value) in values {
                assert!(columns.contains_key(*name), "{table} has no column {name}");
                row.insert((*name).to_string(), value.clone());
            }
            rows.push(Row(row));
        }
    }

    impl SourceDatabase for FakeDatabase {
        fn column_types(&mut self, table: &str) -> Result<HashMap<String, ColumnType>> {
            Ok(self
                .tables
                .get(table)
                .map(|(columns, _)| columns.clone())
                .unwrap_or_default())
        }

        fn select(&mut self, table: &str, columns: &[&str], filter: Filter) -> Result<Vec<Row>> {
            let (_, rows) = self
                .tables
                .get(table)
                .ok_or_else(|| anyhow!("table {table} does not exist"))?;
            Ok(rows
                .iter()
                .filter(|row| match filter {
                    Filter::All => true,
                    Filter::Equals(column, value) => {
                        row.0.get(column) == Some(&Value::Int(value as i64))
                    }
                })
                .map(|row| {
                    Row(columns
                        .iter()
                        .map(|column| ((*column).to_string(), row.0[*column].clone()))
                        .collect())
                })
                .collect())
        }
    }
}
