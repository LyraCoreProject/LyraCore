use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use wow_world_base::vanilla::SpellEffect;

use crate::dump::{push_insert, TableRows};
use crate::world_import_scope::{WorldImportProfile, WorldImportScope};
use crate::world_source::SourceKind;

use super::{
    build_mail_loot, build_quests, collect_gameobject_spawns, creature_drop_item_set,
    gameobject_loot_item_set, loot_findings, push_quest_statements, vendor_stock_item_set,
    DumpPlan, QuestEtl, WorldImportArgs,
};

pub(super) fn learn_spells(dump: &(impl TableRows + ?Sized)) -> Result<HashMap<u32, u32>> {
    let (id, effect, trigger) = if dump.kind() == SourceKind::Vmangos {
        ("entry", "effect", "effectTriggerSpell")
    } else {
        ("Id", "Effect", "EffectTriggerSpell")
    };
    let mut columns = vec![id.to_string()];
    for slot in 1..=3 {
        columns.extend([format!("{effect}{slot}"), format!("{trigger}{slot}")]);
    }
    let mut learned = HashMap::new();
    for row in dump
        .columns(
            "spell_template",
            &columns.iter().map(String::as_str).collect::<Vec<_>>(),
        )?
        .iter()
    {
        let wrapper = row.number::<u32>(id)?;
        let mut effects = Vec::new();
        for slot in 1..=3 {
            let value = row.number::<u32>(&format!("{effect}{slot}"))?;
            if value != 0 {
                effects.push((slot, value));
            }
        }
        if effects.len() != 1 || effects[0].1 != SpellEffect::LearnSpell.as_int() {
            continue;
        }
        let target = row.number::<u32>(&format!("{trigger}{}", effects[0].0))?;
        if target != 0 {
            learned.insert(wrapper, target);
        }
    }
    Ok(learned)
}

fn placed_creatures(dump: &impl TableRows, scope: &WorldImportScope) -> Result<HashSet<u64>> {
    let mut entries = HashSet::new();
    for row in dump.rows("creature")?.iter() {
        if row.number::<u32>("spawn_flags")? & 2 != 0 {
            continue;
        }
        if !scope.contains(
            row.number("map")?,
            row.number("position_x")?,
            row.number("position_y")?,
            row.number("position_z")?,
        ) {
            continue;
        }
        for column in ["id", "id2", "id3", "id4", "id5"] {
            let entry = row.number::<u64>(column)?;
            if entry != 0 {
                entries.insert(entry);
            }
        }
    }
    Ok(entries)
}

fn event_requirements(dump: &impl TableRows, entries: &HashSet<u64>) -> Result<Vec<String>> {
    let explored = dump
        .rows("areatrigger_involvedrelation")?
        .iter()
        .map(|row| row.number::<u64>("quest"))
        .collect::<Result<HashSet<_>>>()?;
    let mut rows = Vec::new();
    for row in dump.rows("quest_template")?.iter() {
        let entry = row.number::<u64>("entry")?;
        if entries.contains(&entry)
            && !explored.contains(&entry)
            && row.number::<u32>("SpecialFlags")? & 2 != 0
        {
            rows.push(format!("({},{entry})", rows.len() + 1));
        }
    }
    Ok(rows)
}

pub(super) fn plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let full = args
        .world_profile
        .is_some_and(WorldImportProfile::imports_all_templates);
    let mut scope = match args.world_profile {
        Some(_) if full => WorldImportScope::canonical(WorldImportProfile::FullWorld)?,
        Some(_) => WorldImportScope::quest_catalogue()?,
        None => args.scope.clone(),
    };
    scope.extend_forced_creatures(&args.include_creatures);
    let templates = dump.columns("creature_template", &["entry", "loot_id"])?;
    let mut entries = if full {
        templates
            .iter()
            .map(|row| row.number("entry"))
            .collect::<Result<_>>()?
    } else {
        placed_creatures(dump, &scope)?
    };
    entries.extend(scope.forced_creature_entries.iter().copied());
    let loot = templates
        .iter()
        .filter_map(|row| {
            let result = (|| -> Result<_> {
                let entry = row.number("entry")?;
                let loot = row.number::<u64>("loot_id")?;
                Ok((entries.contains(&entry) && loot != 0).then_some((entry, loot)))
            })();
            result.transpose()
        })
        .collect::<Result<HashMap<_, _>>>()?;
    let catalogue_go = collect_gameobject_spawns(dump, &scope)?;
    let local_go = collect_gameobject_spawns(dump, &args.scope)?;
    let local_creatures = placed_creatures(dump, &args.scope)?;
    let mut obtainable = creature_drop_item_set(dump, &loot)?;
    obtainable.extend(gameobject_loot_item_set(dump, &catalogue_go.used_go)?);
    obtainable.extend(vendor_stock_item_set(dump, &entries)?);
    let quests = build_quests(
        dump,
        &entries,
        &catalogue_go.used_go,
        &catalogue_go.goober_entries,
        &obtainable,
        Some((&local_creatures, &local_go.used_go)),
    )?;
    let (mail, _) = build_mail_loot(dump, &quests.reward_mail_template_ids)?;
    let mut stmts = Vec::new();
    push_quest_statements(&mut stmts, &quests, &mail);
    let events = event_requirements(dump, &quests.entries)?;
    stmts.push("DELETE FROM game_quest_event_requirement WHERE id > 0".into());
    push_insert(
        &mut stmts,
        "game_quest_event_requirement",
        "id,quest_entry",
        &events,
    );
    Ok(DumpPlan {
        stmts,
        stamps: vec![("quests", quests.templates.len() as u64)],
        world_source_manifest: Some(serde_json::to_string_pretty(&findings(
            dump,
            &quests,
            &obtainable,
            &catalogue_go.goober_entries,
            events.len(),
        )?)?),
        ..Default::default()
    })
}

