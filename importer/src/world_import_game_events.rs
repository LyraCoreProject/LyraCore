// vmangos `GameEventMgr::LoadFromDB` for spawns, creature data, quests and vendor items; read
// docs/schema.md#game-events first.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};
use lyracore_shared::game_event::DARKMOON_FAIRE_EVENTS;

use super::{
    collect_gameobject_spawns, gameobject_pools, go_guid, world_guid, DumpPlan, WorldImportArgs,
};
use crate::creature_population;
use crate::dump::{insert_row_count, parse_table, push_insert, sql_str, TableRows};

pub(crate) const FAMILY: &str = "events";

const DAY_SECS: i64 = 86_400;

// vmangos `CONDITION_ACTIVE_GAME_EVENT`, whose `value1` is the event.
const CONDITION_ACTIVE_GAME_EVENT: i32 = 12;

/// Where a world GameObject placement went in the gameobjects family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    Static,
    Pooled {
        root: u64,
    },
    /// In a pool that never spawns, or outside the World Import Scope.
    Absent,
}

/// The import's Game Event Links: spawn links and the top-level pools events gate.
#[derive(Debug, Default)]
struct Links {
    /// `(spawn guid, event entry, while inactive)`.
    spawns: Vec<(u64, u32, bool)>,
    pools: BTreeMap<u64, u32>,
    absent: usize,
    pooled_absences: usize,
}

impl Links {
    fn add(&mut self, spawn_guid: u64, event: i32) {
        self.spawns
            .push((spawn_guid, event.unsigned_abs(), event < 0));
    }
}

/// Resolves `game_event_gameobject` rows against where their placements went. A positive event on
/// a pooled placement gates its whole top-level pool, the first such event winning, as vmangos
/// `GameEventMgr::LoadFromDB` does.
fn resolve_object_links(
    links: &mut Links,
    rows: impl IntoIterator<Item = (u32, i32)>,
    placement: impl Fn(u32) -> Placement,
) {
    for (guid, event) in rows {
        match placement(guid) {
            Placement::Static => links.add(go_guid(guid), event),
            Placement::Pooled { root } if event > 0 => {
                links.pools.entry(root).or_insert(event.unsigned_abs());
            }
            Placement::Pooled { .. } => links.pooled_absences += 1,
            Placement::Absent => links.absent += 1,
        }
    }
}

/// Unix seconds of a vmangos `YYYY-MM-DD HH:MM:SS` time on the Realm Clock, which is UTC. The
/// zero date reads as 0, as MySQL `UNIX_TIMESTAMP` gives it.
fn source_time_secs(text: &str) -> Result<i64> {
    calendar_secs(text).with_context(|| format!("game_event: invalid time {text:?}"))
}

fn calendar_secs(text: &str) -> Option<i64> {
    let fields = |part: &str, separator: char| -> Option<Vec<i64>> {
        part.split(separator)
            .map(|field| field.parse().ok())
            .collect()
    };
    let (date, time) = text.trim().split_once(' ')?;
    let [year, month, day] = <[i64; 3]>::try_from(fields(date, '-')?).ok()?;
    let [hour, minute, second] = <[i64; 3]>::try_from(fields(time, ':')?).ok()?;
    if [year, month, day, hour, minute, second] == [0; 6] {
        return Some(0);
    }
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }
    let (month, day) = (u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    if !(1..=12).contains(&month) {
        return None;
    }
    let days = lyracore_shared::calendar::days_from_civil(year, month, day);
    (lyracore_shared::calendar::civil_from_days(days) == (year, month, day))
        .then_some(days * DAY_SECS + hour * 3_600 + minute * 60 + second)
}

/// `game_event` rows by entry. Entry 0 is reserved in vmangos and skipped.
fn definitions(dump: &(impl TableRows + ?Sized)) -> Result<BTreeMap<u32, String>> {
    let mut rows = BTreeMap::new();
    for row in parse_table(dump, "game_event")?.iter() {
        let entry: u32 = row.number("entry")?;
        if entry == 0 {
            continue;
        }
        let start = source_time_secs(row.text("start_time")?)?;
        let end = source_time_secs(row.text("end_time")?)?;
        let occurrence: u32 = row.number("occurence")?;
        let length: u32 = row.number("length")?;
        let scripted = row.number::<u8>("hardcoded")? != 0;
        let disabled = row.number::<u8>("disabled")? != 0;
        let description = row.optional_text("description")?.unwrap_or_default();
        let definition = format!(
            "({entry},{start},{end},{occurrence},{length},{scripted},{disabled},{})",
            sql_str(description.trim())
        );
        if rows.insert(entry, definition).is_some() {
            bail!("game_event {entry}: duplicate entry");
        }
    }
    Ok(rows)
}

