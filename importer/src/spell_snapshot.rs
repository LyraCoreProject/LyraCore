//! The **Base Snapshot**: the `game_spell` / `game_spell_effect` rows the DBC import derives,
//! written as canonical JSON so a Datascript can read base data without a shard and without client
//! files.
//!
//! # Why the rows are typed here
//!
//! The import emits SQL. A Datascript needs the same values as data. Rather than parse the SQL back,
//! both come from one typed row: [`SpellHeaderRow`] and [`SpellEffectRow`] carry the derived values,
//! [`SpellHeaderRow::sql_values`] spells them as an SQL tuple, and [`write_snapshot`] spells them as
//! JSON. One row, two spellings, no drift.
//!
//! # Licensing
//!
//! A snapshot holds DERIVED `game_*` values, never client bytes, and it is written only where the
//! operator asks. The author-facing target (`datascripts/generated/`) is git-ignored. See the
//! firewall note at the top of `dbc.rs`.
//!
//! # Canonical form
//!
//! The same rules as a Package Delta artifact (`lyracore-package-delta`'s `canonical` module): no
//! whitespace, no trailing newline, a fixed member order, integers in plain decimal, an unsigned
//! 64-bit value as a decimal string, a float as the shortest decimal that reads back as the same
//! `f32`. Every scalar is spelled by `FieldValue`'s `Display`, which IS that writer, so a snapshot
//! and a delta agree on every value by construction. Rows are primary-key sorted, so the same client
//! data always produces the same bytes.

use std::collections::BTreeMap;

use lyracore_package_delta::{packed_spell_effect_id, FieldValue};

use crate::sql_text;

/// The snapshot format this build writes.
const SNAPSHOT_VERSION: u32 = 1;

/// One `game_spell` row, before it becomes either SQL or JSON.
pub(crate) struct SpellHeaderRow {
    pub(crate) spell_id: u32,
    pub(crate) name: String,
    pub(crate) power_type: u8,
    pub(crate) cost: u32,
    pub(crate) cast_time_ms: u32,
    pub(crate) gcd_ms: u32,
    pub(crate) cooldown_ms: u32,
    pub(crate) range_yd: u32,
    pub(crate) duration_ms: u32,
    pub(crate) school_mask: u8,
    pub(crate) dispel_type: u8,
    pub(crate) mechanic: u8,
    pub(crate) max_stacks: u8,
    pub(crate) aura_interrupt: u16,
    pub(crate) attributes: u32,
    pub(crate) spell_level: u8,
    pub(crate) max_level: u8,
    pub(crate) is_negative: bool,
    pub(crate) cast_flags: u32,
    pub(crate) stances: u8,
    pub(crate) family_name: u8,
    pub(crate) family_flags: u64,
    pub(crate) proc_flags: u32,
    pub(crate) proc_chance: u8,
    pub(crate) proc_charges: u8,
}

impl SpellHeaderRow {
    /// The row's columns in the table's declaration order, which is also the order the INSERT names
    /// them in.
    fn columns(&self) -> Vec<(&'static str, FieldValue)> {
        vec![
            ("spell_id", FieldValue::U32(self.spell_id)),
            ("name", FieldValue::Str(self.name.clone())),
            ("power_type", FieldValue::U8(self.power_type)),
            ("cost", FieldValue::U32(self.cost)),
            ("cast_time_ms", FieldValue::U32(self.cast_time_ms)),
            ("gcd_ms", FieldValue::U32(self.gcd_ms)),
            ("cooldown_ms", FieldValue::U32(self.cooldown_ms)),
            ("range_yd", FieldValue::U32(self.range_yd)),
            ("duration_ms", FieldValue::U32(self.duration_ms)),
            ("school_mask", FieldValue::U8(self.school_mask)),
            ("dispel_type", FieldValue::U8(self.dispel_type)),
            ("mechanic", FieldValue::U8(self.mechanic)),
            ("max_stacks", FieldValue::U8(self.max_stacks)),
            ("aura_interrupt", FieldValue::U16(self.aura_interrupt)),
            ("attributes", FieldValue::U32(self.attributes)),
            ("spell_level", FieldValue::U8(self.spell_level)),
            ("max_level", FieldValue::U8(self.max_level)),
            ("is_negative", FieldValue::Bool(self.is_negative)),
            ("cast_flags", FieldValue::U32(self.cast_flags)),
            ("stances", FieldValue::U8(self.stances)),
            ("family_name", FieldValue::U8(self.family_name)),
            ("family_flags", FieldValue::U64(self.family_flags)),
            ("proc_flags", FieldValue::U32(self.proc_flags)),
            ("proc_chance", FieldValue::U8(self.proc_chance)),
            ("proc_charges", FieldValue::U8(self.proc_charges)),
        ]
    }

