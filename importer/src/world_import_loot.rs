use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{ensure, Result};

use crate::dump::TableRows;

/// A row's `CONDITION_ACTIVE_GAME_EVENT` as `(event, reversed)`.
pub(super) type EventCondition = (u32, bool);

type Templates = BTreeMap<u64, Vec<Entry>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LootDrop {
    pub(super) item: u64,
    pub(super) chance_bp: u32,
    pub(super) min_count: u32,
    pub(super) max_count: u32,
    pub(super) group_id: u32,
    pub(super) quest_only: bool,
    pub(super) event: Option<EventCondition>,
    /// A group member with vmangos chance 0, whose `chance_bp` is its share of the group's rest.
    pub(super) equal_chance: bool,
}

impl LootDrop {
    /// `(id,key,item_entry,chance_bp,count,group_id,quest_only,min_count)`
    pub(super) fn row(&self, id: u64, key: u64) -> String {
        let Self {
            item,
            chance_bp,
            min_count,
            max_count,
            group_id,
            quest_only,
            ..
        } = *self;
        format!("({id},{key},{item},{chance_bp},{max_count},{group_id},{quest_only},{min_count})")
    }

    /// `row` followed by `event_entry,event_reversed,equal_chance`, for creature loot.
    pub(super) fn creature_row(&self, id: u64, key: u64) -> String {
        let Self {
            item,
            chance_bp,
            min_count,
            max_count,
            group_id,
            quest_only,
            event,
            equal_chance,
        } = *self;
        let (event_entry, event_reversed) = event.unwrap_or_default();
        format!(
            "({id},{key},{item},{chance_bp},{max_count},{group_id},{quest_only},{min_count},\
             {event_entry},{event_reversed},{equal_chance})"
        )
    }

    /// `(id,key,item_entry,chance_bp,count,group_id,min_count)` for skinning and fishing.
    pub(super) fn row_without_quest_only(&self, id: u64, key: u64) -> String {
        let Self {
            item,
            chance_bp,
            min_count,
            max_count,
            group_id,
            ..
        } = *self;
        format!("({id},{key},{item},{chance_bp},{max_count},{group_id},{min_count})")
    }

    /// `row_without_quest_only` followed by `event_entry,event_reversed,equal_chance`, for fishing.
    pub(super) fn fishing_row(&self, id: u64, key: u64) -> String {
        let Self {
            item,
            chance_bp,
            min_count,
            max_count,
            group_id,
            event,
            equal_chance,
            ..
        } = *self;
        let (event_entry, event_reversed) = event.unwrap_or_default();
        format!(
            "({id},{key},{item},{chance_bp},{max_count},{group_id},{min_count},\
             {event_entry},{event_reversed},{equal_chance})"
        )
    }
}

struct Entry {
    item: u64,
    chance: f64,
    reference: i64,
    count: u32,
    group: u32,
    quest_only: bool,
    condition: u32,
}

struct Member {
    item: u64,
    chance: f64,
    min_count: u32,
    max_count: u32,
    quest_only: bool,
    event: Option<EventCondition>,
    equal_chance: bool,
}

struct Roll {
    grouped: bool,
    members: Vec<Member>,
}

fn read(
    dump: &(impl TableRows + ?Sized),
    table: &str,
    forbidden: &HashSet<u64>,
) -> Result<Templates> {
    let mut templates = Templates::new();
    for row in dump.rows(table)?.iter() {
        let item = row.number("item")?;
        let reference = row.number("mincountOrRef")?;
        let chance: f64 = row.number("ChanceOrQuestChance")?;
        ensure!(
            chance.is_finite() && chance.abs() <= 100.0,
            "{table}: invalid loot chance {chance}"
        );
        let entry = row.number("entry")?;
        let count = row.number("maxcount")?;
        let group = row.number("groupid")?;
        // vmangos LootStoreItem::IsValid drops a zero minimum and a maximum below the minimum.
        if reference == 0 || reference > i64::from(count) {
            continue;
        }
        if reference > 0 && forbidden.contains(&item) {
            continue;
        }
        templates.entry(entry).or_default().push(Entry {
            item,
            chance: chance.abs(),
            reference,
            count,
            group,
            quest_only: chance < 0.0,
            condition: row.number("condition_id")?,
        });
    }
    Ok(templates)
}

/// A reference's Game Event condition passes to its items; an item that asks for another fails.
fn pass_event(member: &mut Member, event: Option<EventCondition>, reference: u64) -> Result<()> {
    ensure!(
        event.is_none() || member.event.is_none_or(|own| Some(own) == event),
        "reference_loot_template: reference {reference} and its item {} ask for different events",
        member.item
    );
    member.event = member.event.or(event);
    Ok(())
}

