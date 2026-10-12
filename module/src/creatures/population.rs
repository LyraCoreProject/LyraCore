//! Source population choices survive import; each Instance keeps its first selection.

use std::collections::{BTreeMap, BTreeSet};

use spacetimedb::{log, table, ReducerContext, SpacetimeType, Table};

use super::{game_creature_template, CreatureSpawn};

#[table(accessor = game_creature_population)]
pub struct CreaturePopulation {
    #[primary_key]
    pub population_id: u64,
    pub max_count: u32,
    pub world_state_condition: u32,
    pub world_state_expression: u32,
    pub flags: u32,
    /// Rows of entry,min_count,max_count,chance. Zero chance shares the remaining probability.
    pub entries: String,
    #[default(false)]
    pub is_pool: bool,
}

#[table(accessor = game_creature_population_member, index(accessor = by_population, btree(columns = [population_id])))]
#[derive(Clone)]
pub struct CreaturePopulationMember {
    #[primary_key]
    pub source_guid: u64,
    pub population_id: u64,
    pub entry: u32,
    pub choices: String,
    pub chance_bp: u32,
    /// Positive ids require an active game event; negative ids remove a spawn during that event.
    pub event_ids: String,
}

#[derive(Clone, SpacetimeType)]
pub struct PopulationAssignment {
    pub source_guid: u64,
    pub entry: u32,
}

#[table(accessor = game_creature_population_selection, index(accessor = by_instance, btree(columns = [instance_id])))]
pub struct CreaturePopulationSelection {
    #[primary_key]
    pub id: String,
    pub population_id: u64,
    pub instance_id: u64,
    pub assignments: Vec<PopulationAssignment>,
}

#[derive(Clone)]
struct EntryChoice {
    entry: u32,
    minimum: u32,
    maximum: u32,
    chance: u32,
}

struct Member {
    guid: u64,
    entry: u32,
    choices: Vec<u32>,
    chance_bp: u32,
    requires_event: bool,
}

fn numbers<T: std::str::FromStr>(text: &str) -> Result<Vec<T>, String> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|field| {
            field
                .parse()
                .map_err(|_| format!("invalid population number {field:?}"))
        })
        .collect()
}

fn decode_entries(text: &str) -> Result<Vec<EntryChoice>, String> {
    text.split(';')
        .filter(|row| !row.is_empty())
        .map(|row| {
            let values = numbers::<u32>(row)?;
            let [entry, minimum, maximum, chance] = values.as_slice() else {
                return Err(format!("population entry needs four fields: {row}"));
            };
            if *entry == 0 || *chance > 100 || (*maximum != 0 && minimum > maximum) {
                return Err(format!("invalid population entry {row}"));
            }
            Ok(EntryChoice {
                entry: *entry,
                minimum: *minimum,
                maximum: *maximum,
                chance: *chance,
            })
        })
        .collect()
}

fn decode_member(member: &CreaturePopulationMember) -> Result<Member, String> {
    let choices = numbers::<u32>(&member.choices)?;
    let events = numbers::<i32>(&member.event_ids)?;
    if member.chance_bp > 10_000 || choices.contains(&0) || events.contains(&0) {
        return Err(format!("invalid population member {}", member.source_guid));
    }
    Ok(Member {
        guid: member.source_guid,
        entry: member.entry,
        choices,
        chance_bp: member.chance_bp,
        requires_event: events.iter().any(|event| *event > 0),
    })
}