    /// The `VALUES` tuple for the reload INSERT.
    pub(crate) fn sql_values(&self) -> String {
        sql_tuple(&self.columns())
    }
}

/// One `game_spell_effect` row. The packed durable key is derived from the spell and the effect
/// index, never carried, so the two can never disagree.
pub(crate) struct SpellEffectRow {
    pub(crate) spell_id: u32,
    pub(crate) effect_index: u8,
    pub(crate) kind: u8,
    pub(crate) base_points: i32,
    pub(crate) die_sides: i32,
    pub(crate) per_level: f32,
    pub(crate) period_ms: u32,
    pub(crate) target: u8,
    pub(crate) radius_yd: f32,
    pub(crate) chain_targets: u8,
    pub(crate) trigger_spell: u32,
    pub(crate) effect_mechanic: u8,
    pub(crate) p0: i32,
    pub(crate) p0_kind: u8,
    pub(crate) p1: i32,
    pub(crate) script_id: u32,
    pub(crate) enters_combat: bool,
}

impl SpellEffectRow {
    /// `(spell_id << 2) | effect_index`, the deterministic primary key the Module and the importer
    /// have always used.
    pub(crate) fn row_id(&self) -> u64 {
        packed_spell_effect_id(self.spell_id, self.effect_index)
    }

    /// The row's columns in declaration order, without the derived `id`.
    fn columns(&self) -> Vec<(&'static str, FieldValue)> {
        vec![
            ("spell_id", FieldValue::U32(self.spell_id)),
            ("effect_index", FieldValue::U8(self.effect_index)),
            ("kind", FieldValue::U8(self.kind)),
            ("base_points", FieldValue::I32(self.base_points)),
            ("die_sides", FieldValue::I32(self.die_sides)),
            ("per_level", FieldValue::F32(self.per_level)),
            ("period_ms", FieldValue::U32(self.period_ms)),
            ("target", FieldValue::U8(self.target)),
            ("radius_yd", FieldValue::F32(self.radius_yd)),
            ("chain_targets", FieldValue::U8(self.chain_targets)),
            ("trigger_spell", FieldValue::U32(self.trigger_spell)),
            ("effect_mechanic", FieldValue::U8(self.effect_mechanic)),
            ("p0", FieldValue::I32(self.p0)),
            ("p0_kind", FieldValue::U8(self.p0_kind)),
            ("p1", FieldValue::I32(self.p1)),
            ("script_id", FieldValue::U32(self.script_id)),
            ("enters_combat", FieldValue::Bool(self.enters_combat)),
        ]
    }

    /// The `VALUES` tuple for the reload INSERT, whose first column is the derived key.
    pub(crate) fn sql_values(&self) -> String {
        let mut columns = self.columns();
        columns.insert(0, ("id", FieldValue::U64(self.row_id())));
        sql_tuple(&columns)
    }
}

