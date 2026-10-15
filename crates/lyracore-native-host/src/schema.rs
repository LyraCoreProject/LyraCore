use std::collections::{BTreeMap, BTreeSet};

use spacetimedb::sats::{
    bsatn, de::DeserializeSeed, AlgebraicType, ProductType, ProductValue, WithTypespace,
};
use spacetimedb::spacetimedb_lib::db::raw_def::v10::RawModuleDefV10Builder;
use spacetimedb::table::IndexAlgo;
use spacetimedb::Table;
use spacetimedb_primitives::errno;

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
    pub projection: Vec<usize>,
    columns: Vec<ColumnDecode>,
    projected_count: usize,
}

enum ColumnDecode {
    Store(usize),
    Check(usize),
    Skip(usize),
}

impl TableSchema {
    pub fn decode_projection(&self, bytes: &[u8]) -> Result<ProductValue, u16> {
        let mut remaining = bytes;
        let mut values = Vec::with_capacity(self.projected_count);
        let invalid = errno::BSATN_DECODE_ERROR.get();
        for column in &self.columns {
            match *column {
                ColumnDecode::Store(column) => values.push(
                    bsatn::decode(&self.row.elements[column].algebraic_type, &mut remaining)
                        .map_err(|_| invalid)?,
                ),
                ColumnDecode::Check(column) => {
                    WithTypespace::empty(&self.row.elements[column].algebraic_type)
                        .validate(bsatn::Deserializer::new(&mut remaining))
                        .map_err(|_| invalid)?;
                }
                ColumnDecode::Skip(count) => {
                    remaining = remaining.get(count..).ok_or(invalid)?;
                }
            }
        }
        if !remaining.is_empty() {
            return Err(invalid);
        }
        Ok(ProductValue {
            elements: values.into(),
        })
    }
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
        let mut definitions = BTreeMap::new();
        for index in T::INDEXES {
            let index_id = stable_id(index.source_name);
            if self.indexes.contains_key(&index_id) || definitions.contains_key(&index_id) {
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
            definitions.insert(
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
        let mut required: BTreeSet<_> = T::UNIQUE_COLUMNS
            .iter()
            .chain(T::SEQUENCES)
            .copied()
            .map(usize::from)
            .collect();
        for index in &indexes {
            required.extend(&definitions[index].columns);
        }
        let mut columns = Vec::new();
        let mut projection = vec![usize::MAX; row.elements.len()];
        let mut projected_count = 0;
        for (column, ty) in row.elements.iter().enumerate() {
            if required.contains(&column) {
                projection[column] = projected_count;
                projected_count += 1;
                columns.push(ColumnDecode::Store(column));
            } else if let Some(count) = unrestricted_width(&ty.algebraic_type) {
                if let Some(ColumnDecode::Skip(previous)) = columns.last_mut() {
                    *previous += count;
                } else {
                    columns.push(ColumnDecode::Skip(count));
                }
            } else {
                columns.push(ColumnDecode::Check(column));
            }
        }
        self.indexes.extend(definitions);
        self.tables.insert(
            id,
            TableSchema {
                name: T::TABLE_NAME.into(),
                row,
                unique: T::UNIQUE_COLUMNS.iter().copied().map(usize::from).collect(),
                sequences: T::SEQUENCES.iter().copied().map(usize::from).collect(),
                indexes,
                projection,
                columns,
                projected_count,
            },
        );
        Ok(())
    }
}

// Every bit pattern is valid for these BSATN types; other columns still need decoding checks.
fn unrestricted_width(ty: &AlgebraicType) -> Option<usize> {
    match ty {
        AlgebraicType::I8 | AlgebraicType::U8 => Some(1),
        AlgebraicType::I16 | AlgebraicType::U16 => Some(2),
        AlgebraicType::I32 | AlgebraicType::U32 | AlgebraicType::F32 => Some(4),
        AlgebraicType::I64 | AlgebraicType::U64 | AlgebraicType::F64 => Some(8),
        AlgebraicType::I128 | AlgebraicType::U128 => Some(16),
        AlgebraicType::I256 | AlgebraicType::U256 => Some(32),
        AlgebraicType::Product(product) => {
            product.elements.iter().try_fold(0usize, |count, field| {
                count.checked_add(unrestricted_width(&field.algebraic_type)?)
            })
        }
        _ => None,
    }
}

fn stable_id(name: &str) -> u32 {
    u32::from_le_bytes(
        blake3::hash(name.as_bytes()).as_bytes()[..4]
            .try_into()
            .unwrap(),
    )
}
