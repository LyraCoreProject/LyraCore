use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{ensure, Result};

use crate::dump::TableRows;

type Drop = (u64, u32, u32, u32, bool);
type Templates = BTreeMap<u64, Vec<Entry>>;

struct Entry {
    item: u64,
    chance: f64,
    reference: i64,
    count: u32,
    group: u32,
    quest_only: bool,
}

struct Member {
    item: u64,
    chance: f64,
    count: u32,
    quest_only: bool,
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
        if reference >= 0 && forbidden.contains(&item) {
            continue;
        }
        templates.entry(entry).or_default().push(Entry {
            item,
            chance: chance.abs(),
            reference,
            count,
            group,
            quest_only: chance < 0.0,
        });
    }
    Ok(templates)
}

fn expand(
    rows: &[Entry],
    group: u32,
    refs: &Templates,
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
            if let Some(children) = refs.get(&id) {
                for _ in 0..row.count {
                    let mut children = expand(children, row.group, refs, path)?;
                    for roll in &mut children {
                        for member in &mut roll.members {
                            member.chance *= row.chance / 100.0;
                            member.quest_only |= row.quest_only;
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
                count: row.count.max(1),
                quest_only: row.quest_only,
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
) -> Result<HashMap<u64, Vec<Drop>>> {
    let forbidden = super::forbidden_items(dump)?;
    let refs = read(dump, "reference_loot_template", &forbidden)?;
    let templates = read(dump, table, &forbidden)?;
    let mut result = HashMap::new();
    for (entry, rows) in templates {
        let rolls = expand(&rows, 0, &refs, &mut HashSet::new())?;
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
            for (member, chance) in roll.members.into_iter().zip(shares) {
                drops.push((member.item, chance, member.count, group, member.quest_only));
            }
        }
        if !drops.is_empty() {
            result.insert(entry, drops);
        }
    }
    Ok(result)
}