/// The SQL `VALUES` tuple for one row.
///
/// Two spellings differ from the canonical JSON one: a string is SQL-quoted rather than
/// JSON-quoted, and an unsigned 64-bit value is a bare number rather than a decimal string. Every
/// other type has the same literal in both, which is why `Display` carries them.
fn sql_tuple(columns: &[(&'static str, FieldValue)]) -> String {
    let mut out = String::from("(");
    for (index, (_, value)) in columns.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        match value {
            FieldValue::Str(text) => out.push_str(&sql_text(text)),
            FieldValue::U64(n) => out.push_str(&n.to_string()),
            other => out.push_str(&other.to_string()),
        }
    }
    out.push(')');
    out
}

/// Everything one DBC spell pass derived, in the order the reload statements name it.
///
/// The typed halves feed both the SQL and the snapshot. The remaining families are SQL-only: they
/// are not Package Delta tables, so no Datascript can read or claim them.
#[derive(Default)]
pub(crate) struct SpellRows {
    /// `game_spell` headers, in DBC order.
    pub(crate) headers: Vec<SpellHeaderRow>,
    /// `game_spell_effect` rows the DBC mapping produced, including the curated synthetic additions.
    pub(crate) effects: Vec<SpellEffectRow>,
    /// The curated fishing marker rows, which the reload writes by key AFTER the bulk insert and so
    /// replace any DBC-derived row at the same key.
    pub(crate) fishing: Vec<SpellEffectRow>,
    /// `game_spell_reagent` value tuples.
    pub(crate) reagents: Vec<String>,
    /// `game_creature_ai_spell_metadata` value tuples.
    pub(crate) eventai_metadata: Vec<String>,
    /// `game_trainer_spell` value tuples for the curated override rows.
    pub(crate) trainers: Vec<String>,
}

impl SpellRows {
    /// The effect rows a shard ends up holding, primary-key sorted, with a fishing marker replacing
    /// whatever the DBC put at its key — exactly what the reload statements leave behind.
    fn resolved_effects(&self) -> BTreeMap<u64, &SpellEffectRow> {
        let mut resolved = BTreeMap::new();
        for row in self.effects.iter().chain(&self.fishing) {
            resolved.insert(row.row_id(), row);
        }
        resolved
    }

    /// The headers a shard ends up holding, primary-key sorted.
    fn resolved_headers(&self) -> BTreeMap<u32, &SpellHeaderRow> {
        self.headers.iter().map(|row| (row.spell_id, row)).collect()
    }
}

/// The Base Snapshot for these rows, in canonical form.
pub(crate) fn write_snapshot(rows: &SpellRows) -> String {
    let mut out = format!("{{\"version\":{SNAPSHOT_VERSION},\"game_spell\":[");
    for (index, header) in rows.resolved_headers().values().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_row(&mut out, &header.columns());
    }
    out.push_str("],\"game_spell_effect\":[");
    for (index, effect) in rows.resolved_effects().values().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_row(&mut out, &effect.columns());
    }
    out.push_str("]}");
    out
}

