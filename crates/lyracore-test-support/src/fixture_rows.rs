use std::collections::BTreeMap;
use std::fmt::Display;

pub type Field<'a> = (&'a str, &'a dyn Display);

pub(crate) struct FixtureSchema {
    tables: BTreeMap<String, Vec<Column>>,
}

struct Column {
    name: String,
    quoted: bool,
    fill: Result<String, String>,
}

impl FixtureSchema {
    pub(crate) fn from_describe(schema: &serde_json::Value) -> Self {
        let types = crate::schema_section(schema, "Typespace")["types"]
            .as_array()
            .expect("Module definition types");
        let tables = crate::schema_section(schema, "Tables")
            .as_array()
            .expect("Module definition tables")
            .iter()
            .map(|table| {
                let name = table["source_name"]
                    .as_str()
                    .expect("table name")
                    .to_owned();
                let row =
                    &types[table["product_type_ref"].as_u64().expect("table row type") as usize];
                let defaults: BTreeMap<u64, &str> = table["default_values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|default| {
                        (
                            default["col_id"].as_u64().expect("default column"),
                            default["value"].as_str().expect("default value"),
                        )
                    })
                    .collect();
                let columns = row["Product"]["elements"]
                    .as_array()
                    .expect("table row is a product")
                    .iter()
                    .enumerate()
                    .map(|(index, element)| {
                        let kind = &element["algebraic_type"];
                        Column {
                            name: element["name"]["some"]
                                .as_str()
                                .expect("column name")
                                .to_owned(),
                            quoted: kind.get("String").is_some(),
                            fill: match defaults.get(&(index as u64)) {
                                Some(bytes) => declared_default(kind, bytes),
                                None => zero(kind),
                            },
                        }
                    })
                    .collect();
                (name, columns)
            })
            .collect();
        Self { tables }
    }

    pub(crate) fn insert(&self, table: &str, rows: &[&[Field]]) -> String {
        let columns = self
            .tables
            .get(table)
            .unwrap_or_else(|| panic!("the published Module has no table `{table}`"));
        assert!(!rows.is_empty(), "an insert into {table} needs a row");
        let values: Vec<String> = rows
            .iter()
            .map(|fields| {
                let mut named = BTreeMap::new();
                for (name, value) in *fields {
                    let column = columns
                        .iter()
                        .find(|column| column.name == *name)
                        .unwrap_or_else(|| {
                            panic!(
                                "{table} has no column `{name}`; its columns are {}",
                                column_list(columns)
                            )
                        });
                    let text = value.to_string();
                    let literal = if column.quoted {
                        format!("'{}'", text.replace('\'', "''"))
                    } else {
                        text
                    };
                    assert!(
                        named.insert(*name, literal).is_none(),
                        "{table}.{name} is named twice"
                    );
                }
                let cells: Vec<&str> = columns
                    .iter()
                    .map(|column| match named.get(column.name.as_str()) {
                        Some(literal) => literal.as_str(),
                        None => column.fill.as_deref().unwrap_or_else(|kind| {
                            panic!(
                                "{table}.{} is {kind}, which has no SQL zero; name its value",
                                column.name
                            )
                        }),
                    })
                    .collect();
                format!("({})", cells.join(","))
            })
            .collect();
        format!(
            "INSERT INTO {table} ({}) VALUES {}",
            column_list(columns),
            values.join(",")
        )
    }
}

