use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{bail, Context, Result};

use crate::dump::{push_insert, TableRows};
use crate::world_source::Row;
use lyracore_shared::constants::npc_flags;
use wow_world_base::vanilla::SpellEffect;

use super::{vmangos_creature_entries, DumpPlan, WorldImportArgs};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Offer {
    spell: u32,
    cost: u32,
    level: u8,
    skill: u32,
    rank: u32,
}

fn offers(dump: &impl TableRows, table: &str) -> Result<BTreeMap<u64, BTreeMap<u32, Offer>>> {
    let mut owners: BTreeMap<u64, BTreeMap<u32, Offer>> = BTreeMap::new();
    for row in dump.rows(table)?.iter() {
        let entry = row.number("entry")?;
        let offer = Offer {
            spell: row.number("spell")?,
            cost: row.number("spellcost")?,
            level: row.number("reqlevel")?,
            skill: row.number("reqskill")?,
            rank: row.number("reqskillvalue")?,
        };
        if let Some(previous) = owners
            .entry(entry)
            .or_default()
            .insert(offer.spell, offer.clone())
        {
            anyhow::ensure!(
                previous == offer,
                "{table}: conflicting offers for {entry}/{}",
                offer.spell
            );
        }
    }
    Ok(owners)
}

fn spell_columns() -> Vec<String> {
    let mut columns = vec!["entry".into(), "spellLevel".into()];
    for slot in 1..=3 {
        for prefix in [
            "effect",
            "effectTriggerSpell",
            "effectMiscValue",
            "effectBasePoints",
            "effectDieSides",
            "effectRealPointsPerLevel",
            "effectDicePerLevel",
        ] {
            columns.push(format!("{prefix}{slot}"));
        }
    }
    columns
}

fn skill_grant(spell: &Row) -> Result<Option<(u32, u32)>> {
    let mut grant = None;
    for slot in 1..=3 {
        if spell.number::<u32>(&format!("effect{slot}"))? != SpellEffect::SkillStep.as_int() {
            continue;
        }
        let step = spell
            .number::<i64>(&format!("effectBasePoints{slot}"))?
            .checked_add(1)
            .context("trainer skill step overflow")?;
        if !(1..=4).contains(&step)
            || spell.number::<i32>(&format!("effectDieSides{slot}"))? != 1
            || spell.number::<f64>(&format!("effectRealPointsPerLevel{slot}"))? != 0.0
            || spell.number::<f64>(&format!("effectDicePerLevel{slot}"))? != 0.0
            || grant.is_some()
        {
            bail!("trainer skill grant needs a single constant tier from 1 through 4");
        }
        // vmangos EffectLearnSkill gives each constant step 75 ranks; see docs/data-ingestion.md.
        grant = Some((
            spell.number(&format!("effectMiscValue{slot}"))?,
            step as u32 * 75,
        ));
    }
    Ok(grant)
}

