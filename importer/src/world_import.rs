#[path = "world_import_trainers.rs"]
mod trainers;
#[path = "world_import_creatures.rs"]
mod vmangos_creatures;
#[path = "world_import_gossip.rs"]
mod vmangos_gossip;
#[path = "world_import_loot.rs"]
mod vmangos_loot;
#[path = "world_import_quests.rs"]
mod vmangos_quests;
#[path = "world_import_spellmeta.rs"]
mod vmangos_spellmeta;

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use lyracore_package_delta::{
    packed_class_level_id, packed_creature_spawn_guid, packed_gameobject_spawn_guid,
    packed_race_class_id, packed_race_class_level_id,
};

use crate::dump::{
    ai, at, cls, go, got, insert_row_count, it, lvl, mlt, nv, parse_table, pc, pca, pci, pcs,
    push_insert, qr, qt, sql_text, TableRows,
};
use crate::shard::Shard;
use crate::world_import_scope::{WorldImportProfile, WorldImportScope};
use crate::world_source::WorldSource;
use crate::{
    creature_population, dbc, eventai, gossip_conditions, item_property, package_delta, spell,
};

#[derive(Clone)]
pub(crate) struct WorldImportArgs {
    pub(crate) dbc: Option<String>,
    pub(crate) family: Option<String>,
    pub(crate) world_profile: Option<WorldImportProfile>,
    pub(crate) scope: WorldImportScope,
    pub(crate) include_creatures: Vec<u64>,
    pub(crate) packages: Option<String>,
    pub(crate) source_sha: String,
    pub(crate) apply: bool,
    pub(crate) shard: Shard,
}

const ALL_PLAYABLE_CLASS_MASK: u32 = 0x5df;
const ALL_PLAYABLE_RACE_MASK: u32 = 0xff;

pub(crate) fn world_guid(entry: u64, db_guid: u64) -> u64 {
    packed_creature_spawn_guid(entry as u32, db_guid as u32)
}

pub(crate) fn go_guid(db_guid: u32) -> u64 {
    packed_gameobject_spawn_guid(db_guid)
}

const GATHER_RESPAWN_SECS: u32 = 300;

const GATHER_NODES: &[(u64, u32, u32, u32, u32)] = &[
    (1731, 186, 2770, 1, 100),
    (1732, 186, 2771, 65, 140),
    (1617, 182, 765, 1, 75),
    (1618, 182, 2447, 1, 75),
    (1619, 182, 2449, 15, 90),
    (1620, 182, 785, 50, 125),
    (1621, 182, 2450, 70, 145),
    (1622, 182, 2453, 100, 175),
];

pub(crate) fn gather_node(entry: u64) -> Option<(u32, u32, u32, u32)> {
    GATHER_NODES
        .iter()
        .find(|n| n.0 == entry)
        .map(|n| (n.1, n.2, n.3, n.4))
}

pub(crate) const GO_DOOR: u8 = 0;
pub(crate) const GO_BUTTON: u8 = 1;
const GO_QUESTGIVER: u8 = lyracore_shared::constants::go_type::QUESTGIVER;
const GO_CHEST: u8 = 3;
const GO_GOOBER: u8 = 10;
const GO_MEETINGSTONE: u8 = lyracore_shared::constants::go_type::MEETINGSTONE;
// Synthetic GATHER collides with vanilla FISHINGHOLE type 25; classify raw types first.
const GO_GATHER: u8 = 25;
const VANILLA_FISHINGHOLE_TYPE: u32 = 25;

pub(crate) fn classify_go_type(entry: u64, raw_type: u32) -> Option<u8> {
    match raw_type {
        3 if gather_node(entry).is_some() => Some(GO_GATHER),
        VANILLA_FISHINGHOLE_TYPE => None,
        t if t <= u8::MAX as u32 => Some(t as u8),
        _ => None,
    }
}

fn go_template_row(
    entry: u64,
    stored_type: u8,
    disp: u32,
    name: &str,
    data0: u32,
    data1: u32,
    size: f32,
) -> (String, Option<u32>) {
    let n = sql_text(name);
    match stored_type {
        GO_GATHER => {
            let (skill, item, req, gray) = gather_node(entry)
                .expect("classify_go_type only returns GO_GATHER for a GATHER_NODES entry");
            (format!("({entry},{GO_GATHER},{disp},{n},{item},{req},{skill},{GATHER_RESPAWN_SECS},{gray},0,{size})"), None)
        }
        GO_CHEST => {
            let loot_id = data1;
            (
                format!("({entry},{GO_CHEST},{disp},{n},0,{loot_id},0,0,0,{data0},{size})"),
                (loot_id != 0).then_some(loot_id),
            )
        }
        GO_GOOBER => (
            format!("({entry},{GO_GOOBER},{disp},{n},0,0,0,0,0,{data0},{size})"),
            None,
        ),
        GO_DOOR => (
            format!("({entry},{GO_DOOR},{disp},{n},0,0,0,0,0,{data1},{size})"),
            None,
        ),
        GO_BUTTON => (
            format!("({entry},{GO_BUTTON},{disp},{n},0,0,0,0,0,{data1},{size})"),
            None,
        ),
        8 => (
            format!("({entry},8,{disp},{n},{data0},{data1},0,0,0,0,{size})"),
            None,
        ),
        other => (
            format!("({entry},{other},{disp},{n},0,0,0,0,0,0,{size})"),
            None,
        ),
    }
}

// -1 means every playable class or race.
fn normalize_item_mask(source: i64, unrestricted: u32) -> Result<u32> {
    if source == -1 {
        Ok(unrestricted)
    } else {
        u32::try_from(source).context("item_template: invalid class or race mask")
    }
}

fn build_stats_sql(dump: &(impl crate::dump::TableRows + ?Sized)) -> Result<Vec<String>> {
    let mut stmts: Vec<String> = vec![
        "DELETE FROM game_class_level_stats WHERE class_level >= 0".into(),
        "DELETE FROM game_level_stats WHERE race_class_level >= 0".into(),
    ];
    let cls_rows: Vec<String> = parse_table(dump, "player_classlevelstats")?
        .iter()
        .map(|r| -> Result<_> {
            let class = r.number::<u32>(cls::CLASS)?;
            let level = r.number::<u32>(cls::LEVEL)?;
            Ok(format!(
                "({},{class},{level},{},{})",
                packed_class_level_id(class as u8, level as u8),
                r.numeric_text(cls::BASEHP)?,
                r.numeric_text(cls::BASEMANA)?,
            ))
        })
        .collect::<Result<_>>()?;
    push_insert(
        &mut stmts,
        "game_class_level_stats",
        "class_level,class,level,base_health,base_mana",
        &cls_rows,
    );
    let lvl_rows: Vec<String> = parse_table(dump, "player_levelstats")?
        .iter()
        .map(|r| -> Result<_> {
            let race = r.number::<u32>(lvl::RACE)?;
            let class = r.number::<u32>(lvl::CLASS)?;
            let level = r.number::<u32>(lvl::LEVEL)?;
            Ok(format!(
                "({},{race},{class},{level},{},{},{},{},{})",
                packed_race_class_level_id(race as u8, class as u8, level as u8),
                r.numeric_text(lvl::STR)?,
                r.numeric_text(lvl::AGI)?,
                r.numeric_text(lvl::STA)?,
                r.numeric_text(lvl::INTE)?,
                r.numeric_text(lvl::SPI)?,
            ))
        })
        .collect::<Result<_>>()?;
    push_insert(
        &mut stmts,
        "game_level_stats",
        "race_class_level,race,class,level,strength,agility,stamina,intellect,spirit",
        &lvl_rows,
    );
    eprintln!(
        "stats: {} class/level rows, {} race/class/level rows",
        cls_rows.len(),
        lvl_rows.len()
    );
    Ok(stmts)
}

fn build_start_position_sql(dump: &(impl crate::dump::TableRows + ?Sized)) -> Result<Vec<String>> {
    let rows: Vec<String> = parse_table(dump, "playercreateinfo")?
        .iter()
        .map(|r| -> Result<_> {
            let race = r.number::<u32>(pc::RACE)?;
            let class = r.number::<u32>(pc::CLASS)?;
            Ok(format!(
                "({},{race},{class},{},{},{},{},{},{},0)",
                packed_race_class_id(race as u8, class as u8),
                r.numeric_text(pc::MAP)?,
                r.numeric_text(pc::ZONE)?,
                r.numeric_text(pc::X)?,
                r.numeric_text(pc::Y)?,
                r.numeric_text(pc::Z)?,
                r.numeric_text(pc::O)?,
            ))
        })
        .collect::<Result<_>>()?;
    let mut stmts = vec!["DELETE FROM game_start_position WHERE race_class >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_start_position",
        "race_class,race,class,map_id,zone_id,x,y,z,orientation,display_id",
        &rows,
    );
    eprintln!("start positions: {} (race,class) rows", rows.len());
    Ok(stmts)
}

