use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Bound;
use std::sync::Arc;

use spacetimedb::sats::{bsatn, AlgebraicType, AlgebraicValue, ArrayValue, ProductValue};
use spacetimedb_primitives::errno;

use crate::schema::Schema;

type Key = Vec<AlgebraicValue>;
type Rows = BTreeMap<u64, Arc<Row>>;
type Index = BTreeMap<Key, BTreeSet<u64>>;
type Result<T> = std::result::Result<T, u16>;

struct Row {
    value: ProductValue,
    bytes: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub table: String,
    pub before: Option<Arc<[u8]>>,
    pub after: Option<Arc<[u8]>>,
}

pub(crate) struct Storage {
    pub schema: Schema,
    pub fault: Option<String>,
    tables: HashMap<u32, Rows>,
    indexes: HashMap<u32, Index>,
    sequences: HashMap<(u32, usize), u64>,
    before: BTreeMap<(u32, u64), Option<Arc<Row>>>,
    next_row: u64,
    pub iterators: HashMap<u32, VecDeque<Arc<[u8]>>>,
    next_iterator: u32,
}

impl Storage {
    pub fn new(schema: Schema) -> std::result::Result<Self, String> {
        let mut tables = HashMap::new();
        let mut indexes = HashMap::new();
        let mut sequences = HashMap::new();
        for (&id, table) in &schema.tables {
            tables.insert(id, BTreeMap::new());
            for &column in &table.sequences {
                sequences.insert((id, column), 1);
            }
            for &index in &table.indexes {
                indexes.insert(index, BTreeMap::new());
            }
        }
        Ok(Self {
            schema,
            fault: None,
            tables,
            indexes,
            sequences,
            before: BTreeMap::new(),
            next_row: 1,
            iterators: HashMap::new(),
            next_iterator: 1,
        })
    }

    pub fn table_id(&mut self, name: &str) -> Result<u32> {
        self.schema
            .tables
            .iter()
            .find_map(|(&id, table)| (table.name == name).then_some(id))
            .ok_or_else(|| {
                self.fault = Some(format!("unregistered native table: {name}"));
                errno::NO_SUCH_TABLE.get()
            })
    }

    pub fn index_id(&mut self, name: &str) -> Result<u32> {
        self.schema
            .indexes
            .iter()
            .find_map(|(&id, index)| (index.name == name).then_some(id))
            .ok_or_else(|| {
                self.fault = Some(format!("unregistered native index: {name}"));
                errno::NO_SUCH_INDEX.get()
            })
    }

    pub fn count(&self, table: u32) -> Result<u64> {
        Ok(self
            .tables
            .get(&table)
            .ok_or(errno::NO_SUCH_TABLE.get())?
            .len() as u64)
    }

    fn decode_row(&self, table: u32, bytes: &[u8]) -> Result<ProductValue> {
        let schema = &self
            .schema
            .tables
            .get(&table)
            .ok_or(errno::NO_SUCH_TABLE.get())?
            .row;
        let mut remaining = bytes;
        let row =
            bsatn::decode(schema, &mut remaining).map_err(|_| errno::BSATN_DECODE_ERROR.get())?;
        if !remaining.is_empty() {
            return Err(errno::BSATN_DECODE_ERROR.get());
        }
        Ok(row)
    }

    pub fn scan(&mut self, table: u32) -> Result<u32> {
        let rows = self
            .tables
            .get(&table)
            .ok_or(errno::NO_SUCH_TABLE.get())?
            .values()
            .map(|row| Arc::clone(&row.bytes))
            .collect();
        self.iterator(rows)
    }

    fn iterator(&mut self, rows: VecDeque<Arc<[u8]>>) -> Result<u32> {
        let id = self.next_iterator;
        self.next_iterator = id.checked_add(1).ok_or(errno::NO_SPACE.get())?;
        self.iterators.insert(id, rows);
        Ok(id)
    }

