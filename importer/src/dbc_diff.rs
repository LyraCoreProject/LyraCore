use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use anyhow::{ensure, Context, Result};

use crate::dump::TableRows;
use crate::world_source::{Row, WorldSource};

#[derive(Clone, Copy)]
enum Kind {
    Unsigned,
    Signed,
    Float,
    Text,
    Wide,
}

#[derive(Clone)]
struct Field {
    source: String,
    column: usize,
    kind: Kind,
}

struct Comparison {
    source: &'static str,
    client: &'static str,
    width: usize,
    fields: Vec<Field>,
    source_only: &'static str,
}

struct Records {
    rows: BTreeMap<u32, Vec<u32>>,
    strings: Vec<u8>,
}

impl Records {
    fn read(bytes: &[u8], width: usize) -> Result<Self> {
        ensure!(
            bytes.len() >= 20 && &bytes[..4] == b"WDBC",
            "missing WDBC header"
        );
        let word = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let (count, fields, size, strings) = (word(4), word(8), word(12), word(16));
        ensure!(
            fields == width && size == width * 4,
            "unexpected DBC row layout"
        );
        let end = count.checked_mul(size).and_then(|n| n.checked_add(20));
        ensure!(
            end.and_then(|n| n.checked_add(strings)) == Some(bytes.len()),
            "invalid DBC length"
        );
        let end = end.unwrap();
        let mut rows = BTreeMap::new();
        for bytes in bytes[20..end].chunks_exact(size) {
            let row: Vec<_> = bytes
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            ensure!(rows.insert(row[0], row).is_none(), "duplicate DBC ID");
        }
        Ok(Self {
            rows,
            strings: bytes[end..].to_vec(),
        })
    }

    fn value(&self, row: &[u32], field: &Field) -> Result<String> {
        let value = row[field.column];
        Ok(match field.kind {
            Kind::Unsigned => value.to_string(),
            Kind::Signed => (value as i32).to_string(),
            Kind::Wide => (u64::from(value) | (u64::from(row[field.column + 1]) << 32)).to_string(),
            Kind::Float => {
                let value = f32::from_bits(value);
                ensure!(value.is_finite(), "non-finite DBC float");
                if value == 0.0 {
                    "0".into()
                } else {
                    value.to_string()
                }
            }
            Kind::Text => {
                let text = self
                    .strings
                    .get(value as usize..)
                    .context("DBC string offset exceeds block")?;
                let end = text
                    .iter()
                    .position(|b| *b == 0)
                    .context("unterminated DBC string")?;
                std::str::from_utf8(&text[..end])?.into()
            }
        })
    }
}

fn source_value(row: &Row, field: &Field) -> Result<String> {
    Ok(match field.kind {
        Kind::Text => row.optional_text(&field.source)?.unwrap_or_default().into(),
        Kind::Float => {
            let value: f32 = row.number(&field.source)?;
            ensure!(value.is_finite(), "non-finite source float");
            if value == 0.0 {
                "0".into()
            } else {
                value.to_string()
            }
        }
        Kind::Signed => row.number::<i32>(&field.source)?.to_string(),
        Kind::Unsigned => {
            let value: i64 = row.number(&field.source)?;
            if value < 0 {
                u32::from_le_bytes(i32::try_from(value)?.to_le_bytes()).to_string()
            } else {
                value.to_string()
            }
        }
        Kind::Wide => row.number::<u64>(&field.source)?.to_string(),
    })
}

fn fields(names: &str, start: usize, kind: Kind) -> Vec<Field> {
    names
        .split_whitespace()
        .enumerate()
        .map(|(offset, name)| Field {
            source: name.into(),
            column: start + offset,
            kind,
        })
        .collect()
}

fn triple(name: &str, column: usize, kind: Kind) -> Vec<Field> {
    (1..=3)
        .map(|n| Field {
            source: format!("{name}{n}"),
            column: column + n - 1,
            kind,
        })
        .collect()
}

