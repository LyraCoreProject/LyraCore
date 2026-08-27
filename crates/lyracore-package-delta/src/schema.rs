//! The tables a Package Delta may claim, their claimable columns, and the typed values those
//! columns take.
//!
//! The catalogue is hand-written against the Module's own table definitions. It is deliberately a
//! closed list: an unknown table or column fails the parse instead of reaching a writer that would
//! discover the problem against a live shard.
//!
//! # Import Families
//!
//! Every table belongs to exactly one Import Family, the unit a base import clears and reloads and
//! the unit a Package Delta apply runs for. [`Table`] is one closed enum whose variants are grouped
//! by family rather than a family registry, so the catalogue stays one list and every `match` on it
//! stays exhaustive.

use core::fmt;

/// The Import Family that owns the spell tables.
pub const SPELL_FAMILY: &str = "spell";

/// A table a Package Delta may claim rows in. The names are the durable table names, so an applier
/// needs no translation step.
///
/// The variants are grouped by Import Family, in that family's canonical order. A family arrives as
/// a block of variants; every `match` on this enum that carries no wildcard then fails to compile
/// until the new tables are handled, which is how the catalogue, the identifier rules, the key
/// shapes and the Module's setters stay in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Table {
    // ---- spell ----
    /// `game_spell` — the spell header.
    Spell,
    /// `game_spell_effect` — one effect of a spell.
    SpellEffect,
}

impl Table {
    /// Every table this build knows, in canonical order, so a refusal can name the whole catalogue
    /// and a test can walk it. Kept in step with [`Table::parse`] by `tests/families.rs`.
    pub const ALL: &'static [Self] = &[Self::Spell, Self::SpellEffect];

    /// The durable table name, and the value the artifact's `table` member carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spell => "game_spell",
            Self::SpellEffect => "game_spell_effect",
        }
    }

    /// The Import Family that owns this table.
    ///
    /// The same name `game_import_meta.family` records, the `apply_package_deltas` reducer takes,
    /// and `game_package_import.id` carries before the slash. An applier is called for one family
    /// at a time, so this is what tells it a claim belongs to the import it is running.
    #[must_use]
    pub const fn family(self) -> &'static str {
        match self {
            Self::Spell | Self::SpellEffect => SPELL_FAMILY,
        }
    }

    /// Resolves a `table` member. `None` for anything outside the closed list.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "game_spell" => Some(Self::Spell),
            "game_spell_effect" => Some(Self::SpellEffect),
            _ => None,
        }
    }

    /// The columns a claim may name, in the table's declaration order.
    ///
    /// Primary-key columns are absent on purpose. A claim carries them in its `key`, so naming one
    /// in `fields` would let a Package state the same identifier twice and disagree with itself.
    /// `game_spell_effect.id` is absent for the same reason plus one more: it is derived, never
    /// authored.
    #[must_use]
    pub const fn columns(self) -> &'static [Column] {
        match self {
            Self::Spell => SPELL_COLUMNS,
            Self::SpellEffect => SPELL_EFFECT_COLUMNS,
        }
    }

    /// The type of one claimable column, or `None` when the table has no such column.
    #[must_use]
    pub fn column_type(self, name: &str) -> Option<FieldType> {
        self.columns()
            .iter()
            .find(|column| column.name == name)
            .map(|column| column.ty)
    }
}

impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One claimable column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// The durable column name, and the `fields` member name in the artifact.
    pub name: &'static str,
    /// The type a claim on this column must declare.
    pub ty: FieldType,
}

const fn column(name: &'static str, ty: FieldType) -> Column {
    Column { name, ty }
}

// ---- spell ----

/// `game_spell` minus its `spell_id` primary key.
const SPELL_COLUMNS: &[Column] = &[
    column("name", FieldType::Str),
    column("power_type", FieldType::U8),
    column("cost", FieldType::U32),
    column("cast_time_ms", FieldType::U32),
    column("gcd_ms", FieldType::U32),
    column("cooldown_ms", FieldType::U32),
    column("range_yd", FieldType::U32),
    column("duration_ms", FieldType::U32),
    column("school_mask", FieldType::U8),
    column("dispel_type", FieldType::U8),
    column("mechanic", FieldType::U8),
    column("max_stacks", FieldType::U8),
    column("aura_interrupt", FieldType::U16),
    column("attributes", FieldType::U32),
    column("spell_level", FieldType::U8),
    column("max_level", FieldType::U8),
    column("is_negative", FieldType::Bool),
    column("cast_flags", FieldType::U32),
    column("stances", FieldType::U8),
    column("family_name", FieldType::U8),
    column("family_flags", FieldType::U64),
    column("proc_flags", FieldType::U32),
    column("proc_chance", FieldType::U8),
    column("proc_charges", FieldType::U8),
];