fn expand(
    rows: &[Entry],
    group: u32,
    refs: &Templates,
    events: &BTreeMap<u32, EventCondition>,
    path: &mut HashSet<u64>,
) -> Result<Vec<Roll>> {
    let mut rolls = Vec::new();
    let mut groups: BTreeMap<u32, Vec<Member>> = BTreeMap::new();
    for row in rows {
        if group != 0 && (row.reference < 0 || row.group != group) {
            continue;
        }
        if row.reference < 0 {
            if row.chance == 0.0 {
                continue;
            }
            let id = row.reference.unsigned_abs();
            ensure!(
                path.len() < 64 && path.insert(id),
                "reference_loot_template: cyclic or too deep reference {id}"
            );
            // vmangos rolls a reference's chance once and then runs it `count` times, which
            // independent loot rows cannot express.
            ensure!(
                row.count <= 1 || row.chance >= 100.0,
                "reference_loot_template: reference {id} repeats {} times at {}%",
                row.count,
                row.chance
            );
            if let Some(children) = refs.get(&id) {
                let event = events.get(&row.condition).copied();
                for _ in 0..row.count {
                    let mut children = expand(children, row.group, refs, events, path)?;
                    for roll in &mut children {
                        for member in &mut roll.members {
                            member.chance *= row.chance / 100.0;
                            pass_event(member, event, id)?;
                        }
                    }
                    rolls.extend(children);
                }
            }
            path.remove(&id);
        } else if row.item != 0 {
            let member = Member {
                item: row.item,
                chance: row.chance,
                min_count: row.reference as u32,
                max_count: row.count,
                quest_only: row.quest_only,
                event: events.get(&row.condition).copied(),
                equal_chance: false,
            };
            if row.group != 0 {
                groups.entry(row.group).or_default().push(member);
            } else if row.chance > 0.0 {
                rolls.push(Roll {
                    grouped: false,
                    members: vec![member],
                });
            }
        }
    }
    for mut members in groups.into_values() {
        let explicit: f64 = members.iter().map(|member| member.chance).sum();
        let equal = members.iter().filter(|member| member.chance == 0.0).count();
        let share = if equal == 0 {
            0.0
        } else {
            (100.0 - explicit).max(0.0) / equal as f64
        };
        let mut remaining: f64 = 100.0;
        for member in &mut members {
            if member.chance == 0.0 {
                member.chance = share;
                member.equal_chance = true;
            }
            member.chance = member.chance.min(remaining);
            remaining -= member.chance;
        }
        rolls.push(Roll {
            grouped: true,
            members,
        });
    }
    Ok(rolls)
}

fn basis_points(members: &[Member]) -> Vec<u32> {
    let quotas: Vec<_> = members.iter().map(|member| member.chance * 100.0).collect();
    let total = quotas.iter().sum::<f64>().round().min(10000.0) as u32;
    let mut shares: Vec<_> = quotas.iter().map(|quota| quota.floor() as u32).collect();
    let mut order: Vec<_> = (0..members.len()).collect();
    order.sort_by(|&a, &b| {
        (quotas[b] - f64::from(shares[b]))
            .total_cmp(&(quotas[a] - f64::from(shares[a])))
            .then_with(|| a.cmp(&b))
    });
    let remainder = total.saturating_sub(shares.iter().sum());
    for &index in order.iter().take(remainder as usize) {
        shares[index] += 1;
    }
    shares
}

pub(super) fn parse(
    dump: &(impl TableRows + ?Sized),
    table: &str,
) -> Result<HashMap<u64, Vec<LootDrop>>> {
    let forbidden = super::forbidden_items(dump)?;
    let refs = read(dump, "reference_loot_template", &forbidden)?;
    let templates = read(dump, table, &forbidden)?;
    let conditional = refs
        .values()
        .chain(templates.values())
        .flatten()
        .any(|row| row.condition != 0);
    let events = if conditional {
        super::game_events::event_conditions(dump)?
    } else {
        BTreeMap::new()
    };
    let mut result = HashMap::new();
    for (entry, rows) in templates {
        let rolls = expand(&rows, 0, &refs, &events, &mut HashSet::new())?;
        let mut drops = Vec::new();
        let mut next_group = 1_000_000u32;
        for roll in rolls {
            let (group, shares) = if roll.grouped {
                next_group += 1;
                (next_group, basis_points(&roll.members))
            } else {
                (
                    0,
                    roll.members
                        .iter()
                        .map(|member| (member.chance * 100.0).round().clamp(1.0, 10000.0) as u32)
                        .collect(),
                )
            };
            for (member, chance_bp) in roll.members.into_iter().zip(shares) {
                drops.push(LootDrop {
                    item: member.item,
                    chance_bp,
                    min_count: member.min_count,
                    max_count: member.max_count,
                    group_id: group,
                    quest_only: member.quest_only,
                    event: member.event,
                    equal_chance: member.equal_chance,
                });
            }
        }
        if !drops.is_empty() {
            result.insert(entry, drops);
        }
    }
    Ok(result)
}