fn spell_fields() -> Vec<Field> {
    use Kind::{Float as F, Signed as I, Text as T, Unsigned as U, Wide as W};
    let mut spell = fields("entry school category castUI dispel mechanic attributes attributesEx attributesEx2 attributesEx3 attributesEx4 stances stancesNot targets targetCreatureType requiresSpellFocus casterAuraState targetAuraState castingTimeIndex recoveryTime categoryRecoveryTime interruptFlags auraInterruptFlags channelInterruptFlags procFlags procChance procCharges maxLevel baseLevel spellLevel durationIndex powerType manaCost manCostPerLevel manaPerSecond manaPerSecondPerLevel rangeIndex", 0, U);
    spell.extend(fields("speed", 37, F));
    spell.extend(fields("modelNextSpell stackAmount totem1 totem2", 38, U));
    for n in 1..=8 {
        spell.extend(fields(&format!("reagent{n}"), 41 + n, I));
        spell.extend(fields(&format!("reagentCount{n}"), 49 + n, U));
    }
    spell.extend(fields(
        "equippedItemClass equippedItemSubClassMask equippedItemInventoryTypeMask",
        58,
        I,
    ));
    for (name, col, kind) in [
        ("effect", 61, U),
        ("effectDieSides", 64, I),
        ("effectBaseDice", 67, U),
        ("effectDicePerLevel", 70, F),
        ("effectRealPointsPerLevel", 73, F),
        ("effectBasePoints", 76, I),
        ("effectMechanic", 79, U),
        ("effectImplicitTargetA", 82, U),
        ("effectImplicitTargetB", 85, U),
        ("effectRadiusIndex", 88, U),
        ("effectApplyAuraName", 91, U),
        ("effectAmplitude", 94, U),
        ("effectMultipleValue", 97, F),
        ("effectChainTarget", 100, U),
        ("effectItemType", 103, U),
        ("effectMiscValue", 106, I),
        ("effectTriggerSpell", 109, U),
        ("effectPointsPerComboPoint", 112, F),
        ("dmgMultiplier", 167, F),
    ] {
        spell.extend(triple(name, col, kind));
    }
    spell.extend(fields(
        "spellVisual1 spellVisual2 spellIconId activeIconId spellPriority",
        115,
        U,
    ));
    for (name, col) in [
        ("name", 120),
        ("nameSubtext", 129),
        ("description", 138),
        ("auraDescription", 147),
    ] {
        spell.extend(fields(name, col, T));
        spell.extend(fields(&format!("{name}Flags"), col + 8, U));
    }
    spell.extend(fields(
        "manaCostPercentage startRecoveryCategory startRecoveryTime maxTargetLevel spellFamilyName",
        156,
        U,
    ));
    spell.extend(fields("spellFamilyFlags", 161, W));
    spell.extend(fields(
        "maxAffectedTargets dmgClass preventionType stanceBarOrder",
        163,
        U,
    ));
    spell.extend(fields(
        "minFactionId minReputation requiredAuraVision",
        170,
        U,
    ));
    spell
}

fn comparisons() -> Vec<Comparison> {
    use Kind::{Float as F, Signed as I, Text as T, Unsigned as U};
    let spell = spell_fields();
    let mut faction = fields("id", 0, U);
    faction.extend(fields("reputation_list_id", 1, I));
    for (name, col, kind) in [
        ("base_rep_race_mask", 2, U),
        ("base_rep_class_mask", 6, U),
        ("base_rep_value", 10, I),
        ("reputation_flags", 14, U),
    ] {
        for n in 1..=4 {
            faction.extend(fields(&format!("{name}{n}"), col + n - 1, kind));
        }
    }
    faction.extend(fields("team", 18, U));
    faction.extend(fields("name", 19, T));
    faction.extend(fields("description", 28, T));
    vec![
        Comparison {source:"spell_template",client:"Spell",width:173,fields:spell,source_only:"build effectBonusCoefficient1 effectBonusCoefficient2 effectBonusCoefficient3 minTargetLevel customFlags script_name"},
        Comparison {source:"faction",client:"Faction",width:37,fields:faction,source_only:"build"},
        Comparison {source:"faction_template",client:"FactionTemplate",width:14,fields:fields("id faction_id faction_flags our_mask friendly_mask hostile_mask enemy_faction1 enemy_faction2 enemy_faction3 enemy_faction4 friend_faction1 friend_faction2 friend_faction3 friend_faction4",0,U),source_only:"build"},
        Comparison {source:"area_template",client:"AreaTable",width:25,fields:[fields("entry map_id zone_id explore_flag flags",0,U),fields("area_level",10,I),fields("name",11,T),fields("team liquid_type",20,U)].concat(),source_only:""},
        Comparison {source:"map_template",client:"Map",width:42,fields:[fields("entry",0,U),fields("map_type",2,U),fields("map_name",4,T),fields("player_limit",15,U),fields("linked_zone",19,U)].concat(),source_only:"patch parent reset_delay ghost_entrance_map ghost_entrance_x ghost_entrance_y script_name"},
        Comparison {source:"taxi_nodes",client:"TaxiNodes",width:16,fields:[fields("id map_id",0,U),fields("x y z",2,F),fields("name",5,T),fields("mount_creature_id1 mount_creature_id2",14,U)].concat(),source_only:"build"},
        Comparison {source:"skill_line_ability",client:"SkillLineAbility",width:15,fields:[fields("id skill_id spell_id race_mask class_mask",0,U),fields("req_skill_value superseded_by_spell learn_on_get_skill max_value min_value",7,U),fields("req_train_points",14,U)].concat(),source_only:"build"},
        Comparison {source:"areatrigger_template",client:"AreaTrigger",width:10,fields:[fields("id map_id",0,U),fields("x y z radius box_x box_y box_z box_orientation",2,F)].concat(),source_only:"build name cooldown condition_id script_id script_name"},
    ]
}

