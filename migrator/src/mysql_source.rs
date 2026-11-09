use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use mysql::prelude::Queryable;

use crate::source::{ColumnType, Filter, Row, SourceDatabase, Value};

pub(crate) struct MysqlDatabase {
    conn: mysql::Conn,
}

impl MysqlDatabase {
    // Every read through this connection sees one snapshot and cannot write.
    pub(crate) fn open(url: &str) -> Result<Self> {
        let opts = mysql::Opts::from_url(url).context("parse the MySQL URL")?;
        let mut conn = mysql::Conn::new(opts).context("connect to the Migration Source")?;
        conn.query_drop("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .context("select repeatable reads")?;
        conn.query_drop("START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY")
            .context("open a read-only snapshot")?;
        Ok(Self { conn })
    }
}

impl SourceDatabase for MysqlDatabase {
    fn column_types(&mut self, table: &str) -> Result<HashMap<String, ColumnType>> {
        let rows: Vec<(String, String)> = self
            .conn
            .exec(
                "SELECT COLUMN_NAME, DATA_TYPE FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
                (table,),
            )
            .with_context(|| format!("read the columns of {table}"))?;
        Ok(rows
            .into_iter()
            .filter_map(|(name, data_type)| {
                ColumnType::from_mysql(&data_type.to_ascii_lowercase()).map(|kind| (name, kind))
            })
            .collect())
    }

    fn select(&mut self, table: &str, columns: &[&str], filter: Filter) -> Result<Vec<Row>> {
        let list = columns
            .iter()
            .map(|column| format!("`{column}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let rows: Vec<mysql::Row> = match filter {
            Filter::All => self
                .conn
                .exec(format!("SELECT {list} FROM `{table}`"), ())
                .with_context(|| format!("read {table}"))?,
            Filter::Equals(column, value) => self
                .conn
                .exec(
                    format!("SELECT {list} FROM `{table}` WHERE `{column}` = ?"),
                    (value,),
                )
                .with_context(|| format!("read {table} where {column} = {value}"))?,
        };
        rows.into_iter()
            .map(|row| {
                let values = row.unwrap();
                columns
                    .iter()
                    .zip(values)
                    .map(|(column, value)| Ok(((*column).to_string(), value_of(column, value)?)))
                    .collect::<Result<HashMap<_, _>>>()
                    .map(Row)
            })
            .collect()
    }
}

fn value_of(column: &str, value: mysql::Value) -> Result<Value> {
    Ok(match value {
        mysql::Value::NULL => Value::Null,
        mysql::Value::Int(value) => Value::Int(value),
        mysql::Value::UInt(value) => Value::Int(
            i64::try_from(value)
                .map_err(|_| anyhow!("column {column} value {value} is too large"))?,
        ),
        mysql::Value::Float(value) => Value::Float(f64::from(value)),
        mysql::Value::Double(value) => Value::Float(value),
        mysql::Value::Bytes(bytes) => Value::Text(
            String::from_utf8(bytes).with_context(|| format!("column {column} is not UTF-8"))?,
        ),
        mysql::Value::Date(..) | mysql::Value::Time(..) => {
            bail!("column {column} holds a date or time, which no profile reads")
        }
    })
}