fn findings(
    dump: &impl TableRows,
    quests: &QuestEtl,
    obtainable: &HashSet<u64>,
    goobers: &HashSet<u64>,
    event_count: usize,
) -> Result<serde_json::Value> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    if event_count != 0 {
        counts.insert("quests.event-credit", event_count);
    }
    let mut add = |code, present| {
        if present {
            *counts.entry(code).or_default() += 1;
        }
    };
    let learned = learn_spells(dump)?;
    for row in dump.rows("quest_template")?.iter() {
        if !quests.entries.contains(&row.number("entry")?) {
            continue;
        }
        for (column, code) in [
            ("RequiredCondition", "quests.condition"),
            ("RequiredSkill", "quests.skill-requirement"),
            ("RequiredMinRepFaction", "quests.reputation-requirement"),
            ("RequiredMaxRepFaction", "quests.maximum-reputation"),
            ("ExclusiveGroup", "quests.exclusive-group"),
            ("BreadcrumbForQuestId", "quests.breadcrumb"),
            ("SrcSpell", "quests.start-spell"),
            ("StartScript", "quests.start-script"),
            ("CompleteScript", "quests.complete-script"),
            ("RewRepSpilloverMask", "quests.reputation-spillover-mask"),
        ] {
            add(code, row.number::<i64>(column)? != 0);
        }
        add(
            "quests.maximum-level",
            (1..60).contains(&row.number::<u32>("MaxLevel")?),
        );
        add(
            "quests.flags",
            row.number::<u32>("QuestFlags")? != 0 || row.number::<u32>("SpecialFlags")? & !3 != 0,
        );
        for slot in 1..=4 {
            let item = row.number::<u64>(&format!("ReqItemId{slot}"))?;
            let count = row.number::<u32>(&format!("ReqItemCount{slot}"))?;
            add(
                "quests.unavailable-collect-objective",
                item != 0
                    && count != 0
                    && !obtainable.contains(&item)
                    && item != row.number::<u64>("SrcItemId")?,
            );
            let target = row.number::<i64>(&format!("ReqCreatureOrGOId{slot}"))?;
            let count = row.number::<u32>(&format!("ReqCreatureOrGOCount{slot}"))?;
            add(
                "quests.non-goober-objective",
                target < 0 && count != 0 && !goobers.contains(&target.unsigned_abs()),
            );
        }
        add("quests.explicit-zero-xp", row.number::<u32>("RewXP")? == 0);
        add(
            "quests.money-requirement",
            row.number::<i64>("RewOrReqMoney")? < 0,
        );
        add(
            "quests.negative-prerequisite",
            row.number::<i64>("PrevQuestId")? < 0,
        );
        add(
            "quests.extra-reputation-rewards",
            (3..=5)
                .map(|slot| row.number::<u32>(&format!("RewRepFaction{slot}")))
                .collect::<Result<Vec<_>>>()?
                .iter()
                .any(|value| *value != 0),
        );
        let cast = row.number::<u32>("RewSpellCast")?;
        let cast = if cast != 0 {
            cast
        } else {
            row.number("RewSpell")?
        };
        add(
            "quests.reward-cast",
            cast != 0 && !learned.contains_key(&cast),
        );
    }
    let mut findings = counts.into_iter().map(|(code, occurrences)| serde_json::json!({"code":code,"classification":"unsupported","occurrences":occurrences})).collect::<Vec<_>>();
    findings.extend(loot_findings(dump, &["mail_loot_template"])?);
    Ok(
        serde_json::json!({"format_version":1,"source":"vmangos","family":"quests","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":findings}),
    )
}

#[cfg(test)]
#[path = "world_import_quests_tests.rs"]
mod tests;