fn signed_links(dump: &(impl TableRows + ?Sized), table: &str) -> Result<Vec<(u32, i32)>> {
    let mut links = Vec::new();
    for row in parse_table(dump, table)?.iter() {
        let guid: u32 = row.number("guid")?;
        let event: i32 = row.number("event")?;
        if event == 0 {
            bail!("{table} {guid}: event zero");
        }
        links.push((guid, event));
    }
    Ok(links)
}

/// `game_event_quest` as quest to event. Event 0 is reserved in vmangos and skipped; of a quest's
/// several events the first is kept.
fn event_quests(rows: impl IntoIterator<Item = (u32, u32)>) -> BTreeMap<u32, u32> {
    let mut quests = BTreeMap::new();
    for (quest, event) in rows.into_iter().filter(|&(_, event)| event != 0) {
        quests.entry(quest).or_insert(event);
    }
    quests
}

/// The events of conditions that only ask for a running event, by condition entry.
pub(super) fn active_event_conditions(
    dump: &(impl TableRows + ?Sized),
) -> Result<BTreeMap<u32, u32>> {
    let mut events = BTreeMap::new();
    for row in parse_table(dump, "conditions")?.iter() {
        let event: i64 = row.number("value1")?;
        if row.number::<i32>("type")? == CONDITION_ACTIVE_GAME_EVENT
            && row.number::<u8>("flags")? == 0
        {
            if let Ok(event @ 1..) = u32::try_from(event) {
                events.insert(row.number("condition_entry")?, event);
            }
        }
    }
    Ok(events)
}

/// Each vendor item's event, from `(creature, item, condition)` offers in import order. The first
/// offer of an item to a creature is the one the items family keeps, so it alone decides.
fn event_vendor_items(
    offers: impl IntoIterator<Item = (u32, u32, u32)>,
    conditions: &BTreeMap<u32, u32>,
) -> BTreeMap<(u32, u32), u32> {
    let mut first: BTreeMap<(u32, u32), Option<u32>> = BTreeMap::new();
    for (creature, item, condition) in offers {
        first
            .entry((creature, item))
            .or_insert_with(|| conditions.get(&condition).copied());
    }
    first
        .into_iter()
        .filter_map(|(offer, event)| Some((offer, event?)))
        .collect()
}

/// `npc_vendor` offers, then `npc_vendor_template` offers for each creature using the template.
/// An offer with a stock limit but no restock time, or the reverse, is left out, as the items
/// family leaves it out before it keeps the first offer of an item.
fn vendor_offers(dump: &(impl TableRows + ?Sized)) -> Result<Vec<(u32, u32, u32)>> {
    let offer = |row: &crate::world_source::Row| -> Result<Option<(u32, u32, u32)>> {
        let limited = row.number::<u32>("maxcount")? != 0;
        let restocks = row.number::<u32>("incrtime")? != 0;
        Ok((limited == restocks).then_some((
            row.number("entry")?,
            row.number("item")?,
            row.number("condition_id")?,
        )))
    };
    let mut offers = Vec::new();
    for row in parse_table(dump, "npc_vendor")?.iter() {
        offers.extend(offer(row)?);
    }
    let mut template_users: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for row in dump
        .columns("creature_template", &["entry", "vendor_id"])?
        .iter()
    {
        let vendor: u32 = row.number("vendor_id")?;
        if vendor != 0 {
            template_users
                .entry(vendor)
                .or_default()
                .push(row.number("entry")?);
        }
    }
    for row in parse_table(dump, "npc_vendor_template")?.iter() {
        let Some((template, item, condition)) = offer(row)? else {
            continue;
        };
        for &creature in template_users.get(&template).into_iter().flatten() {
            offers.push((creature, item, condition));
        }
    }
    Ok(offers)
}

/// A `game_event_creature_data` row after patch selection.
#[derive(Clone, Copy, Debug)]
struct SourceCreatureData {
    guid: u32,
    event: u32,
    entry: u32,
    display: u32,
    equipment: u32,
    start_spell: u32,
    end_spell: u32,
}

