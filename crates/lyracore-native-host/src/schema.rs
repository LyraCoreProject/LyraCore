use std::collections::BTreeMap;

use spacetimedb::sats::{AlgebraicType, ProductType};
use spacetimedb::spacetimedb_lib::db::raw_def::v10::RawModuleDefV10Builder;
use spacetimedb::table::IndexAlgo;
use spacetimedb::Table;

#[derive(Default)]
pub struct Schema {
    pub(crate) tables: BTreeMap<u32, TableSchema>,
    pub(crate) indexes: BTreeMap<u32, IndexSchema>,
}

pub(crate) struct TableSchema {
    pub name: String,
    pub row: ProductType,
    pub unique: Vec<usize>,
    pub sequences: Vec<usize>,
    pub indexes: Vec<u32>,
}

pub(crate) struct IndexSchema {
    pub name: String,
    pub table: u32,
    pub columns: Vec<usize>,
    pub ranged: bool,
}

impl Schema {
    pub fn register<T: Table>(&mut self, _: &T) -> Result<(), String> {
        if T::SCHEDULE.is_some() || T::IS_EVENT {
            return Err(format!(
                "native scheduling and event tables are not implemented: {}",
                T::TABLE_NAME
            ));
        }
        let id = stable_id(T::TABLE_NAME);
        if self.tables.contains_key(&id) {
            return Err(format!(
                "duplicate table registration or identifier collision: {}",
                T::TABLE_NAME
            ));
        }
        let mut builder = RawModuleDefV10Builder::new();
        let mut row = builder.add_type::<T::Row>();
        let definition = builder.finish();
        let mut types = definition
            .typespace()
            .ok_or("table has no typespace")?
            .clone();
        types
            .inline_typerefs_in_type(&mut row)
            .map_err(|e| format!("{e:?}"))?;
        let AlgebraicType::Product(row) = row else {
            return Err("a table row must be a product".into());
        };
        let mut indexes = Vec::new();
        for index in T::INDEXES {
            let index_id = stable_id(index.source_name);
            if self.indexes.contains_key(&index_id) {
                return Err(format!(
                    "duplicate index or identifier collision: {}",
                    index.source_name
                ));
            }
            let (columns, ranged) = match index.algo {
                IndexAlgo::BTree { columns } => (columns.to_vec(), true),
                IndexAlgo::Hash { columns } => (columns.to_vec(), false),
                IndexAlgo::Direct { column } => (vec![column], false),
            };
            self.indexes.insert(
                index_id,
                IndexSchema {
                    name: index.source_name.into(),
                    table: id,
                    columns: columns.into_iter().map(usize::from).collect(),
                    ranged,
                },
            );
            indexes.push(index_id);
        }
        self.tables.insert(
            id,
            TableSchema {
                name: T::TABLE_NAME.into(),
                row,
                unique: T::UNIQUE_COLUMNS.iter().copied().map(usize::from).collect(),
                sequences: T::SEQUENCES.iter().copied().map(usize::from).collect(),
                indexes,
            },
        );
        Ok(())
    }
}

fn stable_id(name: &str) -> u32 {
    u32::from_le_bytes(
        blake3::hash(name.as_bytes()).as_bytes()[..4]
            .try_into()
            .unwrap(),
    )
}