/// Check source references before the first spawn batch changes the live population.
pub(crate) fn check_definitions(ctx: &ReducerContext) -> Result<(), String> {
    let templates = ctx.db.game_creature_template();
    let populations = ctx.db.game_creature_population();
    for population in populations.iter() {
        let entries = decode_entries(&population.entries)?;
        let mut seen = BTreeSet::new();
        let chance_sum: u32 = entries.iter().map(|entry| entry.chance).sum();
        if chance_sum > 100 {
            return Err(format!(
                "population {}: entry chance sum exceeds 100",
                population.population_id
            ));
        }
        for entry in entries {
            if !seen.insert(entry.entry) || templates.entry().find(entry.entry).is_none() {
                return Err(format!(
                    "population {}: duplicate or missing creature entry {}",
                    population.population_id, entry.entry
                ));
            }
        }
    }
    for member in ctx.db.game_creature_population_member().iter() {
        let source = decode_member(&member)?;
        let Some(population) = populations.population_id().find(member.population_id) else {
            return Err(format!(
                "population member {} references missing group {}",
                member.source_guid, member.population_id
            ));
        };
        let entries = decode_entries(&population.entries)?;
        if source.entry == 0
            && source.choices.is_empty()
            && entries.is_empty()
            && population.world_state_condition == 0
            && population.world_state_expression == 0
        {
            return Err(format!(
                "population member {} has no creature entry",
                source.guid
            ));
        }
        for entry in source
            .choices
            .iter()
            .copied()
            .chain((source.entry != 0).then_some(source.entry))
        {
            if templates.entry().find(entry).is_none() {
                return Err(format!(
                    "population member {} references missing creature entry {entry}",
                    source.guid
                ));
            }
        }
    }
    Ok(())
}

fn choose_index(len: usize, draw: &mut impl FnMut() -> u64) -> usize {
    // Every caller checks for an empty candidate list first.
    (draw() % len as u64) as usize
}

fn select_pool(
    limit: u32,
    members: &[Member],
    draw: &mut impl FnMut() -> u64,
) -> Vec<PopulationAssignment> {
    let mut eligible: Vec<_> = members
        .iter()
        .filter(|member| !member.requires_event)
        .collect();
    for index in (1..eligible.len()).rev() {
        let other = choose_index(index + 1, draw);
        eligible.swap(index, other);
    }
    if limit == 1 {
        // The source compares every explicit chance with the same roll, then falls back to
        // an equal-chance member or the first explicit member. It is not a cumulative roll.
        let roll = draw() % 10_000;
        let chosen = eligible
            .iter()
            .find(|member| member.chance_bp != 0 && roll < u64::from(member.chance_bp))
            .or_else(|| eligible.iter().find(|member| member.chance_bp == 0))
            .or_else(|| eligible.first())
            .copied();
        eligible = chosen.into_iter().collect();
    } else {
        eligible.truncate(limit as usize);
    }
    eligible
        .into_iter()
        .map(|member| PopulationAssignment {
            source_guid: member.guid,
            entry: if member.choices.is_empty() {
                member.entry
            } else {
                member.choices[choose_index(member.choices.len(), draw)]
            },
        })
        .collect()
}

fn select(
    population: &CreaturePopulation,
    members: &[Member],
    entries: &[EntryChoice],
    mut draw: impl FnMut() -> u64,
) -> Vec<PopulationAssignment> {
    if population.world_state_condition != 0 || population.world_state_expression != 0 {
        return Vec::new();
    }
    if population.is_pool {
        return select_pool(population.max_count, members, &mut draw);
    }
    let mut eligible: Vec<&Member> = members
        .iter()
        .filter(|member| {
            !member.requires_event
                && (member.chance_bp == 0 || draw() % 10_000 < u64::from(member.chance_bp))
        })
        .collect();
    for index in (1..eligible.len()).rev() {
        let other = choose_index(index + 1, &mut draw);
        eligible.swap(index, other);
    }
    if population.max_count > 0 {
        eligible.truncate(population.max_count as usize);
    }
    // Fixed and independent random entries consume group quotas before group-selected entries.
    eligible.sort_by_key(|member| member.entry == 0 && member.choices.is_empty());
    let mut counts = BTreeMap::<u32, u32>::new();
    let mut selected = Vec::new();
    for member in eligible {
        let entry = if !member.choices.is_empty() {
            Some(member.choices[choose_index(member.choices.len(), &mut draw)])
        } else if member.entry != 0 {
            Some(member.entry)
        } else {
            let available: Vec<&EntryChoice> = entries
                .iter()
                .filter(|entry| {
                    entry.maximum == 0
                        || counts.get(&entry.entry).copied().unwrap_or(0) < entry.maximum
                })
                .collect();
            let minimum: Vec<&EntryChoice> = available
                .iter()
                .copied()
                .filter(|entry| counts.get(&entry.entry).copied().unwrap_or(0) < entry.minimum)
                .collect();
            if !minimum.is_empty() {
                Some(minimum[choose_index(minimum.len(), &mut draw)].entry)
            } else {
                let mut roll = (draw() % 100 + 1) as u32;
                let explicit = available.iter().find(|entry| {
                    if entry.chance == 0 {
                        return false;
                    }
                    if roll <= entry.chance {
                        return true;
                    }
                    roll -= entry.chance;
                    false
                });
                if let Some(entry) = explicit {
                    Some(entry.entry)
                } else {
                    let equal: Vec<_> =
                        available.iter().filter(|entry| entry.chance == 0).collect();
                    if equal.is_empty() {
                        None
                    } else {
                        Some(equal[choose_index(equal.len(), &mut draw)].entry)
                    }
                }
            }
        };
        if let Some(entry) = entry {
            *counts.entry(entry).or_default() += 1;
            selected.push(PopulationAssignment {
                source_guid: member.guid,
                entry,
            });
        }
    }
    selected
}

