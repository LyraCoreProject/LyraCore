use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use wow_world_base::vanilla::SpellEffect;

use crate::dump::{qt, TableRows};
use crate::world_import_scope::{WorldImportProfile, WorldImportScope};

use super::{
    build_mail_loot, build_quests, collect_gameobject_spawns, creature_drop_item_set,
    gameobject_loot_item_set, loot_findings, push_quest_statements, vendor_stock_item_set,
    DumpPlan, QuestEtl, WorldImportArgs,
};

pub(super) fn learn_spells(dump: &(impl TableRows + ?Sized)) -> Result<HashMap<u32, u32>> {
    let (id, effect, trigger) = ("entry", "effect", "effectTriggerSpell");
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

#[derive(Clone, Copy)]
struct QuestLink {
    prev: i64,
    next: i64,
    breadcrumb: bool,
}

/// Every source quest's `PrevQuestId`, `NextQuestId` and breadcrumb mark, imported or not.
pub(super) struct QuestLinks(BTreeMap<u64, QuestLink>);

impl QuestLinks {
    pub(super) fn read(rows: &[crate::world_source::Row]) -> Result<Self> {
        rows.iter()
            .map(|row| {
                Ok((
                    row.number("entry")?,
                    QuestLink {
                        prev: row.number(qt::PREV_QUEST_ID)?,
                        next: row.number(qt::NEXT_QUEST_ID)?,
                        breadcrumb: row.number::<u64>(qt::BREADCRUMB_FOR_QUEST_ID)? != 0,
                    },
                ))
            })
            .collect::<Result<_>>()
            .map(Self)
    }

    // vmangos ObjectMgr::LoadQuests drops a PrevQuestId naming no quest or a breadcrumb.
    fn previous(&self, entry: u64) -> Option<i64> {
        let prev = self.0.get(&entry)?.prev;
        self.0
            .get(&prev.unsigned_abs())
            .filter(|target| prev != 0 && !target.breadcrumb)
            .map(|_| prev)
    }

    pub(super) fn rewarded_prerequisite(&self, entry: u64) -> u64 {
        self.previous(entry)
            .filter(|prev| *prev > 0)
            .map_or(0, i64::unsigned_abs)
    }

    /// `game_quest_prerequisite` rows: the `prevQuests` entries `prev_quest_id` does not hold.
    pub(super) fn prerequisite_rows(&self, imported: &HashSet<u64>) -> Vec<String> {
        let mut lists: BTreeMap<u64, Vec<(u64, bool)>> = BTreeMap::new();
        for &entry in self.0.keys().filter(|entry| imported.contains(entry)) {
            if let Some(prev) = self.previous(entry).filter(|prev| *prev < 0) {
                lists
                    .entry(entry)
                    .or_default()
                    .push((prev.unsigned_abs(), true));
            }
        }
        for (&source, link) in &self.0 {
            let target = link.next.unsigned_abs();
            if link.next == 0 || !imported.contains(&target) {
                continue;
            }
            let predecessor = (source, link.next < 0);
            let list = lists.entry(target).or_default();
            if !list.contains(&predecessor)
                && predecessor != (self.rewarded_prerequisite(target), false)
            {
                list.push(predecessor);
            }
        }
        lists
            .into_iter()
            .flat_map(|(quest, list)| {
                list.into_iter()
                    .map(move |(prev, must_be_active)| (quest, prev, must_be_active))
            })
            .enumerate()
            .map(|(index, (quest, prev, must_be_active))| {
                format!("({},{quest},{prev},{must_be_active})", index + 1)
            })
            .collect()
    }
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
        entries.extend(
            crate::creature_population::vmangos_entry_choices(row)?
                .into_iter()
                .map(u64::from),
        );
    }
    Ok(entries)
}

fn event_requirements(
    dump: &impl TableRows,
    entries: &HashSet<u64>,
    credit_sources: &BTreeSet<u32>,
) -> Result<(Vec<String>, Vec<u32>)> {
    let explored = dump
        .rows("areatrigger_involvedrelation")?
        .iter()
        .map(|row| row.number::<u64>("quest"))
        .collect::<Result<HashSet<_>>>()?;
    let mut rows = Vec::new();
    let mut without_source = Vec::new();
    for row in dump.rows("quest_template")?.iter() {
        let entry = row.number::<u64>("entry")?;
        if entries.contains(&entry)
            && !explored.contains(&entry)
            && row.number::<u32>("SpecialFlags")? & 2 != 0
        {
            let entry: u32 = row.number("entry")?;
            // Preserve the two script-only quest gates; see conformance/unsupported-vmangos.md for 2608.
            if credit_sources.contains(&entry) || matches!(entry, 2460 | 2608) {
                rows.push(format!("({},{entry})", rows.len() + 1));
            }
            if !credit_sources.contains(&entry) {
                without_source.push(entry);
            }
        }
    }
    Ok((rows, without_source))
}

