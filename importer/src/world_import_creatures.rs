use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::*;
use crate::world_source::Row;

type Findings = BTreeMap<&'static str, BTreeSet<String>>;

fn finding(findings: &mut Findings, code: &'static str, key: impl ToString) {
    findings.entry(code).or_default().insert(key.to_string());
}

fn derived_stats(template: &Row, base: &Row) -> Result<(u32, u32, u32, u32)> {
    scaled_stats(
        template,
        base.number("health")?,
        base.number("melee_damage")?,
        base.number("armor")?,
    )
}

fn scaled_stats(
    template: &Row,
    health: f32,
    damage: f32,
    armor: f32,
) -> Result<(u32, u32, u32, u32)> {
    let health_multiplier: f32 = template.number("health_multiplier")?;
    let damage_multiplier: f32 = template.number("damage_multiplier")?;
    let armor_multiplier: f32 = template.number("armor_multiplier")?;
    let variance: f32 = template.number("damage_variance")?;
    anyhow::ensure!(
        [
            health,
            damage,
            armor,
            health_multiplier,
            damage_multiplier,
            armor_multiplier,
            variance
        ]
        .iter()
        .all(|n| n.is_finite() && *n >= 0.0)
            && variance <= 1.0,
        "creature_template {}: invalid stat multiplier",
        template.number::<u32>("entry")?
    );
    let average = damage * damage_multiplier;
    let result = [
        health * health_multiplier,
        average * (1.0 - variance),
        average * (1.0 + variance),
        armor * armor_multiplier,
    ];
    anyhow::ensure!(
        result.iter().all(|n| n.is_finite() && *n < u32::MAX as f32),
        "creature stats exceed u32"
    );
    Ok((
        result[0].round().max(1.0) as u32,
        result[1].round() as u32,
        result[2].round() as u32,
        result[3] as u32,
    ))
}

fn extrapolated_stat(start: f32, end: f32, level: u32, last_level: u32, armor: bool) -> f32 {
    let (start, end) = if armor {
        (start.max(1.0), end.max(1.0))
    } else {
        (start, end)
    };
    start + (level - 1) as f32 / (last_level - 1) as f32 * (end - start)
}

fn flags(row: &Row, restored: u32) -> Result<(u32, u32)> {
    let static1: u32 = row.number("static_flags1")?;
    let static2: u32 = row.number("static_flags2")?;
    let mut unit = (static1 & restored & 0x60) << 3;
    for (source, target) in [
        (0x200, 0x0200_0000),
        (0x1000_0000, 0x8000),
        (0x0040_0000, 0x1000),
    ] {
        if static1 & source != 0 {
            unit |= target;
        }
    }
    if row.number::<u32>("rank")? > 0 {
        unit |= 0x40;
    }
    let mut kind = 0;
    for (source, target) in [
        (0x10, 1),
        (0x0020_0000, 2),
        (0x0001_0000, 4),
        (0x0080_0000, 8),
        (0x0100_0000, 16),
        (0x4000_0000, 32),
    ] {
        if static1 & source != 0 {
            kind |= target;
        }
    }
    if static2 & 8 != 0 {
        kind |= 64;
    }
    Ok((unit, kind))
}

fn model_attributes(dump: &impl TableRows) -> Result<BTreeMap<u32, (u8, u32, f32)>> {
    let models = dump.columns(
        "creature_display_info_addon",
        &["display_id", "gender", "display_id_other_gender", "bounding_radius"],
    )?;
    models
        .iter()
        .map(|row| {
            let gender: u8 = row.number("gender")?;
            anyhow::ensure!(gender <= 2, "creature model has invalid gender {gender}");
            let radius: f32 = row.number("bounding_radius")?;
            anyhow::ensure!(radius.is_finite() && radius >= 0.0, "creature model has invalid radius {radius}");
            Ok((
                row.number::<u32>("display_id")?,
                (gender, row.number::<u32>("display_id_other_gender")?, radius),
            ))
        })
        .collect()
}