fn build_graveyard_zone_sql(dump: &(impl crate::dump::TableRows + ?Sized)) -> Result<Vec<String>> {
    let rows = parse_table(dump, "game_graveyard_zone")?
        .iter()
        .enumerate()
        .map(|(i, row)| -> Result<String> {
            let zone_column = "ghost_zone";
            let safe_loc_id: u32 = row.number("id")?;
            let zone_id: u32 = row.number(zone_column)?;
            let faction: u32 = row.number("faction")?;
            Ok(format!(
                "({},{safe_loc_id},{zone_id},{faction})",
                i as u64 + 1
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut stmts = vec!["DELETE FROM game_graveyard_zone WHERE row_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_graveyard_zone",
        "row_id,safe_loc_id,zone_id,faction",
        &rows,
    );
    eprintln!("graveyard zones: {} rows", rows.len());
    Ok(stmts)
}

fn build_areatrigger_teleport_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<(Vec<String>, usize)> {
    let rows: Vec<String> = parse_table(dump, "areatrigger_teleport")?
        .iter()
        .map(|r| -> Result<_> {
            let trigger_id = r.number::<u32>(at::ID)?;
            let target_map = r.number::<u32>(at::TARGET_MAP)?;
            let x = r.number::<f32>(at::TARGET_X)?;
            let y = r.number::<f32>(at::TARGET_Y)?;
            let z = r.number::<f32>(at::TARGET_Z)?;
            let o = r.number::<f32>(at::TARGET_O)?;
            let name = sql_text(r.text(at::NAME)?.trim());
            Ok(format!(
                "({trigger_id},{target_map},{x},{y},{z},{o},{name})"
            ))
        })
        .collect::<Result<_>>()?;
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_areatrigger_teleport WHERE trigger_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_areatrigger_teleport",
        "trigger_id,target_map,x,y,z,o,name",
        &rows,
    );
    eprintln!("areatrigger_teleport: {n} portal rows");
    Ok((stmts, n))
}

fn build_tavern_area_trigger_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<Vec<String>> {
    let ids = parse_table(dump, "areatrigger_tavern")?
        .iter()
        .map(|row| row.number::<u32>("id"))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    let rows = ids.iter().map(|id| format!("({id})")).collect::<Vec<_>>();
    let mut stmts = vec!["DELETE FROM game_tavern_area_trigger WHERE trigger_id >= 0".to_string()];
    push_insert(&mut stmts, "game_tavern_area_trigger", "trigger_id", &rows);
    eprintln!("tavern area triggers: {} rows", rows.len());
    Ok(stmts)
}

fn distinct_race_class_combos(rows: &[(u32, u32)]) -> usize {
    let mut combos: Vec<(u32, u32)> = rows.to_vec();
    combos.sort_unstable();
    combos.dedup();
    combos.len()
}

fn build_createinfo_spell_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<Vec<String>> {
    let mut race_class_pairs: Vec<(u32, u32)> = Vec::new();
    let rows: Vec<String> = parse_table(dump, "playercreateinfo_spell")?
        .iter()
        .enumerate()
        .map(|(i, r)| -> Result<_> {
            let race = r.number::<u32>(pcs::RACE)?;
            let class = r.number::<u32>(pcs::CLASS)?;
            let spell = r.number::<u32>("spell")?;
            race_class_pairs.push((race, class));
            Ok(format!("({},{race},{class},{spell})", i as u64 + 1))
        })
        .collect::<Result<_>>()?;
    let mut stmts = vec!["DELETE FROM game_createinfo_spell WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_createinfo_spell",
        "id,race,class,spell_id",
        &rows,
    );
    eprintln!(
        "createinfo spells: {} rows across {} (race,class) combos",
        rows.len(),
        distinct_race_class_combos(&race_class_pairs)
    );
    Ok(stmts)
}

fn build_createinfo_action_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<Vec<String>> {
    let mut race_class_pairs: Vec<(u32, u32)> = Vec::new();
    let rows: Vec<String> = parse_table(dump, "playercreateinfo_action")?
        .iter()
        .enumerate()
        .map(|(i, r)| -> Result<_> {
            let race = r.number::<u32>(pca::RACE)?;
            let class = r.number::<u32>(pca::CLASS)?;
            let button = r.number::<u32>(pca::BUTTON)?;
            let action = r.number::<u32>(pca::ACTION)?;
            let action_type = r.number::<u32>(pca::TYPE)?;
            race_class_pairs.push((race, class));
            Ok(format!(
                "({},{race},{class},{button},{action},{action_type})",
                i as u64 + 1
            ))
        })
        .collect::<Result<_>>()?;
    let mut stmts = vec!["DELETE FROM game_createinfo_action WHERE row_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_createinfo_action",
        "row_id,race,class,button,action,action_type",
        &rows,
    );
    eprintln!(
        "createinfo actions: {} rows across {} (race,class) combos",
        rows.len(),
        distinct_race_class_combos(&race_class_pairs)
    );
    Ok(stmts)
}

// Starter extras need an id band disjoint from the DBC outfit clear-and-reload range.
const CREATEINFO_ITEM_ID_FLOOR: u64 = 1_000_000;

fn build_createinfo_item_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
    dbc_dir: Option<&str>,
) -> Result<Vec<String>> {
    let outfit_seen: std::collections::HashSet<(u64, i32)> = match dbc_dir {
        Some(dir) => {
            let mut chain = dbc::open_chain(std::path::Path::new(dir))?;
            dbc::outfit_item_set(&mut chain)?
        }
        None => std::collections::HashSet::new(),
    };
    let mut race_class_pairs: Vec<(u32, u32)> = Vec::new();
    let mut seen_extra: std::collections::HashSet<(u64, i32)> = std::collections::HashSet::new();
    let mut deduped = 0usize;
    let mut id = CREATEINFO_ITEM_ID_FLOOR;
    let mut rows: Vec<String> = Vec::new();
    for r in parse_table(dump, "playercreateinfo_item")?.iter() {
        let race = r.number::<u32>(pci::RACE)?;
        let class = r.number::<u32>(pci::CLASS)?;
        let item = r.number::<i32>(pci::ITEM)?;
        if item <= 0 {
            continue;
        }
        let race_class = packed_race_class_id(race as u8, class as u8);
        if outfit_seen.contains(&(race_class, item)) {
            deduped += 1;
            continue;
        }
        if !seen_extra.insert((race_class, item)) {
            continue;
        }
        race_class_pairs.push((race, class));
        rows.push(format!("({id},{race_class},{item})"));
        id += 1;
    }
    let n = rows.len();
    let mut stmts = vec![format!(
        "DELETE FROM game_start_item WHERE id >= {CREATEINFO_ITEM_ID_FLOOR}"
    )];
    push_insert(
        &mut stmts,
        "game_start_item",
        "id,race_class,item_entry",
        &rows,
    );
    eprintln!(
        "createinfo item extras: {n} rows across {} (race,class) combos ({deduped} deduped against the CharStartOutfit outfit)",
        distinct_race_class_combos(&race_class_pairs)
    );
    Ok(stmts)
}

fn derived_spell_rank_rows(
    links: &[(u32, u32)],
    explicit: &std::collections::BTreeSet<u32>,
) -> Result<Vec<String>> {
    use std::collections::{BTreeMap, BTreeSet};
    let links: BTreeSet<_> = links
        .iter()
        .copied()
        .filter(|(spell, next)| *spell != 0 && *next != 0)
        .collect();
    let mut neighbors: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(spell, next) in &links {
        neighbors.entry(spell).or_default().push(next);
        neighbors.entry(next).or_default().push(spell);
    }
    let mut ignored = explicit.clone();
    let mut pending: Vec<_> = explicit.iter().copied().collect();
    while let Some(spell) = pending.pop() {
        for &next in neighbors.get(&spell).into_iter().flatten() {
            if ignored.insert(next) {
                pending.push(next);
            }
        }
    }
    let mut successors = BTreeMap::new();
    let mut predecessors = BTreeMap::new();
    for &(spell, next) in &links {
        if ignored.contains(&spell) {
            continue;
        }
        if successors
            .insert(spell, next)
            .is_some_and(|seen| seen != next)
            || predecessors
                .insert(next, spell)
                .is_some_and(|seen| seen != spell)
        {
            anyhow::bail!("ambiguous client spell rank link {spell} -> {next}");
        }
    }
    let mut visited = BTreeSet::new();
    let mut rows = Vec::new();
    for &first in successors
        .keys()
        .filter(|spell| !predecessors.contains_key(spell))
    {
        let mut family = vec![first];
        let mut spell = first;
        while let Some(&next) = successors.get(&spell) {
            if !visited.insert(spell) || family.len() >= usize::from(u8::MAX) {
                anyhow::bail!("cyclic or oversized client spell family {first}");
            }
            family.push(next);
            spell = next;
        }
        visited.insert(spell);
        let mut previous = 0;
        for (index, spell) in family.into_iter().enumerate() {
            rows.push(format!("({spell},{previous},{first},{},0)", index + 1));
            previous = spell;
        }
    }
    if successors.keys().any(|spell| !visited.contains(spell)) {
        anyhow::bail!("client spell rank links contain a cycle");
    }
    eprintln!("spell chain: {} additional derived ranks", rows.len());
    Ok(rows)
}

const SYNTHETIC_PROC_EVENT_FLOOR: u32 = 50_000;

type NpcTextSlot = (String, String, f32);

fn slots_first_nonempty(slots: &[NpcTextSlot; 8]) -> String {
    for (male, female, _) in slots {
        if !male.is_empty() {
            return male.clone();
        }
        if !female.is_empty() {
            return female.clone();
        }
    }
    String::new()
}

struct GossipEtl {
    menu_rows: Vec<String>,
    npc_text_rows: Vec<String>,
    npc_text_slot_rows: Vec<String>,
    option_rows: Vec<String>,
    profile_rows: Vec<String>,
    profile_option_rows: Vec<String>,
    condition_rows: Vec<String>,
}

struct QuestEtl {
    entries: std::collections::HashSet<u64>,
    templates: Vec<String>,
    texts: Vec<String>,
    objectives: Vec<String>,
    cast_objectives: Vec<String>,
    reward_items: Vec<String>,
    reward_spells: Vec<String>,
    reward_choices: Vec<String>,
    relations: Vec<String>,
    go_relations: Vec<String>,
    reward_mail: Vec<String>,
    reward_mail_template_ids: std::collections::HashSet<u64>,
}

#[allow(clippy::type_complexity)]
fn parse_creature_drops(
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<std::collections::HashMap<u64, Vec<(u64, u32, u32, u32, bool)>>> {
    vmangos_loot::parse(dump, "creature_loot_template")
}

fn creature_drop_item_set(
    dump: &(impl crate::dump::TableRows + ?Sized),
    creature_loot_ids: &std::collections::HashMap<u64, u64>,
) -> Result<std::collections::HashSet<u64>> {
    let drops_by_lootid = parse_creature_drops(dump)?;
    let mut set = std::collections::HashSet::new();
    for lootid in creature_loot_ids.values() {
        if let Some(drops) = drops_by_lootid.get(lootid) {
            for &(item, ..) in drops {
                set.insert(item);
            }
        }
    }
    Ok(set)
}

fn gameobject_loot_item_set(
    dump: &(impl crate::dump::TableRows + ?Sized),
    used_go: &std::collections::HashSet<u64>,
) -> Result<std::collections::HashSet<u64>> {
    const GO_TYPE_CHEST: u64 = 3;
    let loot = vmangos_loot::parse(dump, "gameobject_loot_template")?;
    let mut set = std::collections::HashSet::new();
    for row in parse_table(dump, "gameobject_template")?.iter() {
        let entry: u64 = row.number(got::ENTRY)?;
        let ty: u64 = row.number(got::TYPE)?;
        if ty != GO_TYPE_CHEST || !used_go.contains(&entry) {
            continue;
        }
        let lootid: u64 = row.number(got::DATA1)?;
        if let Some(drops) = loot.get(&lootid) {
            for &(item, ..) in drops {
                set.insert(item);
            }
        }
    }
    Ok(set)
}

fn vendor_stock_item_set(
    dump: &(impl crate::dump::TableRows + ?Sized),
    local_creatures: &std::collections::HashSet<u64>,
) -> Result<std::collections::HashSet<u64>> {
    let (entry_column, vendor_column) = ("entry", "vendor_id");
    let mut set = std::collections::HashSet::new();
    for row in parse_table(dump, "npc_vendor")?.iter() {
        let entry: u64 = row.number(nv::ENTRY)?;
        if local_creatures.contains(&entry) {
            let item: u64 = row.number(nv::ITEM)?;
            if item > 0 {
                set.insert(item);
            }
        }
    }
    let mut tmpl_items: std::collections::HashMap<u64, Vec<u64>> = std::collections::HashMap::new();
    for row in parse_table(dump, "npc_vendor_template")?.iter() {
        let t: u64 = row.number("entry")?;
        let item: u64 = row.number("item")?;
        if t > 0 && item > 0 {
            tmpl_items.entry(t).or_default().push(item);
        }
    }
    for row in parse_table(dump, "creature_template")?.iter() {
        let entry: u64 = row.number(entry_column)?;
        if !local_creatures.contains(&entry) {
            continue;
        }
        let vt: u64 = row.number(vendor_column)?;
        if let Some(items) = tmpl_items.get(&vt) {
            set.extend(items.iter().copied());
        }
    }
    let forbidden = forbidden_items(dump)?;
    set.retain(|item| !forbidden.contains(item));
    Ok(set)
}

fn quest_relations(
    dump: &(impl TableRows + ?Sized),
    table: &str,
    entries: &std::collections::HashSet<u64>,
) -> Result<Vec<(u64, u64)>> {
    parse_table(dump, table)?
        .iter()
        .map(|row| -> Result<_> {
            let entry = row.number::<u64>(qr::CREATURE)?;
            let quest = row.number::<u64>(qr::QUEST)?;
            Ok((entries.contains(&entry) && quest > 0).then_some((entry, quest)))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_>>()
}

#[allow(clippy::too_many_lines)]
fn build_quests(
    dump: &(impl crate::dump::TableRows + ?Sized),
    catalogue_creatures: &std::collections::HashSet<u64>,
    catalogue_gameobjects: &std::collections::HashSet<u64>,
    goobers: &std::collections::HashSet<u64>,
    obtainable_items: &std::collections::HashSet<u64>,
    local_givers: Option<(
        &std::collections::HashSet<u64>,
        &std::collections::HashSet<u64>,
    )>,
) -> Result<QuestEtl> {
    use std::collections::HashSet;

    let starts = quest_relations(dump, "creature_questrelation", catalogue_creatures)?;
    let ends = quest_relations(dump, "creature_involvedrelation", catalogue_creatures)?;
    let go_starts = quest_relations(dump, "gameobject_questrelation", catalogue_gameobjects)?;
    let go_ends = quest_relations(dump, "gameobject_involvedrelation", catalogue_gameobjects)?;
    let candidate_quests: HashSet<u64> = starts
        .iter()
        .chain(ends.iter())
        .chain(go_starts.iter())
        .chain(go_ends.iter())
        .map(|&(_, q)| q)
        .collect();

    let mut quest_rows: Vec<String> = Vec::new();
    let mut quest_text_rows: Vec<String> = Vec::new();
    let mut objective_rows: Vec<String> = Vec::new();
    let mut cast_objective_rows: Vec<String> = Vec::new();
    let mut reward_item_rows: Vec<String> = Vec::new();
    let mut reward_spell_rows = Vec::new();
    let reward_learn_spells = vmangos_quests::learn_spells(dump)?;
    let mut reward_choice_rows: Vec<String> = Vec::new();

    let mut valid_quests: HashSet<u64> = HashSet::new();
    let mut reward_mail_rows: Vec<String> = Vec::new();
    let mut reward_mail_template_ids: HashSet<u64> = HashSet::new();
    let mut reward_mail_quests: HashSet<u64> = HashSet::new();

    let mut obj_id: u64 = 1;
    let mut cast_obj_id: u64 = 1;
    let mut rew_id: u64 = 1;
    let mut rew_choice_id: u64 = 1;
    let mut quest_obj_count: std::collections::HashMap<u64, u8> = std::collections::HashMap::new();
    for row in parse_table(dump, "quest_template")?.iter() {
        let entry: u64 = row.number(qt::ENTRY)?;
        if entry == 0 || !candidate_quests.contains(&entry) {
            continue;
        }
        valid_quests.insert(entry);
        let text = |column| {
            row.optional_text(column)
                .map(|text| crate::dump::sql_str(text.unwrap_or_default().trim()))
        };
        let mail_template: u32 = row.number(qt::REW_MAIL_TEMPLATE_ID)?;
        if mail_template != 0 {
            let mail_delay: u32 = row.number(qt::REW_MAIL_DELAY_SECS)?;
            let mail_money = row.number::<u32>("RewMailMoney")?;
            reward_mail_rows.push(format!(
                "({entry},{mail_template},{mail_delay},0,{mail_money})"
            ));
            reward_mail_template_ids.insert(mail_template as u64);
            reward_mail_quests.insert(entry);
        }
        let reward_cast: u32 = row.number(qt::REW_SPELL_CAST)?;
        let reward_cast = if reward_cast != 0 {
            reward_cast
        } else {
            row.number(qt::REW_SPELL)?
        };
        if let Some(learned) = reward_learn_spells.get(&reward_cast) {
            reward_spell_rows.push(format!("({entry},{learned})"));
        }

        let reward_xp = row.number::<u32>("RewXP")?;
        let min_level: u32 = row.number(qt::MIN_LEVEL)?;
        let quest_level: i64 = row.number(qt::QUEST_LEVEL)?;
        let money: i64 = row.number(qt::REW_OR_REQ_MONEY)?;
        let reward_money_max_level: i64 = row.number(qt::REW_MONEY_MAX_LEVEL)?;
        let prev_quest: i64 = row.number(qt::PREV_QUEST_ID)?;
        let special_flags: u32 = row.number(qt::SPECIAL_FLAGS)?;
        let repeatable = (special_flags & 1) != 0;
        let req_races: u32 = row.number(qt::REQUIRED_RACES)?;
        let req_classes: u32 = row.number(qt::REQUIRED_CLASSES)?;
        let zone_or_sort: i64 = row.number(qt::ZONE_OR_SORT)?;
        let rf1: u32 = row.number(qt::REW_REP_FACTION1)?;
        let rv1: i64 = row.number(qt::REW_REP_VALUE1)?;
        let rf2: u32 = row.number("RewRepFaction2")?;
        let rv2: i64 = row.number("RewRepValue2")?;
        let src_item: u64 = row.number(qt::SRC_ITEM_ID)?;
        let src_item_count: u32 = row.number(qt::SRC_ITEM_COUNT)?;
        let next_quest_raw: i64 = row.number(qt::NEXT_QUEST_IN_CHAIN)?;
        let limit_time_raw: i64 = row.number(qt::LIMIT_TIME)?;
        let quest_type: u32 = row.number(qt::TYPE)?;
        quest_rows.push(format!(
            "({entry},{min_level},{ql},{title},{money},{reward_xp},{prev},{req_races},{req_classes},{zone_or_sort},{rf1},{rv1},{rf2},{rv2},{src_item},{src_item_count},{repeatable},{next_quest},{limit_time},{rmm},{quest_type})",
            ql = quest_level.max(0),
            title = text(qt::TITLE)?,
            money = money.max(0),
            prev = prev_quest.max(0),
            next_quest = next_quest_raw.max(0),
            limit_time = limit_time_raw.max(0),
            rmm = reward_money_max_level.max(0),
        ));
        quest_text_rows.push(format!(
            "({entry},{details},{objectives},{offer},{request})",
            details = text(qt::DETAILS)?,
            objectives = text(qt::OBJECTIVES)?,
            offer = text(qt::OFFER_REWARD_TEXT)?,
            request = text(qt::REQUEST_ITEMS_TEXT)?,
        ));
        let mut obj_index: u8 = 0;
        for i in 0..4 {
            let id: i64 = row.number(&format!("ReqCreatureOrGOId{}", i + 1))?;
            let count: u32 = row.number(&format!("ReqCreatureOrGOCount{}", i + 1))?;
            let spell_id: u32 = row.number(&format!("ReqSpellCast{}", i + 1))?;
            if count == 0 {
                continue;
            }
            if id > 0 {
                objective_rows.push(format!("({obj_id},{entry},{obj_index},0,{id},{count})",));
                if spell_id != 0 {
                    cast_objective_rows
                        .push(format!("({cast_obj_id},{entry},{obj_index},{spell_id})"));
                    cast_obj_id += 1;
                }
                obj_id += 1;
                obj_index += 1;
            } else if id < 0 {
                let go_entry = id.unsigned_abs();
                if goobers.contains(&go_entry) {
                    objective_rows.push(format!(
                        "({obj_id},{entry},{obj_index},2,{go_entry},{count})",
                    ));
                    if spell_id != 0 {
                        cast_objective_rows
                            .push(format!("({cast_obj_id},{entry},{obj_index},{spell_id})"));
                        cast_obj_id += 1;
                    }
                    obj_id += 1;
                    obj_index += 1;
                }
            }
        }
        for i in 0..4 {
            let item: i64 = row.number(&format!("ReqItemId{}", i + 1))?;
            let count: u32 = row.number(&format!("ReqItemCount{}", i + 1))?;
            let obtainable =
                item > 0 && (obtainable_items.contains(&(item as u64)) || item as u64 == src_item);
            if obtainable && count > 0 {
                objective_rows.push(format!("({obj_id},{entry},{obj_index},1,{item},{count})",));
                obj_id += 1;
                obj_index += 1;
            }
        }
        quest_obj_count.insert(entry, obj_index);
        for i in 0..4 {
            let item: u64 = row.number(&format!("RewItemId{}", i + 1))?;
            let count: u32 = row.number(&format!("RewItemCount{}", i + 1))?;
            if item > 0 {
                reward_item_rows.push(format!(
                    "({rew_id},{entry},{item},{cnt})",
                    cnt = count.max(1)
                ));
                rew_id += 1;
            }
        }
        let mut choice_rank = 0u8;
        for i in 0..6 {
            let item: u64 = row.number(&format!("RewChoiceItemId{}", i + 1))?;
            let count: u32 = row.number(&format!("RewChoiceItemCount{}", i + 1))?;
            if item > 0 {
                reward_choice_rows.push(format!(
                    "({rew_choice_id},{entry},{choice_rank},{item},{cnt})",
                    cnt = count.max(1)
                ));
                rew_choice_id += 1;
                choice_rank += 1;
            }
        }
    }

    for row in parse_table(dump, "areatrigger_involvedrelation")?.iter() {
        let trigger: u64 = row.number(ai::TRIGGER)?;
        let quest: u64 = row.number(ai::QUEST)?;
        if trigger == 0 || !valid_quests.contains(&quest) {
            continue;
        }
        let idx = quest_obj_count.get(&quest).copied().unwrap_or(0);
        objective_rows.push(format!("({obj_id},{quest},{idx},3,{trigger},1)"));
        obj_id += 1;
        quest_obj_count.insert(quest, idx + 1);
    }

    let mut creature_quest_rows: Vec<String> = Vec::new();
    let mut cq_id: u64 = 1;
    let mut emit_relations = |pairs: &[(u64, u64)], role: u8| {
        let mut local: Vec<(u64, u64)> = pairs
            .iter()
            .copied()
            .filter(|(giver, q)| {
                valid_quests.contains(q)
                    && local_givers.is_none_or(|(creatures, _)| creatures.contains(giver))
            })
            .collect();
        local.sort_unstable();
        local.dedup();
        for (creature, quest) in local {
            creature_quest_rows.push(format!("({cq_id},{creature},{quest},{role})"));
            cq_id += 1;
        }
    };
    emit_relations(&starts, 0);
    emit_relations(&ends, 1);

    let mut go_quest_rows: Vec<String> = Vec::new();
    let mut gq_id: u64 = 1;
    let mut emit_go_relations = |pairs: &[(u64, u64)], role: u8| {
        let mut local: Vec<(u64, u64)> = pairs
            .iter()
            .copied()
            .filter(|(giver, q)| {
                valid_quests.contains(q)
                    && local_givers.is_none_or(|(_, gameobjects)| gameobjects.contains(giver))
            })
            .collect();
        local.sort_unstable();
        local.dedup();
        for (go_entry, quest) in local {
            go_quest_rows.push(format!("({gq_id},{go_entry},{quest},{role})"));
            gq_id += 1;
        }
    };
    emit_go_relations(&go_starts, 0);
    emit_go_relations(&go_ends, 1);

    Ok(QuestEtl {
        entries: valid_quests,
        templates: quest_rows,
        texts: quest_text_rows,
        objectives: objective_rows,
        cast_objectives: cast_objective_rows,
        reward_items: reward_item_rows,
        reward_spells: reward_spell_rows,
        reward_choices: reward_choice_rows,
        relations: creature_quest_rows,
        go_relations: go_quest_rows,
        reward_mail: reward_mail_rows,
        reward_mail_template_ids,
    })
}

fn forbidden_items(dump: &(impl TableRows + ?Sized)) -> Result<std::collections::HashSet<u64>> {
    dump.rows("forbidden_items")?
        .iter()
        .map(|row| row.number("entry"))
        .collect()
}

#[allow(clippy::too_many_lines)]
fn build_items_and_loot(
    dump: &(impl crate::dump::TableRows + ?Sized),
    creature_loot_ids: &std::collections::HashMap<u64, u64>,
    vendor_entries: &std::collections::HashSet<u64>,
    extra_item_entries: &std::collections::HashSet<u64>,
    mail_loot_item_entries: &std::collections::HashSet<u64>,
) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
    use std::collections::HashSet;

    let drops_by_lootid = parse_creature_drops(dump)?;
    let forbidden = forbidden_items(dump)?;

    let mut loot_rows: Vec<String> = Vec::new();
    let mut item_ids: HashSet<u64> = [25, 50, 51, 52].into_iter().collect();
    item_ids.extend(extra_item_entries.iter().copied());
    item_ids.extend(mail_loot_item_entries.iter().copied());
    let mut loot_id: u64 = 1;
    let mut creatures: Vec<(&u64, &u64)> = creature_loot_ids.iter().collect();
    creatures.sort();
    for (creature_entry, lootid) in creatures {
        let Some(drops) = drops_by_lootid.get(lootid) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, quest_only) in drops {
            if forbidden.contains(&item) {
                continue;
            }
            item_ids.insert(item);
            loot_rows.push(format!(
                "({loot_id},{creature_entry},{item},{chance_bp},{count},{group_id},{quest_only})"
            ));
            loot_id += 1;
        }
    }

    let mut vendor_rows: Vec<String> = Vec::new();
    let mut vendor_id: u64 = 1;
    let mut nv_seen: HashSet<(u64, u64)> = HashSet::new();
    for row in parse_table(dump, "npc_vendor")?.iter() {
        let entry: u64 = row.number(nv::ENTRY)?;
        if !vendor_entries.contains(&entry) {
            continue;
        }
        let item: u64 = row.number(nv::ITEM)?;
        if item == 0 || forbidden.contains(&item) || !nv_seen.insert((entry, item)) {
            continue;
        }
        let slot: u32 = row.number(nv::SLOT)?;
        let max_count: u32 = row.number(nv::MAXCOUNT)?;
        item_ids.insert(item);
        vendor_rows.push(format!("({vendor_id},{entry},{item},{slot},{max_count})"));
        vendor_id += 1;
    }

    let mut tmpl_stock: HashMap<u64, Vec<(u64, u32, u32)>> = HashMap::new();
    for row in parse_table(dump, "npc_vendor_template")?.iter() {
        let t: u64 = row.number("entry")?;
        let item: u64 = row.number("item")?;
        let max_count: u32 = row.number("maxcount")?;
        let slot: u32 = row.number("slot")?;
        if t != 0 && item != 0 && !forbidden.contains(&item) {
            tmpl_stock
                .entry(t)
                .or_default()
                .push((item, slot, max_count));
        }
    }
    let (creature_entry, vendor_id_column) = ("entry", "vendor_id");
    for row in dump
        .columns("creature_template", &[creature_entry, vendor_id_column])?
        .iter()
    {
        let entry: u64 = row.number(creature_entry)?;
        if !vendor_entries.contains(&entry) {
            continue;
        }
        let vt: u64 = row.number(vendor_id_column)?;
        let Some(stock) = tmpl_stock.get(&vt).filter(|_| vt != 0) else {
            continue;
        };
        for &(item, slot, max_count) in stock {
            if !nv_seen.insert((entry, item)) {
                continue;
            }
            item_ids.insert(item);
            vendor_rows.push(format!("({vendor_id},{entry},{item},{slot},{max_count})"));
            vendor_id += 1;
        }
    }

    let mut item_rows: Vec<String> = Vec::new();
    let mut emitted: HashSet<u64> = HashSet::new();
    let mut resist_nonzero_count: u64 = 0;
    let mut startquest_nonzero_count: u64 = 0;
    for row in parse_table(dump, "item_template")?.iter() {
        let read = |column: &str| it::read(row, column);
        let entry: u64 = row.number(it::ENTRY)?;
        if u32::try_from(entry).is_ok_and(lyracore_package_delta::ids::is_operator_template_id) {
            bail!("item_template entry {entry} belongs to the Operator Template Range");
        }
        if entry == 0 {
            continue;
        }
        emitted.insert(entry);
        let (mut s_str, mut s_agi, mut s_sta, mut s_int, mut s_spi) = (0i32, 0, 0, 0, 0);
        for i in 0..10 {
            let st: u8 = row.number(&format!("stat_type{}", i + 1))?;
            let sv: i32 = row.number(&format!("stat_value{}", i + 1))?;
            match st {
                3 => s_agi += sv,
                4 => s_str += sv,
                5 => s_int += sv,
                6 => s_spi += sv,
                7 => s_sta += sv,
                _ => {}
            }
        }
        let stack: u32 = row.number::<u32>(it::STACKABLE)?.max(1);
        let allowed_class =
            normalize_item_mask(row.number(it::ALLOWED_CLASS)?, ALL_PLAYABLE_CLASS_MASK)?;
        let allowed_race =
            normalize_item_mask(row.number(it::ALLOWED_RACE)?, ALL_PLAYABLE_RACE_MASK)?;
        let is_drink = false;
        let holy: i32 = row.number(it::HOLY_RES)?;
        let fire: i32 = row.number(it::FIRE_RES)?;
        let nature: i32 = row.number(it::NATURE_RES)?;
        let frost: i32 = row.number(it::FROST_RES)?;
        let shadow: i32 = row.number(it::SHADOW_RES)?;
        let arcane: i32 = row.number(it::ARCANE_RES)?;
        if holy != 0 || fire != 0 || nature != 0 || frost != 0 || shadow != 0 || arcane != 0 {
            resist_nonzero_count += 1;
        }
        let sp3: u32 = row.number(it::SPELLID_3)?;
        let spt3: u8 = row.number(it::SPELLTRIGGER_3)?;
        let sp4: u32 = row.number(it::SPELLID_4)?;
        let spt4: u8 = row.number(it::SPELLTRIGGER_4)?;
        let sp5: u32 = row.number(it::SPELLID_5)?;
        let spt5: u8 = row.number(it::SPELLTRIGGER_5)?;
        let req_skill: u32 = row.number(it::REQUIRED_SKILL)?;
        let req_skill_rank: u32 = row.number(it::REQUIRED_SKILL_RANK)?;
        let req_rep_faction: u32 = row.number(it::REQUIRED_REPUTATION_FACTION)?;
        let req_rep_rank: u32 = row.number(it::REQUIRED_REPUTATION_RANK)?;
        let max_count: u32 = row.number(it::MAX_COUNT)?;
        let item_flags: u32 = row.number(it::FLAGS)?;
        let page_text: u32 = row.number(it::PAGE_TEXT)?;
        let start_quest: u32 = row.number(it::START_QUEST)?;
        if start_quest != 0 {
            startquest_nonzero_count += 1;
        }
        let bag_family: u32 = row.number(it::BAG_FAMILY)?;
        let buy_count: u32 = row.number::<u32>(it::BUY_COUNT)?.max(1);
        let food_type: u8 = row.number(it::FOOD_TYPE)?;
        item_rows.push(format!(
            "({entry},{class},{subclass},{name},{disp},{qual},{inv},{ilvl},{rlvl},{dur},{buy},{sell},{stack},{dmin},{dmax},{delay},{s_str},{s_agi},{s_sta},{s_int},{s_spi},0,0,{armor},{block},{drink},{sp1},{spt1},{sp2},{spt2},{cslots},{sheath},{bonding},{holy},{fire},{nature},{frost},{shadow},{arcane},{sp3},{spt3},{sp4},{spt4},{sp5},{spt5},{req_skill},{req_skill_rank},{req_rep_faction},{req_rep_rank},{max_count},{item_flags},{page_text},{start_quest},{bag_family},{buy_count},{food_type},{allowed_class},{allowed_race},{random_property})",
            sheath = row.number::<u8>(it::SHEATH)?,
            sp1 = row.number::<u32>(it::SPELLID_1)?,
            spt1 = row.number::<u8>(it::SPELLTRIGGER_1)?,
            sp2 = row.number::<u32>(it::SPELLID_2)?,
            spt2 = row.number::<u8>(it::SPELLTRIGGER_2)?,
            cslots = row.number::<u8>(it::CONTAINER_SLOTS)?,
            bonding = row.number::<u8>(it::BONDING)?,
            class = read(it::CLASS)?,
            subclass = read(it::SUBCLASS)?,
            name = sql_text(read(it::NAME)?),
            drink = is_drink,
            disp = read(it::DISPLAY_ID)?,
            qual = read(it::QUALITY)?,
            inv = read(it::INVENTORY_TYPE)?,
            ilvl = read(it::ITEM_LEVEL)?,
            rlvl = read(it::REQUIRED_LEVEL)?,
            dur = read(it::MAX_DURABILITY)?,
            buy = read(it::BUY_PRICE)?,
            sell = read(it::SELL_PRICE)?,
            dmin = read(it::DMG_MIN1)?,
            dmax = read(it::DMG_MAX1)?,
            delay = read(it::DELAY)?,
            armor = read(it::ARMOR)?,
            block = read(it::BLOCK)?,
            random_property = item_property::template_pool(row)?,
            allowed_class = allowed_class,
            allowed_race = allowed_race,
        ));
    }
    eprintln!(
        "items: {} of {} imported templates carry a nonzero resistance; {} carry a nonzero startquest",
        resist_nonzero_count,
        emitted.len(),
        startquest_nonzero_count
    );

    let mut missing_mail_items: Vec<u64> = mail_loot_item_entries
        .iter()
        .copied()
        .filter(|e| !emitted.contains(e))
        .collect();
    missing_mail_items.sort_unstable();
    if let Some(&item) = missing_mail_items.first() {
        bail!("item_template: mail_loot_template item {item} has no item_template row");
    }

    let mut dangling: Vec<u64> = item_ids
        .iter()
        .copied()
        .filter(|e| !emitted.contains(e))
        .collect();
    dangling.sort_unstable();
    if !dangling.is_empty() {
        eprintln!(
            "items: WARNING — {} referenced item(s) have NO item_template row (e.g. {:?})",
            dangling.len(),
            dangling.iter().take(8).collect::<Vec<_>>(),
        );
    }

    Ok((item_rows, loot_rows, vendor_rows))
}

fn build_mail_loot(
    dump: &(impl crate::dump::TableRows + ?Sized),
    reward_mail_template_ids: &std::collections::HashSet<u64>,
) -> Result<(Vec<String>, std::collections::HashSet<u64>)> {
    use std::collections::HashSet;

    let forbidden = forbidden_items(dump)?;
    let mut rows: Vec<(u64, String)> = Vec::new();
    let mut item_entries: HashSet<u64> = HashSet::new();
    let mut seen_templates: HashSet<u64> = HashSet::new();
    for row in parse_table(dump, "mail_loot_template")?.iter() {
        let entry: u64 = row.number(mlt::ENTRY)?;
        if !reward_mail_template_ids.contains(&entry)
            || forbidden.contains(&row.number::<u64>(mlt::ITEM)?)
        {
            continue;
        }
        if !seen_templates.insert(entry) {
            bail!("mail_loot_template: mail template {entry} carries more than one loot row");
        }
        let chance: f64 = row.number(mlt::CHANCE)?;
        if !chance.is_finite() || (chance - 100.0).abs() > f64::EPSILON {
            bail!("mail_loot_template: mail template {entry} has a non-100% chance ({chance})");
        }
        let group: i64 = row.number(mlt::GROUP)?;
        if group != 0 {
            bail!("mail_loot_template: mail template {entry} has a non-zero group ({group})");
        }
        let mincount_or_ref: i64 = row.number(mlt::MINCOUNT_OR_REF)?;
        if mincount_or_ref < 0 {
            bail!(
                "mail_loot_template: mail template {entry} references another loot group ({mincount_or_ref})"
            );
        }
        let maxcount: i64 = row.number(mlt::MAXCOUNT)?;
        if maxcount != 1 {
            bail!(
                "mail_loot_template: mail template {entry} has a count other than 1 ({maxcount})"
            );
        }
        let condition: u64 = row.number(mlt::CONDITION_ID)?;
        if condition != 0 {
            bail!("mail_loot_template: mail template {entry} carries a condition ({condition})");
        }
        let item: u64 = row.number(mlt::ITEM)?;
        if item == 0 {
            bail!("mail_loot_template: mail template {entry} has no item");
        }
        item_entries.insert(item);
        rows.push((entry, format!("({entry},{item},1)")));
    }
    rows.sort_unstable_by_key(|(entry, _)| *entry);
    Ok((rows.into_iter().map(|(_, sql)| sql).collect(), item_entries))
}

pub(crate) fn creature_row_kept_in_scope(
    scope: &WorldImportScope,
    map: i64,
    x: f64,
    y: f64,
    z: f64,
    raw_entry: u64,
) -> bool {
    scope.contains_map(map)
        && ((raw_entry != 0 && scope.forced_creature_entries.contains(&raw_entry))
            || scope.contains(map, x, y, z))
}

pub(crate) const FAMILIES: &[&str] = &[
    "creatures",
    "items",
    "loot",
    "gossip",
    "quests",
    "gameobjects",
    "trainers",
    "casts",
    "creature-ai",
    "globals",
    "spellmeta",
];

const SPAWN_BATCH: usize = 250;

pub(crate) fn family_active(args: &WorldImportArgs, name: &str) -> bool {
    match &args.family {
        None => true,
        Some(f) => f == name,
    }
}

fn run_package_stage(args: &WorldImportArgs, family: &str) -> Result<()> {
    let quest_event_reload =
        family == "creature-ai" && args.world_profile.is_some() && family_active(args, "quests");
    if !family_active(args, family) && !quest_event_reload {
        return Ok(());
    }
    match &args.packages {
        Some(root) => package_delta::reapply(&args.scope, args.apply, &args.shard, family, root)?,
        None => package_delta::warn_not_reapplied(family),
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct DumpPlan {
    pub(crate) stmts: Vec<String>,
    pub(crate) spawn_batches: Vec<String>,
    pub(crate) spawn_row_count: usize,
    pub(crate) go_batches: Vec<String>,
    pub(crate) go_row_count: usize,
    pub(crate) eventai_definition_batches: Vec<String>,
    pub(crate) eventai_relay_definition_batches: Vec<String>,
    pub(crate) eventai_definition_count: u64,
    pub(crate) eventai_relay_definition_count: u64,
    pub(crate) eventai_instruction_count: u64,
    pub(crate) world_source_manifest: Option<String>,
    pub(crate) stamps: Vec<(&'static str, u64)>,
}

impl DumpPlan {
    // Load GameObject dependencies before AI, then creatures whose spawn events can invoke AI.
    fn reducer_batches<'a>(
        &'a self,
        args: &WorldImportArgs,
    ) -> Result<Vec<(&'static str, &'a str)>> {
        let mut batches = Vec::new();
        if family_active(args, "gameobjects") {
            for (index, batch) in self.go_batches.iter().enumerate() {
                batches.push((
                    if index == 0 {
                        "import_gameobjects"
                    } else {
                        "import_gameobjects_append"
                    },
                    batch.as_str(),
                ));
            }
        }
        if family_active(args, "creature-ai") {
            let catalogue = self
                .eventai_relay_definition_batches
                .first()
                .context("EventAI relay plan has no atomic catalogue")?;
            batches.push(("import_creature_ai_relay_definitions", catalogue.as_str()));
            for (index, batch) in self.eventai_definition_batches.iter().enumerate() {
                batches.push((
                    if index == 0 {
                        "import_creature_ai_definitions"
                    } else {
                        "import_creature_ai_definitions_append"
                    },
                    batch.as_str(),
                ));
            }
        }
        if family_active(args, "creatures") {
            for (index, batch) in self.spawn_batches.iter().enumerate() {
                batches.push((
                    if index == 0 {
                        "import_creature_spawns"
                    } else {
                        "import_creature_spawns_append"
                    },
                    batch.as_str(),
                ));
            }
        }
        Ok(batches)
    }
}

pub(crate) type CreatureSpawnRow = (u64, u64, f64, f64, f64, f64, u8, u32, i64);

struct GoMeta {
    stored_type: u8,
    disp: u32,
    name: String,
    data0: u32,
    data1: u32,
    trap_spell_id: u32,
    trap_cooldown_secs: u32,
    meeting_stone_area_id: u32,
    size: f32,
}

struct GameobjectSpawns {
    go_meta: std::collections::HashMap<u64, GoMeta>,
    dropped_type25: Vec<u64>,
    goober_entries: std::collections::HashSet<u64>,
    go_packed_rows: Vec<String>,
    used_go: std::collections::HashSet<u64>,
}

fn build_packed_spawn_payload(spawns: &[CreatureSpawnRow]) -> String {
    // Timestamp columns require reducer imports because SpacetimeDB SQL cannot write them.
    let packed_spawns: String = spawns
        .iter()
        .map(|(db_guid, entry, x, y, z, o, mt, respawn_secs, map)| {
            format!(
                "{g},{entry},{map},{x},{y},{z},{o},{mt},{respawn_secs}",
                g = world_guid(*entry, *db_guid),
            )
        })
        .collect::<Vec<_>>()
        .join(";");

    packed_spawns
}

fn collect_gameobject_spawns(
    dump: &(impl crate::dump::TableRows + ?Sized),
    scope: &WorldImportScope,
) -> Result<GameobjectSpawns> {
    let go_tmpls = parse_table(dump, "gameobject_template")?;
    let mut dropped_type25: Vec<u64> = Vec::new();
    let go_meta: std::collections::HashMap<u64, GoMeta> = go_tmpls
        .iter()
        .map(|row| -> Result<_> {
            let entry: u64 = row.number(got::ENTRY)?;
            let raw_type: u32 = row.number(got::TYPE)?;
            let stored_type = match classify_go_type(entry, raw_type) {
                Some(t) => t,
                None => {
                    dropped_type25.push(entry);
                    return Ok(None);
                }
            };
            let disp: u32 = row.number(got::DISPLAY_ID)?;
            let name = row.text(got::NAME)?.trim().to_string();
            let data0: u32 = row.number(got::DATA0)?;
            let data1 = if matches!(
                stored_type,
                GO_CHEST | GO_DOOR | GO_BUTTON | GO_MEETINGSTONE | 8
            ) {
                row.number(got::DATA1)?
            } else {
                0
            };
            let trap_spell_id = if raw_type == 6 {
                row.number(got::DATA3)?
            } else {
                0
            };
            let trap_cooldown_secs = if raw_type == 6 {
                row.number(got::DATA5)?
            } else {
                0
            };
            let meeting_stone_area_id = if raw_type == u32::from(GO_MEETINGSTONE) {
                row.number(got::DATA2)?
            } else {
                0
            };
            let size: f32 = row.number(got::SIZE)?;
            Ok(Some((
                entry,
                GoMeta {
                    stored_type,
                    disp,
                    name,
                    data0,
                    data1,
                    trap_spell_id,
                    trap_cooldown_secs,
                    meeting_stone_area_id,
                    size,
                },
            )))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_>>()?;
    let goober_entries: std::collections::HashSet<u64> = go_meta
        .iter()
        .filter(|(_, m)| m.stored_type == GO_GOOBER)
        .map(|(&e, _)| e)
        .collect();
    let mut go_packed_rows: Vec<String> = Vec::new();
    let mut used_go: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut go_slice_counts = vec![0usize; scope.bounded_slices.len()];
    let mut go_whole_map_counts: std::collections::BTreeMap<i64, usize> =
        std::collections::BTreeMap::new();
    for row in parse_table(dump, "gameobject")?.iter() {
        let map = row.number::<i64>(go::MAP)?;
        let id: u64 = row.number(go::ID)?;
        let Some(meta) = go_meta.get(&id) else {
            continue;
        };
        let x: f64 = row.number(go::X)?;
        let y: f64 = row.number(go::Y)?;
        let z: f64 = row.number(go::Z)?;
        if !scope.contains(map, x, y, z) {
            continue;
        }
        let db_guid: u32 = row.number(go::GUID)?;
        let o: f64 = row.number(go::O)?;
        // A zero quaternion is valid packed identity, not a signal to derive rotation from facing.
        let rot0: f64 = row.number(go::ROT0)?;
        let rot1: f64 = row.number(go::ROT1)?;
        let rot2: f64 = row.number(go::ROT2)?;
        let rot3: f64 = row.number(go::ROT3)?;
        let initial_state = {
            if row.number::<u32>("spawn_flags")? & 2 != 0 {
                continue;
            }
            match row.number::<u8>("state")? {
                0 if meta.stored_type == GO_CHEST => 0,
                0 => 1,
                1 => 0,
                2 => 2,
                state => bail!("gameobject {db_guid}: unknown source state {state}"),
            }
        };
        go_packed_rows.push(format!(
            "{g},{id},{m},{x},{y},{z},{o},{initial_state},{rot0},{rot1},{rot2},{rot3}",
            g = go_guid(db_guid),
            m = map
        ));
        for (index, slice) in scope.bounded_slices.iter().enumerate() {
            if slice.contains(map, x, y, z) {
                go_slice_counts[index] += 1;
            }
        }
        if scope.whole_maps.contains(&map) {
            *go_whole_map_counts.entry(map).or_insert(0) += 1;
        }
        used_go.insert(id);
    }
    for (slice, count) in scope.bounded_slices.iter().zip(go_slice_counts) {
        eprintln!(
            "filter: bounded slice {} (map {}) → {count} gameobject spawns",
            slice.name, slice.map_id
        );
    }
    for &map in &scope.whole_maps {
        let count = go_whole_map_counts.get(&map).copied().unwrap_or(0);
        eprintln!("filter: whole map {map} → {count} gameobject spawns");
    }

    Ok(GameobjectSpawns {
        go_meta,
        dropped_type25,
        goober_entries,
        go_packed_rows,
        used_go,
    })
}

fn gameobject_template_rows(
    spawns: &GameobjectSpawns,
    required_entries: &std::collections::BTreeSet<u64>,
) -> Result<GameobjectTemplateRows> {
    let GameobjectSpawns {
        go_meta,
        dropped_type25,
        go_packed_rows,
        ..
    } = spawns;
    let mut go_template_rows: Vec<String> = Vec::new();
    let mut go_trap_rows: Vec<String> = Vec::new();
    let mut go_meeting_stone_rows: Vec<String> = Vec::new();
    let mut go_type_histogram: std::collections::BTreeMap<u8, u32> =
        std::collections::BTreeMap::new();
    for entry in required_entries {
        if !go_meta.contains_key(entry) {
            bail!("relay requires GameObject definition {entry}, which cannot be imported");
        }
    }
    let mut entries: Vec<_> = go_meta.keys().copied().collect();
    entries.sort_unstable();
    for entry in entries {
        let meta = &go_meta[&entry];
        *go_type_histogram.entry(meta.stored_type).or_insert(0) += 1;
        let (row, _loot_id_used) = go_template_row(
            entry,
            meta.stored_type,
            meta.disp,
            &meta.name,
            meta.data0,
            meta.data1,
            meta.size,
        );
        go_template_rows.push(row);
        if meta.trap_spell_id != 0 {
            go_trap_rows.push(format!(
                "({entry},{},{})",
                meta.trap_spell_id, meta.trap_cooldown_secs
            ));
        }
        if meta.stored_type == GO_MEETINGSTONE {
            go_meeting_stone_rows.push(format!(
                "({entry},{},{},{})",
                meta.data0, meta.data1, meta.meeting_stone_area_id
            ));
        }
    }
    const GO_LIVE_TYPE_NAMES: &[(u8, &str)] = &[
        (GO_DOOR, "DOOR"),
        (GO_BUTTON, "BUTTON"),
        (GO_CHEST, "CHEST"),
        (GO_GOOBER, "GOOBER"),
        (GO_GATHER, "GATHER"),
    ];
    eprintln!(
        "gameobject type coverage ({} templates, {} spawns):",
        go_template_rows.len(),
        go_packed_rows.len()
    );
    for (&type_id, &count) in &go_type_histogram {
        if type_id == GO_QUESTGIVER {
            eprintln!("  type {type_id:>2} QUESTGIVER {count} (quest flow, not use-dispatched)");
            continue;
        }
        if type_id == GO_MEETINGSTONE {
            eprintln!(
                "  type {type_id:>2} MEETINGSTONE {count} (meeting stone flow, not use-dispatched)"
            );
            continue;
        }
        match GO_LIVE_TYPE_NAMES.iter().find(|&&(t, _)| t == type_id) {
            Some((_, name)) => eprintln!("  type {type_id:>2} {name:<10} {count} (LIVE)"),
            None => eprintln!("  type {type_id:>2} (unhandled) {count} INERT — use() no-ops"),
        }
    }
    if !dropped_type25.is_empty() {
        eprintln!(
            "  TYPE-25 COLLISION GUARD: dropped {} source FISHINGHOLE (type 25) template(s) from \
             import entirely (entries {:?}) — module type 25 is reserved for the synthetic GATHER marker",
            dropped_type25.len(), dropped_type25
        );
    }

    Ok(GameobjectTemplateRows {
        templates: go_template_rows,
        traps: go_trap_rows,
        meeting_stones: go_meeting_stone_rows,
    })
}

struct LootFamilyRows {
    pickpocket_rows: Vec<String>,
    skinning_rows: Vec<String>,
    gameobject_loot_rows: Vec<String>,
    fishing_rows: Vec<String>,
}

fn build_loot_family_rows(
    dump: &(impl crate::dump::TableRows + ?Sized),
    creature_pickpocket_ids: &std::collections::HashMap<u64, u64>,
    creature_skin_ids: &std::collections::HashMap<u64, u64>,
    chest_loot_ids_used: &[u32],
) -> Result<LootFamilyRows> {
    let drops_by_pickpocketid = vmangos_loot::parse(dump, "pickpocketing_loot_template")?;
    let mut pickpocket_rows: Vec<String> = Vec::new();
    let mut pickpocket_row_id: u64 = 1;
    let mut pp_creatures: Vec<(&u64, &u64)> = creature_pickpocket_ids.iter().collect();
    pp_creatures.sort();
    for (creature_entry, pp_loot_id) in pp_creatures {
        let Some(drops) = drops_by_pickpocketid.get(pp_loot_id) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, quest_only) in drops {
            pickpocket_rows.push(format!("({pickpocket_row_id},{creature_entry},{item},{chance_bp},{count},{group_id},{quest_only})"));
            pickpocket_row_id += 1;
        }
    }

    let drops_by_skinid = vmangos_loot::parse(dump, "skinning_loot_template")?;
    let mut skin_ids_used: Vec<u64> = creature_skin_ids.values().copied().collect();
    skin_ids_used.sort_unstable();
    skin_ids_used.dedup();
    let mut skinning_rows: Vec<String> = Vec::new();
    let mut skinning_row_id: u64 = 1;
    for skin_id in skin_ids_used {
        let Some(drops) = drops_by_skinid.get(&skin_id) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, _quest_only) in drops {
            skinning_rows.push(format!(
                "({skinning_row_id},{skin_id},{item},{chance_bp},{count},{group_id})"
            ));
            skinning_row_id += 1;
        }
    }

    let drops_by_goloot = vmangos_loot::parse(dump, "gameobject_loot_template")?;
    let mut go_loot_ids_used: Vec<u32> = chest_loot_ids_used.to_vec();
    go_loot_ids_used.sort_unstable();
    go_loot_ids_used.dedup();
    let mut gameobject_loot_rows: Vec<String> = Vec::new();
    let mut go_loot_row_id: u64 = 1;
    for loot_id in go_loot_ids_used {
        let Some(drops) = drops_by_goloot.get(&(loot_id as u64)) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, quest_only) in drops {
            gameobject_loot_rows.push(format!(
                "({go_loot_row_id},{loot_id},{item},{chance_bp},{count},{group_id},{quest_only})"
            ));
            go_loot_row_id += 1;
        }
    }

    let drops_by_zone = vmangos_loot::parse(dump, "fishing_loot_template")?;
    let mut fishing_rows: Vec<String> = Vec::new();
    let mut fishing_row_id: u64 = 1;
    let mut zones: Vec<&u64> = drops_by_zone.keys().collect();
    zones.sort();
    for zone in zones {
        for &(item, chance_bp, count, group_id, _quest_only) in &drops_by_zone[zone] {
            fishing_rows.push(format!(
                "({fishing_row_id},{zone},{item},{chance_bp},{count},{group_id})"
            ));
            fishing_row_id += 1;
        }
    }

    eprintln!(
        "mapped: {} pickpocket_loot rows ({} creatures), {} skinning_loot rows ({} skin tables), \
         {} gameobject_loot rows ({} chest loot ids), {} fishing_loot rows ({} zones)",
        pickpocket_rows.len(),
        creature_pickpocket_ids.len(),
        skinning_rows.len(),
        creature_skin_ids
            .values()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        gameobject_loot_rows.len(),
        chest_loot_ids_used.len(),
        fishing_rows.len(),
        drops_by_zone.len(),
    );

    Ok(LootFamilyRows {
        pickpocket_rows,
        skinning_rows,
        gameobject_loot_rows,
        fishing_rows,
    })
}

fn push_item_statements(
    stmts: &mut Vec<String>,
    item_rows: &[String],
    loot_rows: &[String],
    vendor_rows: &[String],
) {
    stmts.push("DELETE FROM game_item_template WHERE entry > 0".into());
    stmts.push("DELETE FROM game_creature_loot WHERE id > 0".into());
    stmts.push("DELETE FROM game_npc_vendor WHERE id > 0".into());
    push_insert(stmts, "game_item_template", "entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property", item_rows);
    push_insert(
        stmts,
        "game_creature_loot",
        "id,creature_entry,item_entry,chance_bp,count,group_id,quest_only",
        loot_rows,
    );
    push_insert(
        stmts,
        "game_npc_vendor",
        "id,creature_entry,item_entry,slot,max_count",
        vendor_rows,
    );
}

fn push_loot_statements(
    stmts: &mut Vec<String>,
    pickpocket_rows: &[String],
    skinning_rows: &[String],
    gameobject_loot_rows: &[String],
    fishing_rows: &[String],
) {
    stmts.push("DELETE FROM game_pickpocket_loot WHERE id > 0".into());
    stmts.push("DELETE FROM game_skinning_loot WHERE id > 0".into());
    stmts.push("DELETE FROM game_gameobject_loot WHERE id > 0".into());
    stmts.push("DELETE FROM game_fishing_loot WHERE id > 0".into());
    push_insert(
        stmts,
        "game_pickpocket_loot",
        "id,creature_entry,item_entry,chance_bp,count,group_id,quest_only",
        pickpocket_rows,
    );
    push_insert(
        stmts,
        "game_skinning_loot",
        "id,skin_loot_id,item_entry,chance_bp,count,group_id",
        skinning_rows,
    );
    push_insert(
        stmts,
        "game_gameobject_loot",
        "id,loot_id,item_entry,chance_bp,count,group_id,quest_only",
        gameobject_loot_rows,
    );
    push_insert(
        stmts,
        "game_fishing_loot",
        "id,zone_id,item_entry,chance_bp,count,group_id",
        fishing_rows,
    );
}

fn gossip_statements(stmts: &mut Vec<String>, gossip: &GossipEtl) {
    // Entry menus >= 1_000_000 and default option/text row ids >= 50_000 survive reloads.
    stmts.push("DELETE FROM game_gossip_condition WHERE condition_id > 0".into());
    push_insert(
        stmts,
        "game_gossip_condition",
        "condition_id,kind,value1,value2,value3,value4,flags",
        &gossip.condition_rows,
    );
    stmts.push("DELETE FROM game_gossip_menu_profile_option WHERE row_id > 0".into());
    stmts.push("DELETE FROM game_gossip_menu_profile WHERE menu_id > 0".into());
    stmts.push("DELETE FROM game_gossip_menu WHERE entry > 0 AND entry < 1000000".into());
    stmts.push("DELETE FROM game_npc_text WHERE text_id > 0 AND text_id < 50000".into());
    stmts.push("DELETE FROM game_npc_text_slot WHERE id >= 0 AND id < 50000".into());
    stmts.push("DELETE FROM game_gossip_option WHERE row_id >= 0 AND row_id < 50000".into());
    push_insert(
        stmts,
        "game_gossip_menu",
        "entry,text_id",
        &gossip.menu_rows,
    );
    push_insert(
        stmts,
        "game_npc_text",
        "text_id,text",
        &gossip.npc_text_rows,
    );
    push_insert(
        stmts,
        "game_npc_text_slot",
        "id,text_id,slot_index,text_male,text_female,probability",
        &gossip.npc_text_slot_rows,
    );
    push_insert(stmts, "game_gossip_option", "row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2", &gossip.option_rows);
    push_insert(
        stmts,
        "game_gossip_menu_profile",
        "menu_id,text_id",
        &gossip.profile_rows,
    );
    push_insert(
            stmts,
            "game_gossip_menu_profile_option",
            "row_id,menu_id,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2",
            &gossip.profile_option_rows,
        );
}

fn push_gameobject_statements(
    stmts: &mut Vec<String>,
    go_template_rows: &[String],
    go_trap_rows: &[String],
    go_meeting_stone_rows: &[String],
) {
    stmts.push("DELETE FROM game_gameobject_trap WHERE entry > 0".into());
    stmts.push("DELETE FROM game_meeting_stone WHERE entry > 0".into());
    stmts.push("DELETE FROM game_gameobject_template WHERE entry > 0".into());
    push_insert(stmts, "game_gameobject_template", "entry,type_id,display_id,name,data0,data1,gather_skill_line,respawn_secs,gather_gray,lock_id,size", go_template_rows);
    push_insert(
        stmts,
        "game_gameobject_trap",
        "entry,spell_id,cooldown_secs",
        go_trap_rows,
    );
    push_insert(
        stmts,
        "game_meeting_stone",
        "entry,min_level,max_level,area_id",
        go_meeting_stone_rows,
    );
}

fn push_quest_statements(stmts: &mut Vec<String>, quests: &QuestEtl, mail_loot_rows: &[String]) {
    stmts.push("DELETE FROM game_quest_template WHERE entry > 0".into());
    stmts.push("DELETE FROM game_quest_text WHERE quest_entry > 0".into());
    stmts.push("DELETE FROM game_quest_objective WHERE id > 0".into());
    stmts.push("DELETE FROM game_quest_cast_objective WHERE id > 0".into());
    stmts.push("DELETE FROM game_quest_reward_item WHERE id > 0".into());
    stmts.push("DELETE FROM game_quest_reward_spell WHERE quest_entry > 0".into());
    push_insert(
        stmts,
        "game_quest_reward_spell",
        "quest_entry,spell_id",
        &quests.reward_spells,
    );
    stmts.push("DELETE FROM game_quest_reward_choice WHERE id > 0".into());
    stmts.push("DELETE FROM game_creature_quest WHERE id > 0".into());
    stmts.push("DELETE FROM game_gameobject_quest WHERE id > 0".into());
    push_insert(stmts, "game_quest_template", "entry,min_level,quest_level,title,reward_money,reward_xp,prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,reward_money_max_level,quest_type", &quests.templates);
    push_insert(
        stmts,
        "game_quest_text",
        "quest_entry,details,objectives,offer_reward_text,request_items_text",
        &quests.texts,
    );
    push_insert(
        stmts,
        "game_quest_objective",
        "id,quest_entry,obj_index,kind,target_entry,required_count",
        &quests.objectives,
    );
    push_insert(
        stmts,
        "game_quest_cast_objective",
        "id,quest_entry,obj_index,spell_id",
        &quests.cast_objectives,
    );
    push_insert(
        stmts,
        "game_quest_reward_item",
        "id,quest_entry,item_entry,count",
        &quests.reward_items,
    );
    push_insert(
        stmts,
        "game_quest_reward_choice",
        "id,quest_entry,choice_index,item_entry,count",
        &quests.reward_choices,
    );
    push_insert(
        stmts,
        "game_creature_quest",
        "id,creature_entry,quest_entry,role",
        &quests.relations,
    );
    push_insert(
        stmts,
        "game_gameobject_quest",
        "id,go_entry,quest_entry,role",
        &quests.go_relations,
    );
    stmts.push("DELETE FROM game_quest_reward_mail WHERE quest_entry > 0".into());
    push_insert(
        stmts,
        "game_quest_reward_mail",
        "quest_entry,mail_template_id,delay_secs,sender_creature_entry,money",
        &quests.reward_mail,
    );
    stmts.push("DELETE FROM game_mail_loot WHERE mail_template_id > 0".into());
    push_insert(
        stmts,
        "game_mail_loot",
        "mail_template_id,item_entry,count",
        mail_loot_rows,
    );
}

pub(crate) fn globals_statements(
    dump: &(impl TableRows + ?Sized),
    client: Option<&str>,
) -> Result<Vec<String>> {
    let mut statements = build_stats_sql(dump)?;
    statements.extend(build_start_position_sql(dump)?);
    statements.extend(build_graveyard_zone_sql(dump)?);
    statements.extend(build_createinfo_spell_sql(dump)?);
    statements.extend(build_createinfo_action_sql(dump)?);
    statements.extend(build_createinfo_item_sql(dump, client)?);
    statements.extend(build_areatrigger_teleport_sql(dump)?.0);
    statements.extend(build_tavern_area_trigger_sql(dump)?);
    Ok(statements)
}

fn vmangos_creature_entries(
    dump: &impl TableRows,
    args: &WorldImportArgs,
) -> Result<std::collections::HashSet<u64>> {
    if args
        .world_profile
        .is_some_and(WorldImportProfile::imports_all_templates)
    {
        return dump
            .columns("creature_template", &["entry"])?
            .iter()
            .map(|row| row.number("entry"))
            .collect();
    }
    let mut entries = args
        .scope
        .forced_creature_entries
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    for row in dump.rows("creature")?.iter() {
        if row.number::<u32>("spawn_flags")? & 2 != 0 || row.number::<u32>("id")? == 0 {
            continue;
        }
        let choices = creature_population::vmangos_entry_choices(row)?;
        if args.scope.contains_map(row.number("map")?)
            && (args.scope.contains(
                row.number("map")?,
                row.number("position_x")?,
                row.number("position_y")?,
                row.number("position_z")?,
            ) || choices.iter().any(|entry| {
                args.scope
                    .forced_creature_entries
                    .contains(&u64::from(*entry))
            }))
        {
            entries.extend(choices.into_iter().map(u64::from));
        }
    }
    entries.extend([416, 1860, 1863, 5676, 5677, 12861]);
    eventai::vmangos::include_required_templates(dump, &mut entries)?;
    Ok(entries)
}

fn items_findings(
    dump: &impl TableRows,
    entries: &std::collections::HashSet<u64>,
) -> Result<serde_json::Value> {
    use std::collections::{BTreeMap, BTreeSet};
    let forbidden = forbidden_items(dump)?;
    let templates = dump.columns("creature_template", &["entry", "vendor_id"])?;
    let mut template_users: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for row in templates.iter() {
        let entry: u64 = row.number("entry")?;
        let vendor: u64 = row.number("vendor_id")?;
        if entries.contains(&entry) && vendor != 0 {
            template_users.entry(vendor).or_default().push(entry);
        }
    }
    let mut findings: BTreeMap<&str, BTreeSet<(u64, u64)>> = BTreeMap::new();
    for table in ["npc_vendor", "npc_vendor_template"] {
        for row in dump.rows(table)?.iter() {
            let entry: u64 = row.number("entry")?;
            let item: u64 = row.number("item")?;
            if forbidden.contains(&item) || item == 0 {
                continue;
            }
            let users = if table == "npc_vendor" {
                if entries.contains(&entry) {
                    vec![entry]
                } else {
                    Vec::new()
                }
            } else {
                template_users.get(&entry).cloned().unwrap_or_default()
            };
            for (column, code) in [
                ("condition_id", "items.vendor-condition"),
                ("incrtime", "items.vendor-restock"),
                ("itemflags", "items.vendor-item-flags"),
            ] {
                if row.number::<u64>(column)? != 0 {
                    findings
                        .entry(code)
                        .or_default()
                        .extend(users.iter().map(|&user| (user, item)));
                }
            }
        }
    }
    let mut findings = findings.into_iter().filter(|(_, keys)| !keys.is_empty()).map(|(code, keys)| serde_json::json!({"code": code, "classification": "unsupported", "occurrences": keys.len(), "source_keys": keys})).collect::<Vec<_>>();
    findings.extend(loot_findings(
        dump,
        &["creature_loot_template", "reference_loot_template"],
    )?);
    Ok(
        serde_json::json!({"format_version": 1, "source": "vmangos", "family": "items", "patch": crate::source_selection::PATCH, "client_build": crate::source_selection::CLIENT_BUILD, "findings": findings}),
    )
}

fn vmangos_items_plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let entries = vmangos_creature_entries(dump, args)?;
    let mut loot_ids = HashMap::new();
    for row in dump
        .columns("creature_template", &["entry", "loot_id"])?
        .iter()
    {
        let entry = row.number::<u64>("entry")?;
        let loot = row.number::<u64>("loot_id")?;
        if entries.contains(&entry) && loot != 0 {
            loot_ids.insert(entry, loot);
        }
    }
    let (items, loot, vendors) = build_items_and_loot(
        dump,
        &loot_ids,
        &entries,
        &Default::default(),
        &Default::default(),
    )?;
    let (mut stmts, properties) = item_property::dump_sql(dump, args.dbc.as_deref())?;
    push_item_statements(&mut stmts, &items, &loot, &vendors);
    Ok(DumpPlan {
        stmts,
        stamps: vec![("items", items.len() as u64 + properties)],
        world_source_manifest: Some(serde_json::to_string_pretty(&items_findings(
            dump, &entries,
        )?)?),
        ..Default::default()
    })
}

fn vmangos_loot_plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let entries = vmangos_creature_entries(dump, args)?;
    let mut pickpocket = HashMap::new();
    let mut skinning = HashMap::new();
    for row in dump
        .columns(
            "creature_template",
            &["entry", "pickpocket_loot_id", "skinning_loot_id"],
        )?
        .iter()
    {
        let entry = row.number("entry")?;
        if entries.contains(&entry) {
            for (column, ids) in [
                ("pickpocket_loot_id", &mut pickpocket),
                ("skinning_loot_id", &mut skinning),
            ] {
                let id = row.number::<u64>(column)?;
                if id != 0 {
                    ids.insert(entry, id);
                }
            }
        }
    }
    let mut chest_ids = Vec::new();
    for row in dump
        .columns("gameobject_template", &["entry", "type", "data1"])?
        .iter()
    {
        if classify_go_type(row.number("entry")?, row.number("type")?) == Some(GO_CHEST) {
            let id = row.number::<u32>("data1")?;
            if id != 0 {
                chest_ids.push(id);
            }
        }
    }
    let loot = build_loot_family_rows(dump, &pickpocket, &skinning, &chest_ids)?;
    let mut stmts = Vec::new();
    push_loot_statements(
        &mut stmts,
        &loot.pickpocket_rows,
        &loot.skinning_rows,
        &loot.gameobject_loot_rows,
        &loot.fishing_rows,
    );
    let count = insert_row_count(&stmts);
    Ok(DumpPlan {
        stmts,
        stamps: vec![("loot", count)],
        world_source_manifest: Some(serde_json::to_string_pretty(
            &serde_json::json!({"format_version":1,"source":"vmangos","family":"loot","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":loot_findings(dump, &["creature_loot_template", "reference_loot_template", "pickpocketing_loot_template", "skinning_loot_template", "gameobject_loot_template", "fishing_loot_template", "item_loot_template", "disenchant_loot_template", "mail_loot_template"])?}),
        )?),
        ..Default::default()
    })
}

fn loot_findings(dump: &impl TableRows, tables: &[&str]) -> Result<Vec<serde_json::Value>> {
    let forbidden = forbidden_items(dump)?;
    let mut findings = Vec::new();
    for &table in tables {
        let rows = dump.rows(table)?;
        let mut conditions = 0;
        let mut count_ranges = 0;
        let mut quest_only = 0;
        for row in rows.iter() {
            let minimum = row.number::<i64>("mincountOrRef")?;
            if minimum >= 0 && forbidden.contains(&row.number::<u64>("item")?) {
                continue;
            }
            conditions += usize::from(row.number::<u32>("condition_id")? != 0);
            let maximum = row.number::<u32>("maxcount")?;
            count_ranges += usize::from(minimum >= 0 && minimum as u64 != u64::from(maximum));
            if matches!(table, "skinning_loot_template" | "fishing_loot_template") {
                quest_only += usize::from(row.number::<f64>("ChanceOrQuestChance")? < 0.0);
            }
        }
        for (code, occurrences) in [
            ("loot.condition", conditions),
            ("loot.count-range", count_ranges),
            ("loot.quest-only", quest_only),
        ] {
            if occurrences != 0 {
                findings.push(serde_json::json!({"code":code,"classification":"unsupported","table":table,"occurrences":occurrences}));
            }
        }
        if matches!(table, "item_loot_template" | "disenchant_loot_template") && !rows.is_empty() {
            findings.push(serde_json::json!({"code":"loot.unconsumed-table","classification":"unsupported","table":table,"occurrences":rows.len()}));
        }
    }
    Ok(findings)
}

fn transport_statements(dump: &impl TableRows, client: Option<&str>) -> Result<Vec<String>> {
    let Some(client) = client else {
        return Ok(Vec::new());
    };
    let mut chain = dbc::open_chain(std::path::Path::new(client))?;
    let paths = dbc::read_table(&mut chain)?;
    let nodes = dbc::read_table(&mut chain)?;
    let catalogue = crate::transport::catalogue_sql(&paths, &nodes, dump)?;
    for warning in catalogue.warnings {
        eprintln!("{warning}");
    }
    Ok(catalogue.statements)
}

fn vmangos_gameobjects_plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let spawns = collect_gameobject_spawns(dump, &args.scope)?;
    let templates = gameobject_template_rows(&spawns, &Default::default())?;
    let mut stmts = Vec::new();
    push_gameobject_statements(
        &mut stmts,
        &templates.templates,
        &templates.traps,
        &templates.meeting_stones,
    );
    stmts.extend(transport_statements(dump, args.dbc.as_deref())?);
    let count = insert_row_count(&stmts) + spawns.go_packed_rows.len() as u64;
    Ok(DumpPlan {
        stmts,
        go_row_count: spawns.go_packed_rows.len(),
        go_batches: spawns
            .go_packed_rows
            .chunks(SPAWN_BATCH)
            .map(|rows| rows.join(";"))
            .collect(),
        stamps: vec![("gameobjects", count)],
        world_source_manifest: Some(serde_json::to_string_pretty(&gameobject_findings(
            dump, args,
        )?)?),
        ..Default::default()
    })
}

fn gameobject_findings(dump: &impl TableRows, args: &WorldImportArgs) -> Result<serde_json::Value> {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut add = |code, present: bool| {
        if present {
            *counts.entry(code).or_default() += 1;
        }
    };
    let mut placed = std::collections::HashSet::new();
    for row in dump.rows("gameobject")?.iter() {
        if !args.scope.contains(
            row.number("map")?,
            row.number("position_x")?,
            row.number("position_y")?,
            row.number("position_z")?,
        ) {
            continue;
        }
        let flags = row.number::<u32>("spawn_flags")?;
        if flags & 2 != 0 {
            continue;
        }
        placed.insert(row.number::<u32>("guid")?);
        add("gameobjects.spawn-flags", flags & !2 != 0);
        add(
            "gameobjects.summoned-spawn",
            row.number::<i32>("spawntimesecsmin")? < 0,
        );
        add(
            "gameobjects.animation",
            row.number::<u32>("animprogress")? != 100,
        );
        add(
            "gameobjects.visibility",
            row.number::<f32>("visibility_mod")? != 0.0,
        );
        add(
            "gameobjects.respawn-range",
            row.number::<i32>("spawntimesecsmin")? != row.number::<i32>("spawntimesecsmax")?,
        );
    }
    for (table, code) in [
        ("game_event_gameobject", "gameobjects.game-event"),
        ("pool_gameobject", "gameobjects.pool"),
    ] {
        for row in dump.rows(table)?.iter() {
            add(code, placed.contains(&row.number::<u32>("guid")?));
        }
    }
    for row in dump.rows("gameobject_template")?.iter() {
        let entry = row.number::<u64>("entry")?;
        add("gameobjects.fishinghole", row.number::<u32>("type")? == 25);
        add("gameobjects.gather-loot", gather_node(entry).is_some());
        add("gameobjects.script", !row.text("script_name")?.is_empty());
    }
    let findings = counts.into_iter().map(|(code, occurrences)| serde_json::json!({"code":code,"classification":"unsupported","occurrences":occurrences})).collect::<Vec<_>>();
    Ok(
        serde_json::json!({"format_version":1,"source":"vmangos","family":"gameobjects","patch":crate::source_selection::PATCH,"client_build":crate::source_selection::CLIENT_BUILD,"findings":findings}),
    )
}

fn vmangos_creature_ai(
    dump: &impl TableRows,
    args: &WorldImportArgs,
) -> Result<(eventai::EventAiPlan, serde_json::Value)> {
    let source = eventai::vmangos::Source::read(dump)?;
    let templates = dump
        .columns("creature_template", &["entry"])?
        .iter()
        .map(|row| row.number::<u64>("entry"))
        .collect::<Result<std::collections::HashSet<_>>>()?;
    let mut entries = vmangos_creature_entries(dump, args)?;
    let (eventai, manifest) = loop {
        let (plan, manifest) = source.assemble(&entries, &templates);
        if plan
            .forced_template_entries
            .iter()
            .filter(|entry| entries.insert(**entry))
            .count()
            == 0
        {
            break (plan, manifest);
        }
    };
    Ok((eventai, manifest))
}

fn vmangos_eventai_plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let (eventai, manifest) = vmangos_creature_ai(dump, args)?;
    let requirements =
        vmangos_quests::event_requirement_rows(dump, args, &eventai.quest_event_entries)?;
    let mut stmts = vec![
        "DELETE FROM game_quest_event_requirement WHERE id > 0".into(),
        "DELETE FROM game_creature_ai_broadcast_text WHERE id > 0".into(),
        "DELETE FROM game_creature_ai_summon WHERE id > 0".into(),
    ];
    push_insert(&mut stmts, "game_creature_ai_broadcast_text", "id,male_text,female_text,chat_type,language_id,emote_delay_1_ms,emote_id_1,emote_delay_2_ms,emote_id_2,emote_delay_3_ms,emote_id_3,sound_id", &eventai.broadcast_rows);
    push_insert(
        &mut stmts,
        "game_creature_ai_summon",
        "id,x,y,z,orientation,lifetime_ms",
        &eventai.summon_rows,
    );
    push_insert(
        &mut stmts,
        "game_quest_event_requirement",
        "id,quest_entry",
        &requirements,
    );
    Ok(DumpPlan {
        stmts,
        stamps: vec![(
            "creature-ai",
            eventai.row_count() + requirements.len() as u64,
        )],
        eventai_definition_count: eventai.definition_count(),
        eventai_instruction_count: eventai.instruction_count(),
        eventai_relay_definition_count: eventai.relay_definition_rows.len() as u64,
        eventai_definition_batches: eventai.definition_batches,
        eventai_relay_definition_batches: eventai.relay_definition_batches,
        world_source_manifest: Some(serde_json::to_string_pretty(&manifest)?),
        ..Default::default()
    })
}