fn column_list(columns: &[Column]) -> String {
    columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn zero(kind: &serde_json::Value) -> Result<String, String> {
    let name = primitive(kind);
    Ok(match name.as_deref() {
        Some("Bool") => "false".into(),
        Some("String") => "''".into(),
        Some("F32" | "F64") => "0.0".into(),
        Some("U8" | "U16" | "U32" | "U64" | "U128" | "I8" | "I16" | "I32" | "I64" | "I128") => {
            "0".into()
        }
        _ => return Err(kind.to_string()),
    })
}

// SpacetimeDB stores a declared `#[default(..)]` as the value's little-endian BSATN bytes.
fn declared_default(kind: &serde_json::Value, hex: &str) -> Result<String, String> {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("default value is hex"))
        .collect();
    fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
        bytes
            .try_into()
            .expect("default value width matches its type")
    }
    let name = primitive(kind);
    Ok(match name.as_deref() {
        Some("Bool") => (array::<1>(&bytes)[0] != 0).to_string(),
        Some("U8") => u8::from_le_bytes(array(&bytes)).to_string(),
        Some("U16") => u16::from_le_bytes(array(&bytes)).to_string(),
        Some("U32") => u32::from_le_bytes(array(&bytes)).to_string(),
        Some("U64") => u64::from_le_bytes(array(&bytes)).to_string(),
        Some("I8") => i8::from_le_bytes(array(&bytes)).to_string(),
        Some("I16") => i16::from_le_bytes(array(&bytes)).to_string(),
        Some("I32") => i32::from_le_bytes(array(&bytes)).to_string(),
        Some("I64") => i64::from_le_bytes(array(&bytes)).to_string(),
        Some("F32") => format!("{:?}", f32::from_le_bytes(array(&bytes))),
        Some("F64") => format!("{:?}", f64::from_le_bytes(array(&bytes))),
        _ => return Err(format!("{kind} with a declared default")),
    })
}

fn primitive(kind: &serde_json::Value) -> Option<String> {
    let object = kind.as_object()?;
    let (name, payload) = object.iter().next()?;
    (object.len() == 1 && payload.as_array().is_some_and(Vec::is_empty)).then(|| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> FixtureSchema {
        let column =
            |name: &str, kind: &str| json!({"name": {"some": name}, "algebraic_type": {kind: []}});
        FixtureSchema::from_describe(&json!({"sections": [
            {"Typespace": {"types": [{"Product": {"elements": [
                column("id", "U32"),
                column("name", "String"),
                column("hostile", "Bool"),
                column("radius", "F32"),
                column("bonus", "F32"),
                column("class", "I32"),
                {"name": {"some": "owner"}, "algebraic_type": {"Product": {"elements": []}}},
            ]}}]}},
            {"Tables": [{
                "source_name": "game_fixture",
                "product_type_ref": 0,
                "default_values": [
                    {"col_id": 4, "value": "000080bf"},
                    {"col_id": 5, "value": "ffffffff"},
                ],
            }]},
        ]}))
    }

    #[test]
    fn unnamed_columns_take_their_declared_default_or_the_zero_of_their_type() {
        let sql = schema().insert(
            "game_fixture",
            &[&[("id", &7), ("name", &"Hogger's"), ("owner", &"0x01")]],
        );
        assert_eq!(
            sql,
            "INSERT INTO game_fixture (id,name,hostile,radius,bonus,class,owner) \
             VALUES (7,'Hogger''s',false,0.0,-1.0,-1,0x01)"
        );
    }

    #[test]
    fn a_batch_writes_every_row_in_one_statement() {
        let sql = schema().insert(
            "game_fixture",
            &[
                &[("id", &1), ("owner", &"0x01")],
                &[("id", &2), ("hostile", &true), ("owner", &"0x02")],
            ],
        );
        assert!(sql.ends_with("VALUES (1,'',false,0.0,-1.0,-1,0x01),(2,'',true,0.0,-1.0,-1,0x02)"));
    }

    #[test]
    #[should_panic(expected = "game_fixture has no column `nmae`; its columns are \
                               id,name,hostile,radius,bonus,class,owner")]
    fn an_unknown_column_lists_the_real_columns() {
        schema().insert("game_fixture", &[&[("id", &1), ("nmae", &"x")]]);
    }

    #[test]
    #[should_panic(expected = "the published Module has no table `game_missing`")]
    fn an_unknown_table_is_refused() {
        schema().insert("game_missing", &[&[("id", &1)]]);
    }

    #[test]
    #[should_panic(expected = "game_fixture.owner is")]
    fn a_column_without_a_sql_zero_must_be_named() {
        schema().insert("game_fixture", &[&[("id", &1)]]);
    }
}