pub(crate) fn entry_for_spawn(ctx: &ReducerContext, spawn: &CreatureSpawn) -> Option<u32> {
    let source_guid = if spawn.source_guid == 0 {
        spawn.guid
    } else {
        spawn.source_guid
    };
    let Some(member) = ctx
        .db
        .game_creature_population_member()
        .source_guid()
        .find(source_guid)
    else {
        return (spawn.entry != 0).then_some(spawn.entry);
    };
    let selections = ctx.db.game_creature_population_selection();
    let id = format!("{}:{}", member.population_id, spawn.instance_id);
    if let Some(selection) = selections.id().find(&id) {
        return selection
            .assignments
            .iter()
            .find(|assignment| assignment.source_guid == source_guid)
            .map(|assignment| assignment.entry);
    }
    let definition = ctx
        .db
        .game_creature_population()
        .population_id()
        .find(member.population_id)?;
    let parsed = (|| {
        let entries = decode_entries(&definition.entries)?;
        let members = ctx
            .db
            .game_creature_population_member()
            .by_population()
            .filter(member.population_id)
            .map(|member| decode_member(&member))
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, String>(select(&definition, &members, &entries, || {
            ctx.random::<u64>()
        }))
    })();
    let assignments = match parsed {
        Ok(assignments) => assignments,
        Err(error) => {
            log::error!(
                "population {} cannot select creatures: {error}",
                member.population_id
            );
            return None;
        }
    };
    let entry = assignments
        .iter()
        .find(|assignment| assignment.source_guid == source_guid)
        .map(|assignment| assignment.entry);
    selections.insert(CreaturePopulationSelection {
        id,
        population_id: member.population_id,
        instance_id: spawn.instance_id,
        assignments,
    });
    entry
}

pub(crate) fn clear_instance(ctx: &ReducerContext, instance_id: u64) {
    let selections = ctx.db.game_creature_population_selection();
    let ids: Vec<String> = selections
        .by_instance()
        .filter(instance_id)
        .map(|selection| selection.id)
        .collect();
    for id in ids {
        selections.id().delete(id);
    }
}