/// `game_spell_effect` minus its derived `id` key and the `spell_id` / `effect_index` the key names.
const SPELL_EFFECT_COLUMNS: &[Column] = &[
    column("kind", FieldType::U8),
    column("base_points", FieldType::I32),
    column("die_sides", FieldType::I32),
    column("per_level", FieldType::F32),
    column("period_ms", FieldType::U32),
    column("target", FieldType::U8),
    column("radius_yd", FieldType::F32),
    column("chain_targets", FieldType::U8),
    column("trigger_spell", FieldType::U32),
    column("effect_mechanic", FieldType::U8),
    column("p0", FieldType::I32),
    column("p0_kind", FieldType::U8),
    column("p1", FieldType::I32),
    column("script_id", FieldType::U32),
    column("enters_combat", FieldType::Bool),
];

/// The type tag a claimed value carries.
///
/// Every value states its type explicitly. That makes a wrong-width claim a parse failure rather
/// than a silent truncation at apply, and it is what lets `U64` travel as a decimal string: JSON has
/// one number type and it loses precision above 2^53, so an unsigned 64-bit value that went through
/// a JSON number would come back wrong with nothing to notice it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldType {
    /// JSON number, 0..=255.
    U8,
    /// JSON number, 0..=65535.
    U16,
    /// JSON number, 0..=4294967295.
    U32,
    /// JSON **string** of decimal digits, 0..=18446744073709551615.
    U64,
    /// JSON number, -2147483648..=2147483647.
    I32,
    /// JSON number, finite, representable as `f32`.
    F32,
    /// JSON boolean.
    Bool,
    /// JSON string.
    Str,
}

impl FieldType {
    /// The value the artifact's `type` member carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I32 => "i32",
            Self::F32 => "f32",
            Self::Bool => "bool",
            Self::Str => "string",
        }
    }

    /// Resolves a `type` member. `None` for anything outside the closed list.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "u64" => Some(Self::U64),
            "i32" => Some(Self::I32),
            "f32" => Some(Self::F32),
            "bool" => Some(Self::Bool),
            "string" => Some(Self::Str),
            _ => None,
        }
    }
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One claimed column value, already narrowed to the column's type.
///
/// `PartialEq` is total here even though it holds an `f32`: the parser refuses a non-finite float,
/// so no `NaN` can reach a `FieldValue`. Two Packages claiming the same column with the same value
/// therefore compare equal, which is what the tracer needs to describe a conflict exactly.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// An 8-bit unsigned value.
    U8(u8),
    /// A 16-bit unsigned value.
    U16(u16),
    /// A 32-bit unsigned value.
    U32(u32),
    /// A 64-bit unsigned value. Travels as a decimal string.
    U64(u64),
    /// A 32-bit signed value.
    I32(i32),
    /// A finite 32-bit float.
    F32(f32),
    /// A boolean.
    Bool(bool),
    /// A string.
    Str(String),
}

impl FieldValue {
    /// The type tag this value serializes with.
    #[must_use]
    pub const fn field_type(&self) -> FieldType {
        match self {
            Self::U8(_) => FieldType::U8,
            Self::U16(_) => FieldType::U16,
            Self::U32(_) => FieldType::U32,
            Self::U64(_) => FieldType::U64,
            Self::I32(_) => FieldType::I32,
            Self::F32(_) => FieldType::F32,
            Self::Bool(_) => FieldType::Bool,
            Self::Str(_) => FieldType::Str,
        }
    }
}

/// The value in its canonical JSON spelling, without the surrounding type tag, so a conflict report
/// quotes it exactly as the artifact writes it.
impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::canonical::scalar_literal(self))
    }
}