fn build_vmangos_plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    if args.family.is_none() {
        combined_vmangos_plan(dump, args)
    } else {
        vmangos_family_plan(dump, args)
    }
}

fn vmangos_family_plan(dump: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    match args.family.as_deref() {
        Some("dbc") => dbc::replacement_plan(dump, args.dbc.as_deref().context("dbc requires --dbc")?),
        Some("spellmeta") => vmangos_spellmeta::plan(dump),
        Some("spell") => spell::world_plan(dump, args.dbc.as_deref().context("spell requires --dbc")?),
        Some("creatures" | "casts") => vmangos_creatures::plan(dump, args),
        Some("creature-ai") => vmangos_eventai_plan(dump, args),
        Some("items") => vmangos_items_plan(dump, args),
        Some("loot") => vmangos_loot_plan(dump, args),
        Some("gameobjects") => vmangos_gameobjects_plan(dump, args),
        Some("trainers") => trainers::plan(dump, args),
        Some("quests") => vmangos_quests::plan(dump, args),
        Some("gossip") => vmangos_gossip::plan(dump, args),
        Some("globals") => {
            let stmts = globals_statements(dump, args.dbc.as_deref())?;
            let count = insert_row_count(&stmts);
            Ok(DumpPlan {
                stmts,
                stamps: vec![("globals", count)],
                world_source_manifest: Some(serde_json::to_string_pretty(&globals_findings(
                    dump,
                )?)?),
                ..Default::default()
            })
        }
        _ => bail!(
            "vmangos currently supports --family creatures, casts, creature-ai, dbc, globals, items, loot, gameobjects, trainers, quests, gossip, spell or spellmeta"
        ),
    }
}

