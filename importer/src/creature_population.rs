use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{bail, Context, Result};

use crate::dump::{cr, field, parse_complete_table, sql_str};
use crate::{creature_row_kept_in_scope, world_guid, CreatureSpawnRow, WorldImportScope};

pub(crate) struct PopulationPlan {
    pub(crate) spawns: Vec<CreatureSpawnRow>,
    pub(crate) definition_rows: Vec<String>,
    pub(crate) membership_rows: Vec<String>,
    pub(crate) referenced_entries: HashSet<u64>,
    pub(crate) conditional_spawns: usize,
    pub(crate) event_spawns: usize,
}

impl PopulationPlan {
    pub(crate) fn append_statements(&self, statements: &mut Vec<String>) {
        statements
            .push("DELETE FROM game_creature_population_selection WHERE population_id > 0".into());
        statements.push("DELETE FROM game_creature_population_member WHERE source_guid > 0".into());
        statements.push("DELETE FROM game_creature_population WHERE population_id > 0".into());
        crate::dump::push_insert(
            statements,
            "game_creature_population",
            "population_id,max_count,world_state_condition,world_state_expression,flags,entries,is_pool",
            &self.definition_rows,
        );
        crate::dump::push_insert(
            statements,
            "game_creature_population_member",
            "source_guid,population_id,entry,choices,chance_bp,event_ids",
            &self.membership_rows,
        );
    }
}

struct Group {
    max_count: u32,
    condition: u32,
    expression: u32,
    flags: u32,
    entries: Vec<(u32, u32, u32, u32)>,
    is_pool: bool,
}

fn number<T: std::str::FromStr>(row: &[String], column: usize, table: &str) -> Result<T> {
    field(row, column)
        .parse()
        .map_err(|_| anyhow::anyhow!("{table}: invalid column {column} in row {row:?}"))
}

fn source_groups(dump: &str) -> Result<BTreeMap<u64, Group>> {
    let mut groups = BTreeMap::new();
    for row in parse_complete_table(dump, "spawn_group")? {
        if number::<u32>(&row, 2, "spawn_group")? != 0 {
            continue;
        }
        let id: u64 = number(&row, 0, "spawn_group")?;
        if groups
            .insert(
                id,
                Group {
                    max_count: number(&row, 3, "spawn_group")?,
                    condition: number(&row, 4, "spawn_group")?,
                    expression: number(&row, 5, "spawn_group")?,
                    flags: number(&row, 6, "spawn_group")?,
                    entries: Vec::new(),
                    is_pool: false,
                },
            )
            .is_some()
        {
            bail!("duplicate creature spawn group {id}");
        }
    }
    for row in parse_complete_table(dump, "spawn_group_entry")? {
        let id = number(&row, 0, "spawn_group_entry")?;
        let Some(group) = groups.get_mut(&id) else {
            continue;
        };
        let entry: u32 = number(&row, 1, "spawn_group_entry")?;
        let min: u32 = number(&row, 2, "spawn_group_entry")?;
        let max: u32 = number(&row, 3, "spawn_group_entry")?;
        let chance: u32 = number(&row, 4, "spawn_group_entry")?;
        if entry == 0 || chance > 100 || (max > 0 && min > max) {
            bail!("invalid spawn_group_entry for group {id}, entry {entry}");
        }
        if group.entries.iter().any(|candidate| candidate.0 == entry) {
            bail!("duplicate spawn_group_entry for group {id}, entry {entry}");
        }
        if group
            .entries
            .iter()
            .map(|candidate| candidate.3)
            .sum::<u32>()
            + chance
            > 100
        {
            bail!("spawn_group_entry chances exceed 100 for group {id}");
        }
        group.entries.push((entry, min, max, chance));
    }
    Ok(groups)
}

struct Pool {
    limit: u32,
    members: BTreeMap<u64, u32>,
    children: Vec<(u64, u32)>,
}