pub(super) fn plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let entries = vmangos_creature_entries(dump, args)?;
    let direct = offers(dump, "npc_trainer")?;
    let shared = offers(dump, "npc_trainer_template")?;
    let columns = spell_columns();
    let spells = dump.columns(
        "spell_template",
        &columns.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    let spells = spells
        .iter()
        .map(|row| Ok((row.number::<u32>("entry")?, row)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut rows = Vec::new();
    let mut learned_skills = BTreeSet::new();
    let mut findings: BTreeMap<&str, HashSet<(u64, u32)>> = BTreeMap::new();
    for creature in dump
        .columns("creature_template", &["entry", "trainer_id", "npc_flags"])?
        .iter()
    {
        let entry = creature.number::<u64>("entry")?;
        if !entries.contains(&entry)
            || creature.number::<u32>("npc_flags")? & npc_flags::TRAINER == 0
        {
            continue;
        }
        let mut selected = direct.get(&entry).cloned().unwrap_or_default();
        if let Some(offers) = shared.get(&creature.number("trainer_id")?) {
            selected.extend(offers.iter().map(|(&spell, offer)| (spell, offer.clone())));
        }
        for offer in selected.values() {
            let key = (entry, offer.spell);
            let Some(spell) = spells.get(&offer.spell) else {
                findings
                    .entry("trainers.missing-spell")
                    .or_default()
                    .insert(key);
                continue;
            };
            if spell.number::<u32>("effect1")? != SpellEffect::LearnSpell.as_int() {
                findings
                    .entry("trainers.non-learning-spell")
                    .or_default()
                    .insert(key);
                continue;
            }
            let level = if offer.level == 0 {
                spell.number("spellLevel")?
            } else {
                offer.level
            };
            let (line, cap) = skill_grant(spell)
                .with_context(|| format!("trainer {entry} spell {}", offer.spell))?
                .unwrap_or((0, 75));
            if offer.skill != 0 || offer.rank != 0 {
                findings
                    .entry("trainers.skill-requirement")
                    .or_default()
                    .insert(key);
            }
            let extra_learn = spell.number::<u32>("effect2")? == SpellEffect::LearnSpell.as_int()
                || spell.number::<u32>("effect3")? == SpellEffect::LearnSpell.as_int();
            if extra_learn {
                findings
                    .entry("trainers.multiple-learn-targets")
                    .or_default()
                    .insert(key);
            }
            if matches!(line, 148 | 149 | 150 | 152 | 533 | 553 | 554 | 713) {
                findings
                    .entry("trainers.racial-riding")
                    .or_default()
                    .insert(key);
            }
            if line != 0 {
                learned_skills.insert(spell.number::<u32>("effectTriggerSpell1")?);
            }
            rows.push(format!(
                "({},{entry},{},{},{level},{line},{cap})",
                rows.len() + 1,
                offer.spell,
                offer.cost
            ));
        }
    }
    let mut stmts = Vec::new();
    statements(&mut stmts, &rows);
    let authored = super::PROFESSION_LEARN
        .iter()
        .flat_map(|(_, tiers)| tiers.iter().map(|(spell, _, _)| *spell))
        .collect::<BTreeSet<_>>();
    let audit = serde_json::json!({"authored_targets":authored.len(),"offered_skill_targets":learned_skills.len(),"added":learned_skills.difference(&authored).collect::<Vec<_>>(),"not_offered":authored.difference(&learned_skills).collect::<Vec<_>>()});
    let findings = findings.into_iter().map(|(code, keys)| serde_json::json!({"code":code,"classification":"unsupported","occurrences":keys.len()})).collect::<Vec<_>>();
    Ok(DumpPlan {
        stmts,
        stamps: vec![("trainers", rows.len() as u64)],
        world_source_manifest: Some(serde_json::to_string_pretty(
            &serde_json::json!({"format_version":1,"source":"vmangos","family":"trainers","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":findings,"profession_audit":audit}),
        )?),
        ..Default::default()
    })
}

pub(super) fn statements(stmts: &mut Vec<String>, rows: &[String]) {
    stmts.push("DELETE FROM game_trainer_spell WHERE id > 0".into());
    push_insert(
        stmts,
        "game_trainer_spell",
        "id,trainer_entry,spell_id,cost,required_level,learn_skill_line,learn_skill_cap",
        rows,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::{sqlite_fixture, FixtureTable};
    use rusqlite::types::Value::Integer as N;

    #[test]
    fn selected_shared_offers_win_and_skill_steps_replace_synthetic_tiers() {
        let mut names = spell_columns();
        names.push("build".into());
        names.reverse();
        let columns = names
            .iter()
            .map(|name| (name.as_str(), "INTEGER"))
            .collect::<Vec<_>>();
        let spell = |entry, build, level, skill, step, extra| {
            let mut row = columns
                .iter()
                .map(|(column, _)| (*column, N(0)))
                .collect::<Vec<_>>();
            for (key, value) in [
                ("entry", entry),
                ("build", build),
                ("spellLevel", level),
                ("effect1", 36),
                ("effectTriggerSpell1", entry + 100),
                ("effect2", if skill != 0 { 44 } else { extra }),
                ("effectMiscValue2", skill),
                ("effectBasePoints2", step),
                ("effectDieSides2", 1),
            ] {
                row.iter_mut().find(|(column, _)| *column == key).unwrap().1 = N(value);
            }
            row
        };
        let offer_columns = &[
            ("entry", "INTEGER"),
            ("spell", "INTEGER"),
            ("spellcost", "INTEGER"),
            ("reqskill", "INTEGER"),
            ("reqskillvalue", "INTEGER"),
            ("reqlevel", "INTEGER"),
            ("build_min", "INTEGER"),
            ("build_max", "INTEGER"),
        ];
        let offer = |entry, spell, cost, level, last| {
            [
                ("entry", N(entry)),
                ("spell", N(spell)),
                ("spellcost", N(cost)),
                ("reqskill", N(186)),
                ("reqskillvalue", N(50)),
                ("reqlevel", N(level)),
                ("build_min", N(0)),
                ("build_max", N(last)),
            ]
        };
        let source = sqlite_fixture(&[
            FixtureTable {
                name: "spell_template",
                columns: &columns,
                key: &["entry", "build"],
                rows: &[
                    &spell(700, 5860, 1, 0, 0, 0),
                    &spell(700, 5875, 10, 0, 0, 0),
                    &spell(700, 6000, 60, 0, 0, 0),
                    &spell(701, 5875, 0, 186, 1, 0),
                    &spell(702, 5875, 0, 0, 0, 36),
                ],
            },
            FixtureTable {
                name: "creature_template",
                columns: &[
                    ("entry", "INTEGER"),
                    ("patch", "INTEGER"),
                    ("trainer_id", "INTEGER"),
                    ("npc_flags", "INTEGER"),
                ],
                key: &["entry", "patch"],
                rows: &[
                    &[
                        ("entry", N(100)),
                        ("patch", N(9)),
                        ("trainer_id", N(6)),
                        ("npc_flags", N(16)),
                    ],
                    &[
                        ("entry", N(100)),
                        ("patch", N(10)),
                        ("trainer_id", N(7)),
                        ("npc_flags", N(16)),
                    ],
                    &[
                        ("entry", N(101)),
                        ("patch", N(10)),
                        ("trainer_id", N(0)),
                        ("npc_flags", N(16)),
                    ],
                    &[
                        ("entry", N(102)),
                        ("patch", N(10)),
                        ("trainer_id", N(7)),
                        ("npc_flags", N(0)),
                    ],
                ],
            },
            FixtureTable {
                name: "npc_trainer",
                columns: offer_columns,
                key: &[],
                rows: &[
                    &offer(100, 700, 99, 1, 5875),
                    &offer(101, 702, 5, 20, 5875),
                    &offer(101, 999, 1, 1, 5874),
                ],
            },
            FixtureTable {
                name: "npc_trainer_template",
                columns: offer_columns,
                key: &[],
                rows: &[&offer(7, 700, 30, 0, 5875), &offer(7, 701, 40, 15, 5875)],
            },
        ])
        .unwrap();
        let args = crate::parse_args_from([
            "--source",
            "vmangos",
            "--dump",
            "fixture.sqlite",
            "--family",
            "trainers",
            "--world-profile",
            "full-world",
        ])
        .unwrap()
        .world_import_args()
        .unwrap();
        let plan = plan(&source, &args).unwrap();
        assert_eq!(plan.stamps, [("trainers", 3)]);
        assert_eq!(plan.stmts[1], "INSERT INTO game_trainer_spell (id,trainer_entry,spell_id,cost,required_level,learn_skill_line,learn_skill_cap) VALUES (1,100,700,30,10,0,75),(2,100,701,40,15,186,150),(3,101,702,5,20,0,75)");
        let manifest: serde_json::Value =
            serde_json::from_str(plan.world_source_manifest.as_ref().unwrap()).unwrap();
        assert_eq!(manifest["profession_audit"]["offered_skill_targets"], 1);
        assert!(
            manifest["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["code"] == "trainers.multiple-learn-targets"
                    && row["occurrences"] == 1)
        );
        assert!(manifest["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["code"] == "trainers.skill-requirement" && row["occurrences"] == 3));
    }
}