fn combined_vmangos_plan(source: &impl TableRows, args: &WorldImportArgs) -> Result<DumpPlan> {
    let client = args
        .dbc
        .as_deref()
        .context("a complete world import requires --dbc")?;
    let cached = crate::world_source::CachedSource::new(source);
    let source = &cached;
    let mut combined = spell::world_plan(source, client)?;
    let catalogues = dbc::catalogue_statements(client, source)?;
    combined.stamps.push(("dbc", insert_row_count(&catalogues)));
    combined.stmts.extend(catalogues);
    let mut manifests = vec![dbc::findings(source)?];
    if let Some(manifest) = combined.world_source_manifest.take() {
        manifests.push(serde_json::from_str::<serde_json::Value>(&manifest)?);
    }
    for family in FAMILIES {
        let mut selected = args.clone();
        selected.family = Some((*family).into());
        let plan = vmangos_family_plan(source, &selected)?;
        combined.stmts.extend(plan.stmts);
        combined.stamps.extend(plan.stamps);
        combined.spawn_batches.extend(plan.spawn_batches);
        combined.spawn_row_count += plan.spawn_row_count;
        combined.go_batches.extend(plan.go_batches);
        combined.go_row_count += plan.go_row_count;
        combined
            .eventai_definition_batches
            .extend(plan.eventai_definition_batches);
        combined
            .eventai_relay_definition_batches
            .extend(plan.eventai_relay_definition_batches);
        combined.eventai_definition_count += plan.eventai_definition_count;
        combined.eventai_relay_definition_count += plan.eventai_relay_definition_count;
        combined.eventai_instruction_count += plan.eventai_instruction_count;
        if let Some(manifest) = plan.world_source_manifest {
            manifests.push(serde_json::from_str::<serde_json::Value>(&manifest)?);
        }
    }
    combined.world_source_manifest = Some(serde_json::to_string_pretty(&manifests)?);
    Ok(combined)
}

