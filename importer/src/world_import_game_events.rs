// vmangos `GameEventMgr::LoadFromDB` for spawns; read docs/schema.md#game-events first.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context, Result};

use super::{collect_gameobject_spawns, gameobject_pools, go_guid, DumpPlan, WorldImportArgs};
use crate::creature_population;
use crate::dump::{insert_row_count, parse_table, push_insert, sql_str, TableRows};

pub(crate) const FAMILY: &str = "events";

const DAY_SECS: i64 = 86_400;

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

    // An event the World Source leaves out for patch 1.12 is disabled, as vmangos disables it, so
    // its spawns stay hidden.
    let linked = links
        .spawns
        .iter()
        .map(|&(_, event, _)| event)
        .chain(links.pools.values().copied());
    for event in linked {
        definitions
            .entry(event)
            .or_insert_with(|| format!("({event},0,0,0,0,false,true,'')"));
    }

    let mut stmts = vec![
        "DELETE FROM game_event_status WHERE event_entry >= 0".to_string(),
        "DELETE FROM game_event_link WHERE link_id >= 0".to_string(),
        "DELETE FROM game_event_pool WHERE pool_id >= 0".to_string(),
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
    // Each statement commits alone, so a pass that ran part way through the import settled on a
    // partial link set. Dropping the statuses last makes the next pass walk every link again.
    stmts.push("DELETE FROM game_event_status WHERE event_entry >= 0".to_string());
    eprintln!(
        "game events: {} links and {} event pools; {} links name a spawn the import leaves out, \
         {} negative links on pooled placements are unsupported",
        link_rows.len(),
        pool_rows.len(),
        links.absent,
        links.pooled_absences
    );

    let scripted: Vec<u32> = parse_table(dump, "game_event")?
        .iter()
        .filter(|row| row.number::<u8>("hardcoded").is_ok_and(|flag| flag != 0))
        .map(|row| row.number("entry"))
        .collect::<Result<_>>()?;
    let manifest = serde_json::json!({
        "format_version": 1,
        "source": "vmangos",
        "family": FAMILY,
        "patch": crate::source_selection::PATCH,
        "client_build": crate::source_selection::CLIENT_BUILD,
        "findings": [
            {"code": "events.scripted", "classification": "unsupported", "occurrences": scripted.len(), "source_ids": scripted},
            {"code": "events.pooled-absence", "classification": "unsupported", "occurrences": links.pooled_absences},
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