fn quoted(value: &str) -> String {
    let text: String = value.chars().take(100).collect();
    format!(
        "`{}`",
        text.replace('`', "'")
            .replace('|', "\\|")
            .replace(['\n', '\r'], " ")
    )
}

pub(crate) fn run(source: &Path, client: &Path, sha: &str) -> Result<String> {
    let source = WorldSource::open(source)?;
    let mut chain = crate::dbc::open_chain(client)?;
    let mut output = format!("# DBC comparison\n\nGenerated by `lyracore-importer --dump <sqlite> --dbc <client Data> --dbc-diff --source-sha {sha}`. Source selection uses patch 10 and build 5875; skill_line_ability uses exactly build 5875. Fields below use zero-based client column offsets. Float values compare after conversion to f32. Text compares the first client locale. Other locales and client-only columns have no source counterpart. Row and cell differences count matched IDs only; added and removed rows are separate. Examples show client → vmangos, at most one per field.\n\nField layouts follow the existing build-5875 reader and [DBCStructure.h](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Database/DBCStructure.h). Source effectItemType fields can exceed 32 bits, as declared in [SpellEntry.h](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Spells/SpellEntry.h#L656); those values stay intact in this comparison.\n\nMap gameplay fields use map_template. Client column 1, internal_name, supplies only the WDT/ADT archive directory, joined by map id.\n");
    for spec in comparisons() {
        let bytes = chain.read_file(&format!("DBFilesClient\\{}.dbc", spec.client))?;
        let client = Records::read(&bytes, spec.width).with_context(|| spec.client)?;
        let rows = source.rows(spec.source)?;
        output.push_str(&compare(&spec, &client, &rows)?);
    }
    Ok(output)
}