    pub fn scan_index(&mut self, index: u32, keys: Vec<u64>) -> Result<u32> {
        let table = self
            .schema
            .indexes
            .get(&index)
            .ok_or(errno::NO_SUCH_INDEX.get())?
            .table;
        let rows = keys
            .into_iter()
            .map(|id| Arc::clone(&self.tables[&table][&id].bytes))
            .collect();
        self.iterator(rows)
    }

    pub fn point(&self, index: u32, bytes: &[u8]) -> Result<Vec<u64>> {
        let schema = self
            .schema
            .indexes
            .get(&index)
            .ok_or(errno::NO_SUCH_INDEX.get())?;
        let table = &self.schema.tables[&schema.table];
        let mut remaining = bytes;
        let mut key = Vec::new();
        for &column in &schema.columns {
            key.push(decode_value(
                &table.row.elements[column].algebraic_type,
                &mut remaining,
            )?);
        }
        if !remaining.is_empty() {
            return Err(errno::BSATN_DECODE_ERROR.get());
        }
        Ok(self.indexes[&index]
            .get(&key)
            .into_iter()
            .flatten()
            .copied()
            .collect())
    }

    pub fn range(
        &self,
        index: u32,
        prefix: &[u8],
        count: usize,
        start: &[u8],
        end: &[u8],
    ) -> Result<Vec<u64>> {
        let schema = self
            .schema
            .indexes
            .get(&index)
            .ok_or(errno::NO_SUCH_INDEX.get())?;
        if !schema.ranged {
            return Err(errno::WRONG_INDEX_ALGO.get());
        }
        if count >= schema.columns.len() {
            return Err(errno::BSATN_DECODE_ERROR.get());
        }
        let table = &self.schema.tables[&schema.table];
        let mut bytes = prefix;
        let mut fixed = Vec::new();
        for &column in &schema.columns[..count] {
            fixed.push(decode_value(
                &table.row.elements[column].algebraic_type,
                &mut bytes,
            )?);
        }
        if !bytes.is_empty() {
            return Err(errno::BSATN_DECODE_ERROR.get());
        }
        let ty = &table.row.elements[schema.columns[count]].algebraic_type;
        let start = decode_bound(ty, start)?;
        let end = decode_bound(ty, end)?;
        let mut lower = fixed.clone();
        if let Bound::Included(v) | Bound::Excluded(v) = &start {
            lower.push(v.clone());
        }
        let mut found = Vec::new();
        for (key, rows) in self.indexes[&index].range(lower..) {
            if !key.starts_with(&fixed) {
                break;
            }
            let value = &key[count];
            if !above(value, &start) {
                continue;
            }
            if !below(value, &end) {
                break;
            }
            found.extend(rows.iter().copied());
        }
        Ok(found)
    }