pub(crate) fn globals_findings(dump: &impl TableRows) -> Result<serde_json::Value> {
    let mut conditioned = Vec::new();
    for row in dump.rows("areatrigger_teleport")?.iter() {
        if row.number::<u32>("required_condition")? != 0 {
            conditioned.push(row.number::<u32>("id")?);
        }
    }
    conditioned.sort_unstable();
    Ok(serde_json::json!({
        "format_version": 1,
        "source": "vmangos",
        "family": "globals",
        "patch": crate::source_selection::PATCH,
        "client_build": crate::source_selection::CLIENT_BUILD,
        "findings": [{
            "code": "globals.portal-condition",
            "classification": "unsupported",
            "occurrences": conditioned.len(),
            "source_ids": conditioned,
        }],
    }))
}

pub(crate) fn run(dump_path: &str, args: &WorldImportArgs) -> Result<()> {
    let dump = WorldSource::open(std::path::Path::new(dump_path))?;
    let plan = build_vmangos_plan(&dump, args)?;
    emit_plan(&dump, args, &plan)
}

pub(crate) fn emit_plan(
    dump: &impl TableRows,
    args: &WorldImportArgs,
    plan: &DumpPlan,
) -> Result<()> {
    if let Some(profile) = dump.source_profile_report() {
        eprintln!(
            "World Source Profile\n{}",
            serde_json::to_string_pretty(&profile)?
        );
    }
    if let Some(manifest) = &plan.world_source_manifest {
        eprintln!("World Source Compatibility Manifest\n{manifest}");
    }
    if !args.apply {
        println!(
            "-- DRY RUN: {} SQL statements, {} EventAI definition batch(es) for {} definitions and {} instructions, {} relay definition batch(es) for {} definitions, {} creature-spawn batch(es) for {} spawns, and {} gameobject batch(es) for {} spawns (of {SPAWN_BATCH}).\n",
            plan.stmts.len(),
            plan.eventai_definition_batches.len(),
            plan.eventai_definition_count,
            plan.eventai_instruction_count,
            plan.eventai_relay_definition_batches.len(),
            plan.eventai_relay_definition_count,
            plan.spawn_batches.len(),
            plan.spawn_row_count,
            plan.go_batches.len(),
            plan.go_row_count
        );
        for s in &plan.stmts {
            println!("{s};\n");
        }
        if family_active(args, "creature-ai") {
            println!(
                "-- load native EventAI definitions via reducer: batch 0 replaces the family, and later batches append.\n  e.g. spacetime call -s {} {} import_creature_ai_definitions '<batch>'\n  (sample definition: {})",
                args.shard.server,
                args.shard.db,
                plan.eventai_definition_batches
.first()
.and_then(|batch| batch.lines().next())
.unwrap_or("")
            );
            println!(
                "-- load the typed EventAI relay catalogue atomically through the reducer.\n  e.g. spacetime call -s {} {} import_creature_ai_relay_definitions '<catalogue>'",
                args.shard.server,
                args.shard.db,
            );
        }
        println!(
            "-- load spawns via reducer: batch 0 = import_creature_spawns (clears+loads), the rest = import_creature_spawns_append (load only).\n  e.g. spacetime call -s {} {} import_creature_spawns '<batch>'\n  (sample row: {})",
            args.shard.server, args.shard.db, plan.spawn_batches.first().and_then(|b| b.split(';').next()).unwrap_or("")
        );
        for family in FAMILIES {
            run_package_stage(args, family)?;
        }
        if family_active(args, "spell") {
            run_package_stage(args, "spell")?;
        }
        return Ok(());
    }

    if family_active(args, "creatures") && plan.spawn_batches.is_empty() {
        bail!("no spawns matched the filter — check --box / --radius / --center");
    }

    args.shard.run_sql_statements(&plan.stmts, "creatures")?;
    for (reducer, batch) in plan.reducer_batches(args)? {
        args.shard
            .call_reducer(reducer, batch)
            .with_context(|| format!("{reducer} batch"))?;
    }
    eprintln!("import applied.");

    let file_hash = dump.source_hash()?;
    for (family, row_count) in &plan.stamps {
        args.shard
            .stamp_family(family, &args.source_sha, &file_hash, *row_count)
            .with_context(|| format!("stamp_import_meta({family})"))?;
    }

    for family in FAMILIES {
        run_package_stage(args, family)?;
    }
    if family_active(args, "spell") {
        run_package_stage(args, "spell")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "world_import_gameobjects_tests.rs"]
mod gameobjects_tests;

#[cfg(test)]
#[path = "world_import_loot_tests.rs"]
mod loot_tests;

#[cfg(test)]
#[path = "world_import_items_tests.rs"]
mod items_tests;

struct GameobjectTemplateRows {
    templates: Vec<String>,
    traps: Vec<String>,
    meeting_stones: Vec<String>,
}

#[cfg(test)]
#[path = "world_import_source_tests.rs"]
mod source_tests;