pub(crate) fn clear_all(ctx: &ReducerContext) {
    let selections = ctx.db.game_creature_population_selection();
    let ids: Vec<String> = selections.iter().map(|selection| selection.id).collect();
    for id in ids {
        selections.id().delete(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(max_count: u32, entries: &str) -> CreaturePopulation {
        CreaturePopulation {
            population_id: 1,
            max_count,
            world_state_condition: 0,
            world_state_expression: 0,
            flags: 0,
            entries: entries.to_owned(),
            is_pool: false,
        }
    }

    fn member(guid: u64, entry: u32) -> Member {
        Member {
            guid,
            entry,
            choices: Vec::new(),
            chance_bp: 0,
            requires_event: false,
        }
    }

    #[test]
    fn legacy_pool_uses_one_fractional_roll_then_the_default() {
        let mut rare = member(1, 10);
        rare.chance_bp = 750;
        let members = [rare, member(2, 20)];
        assert_eq!(select_pool(1, &members, &mut || 749)[0].entry, 10);
        assert_eq!(select_pool(1, &members, &mut || 750)[0].entry, 20);
        assert_eq!(select_pool(1, &members, &mut || 9_999)[0].entry, 20);
    }

    #[test]
    fn multi_member_pool_ignores_explicit_chances_and_preserves_event_gates() {
        let members: Vec<_> = (1..=8)
            .map(|guid| {
                let mut member = member(guid, 10);
                member.chance_bp = 75;
                member.requires_event = guid > 6;
                member
            })
            .collect();
        let selected = select_pool(4, &members, &mut || 9_999);
        assert_eq!(selected.len(), 4);
        assert!(selected.iter().all(|choice| choice.source_guid <= 6));
    }

    #[test]
    fn group_selection_obeys_entry_minimum_and_maximum() {
        let group = definition(3, "10,2,2,0;20,1,1,0");
        let members = vec![member(1, 0), member(2, 0), member(3, 0), member(4, 0)];
        let assignments = select(
            &group,
            &members,
            &decode_entries(&group.entries).unwrap(),
            || 0,
        );
        assert_eq!(assignments.len(), 3);
        assert_eq!(assignments.iter().filter(|a| a.entry == 10).count(), 2);
        assert_eq!(assignments.iter().filter(|a| a.entry == 20).count(), 1);
    }

    #[test]
    fn fixed_members_consume_minimum_before_group_choices() {
        let group = definition(0, "10,1,1,0;20,1,1,0");
        let assignments = select(
            &group,
            &[member(1, 0), member(2, 10)],
            &decode_entries(&group.entries).unwrap(),
            || 0,
        );
        assert_eq!(
            assignments
                .iter()
                .find(|a| a.source_guid == 1)
                .unwrap()
                .entry,
            20
        );
    }

    #[test]
    fn explicit_entry_probability_leaves_remaining_share_to_equal_choices() {
        let group = definition(1, "10,0,0,25;20,0,0,0");
        let entries = decode_entries(&group.entries).unwrap();
        assert_eq!(
            select(&group, &[member(1, 0)], &entries, || 24)[0].entry,
            10
        );
        assert_eq!(
            select(&group, &[member(1, 0)], &entries, || 25)[0].entry,
            20
        );
    }

    #[test]
    fn conditional_populations_remain_dormant() {
        let mut group = definition(0, "");
        group.world_state_condition = 19998;
        assert!(select(&group, &[member(1, 0)], &[], || 0).is_empty());
    }

    #[test]
    fn positive_event_membership_and_failed_slot_chance_remain_dormant() {
        let group = definition(0, "");
        let mut event = member(1, 10);
        event.requires_event = true;
        let mut chance = member(2, 20);
        chance.chance_bp = 1_000;
        let selected = select(&group, &[event, chance, member(3, 30)], &[], || 5_000);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source_guid, 3);
    }

    #[test]
    fn negative_event_membership_spawns_when_no_events_are_active() {
        let source = CreaturePopulationMember {
            source_guid: 1,
            population_id: 1,
            entry: 10,
            choices: String::new(),
            chance_bp: 0,
            event_ids: "-4".to_owned(),
        };
        let member = decode_member(&source).unwrap();
        let selected = select(&definition(0, ""), &[member], &[], || 0);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].entry, 10);
    }

    #[test]
    fn independent_random_slots_can_choose_each_definition() {
        let group = definition(0, "");
        let mut slot = member(1, 0);
        slot.choices = vec![10, 20];
        assert_eq!(select(&group, &[slot], &[], || 1)[0].entry, 20);
    }
}