fn chance_bp(row: &[String], column: usize, table: &str) -> Result<u32> {
    let chance: f64 = number(row, column, table)?;
    if !chance.is_finite() || !(0.0..=100.0).contains(&chance) {
        bail!("{table}: invalid chance {chance}");
    }
    Ok((chance * 100.0).round() as u32)
}

fn flatten_pool(
    id: u64,
    pools: &BTreeMap<u64, Pool>,
    visiting: &mut BTreeSet<u64>,
) -> Result<BTreeMap<u64, u32>> {
    let pool = pools
        .get(&id)
        .with_context(|| format!("missing pool_template {id}"))?;
    if !visiting.insert(id) {
        bail!("cyclic creature pool {id}");
    }
    let mut members = pool.members.clone();
    if !pool.children.is_empty() {
        if pool.limit != 1 || !members.is_empty() {
            bail!("creature pool {id}: unsupported nested limits or direct members");
        }
        let mut size = None;
        for &(child, chance) in &pool.children {
            let child_pool = pools
                .get(&child)
                .with_context(|| format!("missing pool_template {child}"))?;
            let leaves = flatten_pool(child, pools, visiting)?;
            if chance != 0
                || child_pool.limit != 1
                || leaves.values().any(|chance| *chance != 0)
                || size.is_some_and(|size| size != leaves.len())
            {
                bail!("creature pool {id}: nested choices are not equal single selections");
            }
            size = Some(leaves.len());
            for (guid, chance) in leaves {
                if members.insert(guid, chance).is_some() {
                    bail!("duplicate creature {guid} in pool {id}");
                }
            }
        }
    }
    visiting.remove(&id);
    Ok(members)
}

fn add_legacy_pools(
    dump: &str,
    groups: &mut BTreeMap<u64, Group>,
    memberships: &mut BTreeMap<u64, (u64, u32)>,
    events: &mut BTreeMap<u64, BTreeSet<i32>>,
) -> Result<()> {
    let mut pools = BTreeMap::new();
    for row in parse_complete_table(dump, "pool_template")? {
        let id = number(&row, 0, "pool_template")?;
        if pools
            .insert(
                id,
                Pool {
                    limit: number(&row, 1, "pool_template")?,
                    members: BTreeMap::new(),
                    children: Vec::new(),
                },
            )
            .is_some()
        {
            bail!("duplicate pool_template {id}");
        }
    }
    let creatures = parse_complete_table(dump, "creature")?;
    let mut by_entry = BTreeMap::<u64, Vec<u64>>::new();
    let mut maps = BTreeMap::<u64, i64>::new();
    for row in &creatures {
        let guid = number(row, cr::GUID, "creature")?;
        by_entry
            .entry(number(row, cr::ID, "creature")?)
            .or_default()
            .push(guid);
        maps.insert(guid, number(row, cr::MAP, "creature")?);
    }
    let mut used = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for table in ["pool_creature", "pool_creature_template"] {
        for row in parse_complete_table(dump, table)? {
            let source = number(&row, 0, table)?;
            let id = number(&row, 1, table)?;
            let chance = chance_bp(&row, 2, table)?;
            let pool = pools
                .get_mut(&id)
                .with_context(|| format!("{table}: missing pool_template {id}"))?;
            let guids = if table == "pool_creature" {
                vec![source]
            } else {
                by_entry.get(&source).cloned().unwrap_or_default()
            };
            for guid in guids {
                if !maps.contains_key(&guid) || !seen.insert(guid) {
                    bail!("{table}: missing or multiply pooled creature {guid}");
                }
                pool.members.insert(guid, chance);
                used.insert(id);
            }
        }
    }
    let mut parents = BTreeMap::new();
    for row in parse_complete_table(dump, "pool_pool")? {
        let child = number(&row, 0, "pool_pool")?;
        let parent = number(&row, 1, "pool_pool")?;
        let chance = chance_bp(&row, 2, "pool_pool")?;
        if parents.insert(child, parent).is_some() {
            bail!("pool {child} has several parents");
        }
        if let Some(pool) = pools.get_mut(&parent) {
            pool.children.push((child, chance));
        }
    }
    let mut roots = BTreeSet::new();
    for mut id in used {
        let mut chain = BTreeSet::new();
        while let Some(parent) = parents.get(&id) {
            if !chain.insert(id) {
                bail!("cyclic creature pool {id}");
            }
            id = *parent;
        }
        roots.insert(id);
    }
    let mut object_pools = BTreeSet::new();
    for table in ["pool_gameobject", "pool_gameobject_template"] {
        for row in parse_complete_table(dump, table)? {
            let mut id = number(&row, 1, table)?;
            let mut chain = BTreeSet::new();
            while let Some(parent) = parents.get(&id) {
                if !chain.insert(id) {
                    bail!("cyclic GameObject pool {id}");
                }
                id = *parent;
            }
            object_pools.insert(id);
        }
    }
    for root in roots {
        if object_pools.contains(&root) {
            bail!("pool {root} mixes creatures and GameObjects");
        }
        let members = flatten_pool(root, &pools, &mut BTreeSet::new())?;
        let pool = &pools[&root];
        if pool.limit == 0 {
            bail!("creature pool {root}: zero limit is unsupported");
        }
        let map_ids: BTreeSet<_> = members.keys().map(|guid| maps[guid]).collect();
        if map_ids.len() != 1 {
            bail!("creature pool {root} spans several maps");
        }
        if pool.limit == 1
            && members.values().all(|chance| *chance != 0)
            && members.values().sum::<u32>() != 10_000
        {
            bail!("creature pool {root}: explicit chances must sum to 100 percent");
        }
        let positive_events: BTreeSet<i32> = members
            .keys()
            .filter_map(|guid| events.get(guid))
            .flatten()
            .copied()
            .filter(|event| *event > 0)
            .collect();
        let population_id = (2_u64 << 32) + root;
        if groups
            .insert(
                population_id,
                Group {
                    max_count: pool.limit,
                    condition: 0,
                    expression: 0,
                    flags: 0,
                    entries: Vec::new(),
                    is_pool: true,
                },
            )
            .is_some()
        {
            bail!("population id collision for pool {root}");
        }
        for (guid, chance) in members {
            if memberships.insert(guid, (population_id, chance)).is_some() {
                bail!("creature {guid} belongs to both a spawn group and a pool");
            }
            events.entry(guid).or_default().extend(&positive_events);
        }
    }
    Ok(())
}

