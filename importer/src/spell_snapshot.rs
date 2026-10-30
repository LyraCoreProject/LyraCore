use std::collections::BTreeMap;

use lyracore_package_delta::{packed_spell_effect_id, FieldValue};

use crate::dump::sql_text;

const SNAPSHOT_VERSION: u32 = 1;

#[derive(serde::Serialize)]
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
    pub(crate) max_duration_ms: u32,
    pub(crate) custom_flags: u32,
    pub(crate) min_target_level: u32,
    pub(crate) single_target_key: u64,
}

impl SpellHeaderRow {
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
            ("custom_flags", FieldValue::U32(self.custom_flags)),
            ("min_target_level", FieldValue::U32(self.min_target_level)),
            ("single_target_key", FieldValue::U64(self.single_target_key)),
        ]
    }

    pub(crate) fn sql_values(&self) -> String {
        let mut columns = self.columns();
        // SQL retains combo scaling in published order; the Base Snapshot omits it.
        let position = columns
            .iter()
            .position(|(name, _)| *name == "custom_flags")
            .expect("spell headers contain custom_flags");
        columns.insert(
            position,
            ("max_duration_ms", FieldValue::U32(self.max_duration_ms)),
        );
        sql_tuple(&columns)
    }
}

#[derive(serde::Serialize)]
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
    pub(crate) points_per_combo: f32,
    pub(crate) bonus_coefficient: f32,
}

impl SpellEffectRow {
    pub(crate) fn row_id(&self) -> u64 {
        packed_spell_effect_id(self.spell_id, self.effect_index)
    }

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
            ("bonus_coefficient", FieldValue::F32(self.bonus_coefficient)),
        ]
    }

    pub(crate) fn sql_values(&self) -> String {
        let mut columns = self.columns();
        columns.insert(0, ("id", FieldValue::U64(self.row_id())));
        let position = columns
            .iter()
            .position(|(name, _)| *name == "bonus_coefficient")
            .expect("spell effects contain bonus_coefficient");
        columns.insert(
            position,
            ("points_per_combo", FieldValue::F32(self.points_per_combo)),
        );
        sql_tuple(&columns)
    }
}

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

#[derive(serde::Serialize)]
pub(crate) struct ReagentRow {
    pub id: u64,
    pub spell_id: u32,
    pub item_entry: u32,
    pub count: u32,
}

impl ReagentRow {
    pub fn sql_values(&self) -> String {
        format!(
            "({},{},{},{})",
            self.id, self.spell_id, self.item_entry, self.count
        )
    }
}

#[derive(serde::Serialize)]
pub(crate) struct AiMetadataRow {
    pub spell_id: u32,
    pub exclude_caster: bool,
}

impl AiMetadataRow {
    pub fn sql_values(&self) -> String {
        format!("({},{})", self.spell_id, self.exclude_caster)
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrainerRow {
    pub id: u64,
    pub trainer_entry: u32,
    pub spell_id: u32,
    pub cost: u32,
    pub required_level: u8,
    pub learn_skill_line: u32,
    pub learn_skill_cap: u32,
}

impl TrainerRow {
    pub fn sql_values(&self) -> String {
        format!(
            "({},{},{},{},{},{},{})",
            self.id,
            self.trainer_entry,
            self.spell_id,
            self.cost,
            self.required_level,
            self.learn_skill_line,
            self.learn_skill_cap
        )
    }
}

#[derive(Default)]
pub(crate) struct SpellRows {
    pub(crate) headers: Vec<SpellHeaderRow>,
    pub(crate) effects: Vec<SpellEffectRow>,
    // Fishing markers replace source effects at the same key after the bulk insert.
    pub(crate) fishing: Vec<SpellEffectRow>,
    pub(crate) reagents: Vec<ReagentRow>,
    pub(crate) eventai_metadata: Vec<AiMetadataRow>,
    pub(crate) trainers: Vec<TrainerRow>,
}

impl SpellRows {
    fn resolved_effects(&self) -> BTreeMap<u64, &SpellEffectRow> {
        let mut resolved = BTreeMap::new();
        for row in self.effects.iter().chain(&self.fishing) {
            resolved.insert(row.row_id(), row);
        }
        resolved
    }

    fn resolved_headers(&self) -> BTreeMap<u32, &SpellHeaderRow> {
        self.headers.iter().map(|row| (row.spell_id, row)).collect()
    }
}

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
            max_duration_ms: 8000,
            custom_flags: 0,
            min_target_level: 0,
            single_target_key: 0,
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
            points_per_combo: 0.0,
            bonus_coefficient: -1.0,
        }
    }

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

    #[test]
    fn sql_quotes_text_and_writes_a_bare_unsigned_64_bit_value() {
        let sql = header(133, "Fireball").sql_values();

        assert!(sql.starts_with("(133,'Fireball',0,30,"), "{sql}");
        assert!(sql.contains(",18446744073709551615,"), "{sql}");
        assert!(
            sql.ends_with(",true,0,0,3,18446744073709551615,0,0,0,8000,0,0,0)"),
            "{sql}"
        );
    }

    #[test]
    fn sql_keeps_combo_scaling_before_the_vmangos_columns() {
        let mut spell = header(133, "Fireball");
        spell.max_duration_ms = 13000;
        spell.custom_flags = 64;
        spell.min_target_level = 12;
        spell.single_target_key = 99;
        assert!(spell.sql_values().ends_with(",13000,64,12,99)"));

        let mut effect = effect(133, 0);
        effect.points_per_combo = 3.5;
        effect.bonus_coefficient = 0.75;
        assert!(effect.sql_values().ends_with(",3.5,0.75)"));
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