fn templates(
    dump: &impl TableRows,
    entries: &HashSet<u64>,
    spawned: &HashSet<u64>,
    client: Option<&str>,
    statements: &mut Vec<String>,
    findings: &mut Findings,
    immunities: &BTreeMap<u64, u32>,
) -> Result<()> {
    let scales = client
        .map(dbc::load_display_scales)
        .transpose()?
        .unwrap_or_default();
    let models = model_attributes(dump)?;
    let base_rows = dump.rows("creature_classlevelstats")?;
    let base = base_rows
        .iter()
        .map(|r| Ok(((r.number::<u32>("class")?, r.number::<u32>("level")?), r)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut templates = Vec::new();
    let mut levels = Vec::new();
    let mut displays = Vec::new();
    for row in dump.rows("creature_template")?.iter() {
        let entry: u64 = row.number("entry")?;
        if !entries.contains(&entry) {
            continue;
        }
        let class = row.number::<u32>("unit_class")?;
        let min = row.number::<u32>("level_min")?;
        let max = row.number::<u32>("level_max")?;
        if min == 0 {
            anyhow::ensure!(
                !spawned.contains(&entry),
                "creature {entry}: spawned template has level zero"
            );
            finding(findings, "creatures.level-zero", entry);
            continue;
        }
        anyhow::ensure!(
            min <= max && max <= 255,
            "creature {entry}: invalid level range"
        );
        let stats = (min..=max)
            .map(|level| {
                if let Some(base) = base.get(&(class, level)) {
                    return derived_stats(row, base);
                }
                let (last_level, last) = base
                    .iter()
                    .filter(|((c, _), _)| *c == class)
                    .map(|((_, level), row)| (*level, *row))
                    .next_back()
                    .context("missing creature class stats")?;
                anyhow::ensure!(
                    level > last_level && last_level > 1,
                    "creature {entry}: missing class {class}, level {level} stats"
                );
                let first = base
                    .get(&(class, 1))
                    .context("missing level-one creature stats")?;
                let extrapolate = |column: &str, integral: bool| -> Result<f32> {
                    let value = extrapolated_stat(
                        first.number(column)?,
                        last.number(column)?,
                        level,
                        last_level,
                        column == "armor",
                    );
                    Ok(if integral { value.trunc() } else { value })
                };
                scaled_stats(
                    row,
                    extrapolate("health", true)?,
                    extrapolate("melee_damage", false)?,
                    extrapolate("armor", true)?,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        for (offset, &(health, low, high, armor)) in stats.iter().enumerate() {
            let level = min + offset as u32;
            let id = (entry << 8) | u64::from(level);
            levels.push(format!(
                "({id},{entry},{level},{health},{low},{high},{armor})"
            ));
        }
        let (health, low, high, armor) = stats[0];
        let max_health = stats.last().unwrap().0;
        let restored = immunities.get(&entry).copied().unwrap_or(0);
        let (unit, kind) = flags(row, restored)?;
        let mut display = 0;
        let mut scale = 1.0;
        let mut display_rows = Vec::new();
        let total: u32 = row.number("display_total_probability")?;
        let mut weight_sum = 0_u64;
        let mut contiguous = true;
        for slot in 1..=4 {
            let model: u32 = row.number(&format!("display_id{slot}"))?;
            let weight: u32 = row.number(&format!("display_probability{slot}"))?;
            let model_scale: f32 = row.number(&format!("display_scale{slot}"))?;
            anyhow::ensure!(
                model_scale.is_finite() && model_scale >= 0.0,
                "creature {entry}: invalid display scale"
            );
            if model == 0 {
                contiguous = false;
                continue;
            }
            let model_scale = if model_scale > 0.0 {
                model_scale
            } else {
                scales
                    .get(&model)
                    .copied()
                    .filter(|n| *n > 0.0)
                    .unwrap_or(1.0)
            };
            if display == 0 {
                display = model;
                scale = model_scale;
            }
            let weight = if total == 0 {
                u32::from(contiguous)
            } else {
                weight
            };
            weight_sum += u64::from(weight);
            let id = (entry << 3) | slot;
            let (gender, alternate, radius) = models.get(&model).copied().unwrap_or((0, 0, 0.389));
            if !models.contains_key(&model) {
                finding(findings, "creatures.missing-model-gender", entry);
            }
            if alternate != 0 {
                finding(findings, "creatures.alternate-gender-model", entry);
            }
            display_rows.push(format!(
                "({id},{entry},{model},{model_scale},{weight},{gender},{radius})"
            ));
        }
        anyhow::ensure!(
            total == 0 || weight_sum == u64::from(total),
            "creature {entry}: display probability total differs from slots"
        );
        displays.extend(display_rows);
        let fields = [
            "faction",
            "npc_flags",
            "type",
            "pet_family",
            "rank",
            "base_attack_time",
            "gold_min",
            "gold_max",
            "pickpocket_loot_id",
            "skinning_loot_id",
            "trainer_type",
            "trainer_class",
        ]
        .iter()
        .map(|c| row.number::<u32>(c))
        .collect::<Result<Vec<_>>>()?;
        templates.push(format!("({entry},{},{},{display},{min},{health},{},{},{unit},{},{},{kind},{},{scale},{},{},{},{max},{max_health},0,{low},{high},{armor},{},{},{},{})",
            sql_text(row.text("name")?), sql_text(row.optional_text("subname")?.unwrap_or_default()), fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], fields[6], fields[7], fields[8], fields[9], fields[10], fields[11]));
        if !row
            .optional_text("script_name")?
            .unwrap_or_default()
            .is_empty()
        {
            finding(findings, "creatures.script", entry);
        }
        if !row.optional_text("auras")?.unwrap_or_default().is_empty() {
            finding(findings, "creatures.auras", entry);
        }
        if row.number::<u32>("static_flags1")? & 0x60 & !restored != 0 {
            finding(findings, "creatures.scripted-immunity", entry);
        }
        if row.number::<u32>("static_flags1")? & !0x51e1_0270 != 0
            || row.number::<u32>("static_flags2")? & !8 != 0
        {
            finding(findings, "creatures.static-behavior", entry);
        }
        for name in [
            "spawn_spell_id",
            "totem_spell_id",
            "immunity_flags",
            "mechanic_immune_mask",
            "school_immune_mask",
            "flags_extra",
        ] {
            if row.number::<u32>(name)? != 0 {
                finding(findings, "creatures.template-behavior", entry);
            }
        }
    }
    for (table, key, columns, rows) in [
        ("game_creature_template", "entry", "entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class", templates),
        ("game_creature_level_stats", "id", "id,entry,level,health,damage_min,damage_max,armor", levels),
        ("game_creature_display", "id", "id,entry,display_id,scale,weight,gender,bounding_radius", displays),
    ] {
        statements.push(format!("DELETE FROM {table} WHERE {key} > 0"));
        push_insert(statements, table, columns, &rows);
    }
    Ok(())
}

fn waypoints(
    dump: &impl TableRows,
    spawns: &[CreatureSpawnRow],
    statements: &mut Vec<String>,
    findings: &mut Findings,
) -> Result<()> {
    let mut paths = [
        BTreeMap::<u64, BTreeMap<u32, (f32, f32, f32)>>::new(),
        BTreeMap::new(),
    ];
    for (index, table, key) in [
        (0, "creature_movement", "id"),
        (1, "creature_movement_template", "entry"),
    ] {
        for row in dump.rows(table)?.iter() {
            let owner: u64 = row.number(key)?;
            let point: u32 = row.number("point")?;
            if row.number::<u32>("path_id")? != 0 {
                finding(
                    findings,
                    "creatures.alternate-path",
                    format!("{table}:{owner}"),
                );
                continue;
            }
            let position = (
                row.number::<f32>("position_x")?,
                row.number::<f32>("position_y")?,
                row.number::<f32>("position_z")?,
            );
            anyhow::ensure!(
                [position.0, position.1, position.2]
                    .iter()
                    .all(|n| n.is_finite()),
                "{table} {owner}: invalid position"
            );
            anyhow::ensure!(
                paths[index]
                    .entry(owner)
                    .or_default()
                    .insert(point, position)
                    .is_none(),
                "{table} {owner}: duplicate point {point}"
            );
            if row.number::<u32>("script_id")? != 0
                || row.number::<u32>("waittime")? != 0
                || row.number::<f32>("wander_distance")? != 0.0
                || row.number::<f32>("orientation")? != 0.0
            {
                finding(
                    findings,
                    "creatures.waypoint-detail",
                    format!("{table}:{owner}:{point}"),
                );
            }
        }
    }
    let mut rows = Vec::new();
    for &(guid, entry, _, _, _, _, movement, _, _) in spawns {
        if movement != 2 {
            continue;
        }
        let path = paths[0].get(&guid).or_else(|| paths[1].get(&entry));
        if let Some(path) = path {
            for &(x, y, z) in path.values() {
                rows.push(format!(
                    "({},{},{x},{y},{z})",
                    rows.len() + 1,
                    world_guid(entry, guid)
                ));
            }
        }
    }
    statements.push("DELETE FROM game_creature_waypoint WHERE id > 0".into());
    push_insert(
        statements,
        "game_creature_waypoint",
        "id,creature_guid,x,y,z",
        &rows,
    );
    Ok(())
}

fn casts(
    dump: &impl TableRows,
    entries: &HashSet<u64>,
    statements: &mut Vec<String>,
    findings: &mut Findings,
) -> Result<()> {
    let ids = dump
        .columns("spell_template", &["entry"])?
        .iter()
        .map(|r| r.number::<u32>("entry"))
        .collect::<Result<HashSet<_>>>()?;
    let lists = dump.rows("creature_spells")?;
    let lists = lists
        .iter()
        .map(|r| Ok((r.number::<u32>("entry")?, r)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut casts = Vec::new();
    let mut rotations = Vec::new();
    for row in dump
        .columns("creature_template", &["entry", "spell_list_id"])?
        .iter()
    {
        let entry: u64 = row.number("entry")?;
        let list: u32 = row.number("spell_list_id")?;
        if !entries.contains(&entry) || list == 0 {
            continue;
        }
        let spells = lists
            .get(&list)
            .with_context(|| format!("creature {entry}: missing spell list {list}"))?;
        let mut selected = Vec::new();
        for slot in 1..=8 {
            let spell: u32 = spells.number(&format!("spellId_{slot}"))?;
            if spell == 0 {
                continue;
            }
            if ids.contains(&spell) {
                selected.push(spell);
            } else {
                finding(
                    findings,
                    "creatures.missing-spell",
                    format!("{entry}:{spell}"),
                );
            }
            finding(
                findings,
                "creatures.spell-slot-detail",
                format!("{entry}:{list}:{slot}"),
            );
        }
        if let Some(first) = selected.first() {
            casts.push(format!("({entry},{first})"));
            if selected.len() > 1 {
                for (index, spell) in selected.iter().enumerate() {
                    let (priority, condition) = if index == 0 { (10, 0) } else { (20, 3) };
                    rotations.push(format!(
                        "({},{entry},{spell},{priority},{condition},0)",
                        rotations.len() + 1
                    ));
                }
            }
        }
    }
    statements.push("DELETE FROM game_creature_cast WHERE creature_entry > 0".into());
    statements.push("DELETE FROM game_creature_spell WHERE id > 0".into());
    push_insert(
        statements,
        "game_creature_cast",
        "creature_entry,spell_id",
        &casts,
    );
    push_insert(
        statements,
        "game_creature_spell",
        "id,creature_entry,spell_id,priority,condition,condition_value",
        &rotations,
    );
    Ok(())
}

pub(super) fn plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let mut findings = Findings::new();
    let entries = vmangos_creature_entries(dump, args)?;
    let mut plan = DumpPlan::default();
    let mut immunities = BTreeMap::new();
    if args.family.as_deref() == Some("casts") {
        casts(dump, &entries, &mut plan.stmts, &mut findings)?;
    } else {
        let population = creature_population::collect(dump, &args.scope)?;
        for pool in &population.unsupported_pools {
            finding(&mut findings, "creatures.nested-pool", pool);
        }
        immunities = eventai::vmangos::restorable_immunities(dump, &entries)?;
        templates(
            dump,
            &entries,
            &population.referenced_entries,
            args.dbc.as_deref(),
            &mut plan.stmts,
            &mut findings,
            &immunities,
        )?;
        waypoints(dump, &population.spawns, &mut plan.stmts, &mut findings)?;
        population.append_statements(&mut plan.stmts);
        let payload = build_packed_spawn_payload(&population.spawns);
        plan.spawn_batches = payload
            .packed_spawns
            .split(';')
            .filter(|r| !r.is_empty())
            .collect::<Vec<_>>()
            .chunks(SPAWN_BATCH)
            .map(|rows| rows.join(";"))
            .collect();
        plan.spawn_row_count = population.spawns.len();
        let retained = population
            .spawns
            .iter()
            .map(|s| s.0)
            .collect::<HashSet<_>>();
        for row in dump.rows("creature")?.iter() {
            let guid: u64 = row.number("guid")?;
            if row.number::<u32>("id")? == 0 {
                finding(&mut findings, "creatures.spawn-entry-zero", guid);
            }
            if !retained.contains(&guid) {
                continue;
            }
            if row.number::<u32>("spawn_flags")? & !2 != 0 {
                finding(&mut findings, "creatures.spawn-flags", guid);
            }
            if row.number::<i64>("spawntimesecsmin")? != row.number::<i64>("spawntimesecsmax")? {
                finding(&mut findings, "creatures.respawn-range", guid);
            }
            if row.number::<f32>("health_percent")? != 100.0
                || row.number::<f32>("mana_percent")? != 100.0
            {
                finding(&mut findings, "creatures.spawn-resources", guid);
            }
            if row.number::<f32>("visibility_mod")? != 0.0
                || row.number::<f32>("wander_distance")? != 0.0
            {
                finding(&mut findings, "creatures.spawn-distance", guid);
            }
        }
        for (table, code) in [
            ("creature_groups", "creatures.formation"),
            ("game_event_creature", "creatures.game-event"),
        ] {
            for row in dump.rows(table)?.iter() {
                finding(
                    &mut findings,
                    code,
                    row.number::<u64>(if table == "creature_groups" {
                        "member_guid"
                    } else {
                        "guid"
                    })?,
                );
            }
        }
        for table in [
            "pool_template",
            "pool_creature",
            "pool_creature_template",
            "pool_pool",
        ] {
            for row in dump.rows(table)?.iter() {
                if row.number::<u32>("flags")? != 0 {
                    finding(
                        &mut findings,
                        "creatures.pool-flags",
                        format!(
                            "{table}:{}",
                            row.raw(if table == "pool_template" {
                                "entry"
                            } else if table == "pool_pool" {
                                "pool_id"
                            } else if table == "pool_creature" {
                                "guid"
                            } else {
                                "id"
                            })?
                        ),
                    );
                }
            }
        }
    }
    let family = if args.family.as_deref() == Some("casts") {
        "casts"
    } else {
        "creatures"
    };
    plan.stamps = vec![(
        family,
        insert_row_count(&plan.stmts) + plan.spawn_row_count as u64,
    )];
    plan.world_source_manifest = Some(serde_json::to_string_pretty(&serde_json::json!({
        "format_version": 1, "source": "vmangos", "family": family, "patch": 10, "client_build": 5875,
        "restored_spawn_immunities": immunities,
        "findings": findings.into_iter().map(|(code, keys)| serde_json::json!({"code": code, "classification": "unsupported", "occurrences": keys.len(), "source_ids": keys})).collect::<Vec<_>>()
    }))?);
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::{Cell, TableBuilder};

    fn numbers(table: &str, fields: &[(&str, &str)]) -> Row {
        let mut builder = TableBuilder::new(table, fields.iter().map(|(name, _)| name.to_string()));
        builder
            .push(
                fields
                    .iter()
                    .map(|(_, value)| Cell::Number((*value).into()))
                    .collect(),
            )
            .unwrap();
        builder.finish().remove(0)
    }

    fn template_source() -> WorldSource {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let fields = "entry patch unit_class level_min level_max health_multiplier damage_multiplier armor_multiplier damage_variance static_flags1 static_flags2 display_total_probability display_id1 display_probability1 display_scale1 display_id2 display_probability2 display_scale2 display_id3 display_probability3 display_scale3 display_id4 display_probability4 display_scale4 faction npc_flags type pet_family rank base_attack_time gold_min gold_max pickpocket_loot_id skinning_loot_id trainer_type trainer_class spawn_spell_id totem_spell_id immunity_flags mechanic_immune_mask school_immune_mask flags_extra";
        let columns = fields
            .split_whitespace()
            .map(|name| format!("{name} INTEGER DEFAULT 0"))
            .collect::<Vec<_>>()
            .join(",");
        connection.execute_batch(&format!("CREATE TABLE creature_template ({columns}, name TEXT DEFAULT '', subname TEXT DEFAULT '', script_name TEXT DEFAULT '', auras TEXT DEFAULT '', PRIMARY KEY (entry,patch));
            INSERT INTO creature_template (entry,patch,unit_class,level_min,level_max,health_multiplier,damage_multiplier,armor_multiplier,display_id1,faction,rank,base_attack_time) VALUES (5090042,10,1,10,10,1,1,1,100,35,3,2000);
            CREATE TABLE creature_ai_events (creature_id INTEGER,action1_script INTEGER,action2_script INTEGER,action3_script INTEGER);
            CREATE TABLE creature_ai_scripts (id INTEGER,command INTEGER,datalong INTEGER,datalong2 INTEGER);
            CREATE TABLE creature_display_info_addon (display_id INTEGER, build INTEGER, gender INTEGER, display_id_other_gender INTEGER, bounding_radius REAL DEFAULT 0.389, PRIMARY KEY(display_id,build));
            CREATE TABLE creature_classlevelstats (class INTEGER, level INTEGER, health REAL, melee_damage REAL, armor REAL);
            INSERT INTO creature_classlevelstats VALUES (1,10,100,20,50);")).unwrap();
        WorldSource::Vmangos(connection)
    }

    #[test]
    #[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
    fn classic_reimport_restores_rank_health_and_melee_damage() {
        let mut statements = Vec::new();
        templates(
            &template_source(),
            &HashSet::from([5090042]),
            &HashSet::new(),
            None,
            &mut statements,
            &mut Findings::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let mut node = crate::standalone_support::Standalone::start("classic-creature-reimport");
        node.publish_module();
        node.assert_call("claim_operator", &[]);
        node.assert_call("install_guid_range", &["0"]);
        node.assert_call("debug_seed_scenario_fixtures", &[]);
        node.assert_call("debug_spawn_player_entity", &["1"]);
        for statement in &statements {
            node.assert_sql(statement);
        }
        let classic = "INSERT INTO `creature_template` (`Entry`,`Name`,`MinLevel`,`MaxLevel`,`MinLevelHealth`,`MaxLevelHealth`,`MinMeleeDmg`,`MaxMeleeDmg`,`Rank`,`Faction`,`ModelId1`,`Scale`,`MeleeBaseAttackTime`) VALUES (5090042,'Classic fixture',10,10,100,100,20,20,3,35,100,1,2000);";
        let args = crate::parse_args_from([
            "--dump",
            "fixture.sql",
            "--family",
            "creatures",
            "--world-profile",
            "full-world",
        ])
        .unwrap()
        .world_import_args()
        .unwrap();
        for (reimport, health, damage) in [(false, 100, 20), (true, 400, 50)] {
            if reimport {
                let plan = build_dump_plan(classic, &args, &None, &None).unwrap();
                for statement in plan.stmts {
                    node.assert_sql(&statement);
                }
                assert!(node
                    .query_rows("SELECT id FROM game_creature_level_stats WHERE entry = 5090042")
                    .is_empty());
            }
            node.assert_call("debug_spawn_at_feet", &["1", "5090042", "5"]);
            let rows =
                node.query_rows("SELECT guid,health FROM game_world_entity WHERE entry = 5090042");
            let creature = rows
                .iter()
                .find(|row| row["health"] == health.to_string())
                .unwrap();
            node.assert_call("debug_compute_swing", &[&creature["guid"], "1"]);
            let swing = node
                .query_rows("SELECT base_min,base_max FROM game_debug_readout WHERE key = 'swing'");
            assert_eq!(swing[0]["base_min"], damage.to_string());
            assert_eq!(swing[0]["base_max"], damage.to_string());
        }
    }

    #[test]
    fn vmangos_spawn_choices_and_dependencies_stop_at_the_first_zero() {
        let source = template_source();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection.execute_batch("CREATE TABLE creature (guid INTEGER, id INTEGER, id2 INTEGER, id3 INTEGER, id4 INTEGER, id5 INTEGER, spawn_flags INTEGER, map INTEGER, position_x REAL, position_y REAL, position_z REAL, patch_min INTEGER, patch_max INTEGER);
            INSERT INTO creature VALUES (1,5090042,0,5090043,0,0,0,0,1,2,3,0,10),(2,0,5090044,0,0,0,0,0,1,2,3,0,10);").unwrap();
        let args = crate::parse_args_from([
            "--dump",
            "fixture.sqlite",
            "--map",
            "1",
            "--include-map",
            "0",
        ])
        .unwrap()
        .world_import_args()
        .unwrap();
        let entries = vmangos_creature_entries(&source, &args).unwrap();
        assert!(entries.contains(&5090042));
        assert!(!entries.contains(&5090043));
        assert!(!entries.contains(&5090044));
    }

    #[test]
    fn zero_primary_entry_skips_the_spawn_and_reports_its_guid() {
        let source = template_source();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection.execute_batch("CREATE TABLE creature (guid INTEGER, id INTEGER, id2 INTEGER, id3 INTEGER, id4 INTEGER, id5 INTEGER, spawn_flags INTEGER, map INTEGER, patch_min INTEGER, patch_max INTEGER);
            INSERT INTO creature VALUES (42,0,5090042,0,0,0,0,0,0,10);").unwrap();
        for table in [
            "game_event_creature",
            "pool_template",
            "pool_creature",
            "pool_creature_template",
            "pool_pool",
            "pool_gameobject",
            "pool_gameobject_template",
            "creature_movement",
            "creature_movement_template",
            "creature_groups",
        ] {
            connection
                .execute_batch(&format!(
                    "CREATE TABLE {table} (id INTEGER, patch_min INTEGER, patch_max INTEGER)"
                ))
                .unwrap();
        }
        let args = crate::parse_args_from([
            "--dump",
            "fixture.sqlite",
            "--family",
            "creatures",
            "--world-profile",
            "full-world",
        ])
        .unwrap()
        .world_import_args()
        .unwrap();
        let imported = plan(&source, &args).unwrap();
        assert_eq!(imported.spawn_row_count, 0);
        assert!(imported.spawn_batches.is_empty());
        let manifest: serde_json::Value =
            serde_json::from_str(imported.world_source_manifest.as_ref().unwrap()).unwrap();
        assert!(manifest["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["code"] == "creatures.spawn-entry-zero"
                && finding["source_ids"] == serde_json::json!(["42"])));
    }

    #[test]
    fn class_level_stats_apply_multipliers_variance_and_source_rounding() {
        let base = numbers(
            "creature_classlevelstats",
            &[
                ("health", "101"),
                ("melee_damage", "12.5"),
                ("armor", "101"),
            ],
        );
        let template = numbers(
            "creature_template",
            &[
                ("entry", "10"),
                ("health_multiplier", "1.5"),
                ("damage_multiplier", "2"),
                ("armor_multiplier", "0.5"),
                ("damage_variance", "0.2"),
            ],
        );
        assert_eq!(derived_stats(&template, &base).unwrap(), (152, 20, 30, 50));
        let zero = numbers(
            "creature_classlevelstats",
            &[("health", "0"), ("melee_damage", "0"), ("armor", "0")],
        );
        assert_eq!(derived_stats(&template, &zero).unwrap().0, 1);
    }

    #[test]
    fn extrapolated_armor_uses_nonzero_endpoints() {
        assert_eq!(extrapolated_stat(0.0, 0.0, 3, 2, true), 1.0);
        assert_eq!(extrapolated_stat(0.0, 5.0, 3, 2, true), 9.0);
        assert_eq!(extrapolated_stat(0.0, 5.0, 3, 2, false), 10.0);
    }

    #[test]
    fn only_waypoint_movement_emits_source_paths() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE creature_movement (id INTEGER, point INTEGER, path_id INTEGER, position_x REAL, position_y REAL, position_z REAL, script_id INTEGER, waittime INTEGER, wander_distance REAL, orientation REAL);
            INSERT INTO creature_movement VALUES (1,1,0,10,20,30,0,0,0,0);
            CREATE TABLE creature_movement_template (entry INTEGER, point INTEGER, path_id INTEGER, position_x REAL, position_y REAL, position_z REAL, script_id INTEGER, waittime INTEGER, wander_distance REAL, orientation REAL);
            INSERT INTO creature_movement_template VALUES (10,1,0,40,50,60,0,0,0,0);").unwrap();
        let source = crate::world_source::WorldSource::Vmangos(connection);
        for movement in [0, 1, 2, 3] {
            let spawns = [
                (1, 10, 0.0, 0.0, 0.0, 0.0, movement, 60, 0),
                (2, 10, 0.0, 0.0, 0.0, 0.0, movement, 60, 0),
            ];
            let mut statements = Vec::new();
            waypoints(&source, &spawns, &mut statements, &mut Findings::new()).unwrap();
            if movement == 2 {
                assert_eq!(insert_row_count(&statements), 2);
                let sql = statements.join("\n");
                assert!(sql.contains(&format!("(1,{},10,20,30)", world_guid(10, 1))));
                assert!(sql.contains(&format!("(2,{},40,50,60)", world_guid(10, 2))));
            } else {
                assert_eq!(insert_row_count(&statements), 0);
            }
        }
    }

    #[test]
    fn scripted_immunity_does_not_make_imported_creatures_unattackable() {
        let row = numbers(
            "creature_template",
            &[
                ("static_flags1", "96"),
                ("static_flags2", "0"),
                ("rank", "3"),
            ],
        );
        assert_eq!(flags(&row, 0).unwrap().0, 0x40);
    }

    #[test]
    fn invalid_stats_fail_before_sql_generation() {
        let base = numbers(
            "creature_classlevelstats",
            &[
                ("health", "101"),
                ("melee_damage", "12.5"),
                ("armor", "101"),
            ],
        );
        for variance in ["-1", "1.1", "NaN"] {
            let template = numbers(
                "creature_template",
                &[
                    ("entry", "10"),
                    ("health_multiplier", "1"),
                    ("damage_multiplier", "1"),
                    ("armor_multiplier", "1"),
                    ("damage_variance", variance),
                ],
            );
            assert!(derived_stats(&template, &base).is_err());
        }
    }

    #[test]
    fn source_static_flags_supply_unit_and_client_type_flags() {
        let row = numbers(
            "creature_template",
            &[
                ("static_flags1", "4294967295"),
                ("static_flags2", "8"),
                ("rank", "1"),
            ],
        );
        assert_eq!(flags(&row, 0).unwrap(), (0x0200_9040, 0x7f));
        let row = numbers(
            "creature_template",
            &[
                ("static_flags1", "0"),
                ("static_flags2", "0"),
                ("rank", "0"),
            ],
        );
        assert_eq!(flags(&row, 0).unwrap(), (0, 0));
    }

    #[test]
    fn only_immunity_bits_with_a_proven_clear_are_restored() {
        let row = numbers(
            "creature_template",
            &[
                ("static_flags1", "96"),
                ("static_flags2", "0"),
                ("rank", "0"),
            ],
        );
        assert_eq!(flags(&row, 0).unwrap().0, 0);
        assert_eq!(flags(&row, 0x20).unwrap().0, 0x100);
        assert_eq!(flags(&row, 0x40).unwrap().0, 0x200);
    }
    #[test]
    fn selected_model_gender_follows_the_display_and_build() {
        let source = template_source();
        let WorldSource::Vmangos(connection) = &source else {
            unreachable!()
        };
        connection.execute_batch("INSERT INTO creature_display_info_addon VALUES (100,4222,0,0,0.5),(100,5875,1,0,0.75),(100,6000,2,0,1.0);").unwrap();
        let mut statements = Vec::new();
        templates(
            &source,
            &HashSet::from([5090042]),
            &HashSet::new(),
            None,
            &mut statements,
            &mut Findings::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let display = statements
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_display "))
            .unwrap();
        assert!(display.contains("(id,entry,display_id,scale,weight,gender,bounding_radius)"));
        assert!(display.ends_with(",5090042,100,1,1,1,0.75)"));
    }
}