fn compare(spec: &Comparison, client: &Records, rows: &[Row]) -> Result<String> {
    let mut output = String::new();
    let key = &spec.fields[0].source;
    let source_rows = rows
        .iter()
        .map(|row| Ok((row.number::<u32>(key)?, row)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let client_ids: BTreeSet<_> = client.rows.keys().copied().collect();
    let source_ids: BTreeSet<_> = source_rows.keys().copied().collect();
    let shared: Vec<_> = client_ids.intersection(&source_ids).copied().collect();
    let mut differing = BTreeSet::new();
    let mut nonlocalized = BTreeSet::new();
    let mut cells = 0;
    let mut field_rows = String::new();
    let mut used = BTreeSet::new();
    for field in &spec.fields {
        used.insert(field.column);
        if matches!(field.kind, Kind::Wide) {
            used.insert(field.column + 1);
        }
        let mut count = 0;
        let mut example = String::new();
        for id in &shared {
            let before = client.value(&client.rows[id], field)?;
            let after = source_value(source_rows[id], field)
                .with_context(|| format!("{} {id}, {}", spec.source, field.source))?;
            if before != after {
                count += 1;
                differing.insert(*id);
                if !matches!(field.kind, Kind::Text)
                    && ![
                        "nameFlags",
                        "nameSubtextFlags",
                        "descriptionFlags",
                        "auraDescriptionFlags",
                    ]
                    .contains(&field.source.as_str())
                {
                    nonlocalized.insert(*id);
                }
                if example.is_empty() {
                    example = format!("{id}: {} → {}", quoted(&before), quoted(&after));
                }
            }
        }
        cells += count;
        writeln!(
            field_rows,
            "| `{}` | {} | {count} | {example} |",
            field.source, field.column
        )?;
    }
    let added: Vec<_> = source_ids.difference(&client_ids).collect();
    let removed: Vec<_> = client_ids.difference(&source_ids).collect();
    writeln!(output,"\n## {} vs {}.dbc\n\nClient rows: {}. Selected source rows: {}. Matched IDs: {}. Differing rows: {}. Differing cells: {cells}.\n\nAdded IDs: {}, examples {:?}. Removed IDs: {}, examples {:?}.\n\nSource-only fields: `{}`. Client columns without a source mapping: {:?}.\n\n| Source field | Client column | Differing rows | Example |\n| --- | ---: | ---: | --- |\n{field_rows}",spec.source,spec.client,client.rows.len(),rows.len(),shared.len(),differing.len(),added.len(),&added[..added.len().min(8)],removed.len(),&removed[..removed.len().min(8)],spec.source_only,(0..spec.width).filter(|c|!used.contains(c)).collect::<Vec<_>>())?;
    writeln!(
        output,
        "\nDiffering rows excluding localized text and locale flags: {}.",
        nonlocalized.len()
    )?;
    if !spec.source_only.is_empty() {
        writeln!(
            output,
            "\n| Source-only field | Rows with a nonzero or nonempty value |\n| --- | ---: |"
        )?;
        for name in spec.source_only.split_whitespace() {
            let mut populated = 0;
            for row in rows {
                let value = row.raw(name)?;
                if value != "NULL" && !value.is_empty() && value.parse::<f64>() != Ok(0.0) {
                    populated += 1;
                }
            }
            writeln!(output, "| `{name}` | {populated} |")?;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::{Cell, TableBuilder};

    #[test]
    fn report_distinguishes_changed_cells_from_added_and_removed_ids() {
        let client = Records {
            rows: BTreeMap::from([(1, vec![1, 2]), (2, vec![2, 3])]),
            strings: vec![],
        };
        let mut source = TableBuilder::new("sample", ["id", "value", "extra"].map(String::from));
        for values in [[1, 4, 0], [3, 3, 1]] {
            source
                .push(
                    values
                        .into_iter()
                        .map(|n| Cell::Number(n.to_string().into()))
                        .collect(),
                )
                .unwrap();
        }
        let spec = Comparison {
            source: "sample",
            client: "Sample",
            width: 2,
            fields: fields("id value", 0, Kind::Unsigned),
            source_only: "extra",
        };
        let report = compare(&spec, &client, &source.finish()).unwrap();
        assert!(report.contains("Differing rows: 1. Differing cells: 1."));
        assert!(report.contains("Added IDs: 1, examples [3]. Removed IDs: 1, examples [2]."));
        assert!(report.contains("| `value` | 1 | 1 | 1: `2` → `4` |"));
        assert!(report.contains("| `extra` | 1 |"));
    }

    #[test]
    fn dbc_values_decode_signed_numbers_floats_and_strings() {
        let mut bytes = b"WDBC".to_vec();
        for value in [1u32, 4, 16, 6, 7, u32::MAX, 1.5f32.to_bits(), 1] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(b"\0name\0");
        let records = Records::read(&bytes, 4).unwrap();
        for (column, kind, expected) in [
            (1, Kind::Signed, "-1"),
            (2, Kind::Float, "1.5"),
            (3, Kind::Text, "name"),
        ] {
            let field = Field {
                source: "x".into(),
                column,
                kind,
            };
            assert_eq!(records.value(&records.rows[&7], &field).unwrap(), expected);
        }
        bytes.pop();
        assert_eq!(
            Records::read(&bytes, 4)
                .err()
                .expect("input must be refused")
                .to_string(),
            "invalid DBC length"
        );
    }
}