    pub fn insert(
        &mut self,
        table: u32,
        bytes: &[u8],
        update_index: Option<u32>,
    ) -> Result<Vec<u8>> {
        let mut value = self.decode_row(table, bytes)?;
        let mut generated = Vec::new();
        for &column in &self.schema.tables[&table].sequences {
            let next = self.sequences.get_mut(&(table, column)).unwrap();
            if generate(&mut value.elements[column], *next)? {
                generated.extend(bsatn::to_vec(&value.elements[column]).unwrap());
                *next = next.checked_add(1).ok_or(errno::AUTO_INC_OVERFLOW.get())?;
            }
        }
        let existing = if let Some(index) = update_index {
            let schema = self
                .schema
                .indexes
                .get(&index)
                .ok_or(errno::NO_SUCH_INDEX.get())?;
            if schema.table != table {
                return Err(errno::NO_SUCH_INDEX.get());
            }
            if schema.columns.len() != 1
                || !self.schema.tables[&table]
                    .unique
                    .contains(&schema.columns[0])
            {
                return Err(errno::INDEX_NOT_UNIQUE.get());
            }
            let key = project(&value, &schema.columns);
            Some(
                *self.indexes[&index]
                    .get(&key)
                    .and_then(|ids| ids.first())
                    .ok_or(errno::NO_SUCH_ROW.get())?,
            )
        } else {
            None
        };
        for &column in &self.schema.tables[&table].unique {
            let index = self.schema.tables[&table]
                .indexes
                .iter()
                .find(|id| self.schema.indexes[id].columns == [column])
                .ok_or(errno::NO_SUCH_INDEX.get())?;
            if let Some(ids) = self.indexes[index].get(&vec![value.elements[column].clone()]) {
                for &id in ids {
                    if Some(id) != existing {
                        if existing.is_none() && self.tables[&table][&id].value == value {
                            return Ok(generated);
                        }
                        return Err(errno::UNIQUE_ALREADY_EXISTS.get());
                    }
                }
            }
        }
        if existing.is_none()
            && self.schema.tables[&table].unique.is_empty()
            && self.tables[&table].values().any(|row| row.value == value)
        {
            return Ok(generated);
        }
        let id = existing.unwrap_or_else(|| {
            let id = self.next_row;
            self.next_row += 1;
            id
        });
        self.remember(table, id);
        let bytes: Arc<[u8]> = bsatn::to_vec(&value).unwrap().into();
        self.replace(table, id, Some(Arc::new(Row { value, bytes })));
        Ok(generated)
    }

    fn remember(&mut self, table: u32, row: u64) {
        self.before
            .entry((table, row))
            .or_insert_with(|| self.tables[&table].get(&row).cloned());
    }

    fn replace(&mut self, table: u32, id: u64, replacement: Option<Arc<Row>>) {
        if let Some(old) = self.tables.get_mut(&table).unwrap().remove(&id) {
            for index in &self.schema.tables[&table].indexes {
                let key = project(&old.value, &self.schema.indexes[index].columns);
                let entries = self.indexes.get_mut(index).unwrap();
                let ids = entries.get_mut(&key).unwrap();
                ids.remove(&id);
                if ids.is_empty() {
                    entries.remove(&key);
                }
            }
        }
        if let Some(row) = replacement {
            for index in &self.schema.tables[&table].indexes {
                let key = project(&row.value, &self.schema.indexes[index].columns);
                self.indexes
                    .get_mut(index)
                    .unwrap()
                    .entry(key)
                    .or_default()
                    .insert(id);
            }
            self.tables.get_mut(&table).unwrap().insert(id, row);
        }
    }

    pub fn delete_index(&mut self, index: u32, rows: Vec<u64>) -> Result<u32> {
        let table = self
            .schema
            .indexes
            .get(&index)
            .ok_or(errno::NO_SUCH_INDEX.get())?
            .table;
        let count = rows.len() as u32;
        for id in rows {
            self.remember(table, id);
            self.replace(table, id, None);
        }
        Ok(count)
    }