struct Catalogue {
    quests: QuestEtl,
    obtainable: HashSet<u64>,
    goober_entries: HashSet<u64>,
}

fn catalogue_scope(args: &WorldImportArgs) -> Result<WorldImportScope> {
    let mut scope = match args.world_profile {
        Some(profile) => WorldImportScope::quest_catalogue(profile)?,
        None => args.scope.clone(),
    };
    scope.extend_forced_creatures(&args.include_creatures);
    Ok(scope)
}

fn catalogue_creatures(
    dump: &impl TableRows,
    args: &WorldImportArgs,
    scope: &WorldImportScope,
) -> Result<HashSet<u64>> {
    let mut entries = if args
        .world_profile
        .is_some_and(WorldImportProfile::imports_all_templates)
    {
        dump.columns("creature_template", &["entry"])?
            .iter()
            .map(|row| row.number("entry"))
            .collect::<Result<_>>()?
    } else {
        placed_creatures(dump, scope)?
    };
    entries.extend(scope.forced_creature_entries.iter().copied());
    Ok(entries)
}

fn catalogue(dump: &impl TableRows, args: &WorldImportArgs) -> Result<Catalogue> {
    let scope = catalogue_scope(args)?;
    let entries = catalogue_creatures(dump, args, &scope)?;
    let templates = dump.columns("creature_template", &["entry", "loot_id"])?;
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
    Ok(Catalogue {
        quests,
        obtainable,
        goober_entries: catalogue_go.goober_entries,
    })
}

pub(super) fn event_requirement_rows(
    dump: &impl TableRows,
    args: &WorldImportArgs,
    credit_sources: &BTreeSet<u32>,
) -> Result<Vec<String>> {
    let scope = catalogue_scope(args)?;
    let creatures = catalogue_creatures(dump, args, &scope)?;
    let gameobjects = collect_gameobject_spawns(dump, &scope)?.used_go;
    let mut entries = HashSet::new();
    for (table, owners) in [
        ("creature_questrelation", &creatures),
        ("creature_involvedrelation", &creatures),
        ("gameobject_questrelation", &gameobjects),
        ("gameobject_involvedrelation", &gameobjects),
    ] {
        entries.extend(
            super::quest_relations(dump, table, owners)?
                .into_iter()
                .map(|(_, quest)| quest),
        );
    }
    Ok(event_requirements(dump, &entries, credit_sources)?.0)
}

pub(super) fn plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let Catalogue {
        quests,
        obtainable,
        goober_entries,
    } = catalogue(dump, args)?;
    let (mail, _) = build_mail_loot(dump, &quests.reward_mail_template_ids)?;
    let mut stmts = Vec::new();
    push_quest_statements(&mut stmts, &quests, &mail);
    let (creature_ai, _) = super::vmangos_creature_ai(dump, args)?;
    let (_, without_source) =
        event_requirements(dump, &quests.entries, &creature_ai.quest_event_entries)?;
    Ok(DumpPlan {
        stmts,
        stamps: vec![(
            lyracore_package_delta::QUEST_FAMILY,
            quests.templates.len() as u64,
        )],
        world_source_manifest: Some(serde_json::to_string_pretty(&findings(
            dump,
            &quests,
            &obtainable,
            &goober_entries,
            &without_source,
        )?)?),
        ..Default::default()
    })
}

fn findings(
    dump: &impl TableRows,
    quests: &QuestEtl,
    obtainable: &HashSet<u64>,
    goobers: &HashSet<u64>,
    without_source: &[u32],
) -> Result<serde_json::Value> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
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
            ("BreadcrumbForQuestId", "quests.breadcrumb"),
            ("SrcSpell", "quests.start-spell"),
            ("StartScript", "quests.start-script"),
            ("CompleteScript", "quests.complete-script"),
            ("RewRepSpilloverMask", "quests.reputation-spillover-mask"),
        ] {
            add(code, row.number::<i64>(column)? != 0);
        }
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
    if !without_source.is_empty() {
        findings.push(serde_json::json!({
            "code": "quests.event-credit-without-source",
            "classification": "unsupported",
            "occurrences": without_source.len(),
            "source_ids": without_source,
        }));
    }
    findings.extend(loot_findings(dump, &["mail_loot_template"])?);
    Ok(
        serde_json::json!({"format_version":1,"source":"vmangos","family":"quests","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":findings}),
    )
}

#[cfg(test)]
#[path = "world_import_quests_tests.rs"]
mod tests;