pub(crate) fn collect(dump: &str, scope: &WorldImportScope) -> Result<PopulationPlan> {
    let mut groups = source_groups(dump)?;
    let mut memberships = BTreeMap::new();
    for row in parse_complete_table(dump, "spawn_group_spawn")? {
        let id = number(&row, 0, "spawn_group_spawn")?;
        if !groups.contains_key(&id) {
            continue;
        }
        let guid: u64 = number(&row, 1, "spawn_group_spawn")?;
        let chance: u32 = number(&row, 3, "spawn_group_spawn")?;
        if chance > 100 {
            bail!("spawn_group_spawn {guid}: chance {chance} exceeds 100");
        }
        if memberships.insert(guid, (id, chance * 100)).is_some() {
            bail!("creature source guid {guid} belongs to several groups");
        }
    }
    let mut choices: BTreeMap<u64, BTreeSet<u32>> = BTreeMap::new();
    for row in parse_complete_table(dump, "creature_spawn_entry")? {
        let guid = number(&row, 0, "creature_spawn_entry")?;
        let entry = number(&row, 1, "creature_spawn_entry")?;
        if entry == 0 {
            bail!("creature_spawn_entry {guid}: entry zero");
        }
        choices.entry(guid).or_default().insert(entry);
    }
    let mut events: BTreeMap<u64, BTreeSet<i32>> = BTreeMap::new();
    for row in parse_complete_table(dump, "game_event_creature")? {
        let guid = number(&row, 0, "game_event_creature")?;
        let event = number(&row, 1, "game_event_creature")?;
        if event == 0 {
            bail!("game_event_creature {guid}: event zero");
        }
        events.entry(guid).or_default().insert(event);
    }
    add_legacy_pools(dump, &mut groups, &mut memberships, &mut events)?;
    let templates: BTreeSet<u32> = parse_complete_table(dump, "creature_template")?
        .iter()
        .map(|row| number(row, 0, "creature_template"))
        .collect::<Result<_>>()?;
    let mut plan = PopulationPlan {
        spawns: Vec::new(),
        definition_rows: Vec::new(),
        membership_rows: Vec::new(),
        referenced_entries: HashSet::new(),
        conditional_spawns: 0,
        event_spawns: 0,
    };
    let mut used_groups = BTreeSet::new();
    let mut seen_guids = BTreeSet::new();
    for row in parse_complete_table(dump, "creature")? {
        let map: i64 = number(&row, cr::MAP, "creature")?;
        let x: f64 = number(&row, cr::X, "creature")?;
        let y: f64 = number(&row, cr::Y, "creature")?;
        let z: f64 = number(&row, cr::Z, "creature")?;
        let raw_entry: u64 = number(&row, cr::ID, "creature")?;
        if !creature_row_kept_in_scope(scope, map, x, y, z, raw_entry) {
            continue;
        }
        let guid: u64 = number(&row, cr::GUID, "creature")?;
        if guid == 0 || guid > 0xff_ffff || !seen_guids.insert(guid) {
            bail!("invalid or duplicate creature source guid {guid}");
        }
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            bail!("creature {guid}: non-finite position");
        }
        let member = memberships.get(&guid);
        let group = member.and_then(|(id, _)| groups.get(id));
        let conditional = group.is_some_and(|g| g.condition != 0 || g.expression != 0);
        let direct_choices = choices.get(&guid);
        let entry = if raw_entry != 0 {
            raw_entry
        } else {
            direct_choices.and_then(|entries| entries.first()).copied()
                .or_else(|| group.and_then(|g| g.entries.first().map(|e| e.0)))
                .map(u64::from)
                .or_else(|| conditional.then_some(0))
                .with_context(|| format!("creature {guid}: entry zero has no creature_spawn_entry, spawn_group_entry, or conditional source"))?
        };
        let mut referenced = Vec::new();
        if raw_entry != 0 {
            referenced.push(raw_entry as u32);
        }
        if let Some(entries) = direct_choices {
            referenced.extend(entries);
        }
        if let Some(group) = group {
            referenced.extend(group.entries.iter().map(|e| e.0));
        }
        for candidate in referenced {
            plan.referenced_entries.insert(u64::from(candidate));
            if !templates.contains(&candidate) {
                bail!(
                    "creature {guid}: population references missing creature_template {candidate}"
                );
            }
        }
        let orientation: f64 = number(&row, cr::O, "creature")?;
        if !orientation.is_finite() {
            bail!("creature {guid}: non-finite orientation");
        }
        plan.spawns.push((
            guid,
            entry,
            x,
            y,
            z,
            orientation,
            number(&row, cr::MOVEMENT_TYPE, "creature")?,
            number(&row, cr::SPAWN_TIME_SECS_MIN, "creature")?,
            map,
        ));
        plan.conditional_spawns += usize::from(conditional);
        let event_ids = events.get(&guid);
        plan.event_spawns += usize::from(event_ids.is_some_and(|ids| ids.iter().any(|id| *id > 0)));
        if member.is_none() && direct_choices.is_none() && event_ids.is_none() {
            continue;
        }
        let (population_id, chance) = member.copied().unwrap_or(((1_u64 << 32) + guid, 0));
        used_groups.insert(population_id);
        let choices = direct_choices
            .map(|entries| {
                entries
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let event_ids = event_ids
            .map(|ids| ids.iter().map(i32::to_string).collect::<Vec<_>>().join(","))
            .unwrap_or_default();
        plan.membership_rows.push(format!(
            "({},{population_id},{raw_entry},{},{chance},{})",
            world_guid(entry, guid),
            sql_str(&choices),
            sql_str(&event_ids)
        ));
    }
    for id in used_groups {
        let (max, condition, expression, flags, entries, is_pool) =
            if let Some(group) = groups.get(&id) {
                (
                    group.max_count,
                    group.condition,
                    group.expression,
                    group.flags,
                    group
                        .entries
                        .iter()
                        .map(|(e, min, max, chance)| format!("{e},{min},{max},{chance}"))
                        .collect::<Vec<_>>()
                        .join(";"),
                    group.is_pool,
                )
            } else {
                (1, 0, 0, 0, String::new(), false)
            };
        plan.definition_rows.push(format!(
            "({id},{max},{condition},{expression},{flags},{},{is_pool})",
            sql_str(&entries)
        ));
    }
    eprintln!("creature populations: {} source positions retained, {} groups or random/event slots; {} positions require world-state conditions; {} positions have positive game-event membership", plan.spawns.len(), plan.definition_rows.len(), plan.conditional_spawns, plan.event_spawns);
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_scope() -> WorldImportScope {
        WorldImportScope::canonical(crate::WorldImportProfile::parse("full-world").unwrap())
            .unwrap()
    }

    #[test]
    fn rejects_duplicate_entries_and_excess_total_chance_before_emitting_sql() {
        let scope = WorldImportScope::canonical(crate::WorldImportProfile::FullWorld).unwrap();
        for (entries, reason) in [
            ("(1,10,0,0,0),(1,10,0,0,0)", "duplicate spawn_group_entry"),
            ("(1,10,0,0,60),(1,20,0,0,50)", "chances exceed 100"),
        ] {
            let dump = format!("INSERT INTO `spawn_group` VALUES (1,'fixture',0,0,0,0,0,''); INSERT INTO `spawn_group_entry` VALUES {entries};");
            let error = collect(&dump, &scope).err().unwrap();
            assert!(error.to_string().contains(reason), "{error}");
        }
    }

    #[test]
    fn capped_groups_keep_all_positions_and_every_entry_choice() {
        let dump = "INSERT INTO `creature_template` VALUES (10),(20);
            INSERT INTO `creature` VALUES (1,0,0,0,1,2,3,0,60,60,0,0),(2,0,0,0,4,5,6,0,60,60,0,0);
            INSERT INTO `spawn_group` VALUES (7,'rare',0,1,0,0,0,0);
            INSERT INTO `spawn_group_spawn` VALUES (7,1,0,0),(7,2,0,0);
            INSERT INTO `spawn_group_entry` VALUES (7,10,0,1,25),(7,20,0,1,0);";
        let plan = collect(dump, &full_scope()).unwrap();
        assert_eq!(plan.spawns.len(), 2);
        assert_eq!(plan.membership_rows.len(), 2);
        assert_eq!(plan.referenced_entries, HashSet::from([10, 20]));
        assert!(plan.definition_rows[0].contains("10,0,1,25;20,0,1,0"));
    }

    #[test]
    fn conditional_source_without_sql_entry_choices_remains_inspectable() {
        let dump = "INSERT INTO `creature` VALUES (11559,0,0,0,1,2,3,0,60,60,0,0);
            INSERT INTO `spawn_group` VALUES (19980,'emissary',0,0,19998,0,10,0);
            INSERT INTO `spawn_group_spawn` VALUES (19980,11559,0,0);";
        let plan = collect(dump, &full_scope()).unwrap();
        assert_eq!(plan.spawns.len(), 1);
        assert_eq!(plan.spawns[0].1, 0);
        assert_eq!(plan.conditional_spawns, 1);
        assert_eq!(plan.membership_rows.len(), 1);
    }

    #[test]
    fn missing_entry_dependency_names_the_source_guid() {
        let dump = "INSERT INTO `creature` VALUES (123,0,0,0,1,2,3,0,60,60,0,0);";
        let error = collect(dump, &full_scope()).err().unwrap().to_string();
        assert!(error.contains("creature 123"), "{error}");
    }

    #[test]
    fn missing_template_dependency_names_the_source_guid_and_entry() {
        let dump = "INSERT INTO `creature` VALUES (123,456,0,0,1,2,3,0,60,60,0,0);";
        let error = collect(dump, &full_scope()).err().unwrap().to_string();
        assert!(error.contains("creature 123"), "{error}");
        assert!(error.contains("missing creature_template 456"), "{error}");
    }

    #[test]
    fn event_source_keeps_the_sign_of_each_membership() {
        let dump = "INSERT INTO `creature_template` VALUES (10);
            INSERT INTO `creature` VALUES (1,10,0,0,1,2,3,0,60,60,0,0),(2,10,0,0,4,5,6,0,60,60,0,0);
            INSERT INTO `game_event_creature` VALUES (1,4),(2,-4);";
        let plan = collect(dump, &full_scope()).unwrap();
        assert_eq!(plan.spawns.len(), 2);
        assert_eq!(plan.event_spawns, 1);
        assert!(plan.membership_rows.iter().any(|row| row.contains("'-4'")));
    }

    #[test]
    fn rejects_incomplete_population_source() {
        let scope = WorldImportScope::quest_catalogue().unwrap();
        assert!(collect("INSERT INTO `spawn_group` VALUES (1", &scope).is_err());
    }

    #[test]
    fn template_and_nested_pools_keep_limits_and_fractional_chances() {
        let dump = "INSERT INTO `creature_template` VALUES (10),(20);
            INSERT INTO `creature` VALUES (1,10,0,0,1,2,3,0,60,60,0,0),(2,10,0,0,4,5,6,0,60,60,0,0),(3,20,0,0,4,5,6,0,60,60,0,0),(4,20,0,0,4,5,6,0,60,60,0,0);
            INSERT INTO `pool_template` VALUES (1,1,'root'),(2,1,'child'),(3,1,'child');
            INSERT INTO `pool_creature_template` VALUES (10,2,0,''),(20,3,0,'');
            INSERT INTO `pool_pool` VALUES (2,1,0,''),(3,1,0,'');";
        let plan = collect(dump, &full_scope()).unwrap();
        assert_eq!(plan.spawns.len(), 4);
        assert_eq!(plan.definition_rows, ["(8589934593,1,0,0,0,'',true)"]);
        assert_eq!(plan.membership_rows.len(), 4);
        let unequal = dump.replace("(3,1,'child')", "(3,2,'child')");
        assert!(collect(&unequal, &full_scope())
            .err()
            .unwrap()
            .to_string()
            .contains("nested choices"));
        let fractional = dump.split("INSERT INTO `pool_template`").next().unwrap().to_owned()
            + "INSERT INTO `pool_template` VALUES (1,1,'rare'); INSERT INTO `pool_creature` VALUES (1,1,7.5,''),(2,1,0,''); INSERT INTO `game_event_creature` VALUES (1,27);";
        let plan = collect(&fractional, &full_scope()).unwrap();
        assert!(plan
            .membership_rows
            .iter()
            .any(|row| row.contains(",750,'27')")));
        assert!(plan
            .membership_rows
            .iter()
            .all(|row| row.ends_with("'27')")));
    }

    #[test]
    #[ignore = "requires pinned LYRACORE_CLASSIC_DB_SQL"]
    fn pinned_legacy_pools_cover_every_direct_and_template_position() {
        let dump =
            crate::dump::read_dump(&std::env::var("LYRACORE_CLASSIC_DB_SQL").unwrap()).unwrap();
        let plan = collect(&dump, &full_scope()).unwrap();
        assert_eq!(plan.spawns.len(), 66_310);
        assert_eq!(
            plan.definition_rows
                .iter()
                .filter(|row| row.ends_with(",true)"))
                .count(),
            270
        );
        let members = plan
            .membership_rows
            .iter()
            .filter(|row| row.split(',').nth(1).unwrap().parse::<u64>().unwrap() >= (2_u64 << 32))
            .count();
        assert_eq!(members, 998);
    }
}