/// One row as a JSON object. `FieldValue`'s `Display` is the Package Delta canonical writer, so
/// every scalar here is spelled exactly as a claim on the same column would spell it.
fn write_row(out: &mut String, columns: &[(&'static str, FieldValue)]) {
    out.push('{');
    for (index, (name, value)) in columns.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(name);
        out.push_str("\":");
        out.push_str(&value.to_string());
    }
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;

    use lyracore_package_delta::Table;

    fn header(spell_id: u32, name: &str) -> SpellHeaderRow {
        SpellHeaderRow {
            spell_id,
            name: name.to_owned(),
            power_type: 0,
            cost: 30,
            cast_time_ms: 2500,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 35,
            duration_ms: 8000,
            school_mask: 4,
            dispel_type: 1,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 1,
            attributes: 0,
            spell_level: 1,
            max_level: 0,
            is_negative: true,
            cast_flags: 0,
            stances: 0,
            family_name: 3,
            family_flags: 18_446_744_073_709_551_615,
            proc_flags: 0,
            proc_chance: 0,
            proc_charges: 0,
        }
    }

    fn effect(spell_id: u32, effect_index: u8) -> SpellEffectRow {
        SpellEffectRow {
            spell_id,
            effect_index,
            kind: 2,
            base_points: 14,
            die_sides: 4,
            per_level: 0.0,
            period_ms: 0,
            target: 1,
            radius_yd: 12.5,
            chain_targets: 0,
            trigger_spell: 0,
            effect_mechanic: 0,
            p0: 0,
            p0_kind: 255,
            p1: 0,
            script_id: 0,
            enters_combat: false,
        }
    }

    /// The snapshot column list IS the durable schema the Package Delta catalogue names. A column
    /// added to one and not the other would let a Datascript claim a field its base data never
    /// carries, or read a field no claim can set.
    #[test]
    fn the_snapshot_names_every_claimable_column_of_both_tables() {
        let header_names: Vec<&str> = header(133, "Fireball")
            .columns()
            .iter()
            .map(|(name, _)| *name)
            .collect();
        let effect_names: Vec<&str> = effect(133, 0)
            .columns()
            .iter()
            .map(|(name, _)| *name)
            .collect();

        let mut expected_header = vec!["spell_id"];
        expected_header.extend(Table::Spell.columns().iter().map(|c| c.name));
        let mut expected_effect = vec!["spell_id", "effect_index"];
        expected_effect.extend(Table::SpellEffect.columns().iter().map(|c| c.name));

        assert_eq!(header_names, expected_header);
        assert_eq!(effect_names, expected_effect);
    }

    #[test]
    fn an_effect_row_derives_its_packed_key_rather_than_carrying_one() {
        assert_eq!(effect(133, 2).row_id(), (133 << 2) | 2);
    }

    /// SQL and JSON disagree about exactly two spellings, and about nothing else.
    #[test]
    fn sql_quotes_text_and_writes_a_bare_unsigned_64_bit_value() {
        let sql = header(133, "Fireball").sql_values();

        assert!(sql.starts_with("(133,'Fireball',0,30,"), "{sql}");
        assert!(sql.contains(",18446744073709551615,"), "{sql}");
        assert!(
            sql.ends_with(",true,0,0,3,18446744073709551615,0,0,0)"),
            "{sql}"
        );
    }

    #[test]
    fn the_snapshot_writes_an_unsigned_64_bit_value_as_a_decimal_string() {
        let rows = SpellRows {
            headers: vec![header(133, "Fireball")],
            ..SpellRows::default()
        };

        let json = write_snapshot(&rows);

        assert!(
            json.contains(r#""family_flags":"18446744073709551615""#),
            "{json}"
        );
        assert!(json.contains(r#""name":"Fireball""#), "{json}");
        assert!(json.contains(r#""is_negative":true"#), "{json}");
    }

    /// A float always carries a decimal point, so a float column stays distinct from an integer one
    /// in both spellings.
    #[test]
    fn a_float_column_always_carries_a_decimal_point() {
        let rows = SpellRows {
            effects: vec![effect(133, 0)],
            ..SpellRows::default()
        };

        let json = write_snapshot(&rows);

        assert!(json.contains(r#""per_level":0.0"#), "{json}");
        assert!(json.contains(r#""radius_yd":12.5"#), "{json}");
        assert!(effect(133, 0).sql_values().contains(",0.0,"));
    }

    #[test]
    fn rows_are_primary_key_sorted_whatever_order_they_arrived_in() {
        let rows = SpellRows {
            headers: [133_u32, 6, 400]
                .into_iter()
                .map(|id| header(id, "spell"))
                .collect(),
            effects: [(133_u32, 2_u8), (6, 0), (133, 0)]
                .into_iter()
                .map(|(spell, index)| effect(spell, index))
                .collect(),
            ..SpellRows::default()
        };

        let json = write_snapshot(&rows);

        let spells: Vec<&str> = json
            .match_indices(r#"{"spell_id":"#)
            .map(|(_, s)| s)
            .collect();
        assert_eq!(spells.len(), 6);
        assert!(
            json.find(r#""spell_id":6,"name""#) < json.find(r#""spell_id":133,"name""#),
            "{json}"
        );
        assert!(
            json.find(r#""spell_id":133,"effect_index":0"#)
                < json.find(r#""spell_id":133,"effect_index":2"#),
            "{json}"
        );
    }

    #[test]
    fn the_same_rows_always_produce_the_same_bytes() {
        let rows = SpellRows {
            headers: vec![header(133, "Fireball")],
            effects: vec![effect(133, 0)],
            ..SpellRows::default()
        };

        assert_eq!(write_snapshot(&rows), write_snapshot(&rows));
    }

    /// The reload writes the fishing marker by key after the bulk insert, so a shard holds the
    /// marker and not the DBC row it landed on. The snapshot has to describe the shard, not the
    /// intermediate.
    #[test]
    fn a_curated_fishing_marker_replaces_the_row_it_lands_on() {
        let mut marker = effect(7620, 0);
        marker.kind = 0x1C;
        let rows = SpellRows {
            effects: vec![effect(7620, 0)],
            fishing: vec![marker],
            ..SpellRows::default()
        };

        let json = write_snapshot(&rows);

        assert_eq!(json.matches(r#""spell_id":7620"#).count(), 1, "{json}");
        assert!(json.contains(r#""effect_index":0,"kind":28"#), "{json}");
    }

    #[test]
    fn an_empty_snapshot_is_still_a_snapshot() {
        assert_eq!(
            write_snapshot(&SpellRows::default()),
            r#"{"version":1,"game_spell":[],"game_spell_effect":[]}"#
        );
    }
}