/// What the World Source holds that a creature data row names.
struct CreatureDataSources<'a> {
    /// Each kept creature placement's spawn guid, by source guid.
    spawns: &'a BTreeMap<u32, u64>,
    /// `creature_equip_template` item choices by equipment entry.
    equipment: &'a BTreeMap<u32, Vec<[u32; 3]>>,
    creature_templates: &'a BTreeSet<u32>,
    spells: &'a BTreeSet<u32>,
}

/// A `game_event_creature_data` row as the Module applies it.
#[derive(Debug, PartialEq)]
struct CreatureData {
    spawn_guid: u64,
    event: u32,
    entry: u32,
    equipment: [u32; 3],
    start_spell: u32,
    end_spell: u32,
}

#[derive(Debug, Default)]
struct CreatureDataPlan {
    rows: Vec<CreatureData>,
    /// Rows whose placement the import leaves out.
    absent: usize,
    /// Source guids of rows that name a model, which the Module does not apply.
    displays: Vec<u32>,
    /// Equipment entries with several item choices, of which the Module shows none.
    equipment_choices: BTreeSet<u32>,
}

/// vmangos `GameEventMgr::LoadFromDB` for `game_event_creature_data`: event 0 is reserved, and an
/// equipment, entry or spell the World Source lacks becomes none. A row left with nothing to change
/// is dropped. Rows keep source guid then event order, the order the Module tries them in.
fn creature_data(
    rows: impl IntoIterator<Item = SourceCreatureData>,
    sources: &CreatureDataSources,
) -> CreatureDataPlan {
    let mut rows: Vec<SourceCreatureData> = rows.into_iter().filter(|row| row.event != 0).collect();
    rows.sort_by_key(|row| (row.guid, row.event));
    let mut plan = CreatureDataPlan::default();
    for row in rows {
        let Some(&spawn_guid) = sources.spawns.get(&row.guid) else {
            plan.absent += 1;
            continue;
        };
        if row.display != 0 {
            plan.displays.push(row.guid);
        }
        let equipment = match sources.equipment.get(&row.equipment).map(Vec::as_slice) {
            Some([items]) => *items,
            Some([]) | None => [0; 3],
            Some(_) => {
                plan.equipment_choices.insert(row.equipment);
                [0; 3]
            }
        };
        let known = |spell: u32| {
            if sources.spells.contains(&spell) {
                spell
            } else {
                0
            }
        };
        let data = CreatureData {
            spawn_guid,
            event: row.event,
            entry: if sources.creature_templates.contains(&row.entry) {
                row.entry
            } else {
                0
            },
            equipment,
            start_spell: known(row.start_spell),
            end_spell: known(row.end_spell),
        };
        if data.entry != 0
            || data.equipment != [0; 3]
            || data.start_spell != 0
            || data.end_spell != 0
        {
            plan.rows.push(data);
        }
    }
    plan
}

fn source_creature_data(dump: &(impl TableRows + ?Sized)) -> Result<Vec<SourceCreatureData>> {
    parse_table(dump, "game_event_creature_data")?
        .iter()
        .map(|row| {
            Ok(SourceCreatureData {
                guid: row.number("guid")?,
                event: row.number("event")?,
                entry: row.number("entry_id")?,
                display: row.number("display_id")?,
                equipment: row.number("equipment_id")?,
                start_spell: row.number("spell_start")?,
                end_spell: row.number("spell_end")?,
            })
        })
        .collect()
}

fn equipment_choices(dump: &(impl TableRows + ?Sized)) -> Result<BTreeMap<u32, Vec<[u32; 3]>>> {
    let mut choices: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    for row in parse_table(dump, "creature_equip_template")?.iter() {
        choices.entry(row.number("entry")?).or_default().push([
            row.number("item1")?,
            row.number("item2")?,
            row.number("item3")?,
        ]);
    }
    Ok(choices)
}

fn entries(dump: &(impl TableRows + ?Sized), table: &str) -> Result<BTreeSet<u32>> {
    dump.columns(table, &["entry"])?
        .iter()
        .map(|row| row.number("entry"))
        .collect()
}

/// The scripted events whose script the Module lacks, for the `events.scripted` finding.
fn unscripted_events(scripted: Vec<u32>) -> Vec<u32> {
    scripted
        .into_iter()
        .filter(|entry| !DARKMOON_FAIRE_EVENTS.contains(entry))
        .collect()
}