    pub fn delete_equal(&mut self, table: u32, bytes: &[u8]) -> Result<u32> {
        let schema = self
            .schema
            .tables
            .get(&table)
            .ok_or(errno::NO_SUCH_TABLE.get())?;
        let ty = AlgebraicType::array(AlgebraicType::Product(schema.row.clone()));
        let mut remaining = bytes;
        let values = decode_value(&ty, &mut remaining)?;
        if !remaining.is_empty() {
            return Err(errno::BSATN_DECODE_ERROR.get());
        }
        let mut count = 0;
        let AlgebraicValue::Array(ArrayValue::Product(values)) = values else {
            return Err(errno::BSATN_DECODE_ERROR.get());
        };
        for row in values.into_vec() {
            let id = self.tables[&table]
                .iter()
                .find_map(|(&id, stored)| (stored.value == row).then_some(id));
            if let Some(id) = id {
                self.remember(table, id);
                self.replace(table, id, None);
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn clear(&mut self, table: u32) -> Result<u64> {
        let ids: Vec<_> = self
            .tables
            .get(&table)
            .ok_or(errno::NO_SUCH_TABLE.get())?
            .keys()
            .copied()
            .collect();
        let count = ids.len() as u64;
        for id in ids {
            self.remember(table, id);
            self.replace(table, id, None);
        }
        Ok(count)
    }

    pub fn commit(&mut self) -> Vec<Change> {
        self.iterators.clear();
        let mut changes: Vec<Change> = Vec::new();
        let mut removed: BTreeMap<(u32, Arc<[u8]>), usize> = BTreeMap::new();
        let mut inserted: BTreeMap<(u32, Arc<[u8]>), usize> = BTreeMap::new();
        for ((table, id), original) in std::mem::take(&mut self.before) {
            let mut before = original.map(|row| Arc::clone(&row.bytes));
            let mut after = self.tables[&table]
                .get(&id)
                .map(|row| Arc::clone(&row.bytes));
            if before == after {
                continue;
            }
            let next = changes.len();
            if let Some(row) = &before {
                let key = (table, Arc::clone(row));
                if let Some(index) = inserted.remove(&key) {
                    changes[index].after = None;
                    before = None;
                } else {
                    removed.insert(key, next);
                }
            }
            if let Some(row) = &after {
                let key = (table, Arc::clone(row));
                if let Some(index) = removed.remove(&key) {
                    changes[index].before = None;
                    after = None;
                } else {
                    inserted.insert(key, next);
                }
            }
            changes.push(Change {
                table: self.schema.tables[&table].name.clone(),
                before,
                after,
            });
        }
        changes.retain(|change| change.before.is_some() || change.after.is_some());
        changes
    }

    pub fn rollback(&mut self) {
        self.iterators.clear();
        for ((table, id), before) in std::mem::take(&mut self.before) {
            self.replace(table, id, before);
        }
    }
}

fn project(row: &ProductValue, columns: &[usize]) -> Key {
    columns
        .iter()
        .map(|&column| row.elements[column].clone())
        .collect()
}

fn decode_value(ty: &AlgebraicType, bytes: &mut &[u8]) -> Result<AlgebraicValue> {
    bsatn::decode(ty, bytes).map_err(|_| errno::BSATN_DECODE_ERROR.get())
}

fn decode_bound(ty: &AlgebraicType, bytes: &[u8]) -> Result<Bound<AlgebraicValue>> {
    let (&tag, mut remaining) = bytes.split_first().ok_or(errno::BSATN_DECODE_ERROR.get())?;
    let bound = match tag {
        0 => Bound::Included(decode_value(ty, &mut remaining)?),
        1 => Bound::Excluded(decode_value(ty, &mut remaining)?),
        2 => Bound::Unbounded,
        _ => return Err(errno::BSATN_DECODE_ERROR.get()),
    };
    if !remaining.is_empty() {
        return Err(errno::BSATN_DECODE_ERROR.get());
    }
    Ok(bound)
}

fn above(value: &AlgebraicValue, bound: &Bound<AlgebraicValue>) -> bool {
    match bound {
        Bound::Unbounded => true,
        Bound::Included(v) => value >= v,
        Bound::Excluded(v) => value > v,
    }
}

fn below(value: &AlgebraicValue, bound: &Bound<AlgebraicValue>) -> bool {
    match bound {
        Bound::Unbounded => true,
        Bound::Included(v) => value <= v,
        Bound::Excluded(v) => value < v,
    }
}

fn generate(value: &mut AlgebraicValue, next: u64) -> Result<bool> {
    macro_rules! numeric {
        ($($variant:ident),*) => { match value {
            $(AlgebraicValue::$variant(v) => {
                if *v != 0 { return Ok(false); }
                *v = next.try_into().map_err(|_| errno::AUTO_INC_OVERFLOW.get())?;
                Ok(true)
            },)*
            _ => Err(errno::HOST_CALL_FAILURE.get()),
        } };
    }
    numeric!(U8, U16, U32, U64, I8, I16, I32, I64)
}