pub(super) fn plan(dump: &(impl TableRows + ?Sized), args: &WorldImportArgs) -> Result<DumpPlan> {
    let mut definitions = definitions(dump)?;
    let creatures = creature_population::collect(dump, &args.scope)?;
    let objects = collect_gameobject_spawns(dump, &args.scope)?;
    let pools = gameobject_pools::collect(dump, &objects.placements)?;
    let kept: BTreeSet<u32> = objects.placements.iter().map(|p| p.db_guid).collect();

    let mut links = Links::default();
    for &(spawn_guid, event) in &creatures.event_links {
        links.add(spawn_guid, event);
    }
    resolve_object_links(
        &mut links,
        signed_links(dump, "game_event_gameobject")?,
        |guid| match (kept.contains(&guid), pools.owns(guid), pools.root_of(guid)) {
            (false, _, _) | (true, true, None) => Placement::Absent,
            (true, true, Some(root)) => Placement::Pooled { root },
            (true, false, _) => Placement::Static,
        },
    );

    let quests = event_quests(
        parse_table(dump, "game_event_quest")?
            .iter()
            .map(|row| Ok((row.number("quest")?, row.number("event")?)))
            .collect::<Result<Vec<_>>>()?,
    );
    let vendor_items = event_vendor_items(vendor_offers(dump)?, &active_event_conditions(dump)?);
    let event_creatures = creature_data(
        source_creature_data(dump)?,
        &CreatureDataSources {
            spawns: &creatures
                .spawns
                .iter()
                .map(|spawn| (spawn.0 as u32, world_guid(spawn.1, spawn.0)))
                .collect(),
            equipment: &equipment_choices(dump)?,
            creature_templates: &entries(dump, "creature_template")?,
            spells: &entries(dump, "spell_template")?,
        },
    );

    // An event the World Source leaves out for patch 1.12 is disabled, as vmangos disables it, so
    // its spawns stay hidden and its quests and vendor items are not offered.
    let linked = links
        .spawns
        .iter()
        .map(|&(_, event, _)| event)
        .chain(links.pools.values().copied())
        .chain(quests.values().copied())
        .chain(vendor_items.values().copied())
        .chain(event_creatures.rows.iter().map(|data| data.event));
    for event in linked {
        definitions
            .entry(event)
            .or_insert_with(|| format!("({event},0,0,0,0,false,true,'')"));
    }

    let mut stmts = vec![
        "DELETE FROM game_event_status WHERE event_entry >= 0".to_string(),
        "DELETE FROM game_event_link WHERE link_id >= 0".to_string(),
        "DELETE FROM game_event_pool WHERE pool_id >= 0".to_string(),
        "DELETE FROM game_event_quest WHERE quest_entry >= 0".to_string(),
        "DELETE FROM game_event_vendor_item WHERE id >= 0".to_string(),
        "DELETE FROM game_event_creature_data WHERE data_id >= 0".to_string(),
        "DELETE FROM game_event WHERE event_entry >= 0".to_string(),
    ];
    push_insert(
        &mut stmts,
        "game_event",
        "event_entry,start_secs,end_secs,occurrence_mins,length_mins,scripted,disabled,description",
        &definitions.into_values().collect::<Vec<_>>(),
    );
    let link_rows: Vec<String> = links
        .spawns
        .iter()
        .zip(1u64..)
        .map(|(&(spawn_guid, event, while_inactive), link_id)| {
            format!("({link_id},{spawn_guid},{event},{while_inactive})")
        })
        .collect();
    push_insert(
        &mut stmts,
        "game_event_link",
        "link_id,spawn_guid,event_entry,while_inactive",
        &link_rows,
    );
    let pool_rows: Vec<String> = links
        .pools
        .iter()
        .map(|(pool, event)| format!("({pool},{event})"))
        .collect();
    push_insert(
        &mut stmts,
        "game_event_pool",
        "pool_id,event_entry",
        &pool_rows,
    );
    let quest_rows: Vec<String> = quests
        .iter()
        .map(|(quest, event)| format!("({quest},{event})"))
        .collect();
    push_insert(
        &mut stmts,
        "game_event_quest",
        "quest_entry,event_entry",
        &quest_rows,
    );
    let vendor_rows: Vec<String> = vendor_items
        .iter()
        .zip(1u64..)
        .map(|(((creature, item), event), id)| format!("({id},{creature},{item},{event})"))
        .collect();
    push_insert(
        &mut stmts,
        "game_event_vendor_item",
        "id,creature_entry,item_entry,event_entry",
        &vendor_rows,
    );
    let creature_data_rows: Vec<String> = event_creatures
        .rows
        .iter()
        .zip(1u64..)
        .map(|(data, data_id)| {
            let [main_hand, off_hand, ranged] = data.equipment;
            format!(
                "({data_id},{},{},{},{main_hand},{off_hand},{ranged},{},{})",
                data.spawn_guid, data.event, data.entry, data.start_spell, data.end_spell
            )
        })
        .collect();
    push_insert(
        &mut stmts,
        "game_event_creature_data",
        "data_id,spawn_guid,event_entry,creature_entry,main_hand,off_hand,ranged,start_spell,end_spell",
        &creature_data_rows,
    );
    // Each statement commits alone, so a pass that ran part way through the import settled on a
    // partial link set. Dropping the statuses last makes the next pass walk every link again.
    stmts.push("DELETE FROM game_event_status WHERE event_entry >= 0".to_string());
    eprintln!(
        "game events: {} links, {} event pools, {} event quests, {} event vendor items and {} \
         creature data rows; {} links and {} creature data rows name a spawn the import leaves out, \
         {} negative links on pooled placements are unsupported",
        link_rows.len(),
        pool_rows.len(),
        quest_rows.len(),
        vendor_rows.len(),
        creature_data_rows.len(),
        links.absent,
        event_creatures.absent,
        links.pooled_absences
    );

    let scripted = unscripted_events(
        parse_table(dump, "game_event")?
            .iter()
            .filter(|row| row.number::<u8>("hardcoded").is_ok_and(|flag| flag != 0))
            .map(|row| row.number("entry"))
            .collect::<Result<Vec<u32>>>()?,
    );
    let manifest = serde_json::json!({
        "format_version": 1,
        "source": "vmangos",
        "family": FAMILY,
        "patch": crate::source_selection::PATCH,
        "client_build": crate::source_selection::CLIENT_BUILD,
        "findings": [
            {"code": "events.scripted", "classification": "unsupported", "occurrences": scripted.len(), "source_ids": scripted},
            {"code": "events.pooled-absence", "classification": "unsupported", "occurrences": links.pooled_absences},
            {"code": "events.creature-display", "classification": "unsupported", "occurrences": event_creatures.displays.len(), "source_ids": event_creatures.displays},
            {"code": "events.equipment-choice", "classification": "unsupported", "occurrences": event_creatures.equipment_choices.len(), "source_ids": event_creatures.equipment_choices},
        ],
    });
    Ok(DumpPlan {
        stamps: vec![(FAMILY, insert_row_count(&stmts))],
        stmts,
        world_source_manifest: Some(serde_json::to_string_pretty(&manifest)?),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(rows: &[(u32, i32)], placement: impl Fn(u32) -> Placement) -> Links {
        let mut links = Links::default();
        resolve_object_links(&mut links, rows.iter().copied(), placement);
        links
    }

    #[test]
    fn a_negative_event_links_its_spawn_to_show_while_the_event_does_not_run() {
        let links = resolved(&[(5, 7), (6, -27)], |_| Placement::Static);
        assert_eq!(
            links.spawns,
            [(go_guid(5), 7, false), (go_guid(6), 27, true)]
        );
    }

    #[test]
    fn a_link_to_a_placement_the_import_leaves_out_is_dropped() {
        let links = resolved(&[(5, 7)], |_| Placement::Absent);
        assert!(links.spawns.is_empty());
        assert_eq!(links.absent, 1);
    }

    #[test]
    fn a_pooled_placements_event_gates_its_top_level_pool_and_the_first_event_wins() {
        let links = resolved(&[(5, 7), (6, 11), (8, 12)], |guid| match guid {
            5 | 6 => Placement::Pooled { root: 40 },
            _ => Placement::Pooled { root: 41 },
        });
        assert!(links.spawns.is_empty());
        assert_eq!(links.pools, BTreeMap::from([(40, 7), (41, 12)]));
    }

    #[test]
    fn a_negative_event_on_a_pooled_placement_is_counted_and_not_linked() {
        let links = resolved(&[(5, -7)], |_| Placement::Pooled { root: 40 });
        assert!(links.pools.is_empty());
        assert_eq!(links.pooled_absences, 1);
    }

    #[test]
    fn a_quest_keeps_its_first_event_and_the_reserved_event_is_skipped() {
        assert_eq!(
            event_quests([(8, 10), (9, 0), (8, 12), (11, 12)]),
            BTreeMap::from([(8, 10), (11, 12)])
        );
    }

    #[test]
    fn a_vendor_item_takes_the_event_of_its_first_offer_only() {
        let conditions = BTreeMap::from([(440, 8)]);
        let items = event_vendor_items(
            [
                (295, 21_813, 440),
                (295, 117, 0),
                (6_736, 117, 0),
                (6_736, 117, 440),
                (7_733, 21_815, 73),
            ],
            &conditions,
        );
        assert_eq!(items, BTreeMap::from([((295, 21_813), 8)]));
    }

    fn source_row(
        guid: u32,
        event: u32,
        entry: u32,
        equipment: u32,
        start_spell: u32,
    ) -> SourceCreatureData {
        SourceCreatureData {
            guid,
            event,
            entry,
            display: 0,
            equipment,
            start_spell,
            end_spell: 0,
        }
    }

    fn creature_data_of(rows: &[SourceCreatureData]) -> CreatureDataPlan {
        creature_data(
            rows.iter().copied(),
            &CreatureDataSources {
                spawns: &BTreeMap::from([(76, 900), (77, 901)]),
                equipment: &BTreeMap::from([
                    (50_001, vec![[2_715, 143, 0]]),
                    (60, vec![[1, 0, 0], [2, 0, 0]]),
                ]),
                creature_templates: &BTreeSet::from([15_724]),
                spells: &BTreeSet::from([26_115]),
            },
        )
    }

    #[test]
    fn a_creature_data_row_drops_what_the_world_source_lacks_and_a_row_left_empty() {
        let plan = creature_data_of(&[
            source_row(77, 34, 15_724, 0, 26_115),
            source_row(76, 35, 99, 7, 33_014),
            source_row(76, 27, 0, 50_001, 0),
            source_row(76, 0, 15_724, 0, 0),
            source_row(78, 34, 15_724, 0, 26_115),
        ]);
        assert_eq!(
            plan.rows,
            [
                CreatureData {
                    spawn_guid: 900,
                    event: 27,
                    entry: 0,
                    equipment: [2_715, 143, 0],
                    start_spell: 0,
                    end_spell: 0,
                },
                CreatureData {
                    spawn_guid: 901,
                    event: 34,
                    entry: 15_724,
                    equipment: [0; 3],
                    start_spell: 26_115,
                    end_spell: 0,
                },
            ]
        );
        assert_eq!(plan.absent, 1);
    }

    #[test]
    fn a_creature_data_model_and_an_equipment_with_several_choices_are_counted() {
        let plan = creature_data_of(&[SourceCreatureData {
            display: 1_234,
            ..source_row(76, 8, 15_724, 60, 0)
        }]);
        assert_eq!(plan.displays, [76]);
        assert_eq!(plan.equipment_choices, BTreeSet::from([60]));
        assert_eq!(plan.rows[0].equipment, [0; 3]);
    }

    #[test]
    fn the_darkmoon_faire_events_leave_the_scripted_finding() {
        assert_eq!(
            unscripted_events(vec![4, 5, 6, 13, 23, 24, 66]),
            [6, 13, 66]
        );
    }

    #[test]
    fn a_source_time_reads_as_utc_seconds_and_the_zero_date_as_zero() {
        assert_eq!(
            source_time_secs("2020-06-21 20:00:00").unwrap(),
            1_592_769_600
        );
        assert_eq!(
            source_time_secs("2037-12-31 23:59:59").unwrap(),
            2_145_916_799
        );
        assert_eq!(source_time_secs("0000-00-00 00:00:00").unwrap(), 0);
    }

    #[test]
    fn a_source_time_that_is_no_calendar_time_is_refused() {
        for text in [
            "2021-02-29 00:00:00",
            "2021-13-01 00:00:00",
            "2021-01-01 24:00:00",
            "2021-01-01",
            "yesterday",
        ] {
            assert!(source_time_secs(text).is_err(), "{text}");
        }
    }
}
