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
    ai, at, bt, cls, clt, cm, cmt, ct, cts, dse, field, gm, gmo, go, got, insert_row_count, it,
    lvl, mlt, npct, nt, nv, parse_table, pc, pca, pci, pcs, push_insert, qr, qt, sc, sls, spe,
    sql_text, TableRows,
};
use crate::shard::Shard;
use crate::world_import_scope::{WorldImportProfile, WorldImportScope};
use crate::world_source::{SourceKind, WorldSource};
use crate::{
    creature_population, dbc, eventai, gossip_conditions, item_property, package_delta, spell,
};

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

fn go_initial_state(stored_type: u8, data0: u32) -> u8 {
    if (stored_type == GO_DOOR || stored_type == GO_BUTTON) && data0 != 0 {
        1
    } else {
        0
    }
}

#[allow(clippy::type_complexity)]
// CMaNGOS teaches profession tiers through gossip, so npc_trainer omits these rows.
pub(crate) const PROFESSION_LEARN: &[(u32, &[(u32, u8, u8)])] = &[
    (186, &[(2575, 0, 1), (2576, 1, 10), (3564, 2, 20)]),
    (182, &[(2366, 0, 1), (2368, 1, 10), (3570, 2, 20)]),
    (393, &[(8613, 0, 1), (8617, 1, 10), (8618, 2, 20)]),
    (185, &[(2550, 0, 1), (3102, 1, 10), (3413, 2, 20)]),
    (129, &[(3273, 0, 1), (3274, 1, 10), (7924, 2, 20)]),
    (
        171,
        &[(2259, 0, 1), (3101, 1, 10), (3464, 2, 20), (11611, 3, 35)],
    ),
    (
        164,
        &[(2018, 0, 1), (3100, 1, 10), (3538, 2, 20), (9785, 3, 35)],
    ),
    (
        197,
        &[(3908, 0, 1), (3909, 1, 10), (3910, 2, 20), (12180, 3, 35)],
    ),
    (
        165,
        &[(2108, 0, 1), (3104, 1, 10), (3811, 2, 20), (10662, 3, 35)],
    ),
    (
        202,
        &[(4036, 0, 1), (4037, 1, 10), (4038, 2, 20), (12656, 3, 35)],
    ),
    (
        333,
        &[(7411, 0, 1), (7412, 1, 10), (7413, 2, 20), (13920, 3, 35)],
    ),
    (356, &[(7620, 0, 1), (7731, 1, 10), (7732, 2, 20)]),
];

pub(crate) const LEGACY_TIER_CAPS: [u32; 4] = [75, 150, 225, 300];

fn profession_tiers(line: u32) -> Option<&'static [(u32, u8, u8)]> {
    PROFESSION_LEARN
        .iter()
        .find(|(l, _)| *l == line)
        .map(|(_, t)| *t)
}

pub(crate) fn assert_profession_tier_parity(
    tier_values: &std::collections::HashMap<u32, [u16; 4]>,
) -> Result<()> {
    for &(line, tiers) in PROFESSION_LEARN {
        let Some(caps) = tier_values.get(&line) else {
            bail!("profession tier parity: skill line {line} has no SkillRaceClassInfo/SkillTiers row in the client data");
        };
        for &(_, tier_index, _) in tiers {
            let dbc_cap = caps[tier_index as usize] as u32;
            let legacy = LEGACY_TIER_CAPS[tier_index as usize];
            if dbc_cap != legacy {
                bail!(
                    "profession tier parity: skill line {line} tier {tier_index} — dbc cap {dbc_cap} != legacy literal {legacy}"
                );
            }
        }
    }
    Ok(())
}
// -1 means every playable class or race in both World Sources.
fn normalize_item_mask(source: i64, unrestricted: u32) -> Result<u32> {
    if source == -1 {
        Ok(unrestricted)
    } else {
        u32::try_from(source).context("item_template: invalid class or race mask")
    }
}
// ClassicDB z2815 uses subclass zero for both food and drink.
pub(crate) fn is_drink_consumable(class: u8, subclass: u8, name: &str) -> bool {
    if class != 0 || (subclass != 0 && subclass != 5) {
        return false;
    }
    const DRINK_WORDS: &[&str] = &[
        "water", "dew", "drink", "milk", "juice", "tea", "nectar", "wine", "ale", "soda", "coffee",
    ];
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| DRINK_WORDS.contains(&w))
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
            let zone_column = match dump.kind() {
                SourceKind::ClassicDb => {
                    let link_kind: u32 = row.number("link_kind")?;
                    if link_kind != 0 {
                        bail!("game_graveyard_zone.link_kind: unsupported value {link_kind}");
                    }
                    "ghost_loc"
                }
                SourceKind::Vmangos => "ghost_zone",
            };
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
            let spell = r.number::<u32>(match dump.kind() {
                SourceKind::ClassicDb => pcs::SPELL,
                SourceKind::Vmangos => "spell",
            })?;
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

fn build_spell_chain_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
    links: &[(u32, u32)],
) -> Result<Vec<String>> {
    let parsed: Vec<(u32, String)> = parse_table(dump, "spell_chain")?
        .iter()
        .map(|r| -> Result<_> {
            let Ok(spell_id) = field(r, sc::SPELL_ID)?.parse::<u32>() else {
                return Ok(None);
            };

            if spell_id == 0 {
                return Ok(None);
            }
            let Ok(prev_spell) = field(r, sc::PREV_SPELL)?.parse::<u32>() else {
                return Ok(None);
            };
            let Ok(first_spell) = field(r, sc::FIRST_SPELL)?.parse::<u32>() else {
                return Ok(None);
            };
            let Ok(rank) = field(r, sc::RANK)?.parse::<u8>() else {
                return Ok(None);
            };
            let Ok(req_spell) = field(r, sc::REQ_SPELL)?.parse::<u32>() else {
                return Ok(None);
            };
            Ok(Some((
                spell_id,
                format!("({spell_id},{prev_spell},{first_spell},{rank},{req_spell})"),
            )))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_>>()?;
    let explicit = parsed.iter().map(|(spell, _)| *spell).collect();
    let mut rows: Vec<_> = parsed.into_iter().map(|(_, sql)| sql).collect();
    rows.extend(derived_spell_rank_rows(links, &explicit)?);
    let mut stmts = vec!["DELETE FROM game_spell_chain WHERE spell_id > 0".to_string()];
    push_insert(
        &mut stmts,
        "game_spell_chain",
        "spell_id,prev_spell,first_spell,rank,req_spell",
        &rows,
    );
    eprintln!("spell chain: {} rows", rows.len());
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

fn build_spell_learn_sql(dump: &(impl crate::dump::TableRows + ?Sized)) -> Result<Vec<String>> {
    use lyracore_shared::constants::armor_proficiency;

    let mut parents_seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut rows: Vec<String> = parse_table(dump, "spell_learn_spell")?
        .iter()
        .enumerate()
        .map(|(i, r)| -> Result<_> {
            let Ok(parent_spell) = field(r, sls::ENTRY)?.parse::<u32>() else {
                return Ok(None);
            };
            let Ok(learn_spell) = field(r, sls::SPELL_ID)?.parse::<u32>() else {
                return Ok(None);
            };
            let _active: u8 = field(r, sls::ACTIVE)?.parse().unwrap_or(1);
            parents_seen.insert(parent_spell);
            Ok(Some(format!(
                "({},{parent_spell},{learn_spell})",
                i as u64 + 1
            )))
        })
        .filter_map(Result::transpose)
        .collect::<Result<_>>()?;
    let mut next_id = rows.len() as u64 + 1;
    for (parent, learn) in [
        (
            armor_proficiency::PLATE_TRAINER_SPELL_ID,
            armor_proficiency::PLATE_PASSIVE_SPELL_ID,
        ),
        (
            armor_proficiency::MAIL_TRAINER_SPELL_ID,
            armor_proficiency::MAIL_PASSIVE_SPELL_ID,
        ),
    ] {
        if parents_seen.contains(&parent) {
            continue;
        }
        rows.push(format!("({next_id},{parent},{learn})"));
        next_id += 1;
    }
    let mut stmts = vec!["DELETE FROM game_spell_learn WHERE id > 0".to_string()];
    push_insert(
        &mut stmts,
        "game_spell_learn",
        "id,parent_spell,learn_spell",
        &rows,
    );
    eprintln!("spell learn dependents: {} rows", rows.len());
    Ok(stmts)
}

fn proc_family_mask(
    row: &crate::world_source::Row,
    spell_id: u32,
    proc_effects: Option<&std::collections::HashMap<u32, Vec<usize>>>,
) -> Result<Option<u64>> {
    let masks = [
        field(row, "SpellFamilyMask0")?,
        field(row, "SpellFamilyMask1")?,
        field(row, "SpellFamilyMask2")?,
    ];
    let Some(masks) = masks
        .into_iter()
        .map(|mask| mask.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(None);
    };
    if masks.iter().all(|mask| *mask == masks[0]) {
        return Ok(Some(masks[0]));
    }
    let effects = proc_effects
        .and_then(|effects| effects.get(&spell_id))
        .with_context(|| {
            format!("spell {spell_id} has per-effect proc masks; supply its client Spell.dbc")
        })?;
    let mut active = effects.iter().map(|index| {
        masks
            .get(*index)
            .copied()
            .context("proc effect index outside the three spell effects")
    });
    let mask = active.next().transpose()?.with_context(|| {
        format!("spell {spell_id} has differing masks but no supported Proc effect")
    })?;
    for next in active {
        if next? != mask {
            bail!("spell {spell_id} needs different masks for active Proc effects; scalar overlay cannot represent it");
        }
    }
    Ok(Some(mask))
}

fn build_spell_proc_event_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
    proc_effects: Option<&std::collections::HashMap<u32, Vec<usize>>>,
) -> Result<Vec<String>> {
    let rows: Vec<String> = parse_table(dump, "spell_proc_event")?
.iter()
.map(|r| -> Result<Option<String>> {
            let Some(spell_id) = field(r, spe::ENTRY)?.parse::<u32>().ok() else { return Ok(None); };
            if spell_id == 0 || spell_id >= SYNTHETIC_PROC_EVENT_FLOOR {
                return Ok(None);
            }
            let Some(school_mask) = field(r, spe::SCHOOL_MASK)?.parse::<u8>().ok() else { return Ok(None); };
            let Some(family_name) = field(r, spe::FAMILY_NAME)?.parse::<u8>().ok() else { return Ok(None); };
            let Some(family_flags) = proc_family_mask(r, spell_id, proc_effects)? else { return Ok(None); };
            let Some(proc_flags) = field(r, spe::PROC_FLAGS)?.parse::<u32>().ok() else { return Ok(None); };
            let Some(proc_ex) = field(r, spe::PROC_EX)?.parse::<u32>().ok() else { return Ok(None); };
            let Some(ppm_rate) = field(r, spe::PPM_RATE)?.parse::<f32>().ok() else { return Ok(None); };
            let Some(custom_chance) = field(r, spe::CUSTOM_CHANCE)?.parse::<f32>().ok() else { return Ok(None); };
            let Some(cooldown_s) = field(r, spe::COOLDOWN)?.parse::<u32>().ok() else { return Ok(None); };
            if !ppm_rate.is_finite() || ppm_rate < 0.0 || !custom_chance.is_finite() {
                return Ok(None);
            }
            let custom_chance = custom_chance.round().clamp(0.0, 100.0) as u8;
            let icd_ms = cooldown_s.saturating_mul(1000);
            Ok(Some(format!(
                "({spell_id},{proc_flags},{proc_ex},{school_mask},{family_name},{family_flags},{ppm_rate},{custom_chance},{icd_ms})"
            )))
        })
.collect::<Result<Vec<_>>>()?.into_iter().flatten().collect();
    let mut stmts = vec![format!(
        "DELETE FROM game_spell_proc_event WHERE spell_id < {SYNTHETIC_PROC_EVENT_FLOOR}"
    )];
    push_insert(
        &mut stmts,
        "game_spell_proc_event",
        "spell_id,proc_flags,proc_ex,school_mask,family_name,family_flags,ppm_rate,custom_chance,icd_ms",
        &rows,
    );
    eprintln!("spell proc events: {} rows", rows.len());
    Ok(stmts)
}

const SYNTHETIC_PROC_EVENT_FLOOR: u32 = 50_000;

fn null_to_empty(s: &str) -> &str {
    let t = s.trim();
    if t == "NULL" {
        ""
    } else {
        t
    }
}

type NpcTextSlot = (String, String, f32);

fn parse_npc_text_slots(row: &crate::world_source::Row) -> Result<[NpcTextSlot; 8]> {
    let mut slots = core::array::from_fn(|_| (String::new(), String::new(), 0.0));
    for (slot, value) in slots.iter_mut().enumerate() {
        let male = null_to_empty(field(row, &format!("text{slot}_0"))?).to_string();
        let female = null_to_empty(field(row, &format!("text{slot}_1"))?).to_string();
        let prob = field(row, &format!("prob{slot}"))?.parse().unwrap_or(0.0);
        *value = (male, female, prob);
    }
    Ok(slots)
}

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

fn resolve_gossip_option_text(
    row: &crate::world_source::Row,
    broadcast: &HashMap<u64, (String, String)>,
) -> Result<String> {
    let raw = field(row, gmo::OPTION_TEXT)?;
    if let Ok(bcast_id) = raw.trim().parse::<u64>() {
        if let Some((male, female)) = broadcast.get(&bcast_id) {
            if !male.is_empty() {
                return Ok(male.clone());
            }
            if !female.is_empty() {
                return Ok(female.clone());
            }
        }
    }
    Ok(null_to_empty(raw).to_string())
}

fn reclassify_gossip_option_action(text: &str, raw_action: u32) -> u32 {
    if text == "I wish to unlearn my talents." {
        lyracore_shared::constants::gossip_option::UNLEARNTALENTS
    } else {
        raw_action
    }
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

fn build_gossip_sql(
    dump: &(impl crate::dump::TableRows + ?Sized),
    creature_entries: &std::collections::HashSet<u64>,
) -> Result<GossipEtl> {
    use std::collections::{HashMap, HashSet};

    let mut broadcast: HashMap<u64, (String, String)> = HashMap::new();
    for row in parse_table(dump, "broadcast_text")?.iter() {
        let id: u64 = field(row, bt::ID)?.parse().unwrap_or(0);
        if id == 0 {
            continue;
        }
        let male = null_to_empty(field(row, bt::TEXT)?).to_string();
        let female = null_to_empty(field(row, bt::TEXT1)?).to_string();
        if !male.is_empty() || !female.is_empty() {
            broadcast.insert(id, (male, female));
        }
    }

    let mut npc_texts: HashMap<u64, String> = HashMap::new();
    let mut npc_text_slots: HashMap<u64, [NpcTextSlot; 8]> = HashMap::new();
    for row in parse_table(dump, "npc_text")?.iter() {
        let id: u64 = field(row, npct::ID)?.parse().unwrap_or(0);
        if id == 0 {
            continue;
        }
        let slots = parse_npc_text_slots(row)?;
        let text = slots_first_nonempty(&slots);
        if !text.is_empty() {
            npc_texts.insert(id, text);
            npc_text_slots.insert(id, slots);
        }
    }

    let mut menu_to_text: HashMap<u64, u64> = HashMap::new();
    let mut menu_text_is_conditional: HashMap<u64, bool> = HashMap::new();
    for row in parse_table(dump, "gossip_menu")?.iter() {
        let entry: u64 = field(row, gm::ENTRY)?.parse().unwrap_or(0);
        let text_id: u64 = field(row, gm::TEXT_ID)?.parse().unwrap_or(0);
        if entry == 0 || text_id == 0 {
            continue;
        }
        let conditional = field(row, gm::CONDITION_ID)?.parse::<u64>().unwrap_or(0) != 0;
        let held_conditional = menu_text_is_conditional.get(&entry).copied();
        if held_conditional.is_none() || (held_conditional == Some(true) && !conditional) {
            menu_to_text.insert(entry, text_id);
            menu_text_is_conditional.insert(entry, conditional);
        }
    }

    let mut gossip_rows: Vec<String> = Vec::new();
    let mut needed_text_ids: HashSet<u64> = HashSet::new();
    let mut entry_to_menu: HashMap<u64, u64> = HashMap::new();

    for row in parse_table(dump, "creature_template")?.iter() {
        let entry: u64 = field(row, ct::ENTRY)?.parse().unwrap_or(0);
        if entry == 0 || !creature_entries.contains(&entry) {
            continue;
        }
        let gossip_menu_id: u64 = field(row, ct::GOSSIP_MENU_ID)?.parse().unwrap_or(0);
        if gossip_menu_id == 0 {
            continue;
        }
        entry_to_menu.insert(entry, gossip_menu_id);
        let text_id = menu_to_text.get(&gossip_menu_id).copied().unwrap_or(0);
        if text_id != 0 && npc_texts.contains_key(&text_id) {
            gossip_rows.push(format!("({entry},{text_id})"));
            needed_text_ids.insert(text_id);
        } else if npc_texts.contains_key(&entry) {
            gossip_rows.push(format!("({entry},{entry})"));
            needed_text_ids.insert(entry);
        }
    }

    const RELAY_GOSSIP_MENUS: [u64; 4] = [6_623, 6_687, 6_688, 6_949];
    let mut profile_rows = Vec::new();
    for menu_id in RELAY_GOSSIP_MENUS {
        let Some(&text_id) = menu_to_text.get(&menu_id) else {
            continue;
        };
        profile_rows.push(format!("({menu_id},{text_id})"));
        needed_text_ids.insert(text_id);
    }

    let mut npc_text_rows: Vec<String> = Vec::new();
    let mut npc_text_slot_rows: Vec<String> = Vec::new();
    let mut slot_row_id: u64 = 1;
    let mut needed_text_ids: Vec<_> = needed_text_ids.into_iter().collect();
    needed_text_ids.sort_unstable();
    for text_id in needed_text_ids {
        let Some(text) = npc_texts.get(&text_id) else {
            continue;
        };
        npc_text_rows.push(format!("({text_id},{})", sql_text(text)));
        if let Some(slots) = npc_text_slots.get(&text_id) {
            for (slot_index, (male, female, prob)) in slots.iter().enumerate() {
                if male.is_empty() && female.is_empty() && *prob == 0.0 {
                    continue;
                }
                npc_text_slot_rows.push(format!(
                    "({slot_row_id},{text_id},{slot_index},{},{},{prob})",
                    sql_text(male),
                    sql_text(female),
                ));
                slot_row_id += 1;
            }
        }
    }

    let conditions = gossip_conditions::read(dump)?;
    let mut menu_to_options: HashMap<u64, Vec<(u64, &crate::world_source::Row)>> = HashMap::new();
    let options = parse_table(dump, "gossip_menu_option")?;
    for row in options.iter() {
        let menu_id: u64 = field(row, gmo::MENU_ID)?.parse().unwrap_or(0);
        if menu_id == 0 {
            continue;
        }
        menu_to_options
            .entry(menu_id)
            .or_default()
            .push((field(row, gmo::ID)?.parse::<u64>().unwrap_or(0), row));
    }
    let mut option_rows: Vec<String> = Vec::new();
    let mut next_row_id: u32 = 1;
    let mut entries_sorted: Vec<&u64> = entry_to_menu.keys().collect();
    entries_sorted.sort();
    for &entry in entries_sorted {
        let menu_id = entry_to_menu[&entry];
        let Some(opts) = menu_to_options.get(&menu_id) else {
            continue;
        };
        let mut sorted_opts = opts.clone();
        sorted_opts.sort_by_key(|(id, _)| *id);
        let mut option_index = 0u32;
        for (_, row) in sorted_opts.iter() {
            let icon: u32 = field(row, gmo::OPTION_ICON)?.parse().unwrap_or(0);
            let text = resolve_gossip_option_text(row, &broadcast)?;
            let action: u32 = reclassify_gossip_option_action(
                &text,
                field(row, gmo::OPTION_TYPE)?.parse().unwrap_or(0),
            );
            if action == lyracore_shared::constants::gossip_option::QUESTGIVER {
                continue;
            }
            let action_menu_id: u32 = field(row, gmo::ACTION_MENU_ID)?.parse().unwrap_or(0);
            let (cond_type, cond_value1, cond_value2) =
                gossip_conditions::resolve(row, &conditions)?;
            option_rows.push(format!(
                "({next_row_id},{entry},{option_index},{icon},{},{action},{action_menu_id},{cond_type},{cond_value1},{cond_value2})",
                sql_text(&text),
            ));
            next_row_id += 1;
            option_index += 1;
        }
    }

    let mut profile_option_rows = Vec::new();
    let mut profile_row_id = 1u32;
    for menu_id in RELAY_GOSSIP_MENUS {
        let Some(opts) = menu_to_options.get(&menu_id) else {
            continue;
        };
        let mut sorted_opts = opts.clone();
        sorted_opts.sort_by_key(|(id, _)| *id);
        let mut option_index = 0u32;
        for (_, row) in sorted_opts {
            let icon: u32 = field(row, gmo::OPTION_ICON)?.parse().unwrap_or(0);
            let text = resolve_gossip_option_text(row, &broadcast)?;
            let action = reclassify_gossip_option_action(
                &text,
                field(row, gmo::OPTION_TYPE)?.parse().unwrap_or(0),
            );
            if action == lyracore_shared::constants::gossip_option::QUESTGIVER {
                continue;
            }
            let action_menu_id: u32 = field(row, gmo::ACTION_MENU_ID)?.parse().unwrap_or(0);
            let (cond_type, cond_value1, cond_value2) =
                gossip_conditions::resolve(row, &conditions)?;
            profile_option_rows.push(format!(
                "({profile_row_id},{menu_id},{option_index},{icon},{},{action},{action_menu_id},{cond_type},{cond_value1},{cond_value2})",
                sql_text(&text),
            ));
            profile_row_id += 1;
            option_index += 1;
        }
    }

    eprintln!(
        "gossip: {} creature→text mappings, {} npc_text strings resolved ({} weighted slot rows), {} menu options, {} relay profiles, {} relay profile options",
        gossip_rows.len(), npc_text_rows.len(), npc_text_slot_rows.len(), option_rows.len(),
        profile_rows.len(), profile_option_rows.len(),
    );
    Ok(GossipEtl {
        menu_rows: gossip_rows,
        npc_text_rows,
        npc_text_slot_rows,
        option_rows,
        profile_rows,
        profile_option_rows,
        condition_rows: gossip_conditions::sql_rows(&conditions),
    })
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
    reward_item_entries: std::collections::HashSet<u64>,
    req_item_entries: std::collections::HashSet<u64>,
    chained_count: usize,
    timed_count: usize,
    raid_quest_count: usize,
}

type ReferenceLoot = std::collections::HashMap<u64, Vec<(u64, f64, i64, u32)>>;

fn parse_ref_pools(dump: &(impl crate::dump::TableRows + ?Sized)) -> Result<ReferenceLoot> {
    if dump.kind() == SourceKind::Vmangos {
        return Ok(Default::default());
    }
    let mut refs: ReferenceLoot = std::collections::HashMap::new();
    for row in parse_table(dump, "reference_loot_template")?.iter() {
        let g: u64 = row.number(clt::ENTRY)?;
        let item: u64 = row.number(clt::ITEM)?;
        let chance: f64 = row.number(clt::CHANCE)?;
        let mincount_or_ref: i64 = row.number(clt::MINCOUNT_OR_REF)?;
        let cnt: u32 = row.number(clt::MAXCOUNT)?;
        refs.entry(g)
            .or_default()
            .push((item, chance, mincount_or_ref, cnt));
    }
    Ok(refs)
}

fn resolve_ref_pool(
    pool_id: u64,
    refs: &ReferenceLoot,
    visited: &mut std::collections::HashSet<u64>,
) -> Vec<(u64, f64, u32)> {
    if !visited.insert(pool_id) {
        return Vec::new();
    }
    let Some(rows) = refs.get(&pool_id) else {
        visited.remove(&pool_id);
        return Vec::new();
    };
    let mut out = Vec::new();
    for &(item, chance, mincount_or_ref, maxcount) in rows {
        if mincount_or_ref < 0 {
            // A classic-db reference chance of zero means always; otherwise it gates the normalized child
            // weights.
            let children = resolve_ref_pool(mincount_or_ref.unsigned_abs(), refs, visited);
            let gate = chance.abs();
            if gate > 0.0 {
                let child_total: f64 = children.iter().map(|(_, c, _)| c.max(0.0)).sum();
                if child_total > 0.0 {
                    let scale = gate / child_total;
                    out.extend(children.into_iter().map(|(i, c, n)| (i, c * scale, n)));
                }
            } else {
                out.extend(children);
            }
        } else if item != 0 && chance >= 0.0 {
            out.push((item, chance, maxcount.max(1)));
        }
    }

    visited.remove(&pool_id);
    out
}

#[allow(clippy::type_complexity)]
fn parse_loot_family(
    dump: &(impl crate::dump::TableRows + ?Sized),
    table: &str,
    refs: &ReferenceLoot,
) -> Result<std::collections::HashMap<u64, Vec<(u64, u32, u32, u32, bool)>>> {
    if dump.kind() == SourceKind::Vmangos {
        return vmangos_loot::parse(dump, table);
    }
    use std::collections::HashMap;
    // Loot chances use basis points, preserving odds below one percent.
    let bp = |pct: f64| (pct * 100.0).round() as i64;
    let mut drops_by_entry: HashMap<u64, Vec<(u64, u32, u32, u32, bool)>> = HashMap::new();
    for row in parse_table(dump, table)?.iter() {
        let entry: u64 = row.number(clt::ENTRY)?;
        let item: u64 = row.number(clt::ITEM)?;
        let raw_chance: f64 = row.number(clt::CHANCE)?;
        let groupid: u32 = row.number(clt::GROUPID)?;
        let ref_or_min: i64 = row.number(clt::MINCOUNT_OR_REF)?;
        let maxcount: u32 = row.number(clt::MAXCOUNT)?;
        let quest_only = raw_chance < 0.0;
        let chance = raw_chance.abs();
        if chance == 0.0 {
            continue;
        }
        let drops = drops_by_entry.entry(entry).or_default();
        if ref_or_min < 0 {
            // Each reference row needs its own recursion path to resolve shared pools.
            let pool_id = ref_or_min.unsigned_abs();
            let mut visited = std::collections::HashSet::new();
            let items = resolve_ref_pool(pool_id, refs, &mut visited);
            if items.is_empty() {
                continue;
            }
            let gate_bp = bp(chance).clamp(1, 10000);
            let sum_explicit: i64 = items
                .iter()
                .filter(|(_, c, _)| *c > 0.0)
                .map(|(_, c, _)| (gate_bp as f64 * c / 100.0).round() as i64)
                .sum();
            let n_zero = items.iter().filter(|(_, c, _)| *c <= 0.0).count().max(1) as i64;
            let leftover = (gate_bp - sum_explicit).max(0);

            let group_id = 1_000_000 + drops.len() as u32;
            for &(ritem, rchance, rcount) in &items {
                let item_bp = if rchance > 0.0 {
                    (gate_bp as f64 * rchance / 100.0).round() as i64
                } else {
                    leftover / n_zero
                };
                drops.push((
                    ritem,
                    item_bp.clamp(0, 10000) as u32,
                    rcount,
                    group_id,
                    quest_only,
                ));
            }
        } else if item != 0 {
            let cbp = bp(chance).clamp(1, 10000) as u32;
            drops.push((item, cbp, maxcount.max(1), groupid, quest_only));
        }
    }
    Ok(drops_by_entry)
}

#[allow(clippy::type_complexity)]
fn parse_creature_drops(
    dump: &(impl crate::dump::TableRows + ?Sized),
) -> Result<std::collections::HashMap<u64, Vec<(u64, u32, u32, u32, bool)>>> {
    let refs = parse_ref_pools(dump)?;
    parse_loot_family(dump, "creature_loot_template", &refs)
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
    let refs = parse_ref_pools(dump)?;
    let loot = parse_loot_family(dump, "gameobject_loot_template", &refs)?;
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
    let (entry_column, vendor_column) = if dump.kind() == SourceKind::Vmangos {
        ("entry", "vendor_id")
    } else {
        ("Entry", "VendorTemplateId")
    };
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

    let relations_for = |table: &str, entries: &HashSet<u64>| -> Result<Vec<(u64, u64)>> {
        parse_table(dump, table)?
            .iter()
            .map(|row| -> Result<_> {
                let entry = row.number::<u64>(qr::CREATURE)?;
                let quest = row.number::<u64>(qr::QUEST)?;
                Ok((entries.contains(&entry) && quest > 0).then_some((entry, quest)))
            })
            .filter_map(Result::transpose)
            .collect::<Result<_>>()
    };
    let starts = relations_for("creature_questrelation", catalogue_creatures)?;
    let ends = relations_for("creature_involvedrelation", catalogue_creatures)?;
    let go_starts = relations_for("gameobject_questrelation", catalogue_gameobjects)?;
    let go_ends = relations_for("gameobject_involvedrelation", catalogue_gameobjects)?;
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
    let mut reward_item_entries: HashSet<u64> = HashSet::new();
    let mut req_item_entries: HashSet<u64> = HashSet::new();
    let mut valid_quests: HashSet<u64> = HashSet::new();
    let mut reward_mail_rows: Vec<String> = Vec::new();
    let mut reward_mail_template_ids: HashSet<u64> = HashSet::new();
    let mut reward_mail_quests: HashSet<u64> = HashSet::new();
    let mut chained_count: usize = 0;
    let mut timed_count: usize = 0;
    let mut raid_quest_count: usize = 0;
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
            if dump.kind() == SourceKind::ClassicDb {
                field(row, column).map(sql_text)
            } else {
                row.optional_text(column)
                    .map(|text| crate::dump::sql_str(text.unwrap_or_default().trim()))
            }
        };
        let mail_template: u32 = row.number(qt::REW_MAIL_TEMPLATE_ID)?;
        if mail_template != 0 {
            let mail_delay: u32 = row.number(qt::REW_MAIL_DELAY_SECS)?;
            let mail_money = if dump.kind() == SourceKind::Vmangos {
                row.number::<u32>("RewMailMoney")?
            } else {
                0
            };
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

        let reward_xp = if dump.kind() == SourceKind::Vmangos {
            row.number::<u32>("RewXP")?
        } else {
            0
        };
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
        if next_quest_raw > 0 {
            chained_count += 1;
        }
        if limit_time_raw > 0 {
            timed_count += 1;
        }
        let quest_type: u32 = row.number(qt::TYPE)?;
        if quest_type == 62 {
            raid_quest_count += 1;
        }
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
        const EMIT_COLLECT_OBJECTIVES: bool = true;
        if src_item > 0 {
            req_item_entries.insert(src_item);
        }
        for i in 0..4 {
            let item: i64 = row.number(&format!("ReqItemId{}", i + 1))?;
            let count: u32 = row.number(&format!("ReqItemCount{}", i + 1))?;
            if item > 0 {
                req_item_entries.insert(item as u64);
            }
            let obtainable =
                item > 0 && (obtainable_items.contains(&(item as u64)) || item as u64 == src_item);
            if EMIT_COLLECT_OBJECTIVES && obtainable && count > 0 {
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
                reward_item_entries.insert(item);
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
                reward_item_entries.insert(item);
                reward_choice_rows.push(format!(
                    "({rew_choice_id},{entry},{choice_rank},{item},{cnt})",
                    cnt = count.max(1)
                ));
                rew_choice_id += 1;
                choice_rank += 1;
            }
        }
    }

    if dump.kind() == SourceKind::ClassicDb {
        for row in parse_table(dump, "dbscripts_on_quest_end")?.iter() {
            let command: u32 = row.number(dse::COMMAND)?;
            if command != dse::SEND_MAIL {
                continue;
            }
            let entry: u64 = row.number(dse::ID)?;
            if entry == 0 || !valid_quests.contains(&entry) {
                continue;
            }
            if !reward_mail_quests.insert(entry) {
                bail!(
                    "dbscripts_on_quest_end: quest {entry} sends reward mail from BOTH \
                 quest_template.RewMailTemplateId and a SCRIPT_COMMAND_SEND_MAIL row"
                );
            }
            let mail_template: u32 = row.number(dse::DATALONG)?;
            let sender: u32 = row.number(dse::DATALONG2)?;
            let money: u32 = row.number(dse::DATALONG3)?;
            let delay: u32 = row.number(dse::DATAINT)?;
            reward_mail_template_ids.insert(mail_template as u64);
            reward_mail_rows.push(format!(
                "({entry},{mail_template},{delay},{sender},{money})"
            ));
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
        reward_item_entries,
        req_item_entries,
        chained_count,
        timed_count,
        raid_quest_count,
    })
}

fn forbidden_items(dump: &(impl TableRows + ?Sized)) -> Result<std::collections::HashSet<u64>> {
    if dump.kind() == SourceKind::ClassicDb {
        return Ok(Default::default());
    }
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
    let (creature_entry, vendor_id_column) = match dump.kind() {
        SourceKind::ClassicDb => (ct::ENTRY, "VendorTemplateId"),
        SourceKind::Vmangos => ("entry", "vendor_id"),
    };
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
        let read = |column: &str| it::read(row, dump.kind(), column);
        let entry: u64 = row.number(it::column(dump.kind(), it::ENTRY))?;
        if entry == 0 {
            continue;
        }
        emitted.insert(entry);
        let (mut s_str, mut s_agi, mut s_sta, mut s_int, mut s_spi) = (0i32, 0, 0, 0, 0);
        for i in 0..10 {
            let st: u8 = row.number(it::column(dump.kind(), &format!("stat_type{}", i + 1)))?;
            let sv: i32 = row.number(it::column(dump.kind(), &format!("stat_value{}", i + 1)))?;
            match st {
                3 => s_agi += sv,
                4 => s_str += sv,
                5 => s_int += sv,
                6 => s_spi += sv,
                7 => s_sta += sv,
                _ => {}
            }
        }
        let stack: u32 = row
            .number::<u32>(it::column(dump.kind(), it::STACKABLE))?
            .max(1);
        let allowed_class = normalize_item_mask(
            row.number(it::column(dump.kind(), it::ALLOWED_CLASS))?,
            ALL_PLAYABLE_CLASS_MASK,
        )?;
        let allowed_race = normalize_item_mask(
            row.number(it::column(dump.kind(), it::ALLOWED_RACE))?,
            ALL_PLAYABLE_RACE_MASK,
        )?;
        let class_v: u8 = row.number(it::column(dump.kind(), it::CLASS))?;
        let subclass_v: u8 = row.number(it::column(dump.kind(), it::SUBCLASS))?;
        let is_drink = dump.kind() == SourceKind::ClassicDb
            && is_drink_consumable(class_v, subclass_v, read(it::NAME)?);
        let holy: i32 = row.number(it::column(dump.kind(), it::HOLY_RES))?;
        let fire: i32 = row.number(it::column(dump.kind(), it::FIRE_RES))?;
        let nature: i32 = row.number(it::column(dump.kind(), it::NATURE_RES))?;
        let frost: i32 = row.number(it::column(dump.kind(), it::FROST_RES))?;
        let shadow: i32 = row.number(it::column(dump.kind(), it::SHADOW_RES))?;
        let arcane: i32 = row.number(it::column(dump.kind(), it::ARCANE_RES))?;
        if holy != 0 || fire != 0 || nature != 0 || frost != 0 || shadow != 0 || arcane != 0 {
            resist_nonzero_count += 1;
        }
        let sp3: u32 = row.number(it::column(dump.kind(), it::SPELLID_3))?;
        let spt3: u8 = row.number(it::column(dump.kind(), it::SPELLTRIGGER_3))?;
        let sp4: u32 = row.number(it::column(dump.kind(), it::SPELLID_4))?;
        let spt4: u8 = row.number(it::column(dump.kind(), it::SPELLTRIGGER_4))?;
        let sp5: u32 = row.number(it::column(dump.kind(), it::SPELLID_5))?;
        let spt5: u8 = row.number(it::column(dump.kind(), it::SPELLTRIGGER_5))?;
        let req_skill: u32 = row.number(it::column(dump.kind(), it::REQUIRED_SKILL))?;
        let req_skill_rank: u32 = row.number(it::column(dump.kind(), it::REQUIRED_SKILL_RANK))?;
        let req_rep_faction: u32 =
            row.number(it::column(dump.kind(), it::REQUIRED_REPUTATION_FACTION))?;
        let req_rep_rank: u32 =
            row.number(it::column(dump.kind(), it::REQUIRED_REPUTATION_RANK))?;
        let max_count: u32 = row.number(it::column(dump.kind(), it::MAX_COUNT))?;
        let item_flags: u32 = row.number(it::column(dump.kind(), it::FLAGS))?;
        let page_text: u32 = row.number(it::column(dump.kind(), it::PAGE_TEXT))?;
        let start_quest: u32 = row.number(it::column(dump.kind(), it::START_QUEST))?;
        if start_quest != 0 {
            startquest_nonzero_count += 1;
        }
        let bag_family: u32 = row.number(it::column(dump.kind(), it::BAG_FAMILY))?;
        let buy_count: u32 = row
            .number::<u32>(it::column(dump.kind(), it::BUY_COUNT))?
            .max(1);
        let food_type: u8 = row.number(it::column(dump.kind(), it::FOOD_TYPE))?;
        item_rows.push(format!(
            "({entry},{class},{subclass},{name},{disp},{qual},{inv},{ilvl},{rlvl},{dur},{buy},{sell},{stack},{dmin},{dmax},{delay},{s_str},{s_agi},{s_sta},{s_int},{s_spi},0,0,{armor},{block},{drink},{sp1},{spt1},{sp2},{spt2},{cslots},{sheath},{bonding},{holy},{fire},{nature},{frost},{shadow},{arcane},{sp3},{spt3},{sp4},{spt4},{sp5},{spt5},{req_skill},{req_skill_rank},{req_rep_faction},{req_rep_rank},{max_count},{item_flags},{page_text},{start_quest},{bag_family},{buy_count},{food_type},{allowed_class},{allowed_race},{random_property})",
            sheath = row.number::<u8>(it::column(dump.kind(), it::SHEATH))?,
            sp1 = row.number::<u32>(it::column(dump.kind(), it::SPELLID_1))?,
            spt1 = row.number::<u8>(it::column(dump.kind(), it::SPELLTRIGGER_1))?,
            sp2 = row.number::<u32>(it::column(dump.kind(), it::SPELLID_2))?,
            spt2 = row.number::<u8>(it::column(dump.kind(), it::SPELLTRIGGER_2))?,
            cslots = row.number::<u8>(it::column(dump.kind(), it::CONTAINER_SLOTS))?,
            bonding = row.number::<u8>(it::column(dump.kind(), it::BONDING))?,
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
            random_property = item_property::template_pool(row, dump.kind())?,
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

#[derive(Clone)]
pub(crate) struct WorldImportArgs {
    pub(crate) source: SourceKind,
    pub(crate) dbc: Option<String>,
    pub(crate) family: Option<String>,
    pub(crate) world_profile: Option<WorldImportProfile>,
    pub(crate) scope: WorldImportScope,
    pub(crate) include_creatures: Vec<u64>,
    pub(crate) eventai_profile: String,
    pub(crate) packages: Option<String>,
    pub(crate) source_sha: String,
    pub(crate) apply: bool,
    pub(crate) shard: Shard,
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
    pub(crate) eventai_manifest: Option<String>,
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

fn build_dump_plan(
    dump: &(impl crate::dump::TableRows + ?Sized),
    args: &WorldImportArgs,
    display_scales: &Option<std::collections::HashMap<u32, f32>>,
    profession_tier_values: &Option<std::collections::HashMap<u32, [u16; 4]>>,
) -> Result<DumpPlan> {
    let scope = &args.scope;
    eprintln!("world import scope: {}", scope.name());
    let population = creature_population::collect(dump, scope)?;
    let spawns = &population.spawns;
    let ScopeEntries {
        entries,
        eventai,
        eventai_manifest,
    } = resolve_scope_entries(dump, args, scope, &population, ScopePurpose::Import)?;
    let CreatureTemplates {
        templates,
        creature_loot_ids,
        creature_pickpocket_ids,
        creature_skin_ids,
        class_trainer_count,
    } = build_creature_templates(dump, &entries, display_scales)?;
    let spell_catalogue = args.dbc.as_deref().map(spell::read_catalogue).transpose()?;
    let (creature_casts, creature_rotation_rows) = build_creature_cast_rows(
        dump,
        &entries,
        spell_catalogue.as_ref().map(|catalogue| &catalogue.ids),
    )?;
    let SpawnPayload {
        guid_of,
        packed_spawns,
    } = build_packed_spawn_payload(spawns);
    let waypoints = build_waypoint_rows(dump, spawns, &guid_of)?;

    eprintln!(
        "mapped: {} templates ({class_trainer_count} class-gated trainers), {} spawns, {} waypoints",
        templates.len(),
        spawns.len(),
        waypoints.len()
    );

    let go_spawns = collect_gameobject_spawns(dump, scope)?;
    let GameobjectTemplateRows {
        templates: go_template_rows,
        traps: go_trap_rows,
        meeting_stones: go_meeting_stone_rows,
        chest_loot_ids: chest_loot_ids_used,
    } = gameobject_template_rows(&go_spawns, &eventai.forced_gameobject_entries)?;
    let trainer_rows = build_trainer_spell_rows(dump, &entries, profession_tier_values)?;

    let catalogue = if args.world_profile.is_some() {
        Some(build_quest_catalogue(dump, args, display_scales)?)
    } else {
        None
    };
    let (quest_entries, quest_gameobjects, quest_loot_ids) = catalogue
        .as_ref()
        .map(|catalogue| {
            (
                &catalogue.entries,
                &catalogue.gameobjects,
                &catalogue.creature_loot_ids,
            )
        })
        .unwrap_or((&entries, &go_spawns, &creature_loot_ids));
    let mut obtainable_items = creature_drop_item_set(dump, quest_loot_ids)?;
    obtainable_items.extend(gameobject_loot_item_set(dump, &quest_gameobjects.used_go)?);
    obtainable_items.extend(vendor_stock_item_set(dump, quest_entries)?);
    let mut local_creature_entries = population.referenced_entries.clone();
    local_creature_entries.extend(&eventai.forced_template_entries);
    let giver_entries = if args
        .world_profile
        .is_some_and(WorldImportProfile::imports_all_templates)
    {
        &local_creature_entries
    } else {
        &entries
    };
    let quests = build_quests(
        dump,
        quest_entries,
        &quest_gameobjects.used_go,
        &quest_gameobjects.goober_entries,
        &obtainable_items,
        Some((giver_entries, &go_spawns.used_go)),
    )?;
    eprintln!(
        "mapped: {} quests, {} text, {} objectives, {} cast objectives, {} reward items, {} choice rewards, \
         {} creature giver relations, {} gameobject giver relations, {} chained (next_quest_id>0) [V], \
         {} timed (limit_time>0) [V], {} raid quests (type=62)",
        quests.templates.len(), quests.texts.len(), quests.objectives.len(),
        quests.cast_objectives.len(), quests.reward_items.len(), quests.reward_choices.len(), quests.relations.len(),
        quests.go_relations.len(), quests.chained_count, quests.timed_count, quests.raid_quest_count,
    );
    let (mail_loot_rows, mail_loot_item_entries) =
        build_mail_loot(dump, &quests.reward_mail_template_ids)?;
    eprintln!(
        "mapped: {} reward-mail quests in scope, {} templates, {} loot rows",
        quests.reward_mail.len(),
        quests.reward_mail_template_ids.len(),
        mail_loot_rows.len(),
    );

    let extra_item_entries: std::collections::HashSet<u64> = quests
        .reward_item_entries
        .iter()
        .chain(quests.req_item_entries.iter())
        .copied()
        .collect();
    let (item_rows, loot_rows, vendor_rows) = build_items_and_loot(
        dump,
        &creature_loot_ids,
        &entries,
        &extra_item_entries,
        &mail_loot_item_entries,
    )?;
    eprintln!(
        "mapped: {} item_templates, {} creature_loot rows, {} npc_vendor rows ({} creatures with loot)",
        item_rows.len(), loot_rows.len(), vendor_rows.len(), creature_loot_ids.len()
    );
    let LootFamilyRows {
        pickpocket_rows,
        skinning_rows,
        gameobject_loot_rows,
        fishing_rows,
    } = build_loot_family_rows(
        dump,
        &creature_pickpocket_ids,
        &creature_skin_ids,
        &chest_loot_ids_used,
    )?;
    let gossip = build_gossip_sql(dump, &entries)?;

    let quest_event_requirements = catalogue
        .as_ref()
        .map(|catalogue| &catalogue.eventai)
        .unwrap_or(&eventai)
        .quest_event_requirement_rows();
    let content = MappedContent {
        templates: &templates,
        waypoints: &waypoints,
        item_rows: &item_rows,
        loot_rows: &loot_rows,
        vendor_rows: &vendor_rows,
        pickpocket_rows: &pickpocket_rows,
        skinning_rows: &skinning_rows,
        gameobject_loot_rows: &gameobject_loot_rows,
        fishing_rows: &fishing_rows,
        gossip: &gossip,
        quests: &quests,
        mail_loot_rows: &mail_loot_rows,
        go_template_rows: &go_template_rows,
        go_trap_rows: &go_trap_rows,
        go_meeting_stone_rows: &go_meeting_stone_rows,
        trainer_rows: &trainer_rows,
        creature_casts: &creature_casts,
        creature_rotation_rows: &creature_rotation_rows,
        eventai: &eventai,
        quest_event_requirements: &quest_event_requirements,
    };
    let mut stmts: Vec<String> = Vec::new();
    let property_row_count = if family_active(args, "items") {
        let (property_sql, row_count) = item_property::dump_sql(dump, args.dbc.as_deref())?;
        stmts.extend(property_sql);
        row_count
    } else {
        0
    };
    push_world_content_statements(args, content, &mut stmts);
    if family_active(args, "creatures") {
        population.append_statements(&mut stmts);
    }
    push_quest_and_gameobject_statements(args, content, &mut stmts);
    push_creature_behaviour_statements(args, content, &mut stmts);
    let (globals_row_count, spellmeta_row_count) = push_global_statements(args, dump, &mut stmts)?;

    // Only the first spawn batch clears live state; later batches must append.
    let spawn_rows: Vec<&str> = packed_spawns.split(';').filter(|r| !r.is_empty()).collect();
    let batches: Vec<String> = spawn_rows
        .chunks(SPAWN_BATCH)
        .map(|c| c.join(";"))
        .collect();
    let go_batches: Vec<String> = go_spawns
        .go_packed_rows
        .chunks(SPAWN_BATCH)
        .map(|c| c.join(";"))
        .collect();

    let stamps = family_stamps(
        args,
        content,
        property_row_count,
        globals_row_count,
        spellmeta_row_count,
    );

    let eventai_definition_count = eventai.definition_count();
    let eventai_instruction_count = eventai.instruction_count();
    let eventai_relay_definition_count = eventai.relay_definition_rows.len() as u64;
    let eventai_relay_definition_batches = eventai.relay_definition_batches;

    Ok(DumpPlan {
        stmts,
        spawn_row_count: spawn_rows.len(),
        go_row_count: go_spawns.go_packed_rows.len(),
        spawn_batches: batches,
        go_batches,
        eventai_definition_batches: eventai.definition_batches,
        eventai_relay_definition_batches,
        eventai_definition_count,
        eventai_relay_definition_count,
        eventai_instruction_count,
        eventai_manifest,
        world_source_manifest: None,
        stamps,
    })
}

pub(crate) type CreatureSpawnRow = (u64, u64, f64, f64, f64, f64, u8, u32, i64);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopePurpose {
    Import,
    QuestCatalogue,
}

struct ScopeEntries {
    entries: std::collections::HashSet<u64>,
    eventai: eventai::EventAiPlan,
    eventai_manifest: Option<String>,
}

struct CreatureTemplates {
    templates: Vec<String>,
    creature_loot_ids: std::collections::HashMap<u64, u64>,
    creature_pickpocket_ids: std::collections::HashMap<u64, u64>,
    creature_skin_ids: std::collections::HashMap<u64, u64>,
    class_trainer_count: usize,
}

struct SpawnPayload {
    guid_of: std::collections::HashMap<u64, u64>,
    packed_spawns: String,
}

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

struct LootFamilyRows {
    pickpocket_rows: Vec<String>,
    skinning_rows: Vec<String>,
    gameobject_loot_rows: Vec<String>,
    fishing_rows: Vec<String>,
}

struct QuestCatalogue {
    entries: std::collections::HashSet<u64>,
    gameobjects: GameobjectSpawns,
    creature_loot_ids: std::collections::HashMap<u64, u64>,
    eventai: eventai::EventAiPlan,
}

fn build_quest_catalogue(
    dump: &(impl crate::dump::TableRows + ?Sized),
    args: &WorldImportArgs,
    display_scales: &Option<std::collections::HashMap<u32, f32>>,
) -> Result<QuestCatalogue> {
    let mut scope = if args
        .world_profile
        .is_some_and(WorldImportProfile::imports_all_templates)
    {
        WorldImportScope::canonical(WorldImportProfile::FullWorld)?
    } else {
        WorldImportScope::quest_catalogue()?
    };
    scope.extend_forced_creatures(&args.include_creatures);
    let population = creature_population::collect(dump, &scope)?;
    let content = resolve_scope_entries(
        dump,
        args,
        &scope,
        &population,
        ScopePurpose::QuestCatalogue,
    )?;
    let templates = build_creature_templates(dump, &content.entries, display_scales)?;
    Ok(QuestCatalogue {
        entries: content.entries,
        gameobjects: collect_gameobject_spawns(dump, &scope)?,
        creature_loot_ids: templates.creature_loot_ids,
        eventai: content.eventai,
    })
}

#[derive(Clone, Copy)]
struct MappedContent<'a> {
    templates: &'a [String],
    waypoints: &'a [String],
    item_rows: &'a [String],
    loot_rows: &'a [String],
    vendor_rows: &'a [String],
    pickpocket_rows: &'a [String],
    skinning_rows: &'a [String],
    gameobject_loot_rows: &'a [String],
    fishing_rows: &'a [String],
    gossip: &'a GossipEtl,
    quests: &'a QuestEtl,
    mail_loot_rows: &'a [String],
    go_template_rows: &'a [String],
    go_trap_rows: &'a [String],
    go_meeting_stone_rows: &'a [String],
    trainer_rows: &'a [String],
    creature_casts: &'a [String],
    creature_rotation_rows: &'a [String],
    eventai: &'a eventai::EventAiPlan,
    quest_event_requirements: &'a [String],
}

fn resolve_scope_entries(
    dump: &(impl crate::dump::TableRows + ?Sized),
    args: &WorldImportArgs,
    scope: &WorldImportScope,
    population: &creature_population::PopulationPlan,
    purpose: ScopePurpose,
) -> Result<ScopeEntries> {
    let spawns = &population.spawns;
    let mut entries = population.referenced_entries.clone();
    let mut all_templates = std::collections::HashSet::new();
    if args
        .world_profile
        .is_some_and(WorldImportProfile::imports_all_templates)
    {
        for row in parse_table(dump, "creature_template")?.iter() {
            let entry = field(row, ct::ENTRY)?
                .parse::<u64>()
                .context("invalid creature entry")?;
            if entry == 0 {
                bail!("creature definition has reserved entry zero");
            }
            all_templates.insert(entry);
        }
    }
    if args.world_profile == Some(WorldImportProfile::FullWorld)
        || purpose == ScopePurpose::QuestCatalogue
    {
        entries.extend(&all_templates);
    }
    const SPELL_TEMPLATE_ENTRIES: &[u64] = &[416, 1860, 1863, 5676, 5677, 12861];
    entries.extend(SPELL_TEMPLATE_ENTRIES.iter().copied());
    let scope_name = scope.name();
    for &e in &scope.forced_creature_entries {
        if !entries.contains(&e) {
            eprintln!(
                "WARNING: forced creature {e} in scope {scope_name}: template will import but NO spawn exists in the \
                 dump — the NPC will NOT physically exist in-world (pure-summon/instanced giver?)"
            );
        }
        entries.insert(e);
    }
    let imported_guid_entries: std::collections::HashMap<u64, u64> = spawns
        .iter()
        .map(|(guid, entry, ..)| (*guid, *entry))
        .collect();
    let importable_templates: std::collections::HashSet<u64> =
        parse_table(dump, "creature_template")?
            .iter()
            .map(|row| -> Result<_> {
                let Ok(entry) = field(row, ct::ENTRY)?.parse::<u64>() else {
                    return Ok(None);
                };
                Ok(Some(entry))
            })
            .filter_map(Result::transpose)
            .collect::<Result<_>>()?;
    let (eventai, eventai_manifest) = if args.world_profile.is_some()
        || family_active(args, "creature-ai")
        || family_active(args, "gameobjects")
    {
        let source = eventai::parse(dump)?;
        let plan = loop {
            let plan = source.assemble(&entries, &imported_guid_entries, &importable_templates);
            let added = plan
                .forced_template_entries
                .iter()
                .copied()
                .filter(|entry| entries.insert(*entry))
                .count();
            if added == 0 {
                break plan;
            }
        };
        let manifest = if purpose == ScopePurpose::Import && family_active(args, "creature-ai") {
            let profile =
                eventai::source_profile(&args.eventai_profile).map_err(anyhow::Error::msg)?;
            let observed_sql_sha256 = dump.source_hash()?;
            let manifest = plan.compatibility_manifest(
                &profile,
                &observed_sql_sha256,
                eventai::LOADER_CONTRACT,
            );
            if args.apply {
                manifest.require_apply_ready().map_err(anyhow::Error::msg)?;
            }
            Some(manifest.render().to_string())
        } else {
            None
        };
        (plan, manifest)
    } else {
        (eventai::EventAiPlan::default(), None)
    };
    entries.extend(all_templates);
    for slice in &scope.bounded_slices {
        let count = spawns
            .iter()
            .filter(|spawn| slice.contains(spawn.8, spawn.2, spawn.3, spawn.4))
            .count();
        eprintln!(
            "filter: bounded slice {} (map {}) → {count} creature spawns",
            slice.name, slice.map_id
        );
    }
    for &map in &scope.whole_maps {
        let count = spawns.iter().filter(|spawn| spawn.8 == map).count();
        eprintln!("filter: whole map {map} → {count} creature spawns");
    }
    eprintln!(
        "filter: scope {} total → {} creature spawns, {} entries",
        scope.name(),
        spawns.len(),
        entries.len()
    );

    Ok(ScopeEntries {
        entries,
        eventai,
        eventai_manifest,
    })
}

fn build_creature_templates(
    dump: &(impl crate::dump::TableRows + ?Sized),
    entries: &std::collections::HashSet<u64>,
    display_scales: &Option<std::collections::HashMap<u32, f32>>,
) -> Result<CreatureTemplates> {
    let mut templates: Vec<String> = Vec::new();
    let mut creature_loot_ids: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    let mut creature_pickpocket_ids: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    let mut creature_skin_ids: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    let mut class_trainer_count: usize = 0;
    for row in parse_table(dump, "creature_template")?.iter() {
        let entry: u64 = field(row, ct::ENTRY)?.parse().unwrap_or(0);
        if !entries.contains(&entry) {
            continue;
        }
        let loot_id: u64 = field(row, ct::LOOT_ID)?.parse().unwrap_or(0);
        if loot_id > 0 {
            creature_loot_ids.insert(entry, loot_id);
        }
        let pickpocket_loot_id: u64 = field(row, ct::PICKPOCKET_LOOT_ID)?.parse().unwrap_or(0);
        if pickpocket_loot_id > 0 {
            creature_pickpocket_ids.insert(entry, pickpocket_loot_id);
        }
        let skin_loot_id: u64 = field(row, ct::SKIN_LOOT_ID)?.parse().unwrap_or(0);
        if skin_loot_id > 0 {
            creature_skin_ids.insert(entry, skin_loot_id);
        }
        // Scale zero means use CreatureDisplayInfo scale.
        let display_id: u32 = field(row, ct::MODEL_ID1)?.parse().unwrap_or(0);
        let cm_scale: f32 = field(row, ct::SCALE)?.parse().unwrap_or(0.0);
        let scale_val: f32 = if cm_scale > 0.0 {
            cm_scale
        } else {
            display_scales
                .as_ref()
                .and_then(|m| m.get(&display_id).copied())
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0)
        };
        let num_or_zero = |idx: &str| -> Result<&str> {
            let v = field(row, idx)?;
            Ok(if v.is_empty() { "0" } else { v })
        };
        let faction_template: u32 = field(row, ct::FACTION)?.parse().unwrap_or(0);
        let dmg_min: u32 = field(row, ct::MIN_MELEE_DMG)?
            .parse::<f32>()
            .unwrap_or(0.0)
            .round() as u32;
        let dmg_max: u32 = field(row, ct::MAX_MELEE_DMG)?
            .parse::<f32>()
            .unwrap_or(0.0)
            .round() as u32;
        let trainer_type_val: u8 = field(row, ct::TRAINER_TYPE)?.parse().unwrap_or(0);
        let trainer_class_val: u8 = field(row, ct::TRAINER_CLASS)?.parse().unwrap_or(0);
        if trainer_class_val != 0 {
            class_trainer_count += 1;
        }
        templates.push(format!(
            "({entry},{name},{subname},{display},{level},{health},{faction},{npc},{unit},{ctype},{family},{tflags},{rank},{scale},{bat},{mmin},{mmax},{maxlvl},{maxhp},0,{dmin},{dmax},{armor},{pploot},{skinloot},{ttype},{tclass})",
            ttype = trainer_type_val,
            tclass = trainer_class_val,
            armor = num_or_zero(ct::ARMOR)?,
            dmin = dmg_min,
            dmax = dmg_max,
            name = sql_text(field(row, ct::NAME)?),
            subname = sql_text(field(row, ct::SUBNAME)?),
            display = display_id,
            level = field(row, ct::MIN_LEVEL)?,
            health = field(row, ct::MIN_LEVEL_HEALTH)?,
            faction = faction_template,
            npc = field(row, ct::NPC_FLAGS)?,
            unit = field(row, ct::UNIT_FLAGS)?,
            ctype = field(row, ct::CREATURE_TYPE)?,
            family = field(row, ct::FAMILY)?,
            tflags = field(row, ct::CREATURE_TYPE_FLAGS)?,
            rank = field(row, ct::RANK)?,
            scale = scale_val,
            bat = field(row, ct::MELEE_BASE_ATTACK_TIME)?,
            mmin = field(row, ct::MIN_LOOT_GOLD)?,
            mmax = field(row, ct::MAX_LOOT_GOLD)?,
            maxlvl = num_or_zero(ct::MAX_LEVEL)?,
            maxhp = num_or_zero(ct::MAX_LEVEL_HEALTH)?,
            pploot = pickpocket_loot_id,
            skinloot = skin_loot_id,
        ));
    }

    Ok(CreatureTemplates {
        templates,
        creature_loot_ids,
        creature_pickpocket_ids,
        creature_skin_ids,
        class_trainer_count,
    })
}

fn build_creature_cast_rows(
    dump: &(impl crate::dump::TableRows + ?Sized),
    entries: &std::collections::HashSet<u64>,
    spell_ids: Option<&std::collections::HashSet<u64>>,
) -> Result<(Vec<String>, Vec<String>)> {
    const COND_ALWAYS: u8 = 0;
    const COND_SELF_MISSING_AURA: u8 = 3;

    let mut creature_casts: Vec<String> = Vec::new();
    let mut rotation_id: u64 = 1;
    let mut creature_rotation_rows: Vec<String> = Vec::new();
    for row in parse_table(dump, "creature_template_spells")?.iter() {
        let entry: u64 = field(row, cts::ENTRY)?.parse().unwrap_or(0);
        if field(row, cts::SET_ID)?.parse::<u64>().unwrap_or(0) != 0 {
            continue;
        }
        if !entries.contains(&entry) {
            continue;
        }
        let spell1: u64 = field(row, cts::SPELL1)?.parse().unwrap_or(0);
        if spell1 == 0 {
            continue;
        }
        if spell_ids.is_some_and(|ids| !ids.contains(&spell1)) {
            eprintln!("casts: WARNING creature {entry} has unknown primary spell {spell1}; derived caster rows omitted");
            continue;
        }
        let secondary: Vec<u64> = (1..=9)
            .map(|i| -> Result<_> {
                let s: u64 = field(row, &format!("spell{}", i + 1))?.parse().unwrap_or(0);
                Ok(if s != 0 && spell_ids.is_none_or(|ids| ids.contains(&s)) {
                    Some(s)
                } else {
                    None
                })
            })
            .filter_map(Result::transpose)
            .collect::<Result<_>>()?;

        if secondary.is_empty() {
            creature_casts.push(format!("({entry},{spell1})"));
        } else {
            creature_casts.push(format!("({entry},{spell1})"));
            creature_rotation_rows.push(format!(
                "({rotation_id},{entry},{spell1},10,{COND_ALWAYS},0)"
            ));
            rotation_id += 1;
            for s in &secondary {
                creature_rotation_rows.push(format!(
                    "({rotation_id},{entry},{s},20,{COND_SELF_MISSING_AURA},0)"
                ));
                rotation_id += 1;
            }
        }
    }
    eprintln!(
        "mapped: {} caster-mob cast rows (game_creature_cast); \
         {} rotation rows for {} multi-spell entries (game_creature_spell)",
        creature_casts.len(),
        creature_rotation_rows.len(),
        creature_rotation_rows
            .iter()
            .filter(|r| r.contains(&format!(",10,{COND_ALWAYS},0)")))
            .count(),
    );

    Ok((creature_casts, creature_rotation_rows))
}

fn build_packed_spawn_payload(spawns: &[CreatureSpawnRow]) -> SpawnPayload {
    // Timestamp columns require reducer imports because SpacetimeDB SQL cannot write them.
    let guid_of: std::collections::HashMap<u64, u64> =
        spawns.iter().map(|s| (s.0, world_guid(s.1, s.0))).collect();
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

    SpawnPayload {
        guid_of,
        packed_spawns,
    }
}

fn build_waypoint_rows(
    dump: &(impl crate::dump::TableRows + ?Sized),
    spawns: &[CreatureSpawnRow],
    guid_of: &std::collections::HashMap<u64, u64>,
) -> Result<Vec<String>> {
    let mut waypoints: Vec<String> = Vec::new();
    let mut wp_id: u64 = 1;
    let mut direct_wp_guids: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for row in parse_table(dump, "creature_movement")?.iter() {
        let db_guid: u64 = field(row, cm::ID)?.parse().unwrap_or(0);
        let Some(&g) = guid_of.get(&db_guid) else {
            continue;
        };
        waypoints.push(format!(
            "({id},{g},{x},{y},{z})",
            id = wp_id,
            x = field(row, cm::X)?,
            y = field(row, cm::Y)?,
            z = field(row, cm::Z)?,
        ));
        direct_wp_guids.insert(db_guid);
        wp_id += 1;
    }
    {
        let mut tmpl_paths: std::collections::HashMap<u64, Vec<(u32, String, String, String)>> =
            std::collections::HashMap::new();
        for row in parse_table(dump, "creature_movement_template")?.iter() {
            let entry: u64 = field(row, cmt::ENTRY)?.parse().unwrap_or(0);
            if entry == 0 {
                continue;
            }
            if field(row, cmt::PATH_ID)?.parse::<u64>().unwrap_or(0) != 0 {
                continue;
            }
            let (pt_i, x_i, y_i, z_i) = ("Point", "PositionX", "PositionY", "PositionZ");
            let point: u32 = field(row, pt_i)?.parse().unwrap_or(0);
            tmpl_paths.entry(entry).or_default().push((
                point,
                field(row, x_i)?.to_string(),
                field(row, y_i)?.to_string(),
                field(row, z_i)?.to_string(),
            ));
        }
        for path in tmpl_paths.values_mut() {
            path.sort_by_key(|p| p.0);
        }
        let mut tmpl_spawns = 0usize;
        if !tmpl_paths.is_empty() {
            for (db_guid, entry, _x, _y, _z, _o, mt, _rs, _map) in spawns {
                if *mt != 2 || direct_wp_guids.contains(db_guid) {
                    continue;
                }
                let Some(path) = tmpl_paths.get(entry) else {
                    continue;
                };
                let Some(&g) = guid_of.get(db_guid) else {
                    continue;
                };
                for (_pt, x, y, z) in path {
                    waypoints.push(format!("({wp_id},{g},{x},{y},{z})"));
                    wp_id += 1;
                }
                tmpl_spawns += 1;
            }
            eprintln!(
                "creature_movement_template: {} entry paths parsed, expanded onto {} waypoint spawns",
                tmpl_paths.len(), tmpl_spawns
            );
        }
    }

    Ok(waypoints)
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
        let initial_state = if dump.kind() == SourceKind::Vmangos {
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
        } else {
            go_initial_state(meta.stored_type, meta.data0)
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

struct GameobjectTemplateRows {
    templates: Vec<String>,
    traps: Vec<String>,
    meeting_stones: Vec<String>,
    chest_loot_ids: Vec<u32>,
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
    let mut chest_loot_ids_used: Vec<u32> = Vec::new();
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
        let (row, loot_id_used) = go_template_row(
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
        if let Some(loot_id) = loot_id_used {
            chest_loot_ids_used.push(loot_id);
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
        chest_loot_ids: chest_loot_ids_used,
    })
}

fn build_trainer_spell_rows(
    dump: &(impl crate::dump::TableRows + ?Sized),
    entries: &std::collections::HashSet<u64>,
    profession_tier_values: &Option<std::collections::HashMap<u32, [u16; 4]>>,
) -> Result<Vec<String>> {
    let mut trainer_rows: Vec<String> = Vec::new();
    let mut trainer_id: u64 = 1;
    let mut trainer_lines: std::collections::BTreeMap<u64, std::collections::BTreeSet<u32>> =
        std::collections::BTreeMap::new();
    let mut trainer_spells: std::collections::HashMap<u64, std::collections::HashSet<u64>> =
        std::collections::HashMap::new();
    for row in parse_table(dump, "npc_trainer")?.iter() {
        let entry: u64 = field(row, nt::ENTRY)?.parse().unwrap_or(0);
        if !entries.contains(&entry) {
            continue;
        }
        let spell: u64 = field(row, nt::SPELL)?.parse().unwrap_or(0);
        if spell == 0 {
            continue;
        }
        let cost: u64 = field(row, nt::COST)?.parse().unwrap_or(0);
        let reqlevel: u32 = field(row, nt::REQ_LEVEL)?.parse().unwrap_or(0);
        let reqskill: u32 = field(row, nt::REQ_SKILL)?.parse().unwrap_or(0);
        if profession_tiers(reqskill).is_some() {
            trainer_lines.entry(entry).or_default().insert(reqskill);
        }
        trainer_spells.entry(entry).or_default().insert(spell);
        trainer_rows.push(format!(
            "({trainer_id},{entry},{spell},{cost},{reqlevel},0,75)"
        ));
        trainer_id += 1;
    }
    let mut synthesized = 0usize;
    for (entry, lines) in &trainer_lines {
        let offered = trainer_spells.get(entry);
        for line in lines {
            for &(spell, tier_index, req) in profession_tiers(*line).unwrap() {
                if offered.is_some_and(|s| s.contains(&(spell as u64))) {
                    continue;
                }
                let cap = profession_tier_values
                    .as_ref()
                    .and_then(|tv| tv.get(line))
                    .map(|caps| caps[tier_index as usize] as u32)
                    .unwrap_or(LEGACY_TIER_CAPS[tier_index as usize]);
                trainer_rows.push(format!(
                    "({trainer_id},{entry},{spell},0,{req},{line},{cap})"
                ));
                trainer_id += 1;
                synthesized += 1;
            }
        }
    }
    eprintln!(
        "mapped: {} trainer spells ({synthesized} synthesized profession learn-rows across {} trainers)",
        trainer_rows.len(), trainer_lines.len(),
    );

    Ok(trainer_rows)
}

fn build_loot_family_rows(
    dump: &(impl crate::dump::TableRows + ?Sized),
    creature_pickpocket_ids: &std::collections::HashMap<u64, u64>,
    creature_skin_ids: &std::collections::HashMap<u64, u64>,
    chest_loot_ids_used: &[u32],
) -> Result<LootFamilyRows> {
    let refs = parse_ref_pools(dump)?;

    let drops_by_pickpocketid = parse_loot_family(dump, "pickpocketing_loot_template", &refs)?;
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

    let drops_by_skinid = parse_loot_family(dump, "skinning_loot_template", &refs)?;
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

    let drops_by_goloot = parse_loot_family(dump, "gameobject_loot_template", &refs)?;
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

    let drops_by_zone = parse_loot_family(dump, "fishing_loot_template", &refs)?;
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

fn push_world_content_statements(
    args: &WorldImportArgs,
    content: MappedContent,
    stmts: &mut Vec<String>,
) {
    let MappedContent {
        templates,
        waypoints,
        item_rows,
        loot_rows,
        vendor_rows,
        pickpocket_rows,
        skinning_rows,
        gameobject_loot_rows,
        fishing_rows,
        gossip,
        ..
    } = content;
    if family_active(args, "creatures") {
        stmts.push("DELETE FROM game_creature_waypoint WHERE id > 0".into());
        stmts.push("DELETE FROM game_creature_template WHERE entry > 0".into());
        stmts.push("DELETE FROM game_creature_level_stats WHERE id > 0".into());
        push_insert(stmts, "game_creature_template", "entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class", templates);
        push_insert(
            stmts,
            "game_creature_waypoint",
            "id,creature_guid,x,y,z",
            waypoints,
        );
    }

    if family_active(args, "items") {
        push_item_statements(stmts, item_rows, loot_rows, vendor_rows);
    }

    if family_active(args, "loot") {
        push_loot_statements(
            stmts,
            pickpocket_rows,
            skinning_rows,
            gameobject_loot_rows,
            fishing_rows,
        );
    }

    if family_active(args, "gossip") {
        gossip_statements(stmts, gossip);
    }
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

fn push_quest_and_gameobject_statements(
    args: &WorldImportArgs,
    content: MappedContent,
    stmts: &mut Vec<String>,
) {
    let MappedContent {
        quests,
        mail_loot_rows,
        go_template_rows,
        go_trap_rows,
        go_meeting_stone_rows,
        trainer_rows,
        ..
    } = content;
    if family_active(args, "quests") {
        push_quest_statements(stmts, quests, mail_loot_rows);
    }

    if family_active(args, "gameobjects") {
        push_gameobject_statements(stmts, go_template_rows, go_trap_rows, go_meeting_stone_rows);
    }
    if family_active(args, "trainers") {
        trainers::statements(stmts, trainer_rows);
    }
}

fn push_creature_behaviour_statements(
    args: &WorldImportArgs,
    content: MappedContent,
    stmts: &mut Vec<String>,
) {
    let MappedContent {
        creature_casts,
        creature_rotation_rows,
        eventai,
        quest_event_requirements,
        ..
    } = content;
    if family_active(args, "casts") {
        stmts.push("DELETE FROM game_creature_cast WHERE creature_entry > 0".into());
        push_insert(
            stmts,
            "game_creature_cast",
            "creature_entry,spell_id",
            creature_casts,
        );
        stmts.push("DELETE FROM game_creature_spell WHERE id > 0".into());
        push_insert(
            stmts,
            "game_creature_spell",
            "id,creature_entry,spell_id,priority,condition,condition_value",
            creature_rotation_rows,
        );
    }

    let quest_events_active = family_active(args, "creature-ai")
        || (args.world_profile.is_some() && family_active(args, "quests"));
    if family_active(args, "creature-ai") {
        stmts.push("DELETE FROM game_world_state_name WHERE variable_id >= 0".into());
        push_insert(
            stmts,
            "game_world_state_name",
            "variable_id",
            &eventai.world_state_rows,
        );
        stmts.push("DELETE FROM game_creature_ai_broadcast_text WHERE id > 0".into());
        stmts.push("DELETE FROM game_creature_ai_summon WHERE id > 0".into());
    }
    if quest_events_active {
        stmts.push("DELETE FROM game_quest_event_requirement WHERE id > 0".into());
    }
    if family_active(args, "creature-ai") {
        push_insert(
            stmts,
            "game_creature_ai_broadcast_text",
            "id,male_text,female_text,chat_type,language_id,emote_delay_1_ms,emote_id_1,emote_delay_2_ms,emote_id_2,emote_delay_3_ms,emote_id_3",
            &eventai.broadcast_rows,
        );
        push_insert(
            stmts,
            "game_creature_ai_summon",
            "id,x,y,z,orientation,lifetime_ms",
            &eventai.summon_rows,
        );
    }
    if quest_events_active {
        push_insert(
            stmts,
            "game_quest_event_requirement",
            "id,quest_entry",
            quest_event_requirements,
        );
    }
}

fn push_global_statements(
    args: &WorldImportArgs,
    dump: &(impl crate::dump::TableRows + ?Sized),
    stmts: &mut Vec<String>,
) -> Result<(u64, u64)> {
    let mut globals_row_count = 0u64;
    if family_active(args, "globals") {
        let globals = globals_statements(dump, args.dbc.as_deref())?;
        globals_row_count = insert_row_count(&globals);
        stmts.extend(globals);
    }

    let mut spellmeta_row_count = 0u64;
    if family_active(args, "spellmeta") {
        let links = args
            .dbc
            .as_deref()
            .map(dbc::read_spell_rank_links)
            .transpose()?
            .unwrap_or_default();
        let spell_chain_sql = build_spell_chain_sql(dump, &links)?;
        let spell_learn_sql = build_spell_learn_sql(dump)?;
        let catalogue = args.dbc.as_deref().map(spell::read_catalogue).transpose()?;
        let spell_proc_event_sql = build_spell_proc_event_sql(
            dump,
            catalogue.as_ref().map(|catalogue| &catalogue.proc_effects),
        )?;
        spellmeta_row_count = insert_row_count(&spell_chain_sql)
            + insert_row_count(&spell_learn_sql)
            + insert_row_count(&spell_proc_event_sql);
        stmts.extend(spell_chain_sql);
        stmts.extend(spell_learn_sql);
        stmts.extend(spell_proc_event_sql);
    }

    Ok((globals_row_count, spellmeta_row_count))
}

fn family_stamps(
    args: &WorldImportArgs,
    content: MappedContent,
    property_row_count: u64,
    globals_row_count: u64,
    spellmeta_row_count: u64,
) -> Vec<(&'static str, u64)> {
    let MappedContent {
        templates,
        item_rows,
        pickpocket_rows,
        skinning_rows,
        gameobject_loot_rows,
        fishing_rows,
        gossip,
        quests,
        go_template_rows,
        go_trap_rows,
        go_meeting_stone_rows,
        trainer_rows,
        creature_casts,
        creature_rotation_rows,
        eventai,
        quest_event_requirements,
        ..
    } = content;
    let mut stamps: Vec<(&'static str, u64)> = Vec::new();
    if family_active(args, "creatures") {
        stamps.push(("creatures", templates.len() as u64));
    }
    if family_active(args, "items") {
        stamps.push(("items", item_rows.len() as u64 + property_row_count));
    }
    if family_active(args, "loot") {
        stamps.push((
            "loot",
            (pickpocket_rows.len()
                + skinning_rows.len()
                + gameobject_loot_rows.len()
                + fishing_rows.len()) as u64,
        ));
    }
    if family_active(args, "gossip") {
        stamps.push((
            "gossip",
            (gossip.menu_rows.len()
                + gossip.npc_text_rows.len()
                + gossip.npc_text_slot_rows.len()
                + gossip.option_rows.len()
                + gossip.profile_rows.len()
                + gossip.profile_option_rows.len()
                + gossip.condition_rows.len()) as u64,
        ));
    }
    if family_active(args, "quests") {
        stamps.push(("quests", quests.templates.len() as u64));
    }
    if family_active(args, "gameobjects") {
        stamps.push((
            "gameobjects",
            (go_template_rows.len() + go_trap_rows.len() + go_meeting_stone_rows.len()) as u64,
        ));
    }
    if family_active(args, "trainers") {
        stamps.push(("trainers", trainer_rows.len() as u64));
    }
    if family_active(args, "casts") {
        stamps.push((
            "casts",
            (creature_casts.len() + creature_rotation_rows.len()) as u64,
        ));
    }
    if family_active(args, "creature-ai") {
        let row_count = eventai.row_count() - eventai.quest_event_entries.len() as u64
            + quest_event_requirements.len() as u64;
        stamps.push(("creature-ai", row_count));
    }
    if family_active(args, "globals") {
        stamps.push(("globals", globals_row_count));
    }
    if family_active(args, "spellmeta") {
        stamps.push(("spellmeta", spellmeta_row_count));
    }

    stamps
}

// The Module refuses EventAI relay definitions that name a spell missing from game_spell.
fn run_client_catalogues(
    dump: &(impl crate::dump::TableRows + ?Sized),
    args: &WorldImportArgs,
    spells: &spell::SpellArgs,
) -> Result<()> {
    if args.family.is_some() {
        return Ok(());
    }
    if args
        .world_profile
        .is_some_and(WorldImportProfile::imports_all_templates)
    {
        let data_dir = args
            .dbc
            .as_deref()
            .context("a complete world import requires --dbc <client Data directory>")?;
        dbc::run_from_dump(data_dir, dump, args.apply, &args.shard)?;
    }
    if let Some(data_dir) = args.dbc.as_deref() {
        spell::run_spells(data_dir, spells)?;
    }
    Ok(())
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
    let mut stmts = vec![
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
    Ok(DumpPlan {
        stmts,
        stamps: vec![("creature-ai", eventai.row_count())],
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

pub(crate) fn census_plan(source: &WorldSource, args: &WorldImportArgs) -> Result<DumpPlan> {
    if args.family.as_deref() == Some("dbc") {
        return dbc::replacement_plan(source, args.dbc.as_deref().context("dbc requires --dbc")?);
    }
    if args.family.as_deref() == Some("spell") {
        return spell::world_plan(source, args.dbc.as_deref().context("spell requires --dbc")?);
    }
    if source.kind() == SourceKind::Vmangos {
        return build_vmangos_plan(source, args);
    }
    let scales = args
        .dbc
        .as_deref()
        .map(dbc::load_display_scales)
        .transpose()?;
    let tiers = args
        .dbc
        .as_deref()
        .map(dbc::profession_tier_values)
        .transpose()?;
    let mut plan = build_dump_plan(source, args, &scales, &tiers)?;
    if args.family.as_deref() == Some("gameobjects") {
        plan.stmts
            .extend(transport_statements(source, args.dbc.as_deref())?);
    }
    Ok(plan)
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

pub(crate) fn run(
    dump_path: &str,
    args: &WorldImportArgs,
    spells: &spell::SpellArgs,
) -> Result<()> {
    let dump = WorldSource::open(std::path::Path::new(dump_path), args.source)?;
    if args.family.as_deref() == Some("dbc") {
        let plan =
            dbc::replacement_plan(&dump, args.dbc.as_deref().context("dbc requires --dbc")?)?;
        return emit_plan(&dump, args, &plan);
    }
    if args.family.as_deref() == Some("spell") {
        let plan = spell::world_plan(&dump, args.dbc.as_deref().context("spell requires --dbc")?)?;
        return emit_plan(&dump, args, &plan);
    }
    if args.source == SourceKind::Vmangos {
        let plan = build_vmangos_plan(&dump, args)?;
        return emit_plan(&dump, args, &plan);
    }

    let display_scales: Option<std::collections::HashMap<u32, f32>> = match &args.dbc {
        Some(dir) => {
            let m = dbc::load_display_scales(dir)?;
            eprintln!(
                "dbc: loaded {} display scales for Scale==0 resolution",
                m.len()
            );
            Some(m)
        }
        None => None,
    };

    let profession_tier_values: Option<std::collections::HashMap<u32, [u16; 4]>> = match &args.dbc {
        Some(dir) => {
            let m = dbc::profession_tier_values(dir)?;
            eprintln!(
                "dbc: loaded profession tier caps for {} skill lines",
                m.len()
            );
            assert_profession_tier_parity(&m)?;
            Some(m)
        }
        None => None,
    };

    let plan = build_dump_plan(&dump, args, &display_scales, &profession_tier_values)?;

    run_client_catalogues(&dump, args, spells)?;

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
        if let Some(manifest) = &plan.eventai_manifest {
            println!("-- EventAI Compatibility Manifest\n{manifest}\n");
        }
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

#[cfg(test)]
mod tests {
    mod qt {
        pub(super) use crate::dump::qt::*;
        pub const REQ_ITEM_ID1: &str = "ReqItemId1";
        pub const REQ_ITEM_COUNT1: &str = "ReqItemCount1";
        pub const REQ_CREATURE_OR_GO_ID1: &str = "ReqCreatureOrGOId1";
        pub const REQ_CREATURE_OR_GO_COUNT1: &str = "ReqCreatureOrGOCount1";
        pub const REQ_SPELL_CAST1: &str = "ReqSpellCast1";
        pub const REW_CHOICE_ITEM_ID1: &str = "RewChoiceItemId1";
        pub const REW_CHOICE_ITEM_COUNT1: &str = "RewChoiceItemCount1";
        pub const REW_ITEM_ID1: &str = "RewItemId1";
        pub const REW_ITEM_COUNT1: &str = "RewItemCount1";
    }
    use super::*;
    use crate::dump::{cr, read_dump};
    use crate::standalone_support;

    fn import_args<const N: usize>(argv: [&str; N]) -> Result<WorldImportArgs> {
        let mut argv = argv.to_vec();
        if !argv.contains(&"--source") {
            argv.extend(["--source", "classic-db"]);
        }
        crate::parse_args_from(argv)?.world_import_args()
    }

    fn test_args() -> WorldImportArgs {
        import_args(["--dump", "synthetic.sql"]).unwrap()
    }

    #[test]
    fn world_guid_is_highguid_unit_encoded_and_unique_per_spawn() {
        let g = world_guid(299, 42);
        assert_eq!(g >> 48, 0xF130);
        assert_eq!((g >> 24) & 0xFF_FFFF, 299);
        assert_eq!(g & 0xFF_FFFF, 42);
        assert_ne!(world_guid(299, 42), world_guid(299, 43));
    }

    #[test]
    fn gather_nodes_map_is_well_formed() {
        assert_eq!(gather_node(1731), Some((186, 2770, 1, 100)));
        assert_eq!(gather_node(1618), Some((182, 2447, 1, 75)));
        assert_eq!(gather_node(1732), Some((186, 2771, 65, 140)));
        assert_eq!(gather_node(106318), None);
        assert_eq!(gather_node(153453), None);
        assert_eq!(gather_node(0), None);
        let mut entries: Vec<u64> = GATHER_NODES.iter().map(|n| n.0).collect();
        entries.sort_unstable();
        let dups = entries.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(dups, 0, "GATHER_NODES has a duplicate GO entry");
        for &(entry, skill, item, _req, _gray) in GATHER_NODES {
            assert!(
                skill == 186 || skill == 182,
                "entry {entry} has skill {skill} (not Mining/Herb)"
            );
            assert!(item > 0, "entry {entry} grants item 0");
        }
    }

    #[test]
    fn classify_go_type_remaps_gather_and_passes_everything_else_through_identity() {
        assert_eq!(classify_go_type(1731, 3), Some(GO_GATHER));
        assert_eq!(classify_go_type(999_999, 3), Some(GO_CHEST));
        assert_eq!(classify_go_type(106318, 3), Some(GO_CHEST));
        assert_eq!(classify_go_type(1, 0), Some(GO_DOOR));
        assert_eq!(classify_go_type(2, 1), Some(GO_BUTTON));
        assert_eq!(classify_go_type(3, 2), Some(GO_QUESTGIVER));
        assert_eq!(classify_go_type(4, 10), Some(GO_GOOBER));
        assert_eq!(classify_go_type(5, 8), Some(8));
        assert_eq!(classify_go_type(6, 19), Some(19));
    }

    #[test]
    fn classify_go_type_25_collision_guard_drops_a_real_fishinghole_but_not_a_gather_node() {
        assert_eq!(
            classify_go_type(70000, 25),
            None,
            "a real FISHINGHOLE (type 25) must be dropped"
        );
        assert_eq!(classify_go_type(1731, 3), Some(GO_GATHER));
        assert_eq!(classify_go_type(70001, 3), Some(GO_CHEST));
    }

    #[test]
    fn go_template_row_carries_lock_id_per_type_and_widens_chest_loot() {
        let (row, loot_used) =
            go_template_row(106318, GO_CHEST, 259, "Battered Chest", 4242, 2277, 1.0);
        assert_eq!(row, "(106318,3,259,'Battered Chest',0,2277,0,0,0,4242,1)");
        assert_eq!(loot_used, Some(2277));
        let (_, none_used) = go_template_row(1, GO_CHEST, 1, "No Loot Chest", 0, 0, 1.0);
        assert_eq!(none_used, None);
        let (door_row, door_loot) =
            go_template_row(2000, GO_DOOR, 100, "Stormwind Gate", 1, 555, 1.0);
        assert_eq!(door_row, "(2000,0,100,'Stormwind Gate',0,0,0,0,0,555,1)");
        assert_eq!(door_loot, None);
        let (button_row, _) = go_template_row(2001, GO_BUTTON, 101, "Lever", 0, 777, 1.0);
        assert_eq!(button_row, "(2001,1,101,'Lever',0,0,0,0,0,777,1)");
        let (goober_row, _) =
            go_template_row(3000, GO_GOOBER, 200, "Suspicious Lever", 888, 999, 1.0);
        assert_eq!(
            goober_row,
            "(3000,10,200,'Suspicious Lever',0,0,0,0,0,888,1)"
        );
        let (gather_row, gather_loot) =
            go_template_row(1731, GO_GATHER, 259, "Copper Vein", 0, 0, 0.5);
        assert_eq!(
            gather_row,
            "(1731,25,259,'Copper Vein',2770,1,186,300,100,0,0.5)"
        );
        assert_eq!(gather_loot, None);
        let (qg_row, _) = go_template_row(4000, GO_QUESTGIVER, 50, "Wanted Poster", 1, 2, 0.0);
        assert_eq!(qg_row, "(4000,2,50,'Wanted Poster',0,0,0,0,0,0,0)");
        let (inert_row, _) = go_template_row(5000, 8, 60, "Spell Focus", 3, 4, 1.75);
        assert_eq!(inert_row, "(5000,8,60,'Spell Focus',3,4,0,0,0,0,1.75)");
    }

    #[test]
    fn go_initial_state_opens_only_a_started_open_door_or_button() {
        assert_eq!(go_initial_state(GO_DOOR, 1), 1);
        assert_eq!(go_initial_state(GO_BUTTON, 1), 1);
        assert_eq!(go_initial_state(GO_DOOR, 0), 0);
        assert_eq!(go_initial_state(GO_BUTTON, 0), 0);
        assert_eq!(go_initial_state(GO_CHEST, 1), 0);
        assert_eq!(go_initial_state(GO_GOOBER, 1), 0);
        assert_eq!(go_initial_state(GO_GATHER, 1), 0);
        assert_eq!(go_initial_state(GO_QUESTGIVER, 1), 0);
    }

    #[test]
    fn gameobject_catalog_includes_unspawned_templates() {
        let dump = "INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`, `data4`, `data5`) VALUES \
            (179584,23,6973,'Meeting Stone',0,0,0,1,15,20,1581,0,0,0),\
            (179585,23,6973,'Unspawned Stone',0,0,0,1,30,40,717,0,0,0); \
            INSERT INTO `gameobject` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`, `rotation0`, `rotation1`, `rotation2`, `rotation3`) VALUES \
            (60,179584,0,1,-8949.95,-132.493,83.5312,0,0,0,0,0);";
        let mut args = test_args();
        args.family = Some("gameobjects".to_string());
        let plan = build_dump_plan(dump, &args, &None, &None).unwrap();

        let stones: Vec<_> = plan
            .stmts
            .iter()
            .filter(|statement| statement.contains("game_meeting_stone"))
            .collect();
        assert_eq!(
            stones,
            [
                "DELETE FROM game_meeting_stone WHERE entry > 0",
                "INSERT INTO game_meeting_stone (entry,min_level,max_level,area_id) VALUES \
                 (179584,15,20,1581),(179585,30,40,717)",
            ]
        );
        assert!(plan.stmts.iter().any(|statement| {
            statement.starts_with("INSERT INTO game_gameobject_template")
                && statement.contains("(179584,23,6973,'Meeting Stone',0,0,0,0,0,0,1)")
        }));
        assert_eq!(plan.stamps, vec![("gameobjects", 4)]);
    }

    #[test]
    fn relay_imports_its_named_world_state() {
        let dump = format!(
            "INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `worldstate_name` VALUES (19990,'Relay state'); \
             INSERT INTO `creature_ai_scripts` VALUES {}; \
             INSERT INTO `dbscripts_on_relay` VALUES (1,0,0,53,0,0,0,0,0,0,19990,1,0,0,0,0,0,0,0,0,0,'set state');",
            creature_template_row(100, 0),
            eventai_rule_row(10,100,4,0,100,0,[0;6],[[53,1,0,0],[0;4],[0;4]])
        );
        let plan = build_dump_plan(&dump, &test_args(), &None, &None).unwrap();
        assert_eq!(plan.eventai_relay_definition_count, 1);
        assert!(plan
            .stmts
            .iter()
            .any(|sql| sql == "INSERT INTO game_world_state_name (variable_id) VALUES (19990)"));
    }

    #[test]
    fn relay_imports_an_unplaced_gameobject_definition() {
        let dump = format!(
            "INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`, `data4`, `data5`) VALUES (777,6,640,'Relay trap',0,0,0,1,0,0,0,24425,0,0); \
             INSERT INTO `spell_template` (`Id`) VALUES (24425); \
             INSERT INTO `creature_ai_scripts` VALUES {}; \
             INSERT INTO `dbscripts_on_relay` VALUES (1,0,0,13,0,0,0,777,20,1025,0,0,0,0,0,0,0,0,0,0,0,'activate');",
            creature_template_row(100, 0),
            eventai_rule_row(10,100,4,0,100,0,[0;6],[[53,1,0,0],[0;4],[0;4]])
        );
        let mut args = test_args();
        args.family = Some("gameobjects".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).unwrap();
        assert_eq!(plan.eventai_relay_definition_count, 1);
        assert!(plan.go_batches.is_empty());
        assert!(plan.stmts.iter().any(|sql| sql
            .starts_with("INSERT INTO game_gameobject_template")
            && sql.contains("(777,6,640,'Relay trap'")));
        assert!(plan
            .stmts
            .iter()
            .any(|sql| sql.starts_with("INSERT INTO game_gameobject_trap")
                && sql.contains("(777,24425,0)")));
    }

    #[test]
    fn cr_columns_map_spawn_time_and_movement_type() {
        let dump = "x INSERT INTO `creature` VALUES (1,299,0,1,10.0,20.0,30.0,1.5,180,300,0,2); y";
        let rows = parse_table(dump, "creature").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(field(&rows[0], cr::GUID).unwrap(), "1");
        assert_eq!(field(&rows[0], cr::ID).unwrap(), "299");
        assert_eq!(field(&rows[0], cr::MAP).unwrap(), "0");
        assert_eq!(field(&rows[0], cr::X).unwrap(), "10.0");
        assert_eq!(field(&rows[0], cr::O).unwrap(), "1.5");
        assert_eq!(field(&rows[0], cr::SPAWN_TIME_SECS_MIN).unwrap(), "180");
        assert_eq!(field(&rows[0], cr::MOVEMENT_TYPE).unwrap(), "2");
        let dump_missing = "x INSERT INTO `creature` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`) VALUES (1,299,0,1,10.0,20.0,30.0,1.5); y";
        let rows_missing = parse_table(dump_missing, "creature").unwrap();
        let respawn_secs: u32 = field(&rows_missing[0], cr::SPAWN_TIME_SECS_MIN)
            .unwrap()
            .parse()
            .unwrap_or(0);
        assert_eq!(respawn_secs, 0);
    }

    fn vmangos_globals_fixture() -> WorldSource {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value::{Integer as N, Real as F, Text};
        sqlite_fixture(&[
            FixtureTable {
                name: "player_classlevelstats",
                columns: &[
                    ("class", "INTEGER"),
                    ("level", "INTEGER"),
                    ("basehp", "INTEGER"),
                    ("basemana", "INTEGER"),
                ],
                key: &["class", "level"],
                rows: &[&[
                    ("class", N(1)),
                    ("level", N(1)),
                    ("basehp", N(20)),
                    ("basemana", N(0)),
                ]],
            },
            FixtureTable {
                name: "player_levelstats",
                columns: &[
                    ("race", "INTEGER"),
                    ("class", "INTEGER"),
                    ("level", "INTEGER"),
                    ("str", "INTEGER"),
                    ("agi", "INTEGER"),
                    ("sta", "INTEGER"),
                    ("inte", "INTEGER"),
                    ("spi", "INTEGER"),
                ],
                key: &["race", "class", "level"],
                rows: &[&[
                    ("race", N(1)),
                    ("class", N(1)),
                    ("level", N(1)),
                    ("str", N(23)),
                    ("agi", N(20)),
                    ("sta", N(22)),
                    ("inte", N(20)),
                    ("spi", N(20)),
                ]],
            },
            FixtureTable {
                name: "playercreateinfo",
                columns: &[
                    ("race", "INTEGER"),
                    ("class", "INTEGER"),
                    ("map", "INTEGER"),
                    ("zone", "INTEGER"),
                    ("position_x", "REAL"),
                    ("position_y", "REAL"),
                    ("position_z", "REAL"),
                    ("orientation", "REAL"),
                ],
                key: &["race", "class"],
                rows: &[&[
                    ("race", N(1)),
                    ("class", N(1)),
                    ("map", N(0)),
                    ("zone", N(12)),
                    ("position_x", F(1.5)),
                    ("position_y", F(2.5)),
                    ("position_z", F(3.5)),
                    ("orientation", F(0.5)),
                ]],
            },
            FixtureTable {
                name: "playercreateinfo_spell",
                columns: &[
                    ("spell", "INTEGER"),
                    ("race", "INTEGER"),
                    ("class", "INTEGER"),
                    ("build_min", "INTEGER"),
                    ("build_max", "INTEGER"),
                ],
                key: &["race", "class", "spell", "build_min"],
                rows: &[
                    &[
                        ("race", N(1)),
                        ("class", N(1)),
                        ("spell", N(78)),
                        ("build_min", N(5875)),
                        ("build_max", N(5875)),
                    ],
                    &[
                        ("race", N(1)),
                        ("class", N(1)),
                        ("spell", N(99)),
                        ("build_min", N(1)),
                        ("build_max", N(5874)),
                    ],
                ],
            },
            FixtureTable {
                name: "playercreateinfo_action",
                columns: &[
                    ("type", "INTEGER"),
                    ("race", "INTEGER"),
                    ("class", "INTEGER"),
                    ("button", "INTEGER"),
                    ("action", "INTEGER"),
                ],
                key: &["race", "class", "button"],
                rows: &[&[
                    ("race", N(1)),
                    ("class", N(1)),
                    ("button", N(0)),
                    ("action", N(78)),
                    ("type", N(0)),
                ]],
            },
            FixtureTable {
                name: "playercreateinfo_item",
                columns: &[
                    ("race", "INTEGER"),
                    ("class", "INTEGER"),
                    ("itemid", "INTEGER"),
                    ("amount", "INTEGER"),
                ],
                key: &["race", "class", "itemid"],
                rows: &[&[
                    ("race", N(1)),
                    ("class", N(1)),
                    ("itemid", N(25)),
                    ("amount", N(1)),
                ]],
            },
            FixtureTable {
                name: "game_graveyard_zone",
                columns: &[
                    ("ghost_zone", "INTEGER"),
                    ("id", "INTEGER"),
                    ("faction", "INTEGER"),
                    ("patch_min", "INTEGER"),
                    ("patch_max", "INTEGER"),
                ],
                key: &["id", "ghost_zone"],
                rows: &[
                    &[
                        ("id", N(7)),
                        ("ghost_zone", N(9)),
                        ("faction", N(469)),
                        ("patch_min", N(10)),
                        ("patch_max", N(10)),
                    ],
                    &[
                        ("id", N(8)),
                        ("ghost_zone", N(12)),
                        ("faction", N(0)),
                        ("patch_min", N(0)),
                        ("patch_max", N(9)),
                    ],
                ],
            },
            FixtureTable {
                name: "areatrigger_teleport",
                columns: &[
                    ("name", "TEXT"),
                    ("id", "INTEGER"),
                    ("patch", "INTEGER"),
                    ("target_map", "INTEGER"),
                    ("target_position_x", "REAL"),
                    ("target_position_y", "REAL"),
                    ("target_position_z", "REAL"),
                    ("target_orientation", "REAL"),
                    ("required_condition", "INTEGER"),
                ],
                key: &["id", "patch"],
                rows: &[
                    &[
                        ("id", N(78)),
                        ("patch", N(0)),
                        ("name", Text("Old".into())),
                        ("target_map", N(0)),
                        ("target_position_x", F(1.0)),
                        ("target_position_y", F(2.0)),
                        ("target_position_z", F(3.0)),
                        ("target_orientation", F(0.0)),
                        ("required_condition", N(0)),
                    ],
                    &[
                        ("id", N(78)),
                        ("patch", N(10)),
                        ("name", Text("Entry".into())),
                        ("target_map", N(36)),
                        ("target_position_x", F(4.0)),
                        ("target_position_y", F(5.0)),
                        ("target_position_z", F(6.0)),
                        ("target_orientation", F(0.5)),
                        ("required_condition", N(4)),
                    ],
                ],
            },
        ])
        .unwrap()
    }

    #[test]
    fn vmangos_globals_plan_uses_named_columns_and_selected_versions() {
        let source = vmangos_globals_fixture();
        let args = import_args([
            "--source",
            "vmangos",
            "--dump",
            "fixture.sqlite",
            "--family",
            "globals",
        ])
        .unwrap();
        let plan = build_vmangos_plan(&source, &args).unwrap();
        assert_eq!(plan.stamps, [("globals", 8)]);
        let sql = plan.stmts.join(";\n");
        for row in [
            "(257,1,1,20,0)",
            "(65793,1,1,1,23,20,22,20,20)",
            "(257,1,1,0,12,1.5,2.5,3.5,0.5,0)",
            "(1,1,1,78)",
            "(1,1,1,0,78,0)",
            "(1000000,257,25)",
            "(1,7,9,469)",
            "(78,36,4,5,6,0.5,'Entry')",
        ] {
            assert!(sql.contains(row), "missing {row}: {sql}");
        }
        assert!(plan.reducer_batches(&args).unwrap().is_empty());
        let manifest: serde_json::Value =
            serde_json::from_str(plan.world_source_manifest.as_deref().unwrap()).unwrap();
        assert_eq!(
            manifest["findings"][0]["source_ids"],
            serde_json::json!([78])
        );
    }

    #[test]
    fn starter_spells_cannot_disappear_on_numeric_overflow() {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value::Integer;
        let source = sqlite_fixture(&[FixtureTable {
            name: "playercreateinfo_spell",
            columns: &[
                ("race", "INTEGER"),
                ("class", "INTEGER"),
                ("spell", "INTEGER"),
                ("build_min", "INTEGER"),
                ("build_max", "INTEGER"),
            ],
            key: &["race", "class", "spell"],
            rows: &[&[
                ("race", Integer(1)),
                ("class", Integer(1)),
                ("spell", Integer(4_294_967_296)),
                ("build_min", Integer(1)),
                ("build_max", Integer(5875)),
            ]],
        }])
        .unwrap();
        let error = build_createinfo_spell_sql(&source).unwrap_err();
        assert!(
            error.to_string().contains("playercreateinfo_spell.spell"),
            "{error:#}"
        );
    }

    #[test]
    fn vmangos_globals_refuse_text_in_a_numeric_cell() {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value;
        let source = sqlite_fixture(&[FixtureTable {
            name: "playercreateinfo",
            columns: &[("race", "INTEGER")],
            key: &[],
            rows: &[&[("race", Value::Text("bad".into()))]],
        }])
        .unwrap();
        assert!(build_start_position_sql(&source)
            .unwrap_err()
            .to_string()
            .contains("playercreateinfo.race: expected a number"));
    }

    #[test]
    fn family_builders_return_sqlite_column_and_table_errors() {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value;
        let source = sqlite_fixture(&[FixtureTable {
            name: "playercreateinfo",
            columns: &[("race", "INTEGER"), ("class", "INTEGER")],
            key: &[],
            rows: &[&[("race", Value::Integer(1)), ("class", Value::Integer(1))]],
        }])
        .unwrap();
        let error = build_start_position_sql(&source).unwrap_err();
        assert!(error
            .to_string()
            .contains("playercreateinfo.map: missing column"));
        let error = build_createinfo_action_sql(&source).unwrap_err();
        assert!(error
            .to_string()
            .contains("missing source table playercreateinfo_action"));
    }

    #[test]
    fn family_builders_return_invalid_patch_ranges() {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value;
        let source = sqlite_fixture(&[FixtureTable {
            name: "game_graveyard_zone",
            columns: &[("patch_min", "INTEGER"), ("patch_max", "INTEGER")],
            key: &[],
            rows: &[&[
                ("patch_min", Value::Integer(10)),
                ("patch_max", Value::Integer(9)),
            ]],
        }])
        .unwrap();
        let error = build_graveyard_zone_sql(&source).unwrap_err();
        assert!(error.to_string().contains("patch_min"), "{error:#}");
    }

    #[test]
    fn the_start_position_family_reads_both_world_sources() {
        use crate::world_source::{sqlite_fixture, FixtureTable};
        use rusqlite::types::Value;
        let sqlite = sqlite_fixture(&[FixtureTable {
            name: "playercreateinfo",
            columns: &[
                ("race", "INTEGER"),
                ("class", "INTEGER"),
                ("map", "INTEGER"),
                ("zone", "INTEGER"),
                ("position_x", "REAL"),
                ("position_y", "REAL"),
                ("position_z", "REAL"),
                ("orientation", "REAL"),
            ],
            key: &["race", "class"],
            rows: &[&[
                ("class", Value::Integer(2)),
                ("race", Value::Integer(1)),
                ("map", Value::Integer(0)),
                ("zone", Value::Integer(12)),
                ("position_x", Value::Real(1.5)),
                ("position_y", Value::Real(2.5)),
                ("position_z", Value::Real(3.5)),
                ("orientation", Value::Real(0.5)),
            ]],
        }])
        .unwrap();
        let classic = "CREATE TABLE `playercreateinfo` (\n`class` int,\n`race` int,\n`map` int,\n`zone` int,\n`orientation` float,\n`position_x` float,\n`position_y` float,\n`position_z` float\n); INSERT INTO `playercreateinfo` VALUES (2,1,0,12,0.5,1.5,2.5,3.5);";
        for sql in [
            build_start_position_sql(classic).unwrap(),
            build_start_position_sql(&sqlite).unwrap(),
        ] {
            assert!(sql[1].contains("(258,1,2,0,12,1.5,2.5,3.5,0.5,0)"));
        }
    }

    #[test]
    fn build_graveyard_zone_sql_parses_id_ghost_zone_and_faction() {
        let dump =
            "CREATE TABLE `game_graveyard_zone` (\n`id` int,\n`ghost_loc` int,\n`link_kind` int,\n`faction` int\n); INSERT INTO `game_graveyard_zone` VALUES (105,12,0,469),(106,12,0,67),(80,40,0,0); y";
        let stmts = build_graveyard_zone_sql(dump).unwrap();
        assert_eq!(
            stmts[0],
            "DELETE FROM game_graveyard_zone WHERE row_id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_graveyard_zone "))
            .unwrap();
        assert!(insert.contains("(1,105,12,469)"), "{insert}");
        assert!(insert.contains("(2,106,12,67)"), "{insert}");
        assert!(insert.contains("(3,80,40,0)"), "{insert}");
    }

    #[test]
    fn graveyard_faction_follows_its_name_after_column_reordering() {
        let dump = "CREATE TABLE `game_graveyard_zone` (\n`faction` int,\n`id` int,\n`ghost_loc` int,\n`link_kind` int\n); INSERT INTO `game_graveyard_zone` VALUES (67,105,12,0);";
        let statements = build_graveyard_zone_sql(dump).unwrap();
        assert!(statements[1].contains("(1,105,12,67)"));
        let error = build_graveyard_zone_sql(&dump.replace("`faction`", "`other`")).unwrap_err();
        assert!(error.to_string().contains("game_graveyard_zone.faction"));
    }

    #[test]
    fn graveyard_map_links_fail_instead_of_treating_a_map_as_a_zone() {
        let schema = "CREATE TABLE `game_graveyard_zone` (\n`link_kind` int,\n`faction` int,\n`id` int,\n`ghost_loc` int\n);";
        for kind in [1, 2] {
            let dump =
                format!("{schema} INSERT INTO `game_graveyard_zone` VALUES ({kind},0,105,1);");
            let error = build_graveyard_zone_sql(&dump).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("game_graveyard_zone.link_kind: unsupported value {kind}")
            );
        }
        assert!(build_graveyard_zone_sql(&format!(
            "{schema} INSERT INTO `game_graveyard_zone` VALUES (0,0,105,12);"
        ))
        .is_ok());
    }

    #[test]
    fn build_areatrigger_teleport_sql_maps_portal_destinations() {
        let dump = "x INSERT INTO `areatrigger_teleport` VALUES \
            (1447,'Deadmines',10,0,0,0,36,-114.8,-9.5,-9.6,3.14,'',0),\
            (1448,'Moonbrook\\'s Exit',0,0,0,0,0,-11235.7,1668.3,23.9,1.57,'',0); y";
        let (stmts, n) = build_areatrigger_teleport_sql(dump).unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            stmts[0],
            "DELETE FROM game_areatrigger_teleport WHERE trigger_id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_areatrigger_teleport "))
            .unwrap();
        assert!(
            insert.contains("(1447,36,-114.8,-9.5,-9.6,3.14,'Deadmines')"),
            "{insert}"
        );
        assert!(
            insert.contains("(1448,0,-11235.7,1668.3,23.9,1.57,'Moonbrook''s Exit')"),
            "{insert}"
        );
    }

    #[test]
    fn drink_discriminator_keys_on_beverage_names_within_food_and_drink() {
        assert!(is_drink_consumable(0, 0, "Conjured Water"));
        assert!(is_drink_consumable(0, 0, "Morning Glory Dew"));
        assert!(is_drink_consumable(0, 0, "Ice Cold Milk"));
        assert!(is_drink_consumable(0, 5, "Refreshing Spring Water"));
        assert!(is_drink_consumable(0, 5, "Bottle of Dalaran Red Wine"));
        assert!(!is_drink_consumable(0, 0, "Tender Wolf Steak"));
        assert!(!is_drink_consumable(0, 0, "Snapvine Watermelon"));
        assert!(!is_drink_consumable(0, 5, "Beer Basted Boar Ribs"));
        assert!(!is_drink_consumable(0, 0, "Tough Jerky"));
        assert!(!is_drink_consumable(2, 7, "Watery Blade"));
        assert!(!is_drink_consumable(0, 1, "Healing Potion"));
        assert!(!is_drink_consumable(0, 2, "Elixir of Water Walking"));
    }

    #[test]
    fn resolve_ref_pool_flattens_two_deep_and_terminates_a_cycle() {
        use std::collections::{HashMap, HashSet};
        let mut refs: HashMap<u64, Vec<(u64, f64, i64, u32)>> = HashMap::new();
        refs.insert(1, vec![(100, 30.0, 0, 1), (0, 0.0, -2, 1)]);
        refs.insert(2, vec![(200, 100.0, 0, 3)]);
        let mut visited = HashSet::new();
        let flat = resolve_ref_pool(1, &refs, &mut visited);
        assert_eq!(
            flat.len(),
            2,
            "pool 1's own item + pool 2's nested item, flattened to one list"
        );
        assert!(flat
            .iter()
            .any(|&(item, chance, _)| item == 100 && chance == 30.0));
        assert!(flat
            .iter()
            .any(|&(item, chance, count)| item == 200 && chance == 100.0 && count == 3));

        let mut cyc: HashMap<u64, Vec<(u64, f64, i64, u32)>> = HashMap::new();
        cyc.insert(3, vec![(300, 50.0, 0, 1), (0, 0.0, -4, 1)]);
        cyc.insert(4, vec![(0, 0.0, -3, 1)]);
        let mut visited2 = HashSet::new();
        let flat2 = resolve_ref_pool(3, &cyc, &mut visited2);
        assert_eq!(
            flat2,
            vec![(300, 50.0, 1)],
            "the cycle terminates; only pool 3's own direct item survives"
        );

        let mut visited3 = HashSet::new();
        assert!(resolve_ref_pool(999, &refs, &mut visited3).is_empty());
    }

    #[test]
    fn parse_loot_family_reads_the_quest_only_sign_and_abs_value() {
        use std::collections::HashMap;
        let dump = "x INSERT INTO `pickpocketing_loot_template` (`entry`, `item`, `ChanceOrQuestChance`, `groupid`, `mincountOrRef`, `maxcount`) VALUES \
            (500,1,50.0,0,0,1),\
            (500,2,-25.5,0,0,1); y";
        let refs: HashMap<u64, Vec<(u64, f64, i64, u32)>> = HashMap::new();
        let drops = parse_loot_family(dump, "pickpocketing_loot_template", &refs).unwrap();
        let rows = drops.get(&500).expect("entry 500 has rows");
        assert_eq!(rows.len(), 2);
        let ordinary = rows.iter().find(|&&(item, ..)| item == 1).unwrap();
        assert_eq!(
            *ordinary,
            (1, 5000, 1, 0, false),
            "positive chance → an ordinary (non-quest) row"
        );
        let quest_row = rows.iter().find(|&&(item, ..)| item == 2).unwrap();
        assert_eq!(
            *quest_row,
            (2, 2550, 1, 0, true),
            "negative chance → quest_only, chance_bp is the ABS value"
        );
    }

    #[test]
    fn hearthstone_item_keeps_its_client_use_spell() {
        use std::collections::{HashMap, HashSet};
        let mut cols = crate::dump::FixtureRow::new("item_template");
        cols[it::ENTRY] = "6948".into();
        cols[it::NAME] = "'Hearthstone'".into();
        cols[it::SPELLID_1] = "8690".into();
        let dump = format!("INSERT INTO `item_template` VALUES ({});", cols.join(","));
        let (rows, _, _) = build_items_and_loot(
            &dump,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        let fields: Vec<_> = rows[0].split(',').collect();
        assert_eq!(&fields[26..28], &["8690", "0"]);
    }

    #[test]
    fn item_template_parses_the_work_item_213_columns_into_the_right_positions() {
        use std::collections::{HashMap, HashSet};

        let mut cols = crate::dump::FixtureRow::new("item_template");
        cols[it::ENTRY] = "90001".into();
        cols[it::CLASS] = "4".into();
        cols[it::SUBCLASS] = "1".into();
        cols[it::NAME] = "__NAME__".into();
        cols[it::DISPLAY_ID] = "1234".into();
        cols[it::QUALITY] = "3".into();
        cols[it::FLAGS] = "512".into();
        cols[it::BUY_PRICE] = "1000".into();
        cols[it::SELL_PRICE] = "200".into();
        cols[it::INVENTORY_TYPE] = "5".into();
        cols[it::ALLOWED_CLASS] = "-1".into();
        cols[it::ALLOWED_RACE] = "-1".into();
        cols[it::ITEM_LEVEL] = "40".into();
        cols[it::REQUIRED_LEVEL] = "30".into();
        cols[it::REQUIRED_SKILL] = "165".into();
        cols[it::REQUIRED_SKILL_RANK] = "150".into();
        cols[it::REQUIRED_REPUTATION_FACTION] = "69".into();
        cols[it::REQUIRED_REPUTATION_RANK] = "3".into();
        cols[it::MAX_COUNT] = "7".into();
        cols[it::STACKABLE] = "1".into();
        cols[it::ARMOR] = "55".into();
        cols[it::HOLY_RES] = "3".into();
        cols[it::FIRE_RES] = "12".into();
        cols[it::SPELLID_3] = "12345".into();
        cols[it::SPELLTRIGGER_3] = "1".into();
        cols[it::SPELLID_4] = "12346".into();
        cols[it::SPELLTRIGGER_4] = "2".into();
        cols[it::SPELLID_5] = "12347".into();
        cols[it::SPELLTRIGGER_5] = "0".into();
        cols[it::BONDING] = "2".into();
        cols[it::PAGE_TEXT] = "999".into();
        cols[it::START_QUEST] = "777".into();
        cols[it::MAX_DURABILITY] = "80".into();
        cols[it::BAG_FAMILY] = "6".into();
        cols[it::FOOD_TYPE] = "1".into();
        cols[it::RANDOM_PROPERTY] = "5090100".into();

        let tuple = cols
            .join(",")
            .replacen("__NAME__", "'Ring of Fire Resistance'", 1);
        let dump = format!("x INSERT INTO `item_template` VALUES ({tuple}); y");

        let (item_rows, _loot, _vendor) = build_items_and_loot(
            &dump,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(item_rows.len(), 1);
        assert_eq!(
            item_rows[0],
            "(90001,4,1,'Ring of Fire Resistance',1234,3,5,40,30,80,1000,200,1,0,0,0,0,0,0,0,0,0,0,55,0,false,0,0,0,0,0,0,2,3,12,0,0,0,0,12345,1,12346,2,12347,0,165,150,69,3,7,512,999,777,6,1,1,1503,255,5090100)",
        );
    }

    #[test]
    fn item_template_with_no_new_data_keeps_the_work_item_213_columns_zero() {
        use std::collections::{HashMap, HashSet};

        let mut cols = crate::dump::FixtureRow::new("item_template");
        cols[it::ENTRY] = "25".into();
        cols[it::CLASS] = "2".into();
        cols[it::SUBCLASS] = "7".into();
        cols[it::NAME] = "__NAME__".into();
        cols[it::DISPLAY_ID] = "1521".into();
        cols[it::QUALITY] = "0".into();
        cols[it::INVENTORY_TYPE] = "21".into();
        cols[it::ITEM_LEVEL] = "1".into();
        cols[it::REQUIRED_LEVEL] = "1".into();
        cols[it::MAX_DURABILITY] = "35".into();
        cols[it::BUY_PRICE] = "100".into();
        cols[it::SELL_PRICE] = "20".into();
        cols[it::STACKABLE] = "1".into();
        cols[it::DMG_MIN1] = "1".into();
        cols[it::DMG_MAX1] = "3".into();
        cols[it::DELAY] = "1800".into();

        let tuple = cols.join(",").replacen("__NAME__", "'Worn Shortsword'", 1);
        let dump = format!("x INSERT INTO `item_template` VALUES ({tuple}); y");

        let (item_rows, _loot, _vendor) = build_items_and_loot(
            &dump,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(item_rows.len(), 1);
        assert_eq!(
            item_rows[0],
            "(25,2,7,'Worn Shortsword',1521,0,21,1,1,35,100,20,1,1,3,1800,0,0,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,0)",
        );
    }

    #[test]
    fn item_template_preserves_restrictive_eligibility_mask_bits() {
        use std::collections::{HashMap, HashSet};

        let mut cols = crate::dump::FixtureRow::new("item_template");
        cols[it::ENTRY] = "90003".into();
        cols[it::NAME] = "__NAME__".into();
        cols[it::ALLOWED_CLASS] = "2147483650".into();
        cols[it::ALLOWED_RACE] = "2147483652".into();

        let tuple = cols.join(",").replacen("__NAME__", "'Opaque Mask Item'", 1);
        let dump = format!("x INSERT INTO `item_template` VALUES ({tuple}); y");
        let (item_rows, _loot, _vendor) = build_items_and_loot(
            &dump,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(item_rows.len(), 1);
        assert!(
            item_rows[0].ends_with(",2147483650,2147483652,0)"),
            "restrictive masks, including unknown high bits, must remain unsigned and verbatim"
        );
    }

    #[test]
    fn unrestricted_sentinel_expands_to_the_playable_masks() {
        assert_eq!(
            normalize_item_mask(-1, ALL_PLAYABLE_CLASS_MASK).unwrap(),
            0x5df
        );
        assert_eq!(
            normalize_item_mask(-1, ALL_PLAYABLE_RACE_MASK).unwrap(),
            0xff
        );
    }

    #[test]
    fn build_createinfo_spell_sql_parses_race_class_spell_and_wipes_wholesale() {
        let dump = "x INSERT INTO `playercreateinfo_spell` (`race`, `class`, `Spell`) VALUES (0,1,78),(1,0,20598),(3,1,78); y";
        let stmts = build_createinfo_spell_sql(dump).unwrap();
        assert_eq!(stmts[0], "DELETE FROM game_createinfo_spell WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_createinfo_spell "))
            .unwrap();
        assert!(insert.contains("(1,0,1,78)"), "{insert}");
        assert!(insert.contains("(2,1,0,20598)"), "{insert}");
        assert!(insert.contains("(3,3,1,78)"), "{insert}");
    }

    #[test]
    fn build_createinfo_action_sql_parses_race_class_button_action_type() {
        let dump = "x INSERT INTO `playercreateinfo_action` VALUES (1,1,0,6603,0),(1,1,1,78,0); y";
        let stmts = build_createinfo_action_sql(dump).unwrap();
        assert_eq!(
            stmts[0],
            "DELETE FROM game_createinfo_action WHERE row_id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_createinfo_action "))
            .unwrap();
        assert!(insert.contains("(1,1,1,0,6603,0)"), "{insert}");
        assert!(insert.contains("(2,1,1,1,78,0)"), "{insert}");
    }

    #[test]
    fn distinct_race_class_combos_dedupes_pairs() {
        assert_eq!(
            distinct_race_class_combos(&[(1, 1), (1, 1), (3, 1), (1, 4)]),
            3
        );
        assert_eq!(distinct_race_class_combos(&[]), 0);
    }

    #[test]
    fn build_createinfo_item_sql_without_dbc_imports_every_row_undeduped() {
        let dump = "x INSERT INTO `playercreateinfo_item` VALUES (1,1,6948,1),(1,1,2092,1); y";
        let stmts = build_createinfo_item_sql(dump, None).expect("no dbc dir needed");
        assert_eq!(
            stmts[0],
            format!("DELETE FROM game_start_item WHERE id >= {CREATEINFO_ITEM_ID_FLOOR}")
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_start_item "))
            .unwrap();
        let race_class = (1u32 << 8) | 1u32;
        assert!(
            insert.contains(&format!("({CREATEINFO_ITEM_ID_FLOOR},{race_class},6948)")),
            "{insert}"
        );
        assert!(
            insert.contains(&format!(
                "({},{race_class},2092)",
                CREATEINFO_ITEM_ID_FLOOR + 1
            )),
            "{insert}"
        );
    }

    #[test]
    fn build_createinfo_item_sql_dedupes_within_itself_and_skips_nonpositive_items() {
        let dump =
            "x INSERT INTO `playercreateinfo_item` VALUES (1,1,6948,1),(1,1,6948,1),(1,1,0,1); y";
        let stmts = build_createinfo_item_sql(dump, None).expect("no dbc dir needed");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_start_item "))
            .unwrap();
        assert_eq!(insert.matches("6948").count(), 1, "{insert}");
        assert!(!insert.contains(",0)"), "item 0 must not import: {insert}");
    }

    #[test]
    fn build_spell_chain_sql_parses_rank_chain_and_wipes_wholesale() {
        let dump = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133,2,0); y";
        let stmts = build_spell_chain_sql(dump, &[]).unwrap();
        assert_eq!(stmts[0], "DELETE FROM game_spell_chain WHERE spell_id > 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_chain "))
            .unwrap();
        assert!(insert.contains("(133,0,133,1,0)"), "{insert}");
        assert!(insert.contains("(143,133,133,2,0)"), "{insert}");
    }

    #[test]
    fn spell_chains_include_client_ranks_missing_from_the_dump() {
        let dump = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133,2,0);";
        let sql = build_spell_chain_sql(dump, &[(78, 284), (284, 285), (78, 284)])
            .unwrap()
            .join("\n");
        for row in [
            "(78,0,78,1,0)",
            "(284,78,78,2,0)",
            "(285,284,78,3,0)",
            "(143,133,133,2,0)",
        ] {
            assert_eq!(
                sql.matches(row).count(),
                1,
                "missing or duplicated rank: {sql}"
            );
        }
    }

    #[test]
    fn spell_chains_preserve_explicit_partial_families() {
        let dump = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133,2,99);";
        assert_eq!(
            build_spell_chain_sql(dump, &[(133, 143), (143, 145)]).unwrap(),
            build_spell_chain_sql(dump, &[]).unwrap(),
        );
    }

    #[test]
    fn spell_chains_ignore_invalid_links_in_explicit_families() {
        let dump = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0);";
        assert_eq!(
            build_spell_chain_sql(dump, &[(133, 143), (133, 145), (143, 133)]).unwrap(),
            build_spell_chain_sql(dump, &[]).unwrap(),
        );
    }

    #[test]
    fn spell_chains_refuse_ambiguous_client_families() {
        for links in [vec![(1, 2), (1, 3)], vec![(1, 3), (2, 3)]] {
            assert!(build_spell_chain_sql("", &links).is_err());
        }
    }

    #[test]
    fn spell_chains_refuse_cyclic_client_families() {
        for links in [
            vec![(1, 1)],
            vec![(1, 2), (2, 1)],
            vec![(1, 2), (2, 3), (3, 2)],
        ] {
            assert!(build_spell_chain_sql("", &links).is_err());
        }
    }

    #[test]
    fn spell_chains_enforce_the_stored_rank_limit() {
        let mut links: Vec<_> = (1..255).map(|spell| (spell, spell + 1)).collect();
        assert!(build_spell_chain_sql("", &links).is_ok());
        links.push((255, 256));
        assert!(build_spell_chain_sql("", &links).is_err());
    }

    #[test]
    fn build_spell_proc_event_sql_maps_the_nine_overlay_columns_by_tuple_position() {
        let dump = "x INSERT INTO `spell_proc_event` (`entry`, `SchoolMask`, `SpellFamilyName`, `SpellFamilyMask0`, `SpellFamilyMask1`, `SpellFamilyMask2`, `procFlags`, `procEx`, `ppmRate`, `CustomChance`, `Cooldown`) VALUES \
                    (12358,4,3,32,32,32,0,2,0,0,0),(20007,0,0,0,0,0,20,0,1.5,15,45); y";
        let stmts = build_spell_proc_event_sql(dump, None).unwrap();
        assert_eq!(
            stmts[0],
            "DELETE FROM game_spell_proc_event WHERE spell_id < 50000"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_proc_event "))
            .expect("two rows must produce one INSERT");
        assert!(insert.contains("(12358,0,2,4,3,32,0,0,0)"), "{insert}");
        assert!(
            insert.contains("(20007,20,0,0,0,0,1.5,15,45000)"),
            "{insert}"
        );
    }

    #[test]
    fn current_proc_overlay_maps_three_equal_effect_masks_without_shifting_fields() {
        let dump = "INSERT INTO `spell_proc_event` VALUES (123,4,3,8,8,8,20,2,1.5,15,45);";
        let sql = build_spell_proc_event_sql(dump, None).unwrap().join("\n");
        assert!(sql.contains("(123,20,2,4,3,8,1.5,15,45000)"), "{sql}");
    }

    #[test]
    fn current_proc_overlay_selects_the_only_active_proc_effect_mask() {
        let dump = "INSERT INTO `spell_proc_event` VALUES (123,4,3,8,4,4,20,2,0,0,0);";
        let effects = std::collections::HashMap::from([(123, vec![0])]);
        let sql = build_spell_proc_event_sql(dump, Some(&effects))
            .unwrap()
            .join("\n");
        assert!(sql.contains("(123,20,2,4,3,8,0,0,0)"), "{sql}");
        assert!(build_spell_proc_event_sql(dump, None).is_err());
    }

    #[test]
    fn different_active_proc_effect_masks_are_refused_instead_of_broadened() {
        let dump = "INSERT INTO `spell_proc_event` VALUES (123,4,3,8,4,4,20,2,0,0,0);";
        let effects = std::collections::HashMap::from([(123, vec![0, 1])]);
        assert!(build_spell_proc_event_sql(dump, Some(&effects)).is_err());
    }

    #[test]
    fn unknown_primary_spell_does_not_create_unusable_caster_rows() {
        let dump = "INSERT INTO `creature_template_spells` VALUES (7,0,2,4,0,0,0,0,0,0,0,0);";
        let entries = std::collections::HashSet::from([7]);
        let spells = std::collections::HashSet::from([4]);
        let (casts, rotation) = build_creature_cast_rows(dump, &entries, Some(&spells)).unwrap();
        assert!(casts.is_empty());
        assert!(rotation.is_empty());
    }

    #[test]
    fn build_spell_proc_event_sql_empty_table_yields_delete_only_no_insert() {
        let stmts = build_spell_proc_event_sql("x y", None).unwrap();
        assert_eq!(
            stmts,
            vec!["DELETE FROM game_spell_proc_event WHERE spell_id < 50000".to_string()]
        );
    }

    #[test]
    fn proc_overlay_ignores_extra_columns_and_excludes_package_spell_ids() {
        let dump = "CREATE TABLE `spell_proc_event` (\n`entry` int,\n`SchoolMask` int,\n`SpellFamilyName` int,\n`SpellFamilyMask0` int,\n`SpellFamilyMask1` int,\n`SpellFamilyMask2` int,\n`procFlags` int,\n`procEx` int,\n`ppmRate` float,\n`CustomChance` float,\n`Cooldown` int,\n`extra` int\n); INSERT INTO `spell_proc_event` VALUES (50143,0,0,0,0,0,0,0,0,0,5,7),(133,0,0,0,0,0,0,0,0,0,0,7);";
        let statements = build_spell_proc_event_sql(dump, None).unwrap();
        assert_eq!(statements.len(), 2);
        assert!(statements[1].contains("(133,0,0,0,0,0,0,0,0)"));
        assert!(!statements[1].contains("50143"));
    }

    #[test]
    fn build_spell_chain_sql_empty_table_yields_delete_only_no_insert() {
        let stmts = build_spell_chain_sql("x y", &[]).unwrap();
        assert_eq!(
            stmts,
            vec!["DELETE FROM game_spell_chain WHERE spell_id > 0".to_string()]
        );
    }

    #[test]
    fn build_spell_learn_sql_parses_parent_and_dependent_ids_densely_from_one() {
        let dump = "x INSERT INTO `spell_learn_spell` VALUES (196,197,1),(196,198,0); y";
        let stmts = build_spell_learn_sql(dump).unwrap();
        assert_eq!(stmts[0], "DELETE FROM game_spell_learn WHERE id > 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        assert!(insert.contains("(1,196,197)"), "{insert}");
        assert!(
            insert.contains("(2,196,198)"),
            "Active=0 row must still import: {insert}"
        );
    }

    #[test]
    fn build_spell_learn_sql_empty_table_still_synthesizes_armor_proficiency_wrappers() {
        let stmts = build_spell_learn_sql("x y").unwrap();
        assert_eq!(stmts[0], "DELETE FROM game_spell_learn WHERE id > 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        assert!(
            insert.contains("(1,7109,750)"),
            "Plate Mail wrapper→passive: {insert}"
        );
        assert!(
            insert.contains("(2,8738,8737)"),
            "Mail wrapper→passive: {insert}"
        );
    }

    #[test]
    fn build_spell_learn_sql_synthesizes_armor_proficiency_wrappers_exactly_once_alongside_real_rows(
    ) {
        let dump = "x INSERT INTO `spell_learn_spell` VALUES (196,197,1); y";
        let stmts = build_spell_learn_sql(dump).unwrap();
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        assert!(insert.contains("(1,196,197)"), "{insert}");
        assert!(insert.contains("(2,7109,750)"), "{insert}");
        assert!(insert.contains("(3,8738,8737)"), "{insert}");
        assert_eq!(insert.matches("7109,750").count(), 1, "{insert}");
        assert_eq!(insert.matches("8738,8737").count(), 1, "{insert}");
    }

    #[test]
    fn build_spell_learn_sql_does_not_double_synthesize_if_dump_already_has_the_wrapper_row() {
        let dump = "x INSERT INTO `spell_learn_spell` VALUES (7109,750,1); y";
        let stmts = build_spell_learn_sql(dump).unwrap();
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        assert_eq!(
            insert.matches("7109,750").count(),
            1,
            "must not double-insert the plate wrapper: {insert}"
        );
        assert!(insert.contains("8738,8737"), "{insert}");
    }

    #[test]
    fn family_spellmeta_filter_produces_only_spellmeta_family_stmts() {
        let dump = format!(
            "{} INSERT INTO `spell_chain` VALUES (133,0,133,1,0); \
             INSERT INTO `spell_learn_spell` VALUES (196,197,1); \
             INSERT INTO `spell_proc_event` (`entry`, `SchoolMask`, `SpellFamilyName`, `SpellFamilyMask0`, `SpellFamilyMask1`, `SpellFamilyMask2`, `procFlags`, `procEx`, `ppmRate`, `CustomChance`, `Cooldown`) VALUES (12358,4,3,32,32,32,0,2,0,0,0); y",
            tiny_quest_dump(102, 502)
        );
        let mut args = test_args();
        args.family = Some("spellmeta".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");

        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("DELETE FROM game_spell_chain")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_spell_chain") && s.contains("133")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("DELETE FROM game_spell_learn")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_spell_learn") && s.contains("196")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("DELETE FROM game_spell_proc_event")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_spell_proc_event") && s.contains("12358")),
            "{:?}",
            plan.stmts
        );
        let non_spellmeta_markers = [
            "game_creature_template",
            "game_creature_waypoint",
            "game_item_template",
            "game_creature_loot",
            "game_npc_vendor",
            "game_gossip_menu",
            "game_npc_text",
            "game_gossip_option",
            "game_gameobject_template",
            "game_trainer_spell",
            "game_creature_cast",
            "game_creature_spell",
            "game_level_stats",
            "game_start_position",
            "game_graveyard_zone",
            "game_createinfo_spell",
            "game_createinfo_action",
            "game_quest_template",
        ];
        for marker in non_spellmeta_markers {
            assert!(
                !plan.stmts.iter().any(|s| s.contains(marker)),
                "family=spellmeta must not touch {marker}: {:?}",
                plan.stmts
            );
        }
        assert_eq!(plan.stamps, vec![("spellmeta", 5u64)], "{:?}", plan.stamps);
    }

    fn creature_template_row(entry: u64, gossip_menu_id: u64) -> String {
        creature_template_row_with_faction(entry, gossip_menu_id, 0, 0, 0, 0)
    }

    fn creature_template_row_with_trainer(
        entry: u64,
        gossip_menu_id: u64,
        trainer_type: u64,
        trainer_class: u64,
    ) -> String {
        creature_template_row_with_faction(entry, gossip_menu_id, 0, 0, trainer_type, trainer_class)
    }

    fn creature_template_row_with_faction(
        entry: u64,
        gossip_menu_id: u64,
        faction_template: u64,
        creature_family: u64,
        trainer_type: u64,
        trainer_class: u64,
    ) -> String {
        let mut cols = crate::dump::FixtureRow::new("creature_template");
        cols[ct::ENTRY] = entry.to_string();
        cols[ct::GOSSIP_MENU_ID] = gossip_menu_id.to_string();
        cols[ct::FACTION] = faction_template.to_string();
        cols[ct::FAMILY] = creature_family.to_string();
        cols[ct::TRAINER_TYPE] = trainer_type.to_string();
        cols[ct::TRAINER_CLASS] = trainer_class.to_string();
        format!("({})", cols.join(","))
    }

    fn gossip_option_row_short(
        menu_id: u64,
        id: u64,
        icon: u64,
        text: &str,
        option_type: u64,
    ) -> String {
        format!("({menu_id},{id},{icon},'{text}',0,{option_type},0,0,0,0,0,0,'',0,0)")
    }

    #[test]
    fn build_gossip_sql_orders_menu_options_by_dense_index_and_maps_actions() {
        let entry = 300u64;
        let menu_id = 900u64;
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {}, {}, {}; y",
            creature_template_row(entry, menu_id),
            gossip_option_row_short(menu_id, 0, 0, "Lets talk.", 1),
            gossip_option_row_short(menu_id, 1, 1, "Id like to browse your goods.", 3),
            gossip_option_row_short(menu_id, 2, 0, "Make this inn your home.", 8),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(etl.option_rows.len(), 3, "{:?}", etl.option_rows);
        assert!(
            etl.option_rows[0].contains(&format!("(1,{entry},0,0,'Lets talk.',1,0,0,0,0)")),
            "{}",
            etl.option_rows[0]
        );
        assert!(
            etl.option_rows[1].contains(&format!(
                "(2,{entry},1,1,'Id like to browse your goods.',3,0,0,0,0)"
            )),
            "{}",
            etl.option_rows[1]
        );
        assert!(
            etl.option_rows[2].contains(&format!(
                "(3,{entry},2,0,'Make this inn your home.',8,0,0,0,0)"
            )),
            "{}",
            etl.option_rows[2]
        );
    }

    #[test]
    fn build_gossip_sql_emits_the_four_relay_menu_profiles_and_dense_options() {
        let dump = "INSERT INTO `gossip_menu` VALUES \
             (6623,5001,0,0),(6687,5002,0,0),(6688,5003,0,0),(6949,5004,0,0); \
             INSERT INTO `npc_text` (`ID`, `prob0`, `text0_0`) VALUES \
             (5001,1,'One'), \
             (5002,1,'Two'), \
             (5003,1,'Three'), \
             (5004,1,'Four'); \
             INSERT INTO `broadcast_text` (`Id`, `Text`, `Text1`, `ChatTypeID`) VALUES \
             (8001,'One','',0),(8002,'Two','',0),(8003,'Three','',0), \
             (8004,'Four','',0),(9001,'First option','',0); \
             INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`) VALUES (501,9,777,0); \
             INSERT INTO `gossip_menu_option` VALUES \
             (6688,7,1,'Second option',0,1,0,12,0,0,0,0,'',0,501), \
             (6687,2,0,'9001',0,1,0,0,0,0,0,0,'',0,0);";
        let etl = build_gossip_sql(dump, &std::collections::HashSet::new()).unwrap();

        assert_eq!(
            etl.profile_rows,
            vec!["(6623,5001)", "(6687,5002)", "(6688,5003)", "(6949,5004)",]
        );
        assert_eq!(etl.profile_option_rows.len(), 2);
        assert_eq!(
            etl.profile_option_rows[0],
            "(1,6687,0,0,'First option',1,0,0,0,0)"
        );
        assert_eq!(
            etl.profile_option_rows[1],
            "(2,6688,0,1,'Second option',1,12,7,501,0)"
        );
        assert_eq!(
            etl.npc_text_rows,
            [
                "(5001,'One')",
                "(5002,'Two')",
                "(5003,'Three')",
                "(5004,'Four')"
            ]
        );
        assert_eq!(
            etl.npc_text_slot_rows,
            [
                "(1,5001,0,'One','',1)",
                "(2,5002,0,'Two','',1)",
                "(3,5003,0,'Three','',1)",
                "(4,5004,0,'Four','',1)",
            ]
        );
    }

    #[test]
    fn build_gossip_sql_reindexes_dense_option_order_even_when_cmangos_ids_have_gaps() {
        let entry = 301u64;
        let menu_id = 901u64;
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {}, {}; y",
            creature_template_row(entry, menu_id),
            gossip_option_row_short(menu_id, 5, 0, "Second.", 1),
            gossip_option_row_short(menu_id, 2, 0, "First.", 1),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(etl.option_rows.len(), 2);
        assert!(
            etl.option_rows[0].contains(&format!("{entry},0,0,'First.'")),
            "{}",
            etl.option_rows[0]
        );
        assert!(
            etl.option_rows[1].contains(&format!("{entry},1,0,'Second.'")),
            "{}",
            etl.option_rows[1]
        );
    }

    #[test]
    fn build_gossip_sql_drops_questgiver_placeholder_rows_and_keeps_the_index_dense() {
        use lyracore_shared::constants::gossip_option;
        let entry = 320u64;
        let menu_id = 920u64;
        let options = [
            gossip_option_row_short(menu_id, 0, 0, "Chat.", gossip_option::GOSSIP as u64),
            gossip_option_row_short(
                menu_id,
                1,
                0,
                "GOSSIP_OPTION_QUESTGIVER",
                gossip_option::QUESTGIVER as u64,
            ),
            gossip_option_row_short(menu_id, 2, 0, "Train me.", gossip_option::TRAINER as u64),
        ]
        .join(",");
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {options}; y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(etl.option_rows.len(), 2, "{:?}", etl.option_rows);
        assert!(
            !etl.option_rows
                .iter()
                .any(|r| r.contains("GOSSIP_OPTION_QUESTGIVER")),
            "{:?}",
            etl.option_rows
        );
        assert!(etl.option_rows[0].contains(&format!("{entry},0,0,'Chat.'")));
        assert!(etl.option_rows[1].contains(&format!("{entry},1,0,'Train me.'")));
    }

    #[test]
    fn build_gossip_sql_takes_the_unconditional_greeting_as_the_menu_title() {
        let entry = 321u64;
        let menu_id = 921u64;
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu` VALUES ({menu_id},5100,0,44),({menu_id},5101,0,0); \
             INSERT INTO `npc_text` (`ID`, `prob0`, `text0_0`) VALUES (5100,1,'Come back when the war is over.'), \
             (5101,1,'Well met, traveler.'); \
             INSERT INTO `broadcast_text` (`Id`, `Text`, `Text1`) VALUES (8100,'Come back when the war is over.','NULL'), \
             (8101,'Well met, traveler.','NULL'); y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(
            etl.menu_rows,
            vec![format!("({entry},5101)")],
            "the unconditional row (condition_id 0) is the default title"
        );
    }

    #[test]
    fn build_gossip_sql_hides_a_seasonal_event_option_instead_of_failing_open() {
        let entry = 322u64;
        let menu_id = 922u64;
        let option = format!("({menu_id},0,0,'What is Children'' Week?',0,1,0,0,0,0,0,0,'',0,501)");
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {option}; INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`) VALUES (501,12,0,0); y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(etl.option_rows.len(), 1);
        assert_eq!(etl.condition_rows, ["(501,12,0,0,0,0,0)"]);
        assert!(
            etl.option_rows[0].ends_with(&format!(
                "{},501,0)",
                lyracore_shared::constants::gossip_condition::REFERENCE
            )),
            "{}",
            etl.option_rows[0]
        );
    }

    #[test]
    fn build_gossip_sql_preserves_a_referenced_quest_completion_operand() {
        let entry = 302u64;
        let menu_id = 902u64;
        let option =
            format!("({menu_id},0,0,'Have you started my quest?',0,1,0,0,0,0,0,0,'',0,501)");
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {option}; INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`) VALUES (501,9,777,2); y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(etl.option_rows.len(), 1);
        assert_eq!(etl.condition_rows, ["(501,9,777,2,0,0,0)"]);
        assert!(
            etl.option_rows[0].ends_with("7,501,0)"),
            "{}",
            etl.option_rows[0]
        );
    }

    #[test]
    fn build_gossip_sql_retains_source_conditions_and_dangling_references() {
        let entry = 303u64;
        let menu_id = 903u64;
        let dump = format!(
            "INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES \
                ({menu_id},0,0,'Finished quest.',0,1,0,0,0,0,0,0,'',0,501), \
                ({menu_id},1,0,'Unsupported gate.',0,1,0,0,0,0,0,0,'',0,502), \
                ({menu_id},2,0,'Dangling gate.',0,1,0,0,0,0,0,0,'',0,599); \
             INSERT INTO `conditions` (`condition_entry`, `type`, `value1`, `value2`, `value3`, `value4`, `flags`) VALUES \
                (501,9,777,2,0,0,0),(502,-3,503,0,0,0,0),(503,5,1,4,0,0,1);",
            creature_template_row(entry, menu_id),
        );
        let entries = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(etl.option_rows.len(), 3);
        for (option, id) in etl.option_rows.iter().zip([501, 502, 599]) {
            assert!(option.ends_with(&format!("7,{id},0)")), "{option}");
        }
        assert_eq!(
            etl.condition_rows,
            [
                "(501,9,777,2,0,0,0)",
                "(502,-3,503,0,0,0,0)",
                "(503,5,1,4,0,0,1)"
            ]
        );
        let conditions = gossip_conditions::read(&dump).unwrap();
        use lyracore_shared::gossip::{evaluate_reference, CharacterFacts, QuestState};
        let facts = CharacterFacts {
            level: 20,
            race: 1,
            class: 1,
        };
        let evaluate =
            |id, state| evaluate_reference(id, facts, |_| state, |id| conditions.get(&id).copied());
        assert_eq!(evaluate(501, QuestState::Complete), Ok(true));
        assert_eq!(evaluate(501, QuestState::Incomplete), Ok(false));
        assert!(evaluate(502, QuestState::Complete).is_err());
        assert!(evaluate(599, QuestState::Complete).is_err());
    }

    #[test]
    fn build_gossip_sql_keeps_npc_text_and_resolves_broadcast_option_labels() {
        let entry = 304u64;
        let menu_id = 904u64;
        let text_id = 5000u64;
        let mut npc_text = crate::dump::FixtureRow::new("npc_text");
        npc_text["ID"] = text_id.to_string();
        npc_text["prob0"] = "1".into();
        npc_text["text0_0"] = "'Greetings, traveler.'".into();
        let npc_text_row = format!("({})", npc_text.join(","));
        let option = gossip_option_row_short(menu_id, 0, 0, "9002", 1);
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu` (`entry`, `text_id`) VALUES ({menu_id},{text_id}); \
             INSERT INTO `npc_text` VALUES {npc_text_row}; \
             INSERT INTO `gossip_menu_option` VALUES {option}; \
             INSERT INTO `broadcast_text` (`Id`, `Text`, `Text1`) VALUES (8001,'Greetings, traveler.','NULL'), (9002,'Care to trade?','NULL'); y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert_eq!(etl.menu_rows, vec![format!("({entry},{text_id})")]);
        assert!(
            etl.npc_text_rows
                .iter()
                .any(|r| r.contains("Greetings, traveler.")),
            "{:?}",
            etl.npc_text_rows
        );
        assert_eq!(etl.option_rows.len(), 1);
        assert!(
            etl.option_rows[0].contains("'Care to trade?'"),
            "{}",
            etl.option_rows[0]
        );
    }

    #[test]
    fn build_gossip_sql_emits_every_populated_npc_text_slot_and_skips_empty_ones() {
        let entry = 305u64;
        let menu_id = 905u64;
        let text_id = 5001u64;
        let mut cols = crate::dump::FixtureRow::new("npc_text");
        for slot in 0..8 {
            cols[&format!("text{slot}_0")] = "NULL".into();
            cols[&format!("text{slot}_1")] = "NULL".into();
        }
        cols["ID"] = text_id.to_string();
        cols["text0_0"] = "'Well met.'".into();
        cols["text0_1"] = "'Well met, traveler.'".into();
        cols["lang0"] = "0".into();
        cols["prob0"] = "0.6".into();
        cols["text3_0"] = "'Watch yourself.'".into();
        cols["text3_1"] = "NULL".into();
        cols["lang3"] = "0".into();
        cols["prob3"] = "0.4".into();
        let npc_text_row = format!("({})", cols.join(","));
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu` (`entry`, `text_id`) VALUES ({menu_id},{text_id}); \
             INSERT INTO `npc_text` VALUES {npc_text_row}; y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries).unwrap();
        assert!(
            etl.npc_text_rows[0].contains("Well met."),
            "{:?}",
            etl.npc_text_rows
        );
        assert_eq!(
            etl.npc_text_slot_rows.len(),
            2,
            "{:?}",
            etl.npc_text_slot_rows
        );
        let slot0 = etl
            .npc_text_slot_rows
            .iter()
            .find(|r| r.contains(&format!(",{text_id},0,")))
            .expect("slot 0 row");
        assert!(
            slot0.contains("'Well met.'")
                && slot0.contains("'Well met, traveler.'")
                && slot0.ends_with("0.6)"),
            "{slot0}"
        );
        let slot3 = etl
            .npc_text_slot_rows
            .iter()
            .find(|r| r.contains(&format!(",{text_id},3,")))
            .expect("slot 3 row");
        assert!(
            slot3.contains("'Watch yourself.'") && slot3.ends_with("0.4)"),
            "{slot3}"
        );
    }

    #[test]
    fn families_fixed_set_accepts_every_named_family_and_rejects_unknown() {
        for name in [
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
        ] {
            assert!(
                FAMILIES.contains(&name),
                "{name} should be a valid --family value"
            );
        }
        assert!(
            !FAMILIES.contains(&"terrain"),
            "terrain is a separate invocation, not a --family"
        );
        assert!(
            !FAMILIES.contains(&"dbc"),
            "dbc is a separate invocation, not a --family"
        );
        assert!(
            !FAMILIES.contains(&"spells"),
            "spells is a separate invocation, not a --family"
        );
        assert!(
            !FAMILIES.contains(&"bogus"),
            "an unrecognized name must not silently validate"
        );
    }

    #[test]
    fn family_active_is_true_for_every_family_when_family_is_none_and_exclusive_otherwise() {
        let mut args = test_args();
        args.family = None;
        for name in FAMILIES {
            assert!(
                family_active(&args, name),
                "family=None must activate every family ({name})"
            );
        }
        args.family = Some("quests".to_string());
        assert!(family_active(&args, "quests"));
        for name in FAMILIES.iter().filter(|&&n| n != "quests") {
            assert!(
                !family_active(&args, name),
                "family=Some(quests) must exclude {name}"
            );
        }
    }

    #[test]
    fn canonical_quests_keep_event_requirement_on_other_destination() {
        let dump = format!(
            "INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),(2,200,1,1,10300,800,1326,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES (100,500); INSERT INTO `creature_involvedrelation` VALUES (200,500); \
             INSERT INTO `creature_ai_scripts` VALUES {};",
            creature_template_row(100, 0), creature_template_row(200, 0),
            quest_template_row_requiring_event(500),
            eventai_rule_row(10, 100, 4, 0, 100, 0, [0; 6], [[15, 500, 6, 0], [0; 4], [0; 4]]),
        );
        let eastern = import_args([
            "--dump",
            "synthetic.sql",
            "--world-profile",
            "starting-eastern",
        ])
        .unwrap();
        let mut kalimdor = import_args([
            "--dump",
            "synthetic.sql",
            "--world-profile",
            "starting-kalimdor",
        ])
        .unwrap();
        kalimdor.family = Some("quests".into());
        let east = build_dump_plan(&dump, &eastern, &None, &None).unwrap();
        let west = build_dump_plan(&dump, &kalimdor, &None, &None).unwrap();
        let reqs = |p: &DumpPlan| {
            p.stmts
                .iter()
                .filter(|s| s.starts_with("INSERT INTO game_quest_event_requirement"))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert!(!reqs(&east).is_empty(), "source needs credit");
        let complete = |p: &DumpPlan| {
            lyracore_shared::quest::objectives_complete([], &[], |_| 0, !reqs(p).is_empty(), false)
        };
        assert!(!complete(&east), "uncredited quest is incomplete on source");
        assert!(
            west.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_creature_quest")
                    && s.contains(",200,500,1)")),
            "destination has end giver"
        );
        assert_eq!(
            complete(&east),
            complete(&west),
            "same uncredited Character can turn in on destination"
        );
        kalimdor.family = Some("creature-ai".into());
        let event_import = build_dump_plan(&dump, &kalimdor, &None, &None).unwrap();
        assert_eq!(reqs(&east), reqs(&event_import));
        assert_eq!(event_import.stamps, vec![("creature-ai", 1)]);
    }

    #[test]
    fn canonical_quest_reimport_keeps_summon_giver_quest() {
        let dump =
            format!(
            "INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `quest_template` VALUES {},{}; \
             INSERT INTO `creature_questrelation` VALUES (100,500),(200,501); \
             INSERT INTO `creature_ai_summons` (`id`, `position_x`, `position_y`, `position_z`, `orientation`, `spawntimesecs`) VALUES (700,1,2,3,4,5); \
             INSERT INTO `creature_ai_scripts` VALUES {};",
            creature_template_row(100, 0), creature_template_row(200, 0),
            quest_template_row(500, "Placed giver"), quest_template_row(501, "Summon giver"),
            eventai_rule_row(10,100,0,0,100,0,[0,1,0,1,0,0],[[32,200,0,700],[0;4],[0;4]]),
        );
        for profile in [
            "starting-eastern",
            "full-world",
            "full-eastern",
            "full-instances",
        ] {
            let dump = if profile == "full-instances" {
                dump.replace("(1,100,0,1,", "(1,100,36,1,")
            } else {
                dump.clone()
            };
            let mut args =
                import_args(["--dump", "synthetic.sql", "--world-profile", profile]).unwrap();
            let full = build_dump_plan(&dump, &args, &None, &None).unwrap();
            args.family = Some("quests".into());
            let partial = build_dump_plan(&dump, &args, &None, &None).unwrap();
            let quests = |p: &DumpPlan| {
                p.stmts
                    .iter()
                    .filter(|s| s.starts_with("INSERT INTO game_quest_template"))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert!(quests(&full).join("").contains("(501,"));
            assert_eq!(
                quests(&full),
                quests(&partial),
                "quest reimport must preserve shared definitions"
            );
            let givers = |plan: &DumpPlan| {
                plan.stmts
                    .iter()
                    .filter(|statement| statement.starts_with("INSERT INTO game_creature_quest"))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            assert!(givers(&full).join("").contains(",200,501,0)"));
            assert_eq!(givers(&full), givers(&partial));
        }
    }

    #[test]
    fn canonical_quest_reimport_checks_creature_ai_package_deltas() {
        let mut args = import_args([
            "--dump",
            "synthetic.sql",
            "--world-profile",
            "starting-eastern",
            "--family",
            "quests",
        ])
        .unwrap();
        let absent_root =
            std::env::temp_dir().join(format!("lyracore-absent-packages-{}", std::process::id(),));
        assert!(!absent_root.exists());
        args.packages = Some(absent_root.to_string_lossy().into_owned());
        assert!(run_package_stage(&args, "creature-ai").is_err());
        args.world_profile = None;
        assert!(run_package_stage(&args, "creature-ai").is_ok());
    }

    fn quest_catalogue_fixture() -> String {
        let mut kalimdor_quest = crate::dump::FixtureRow::new("quest_template");
        kalimdor_quest[qt::ENTRY] = "501".into();
        kalimdor_quest[qt::TITLE] = "'Supplies'".into();
        kalimdor_quest[qt::REQ_ITEM_ID1] = "900".into();
        kalimdor_quest[qt::REQ_ITEM_COUNT1] = "3".into();
        kalimdor_quest[qt::REW_ITEM_ID1] = "901".into();
        kalimdor_quest[qt::REW_ITEM_COUNT1] = "2".into();
        kalimdor_quest[qt::REW_CHOICE_ITEM_ID1] = "902".into();
        kalimdor_quest[qt::REW_CHOICE_ITEM_COUNT1] = "1".into();
        let kalimdor_quest = format!("({})", kalimdor_quest.join(","));
        format!(
            "INSERT INTO `creature` VALUES \
             (1,100,0,1,-9000,0,80,0,300,300,0,0),\
             (2,200,1,1,10300,800,1326,0,300,300,0,0),\
             (3,400,36,1,-16,-383,-33,0,300,300,0,0),\
             (4,500,1,1,0,0,0,0,300,300,0,0),\
             (5,600,0,1,1814,1485,90,0,300,300,0,0),\
             (6,700,1,1,-514,-4344,38,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{},{},{},{},{}; \
             INSERT INTO `quest_template` VALUES {},{},{},{},{},{}; \
             INSERT INTO `creature_questrelation` VALUES (100,500),(400,502),(500,503),(600,504),(700,505); \
             INSERT INTO `creature_involvedrelation` VALUES (100,500),(400,502),(600,504),(700,505); \
             INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`) VALUES (300,2,100,'Quest giver',0,0,0,1,0,0,0,0); \
             INSERT INTO `gameobject` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`, `rotation0`, `rotation1`, `rotation2`, `rotation3`) VALUES (3,300,1,0,10300,800,1326,0,0,0,0,1); \
             INSERT INTO `gameobject_questrelation` VALUES (300,501); \
             INSERT INTO `gameobject_involvedrelation` VALUES (300,501); \
             INSERT INTO `npc_vendor` (`entry`, `item`, `maxcount`, `incrtime`) VALUES (200,900,0,0);",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            creature_template_row(400, 0),
            creature_template_row(500, 0),
            creature_template_row(600, 0),
            creature_template_row(700, 0),
            quest_template_row_with_cast_objective(500, 100, 456),
            kalimdor_quest,
            quest_template_row(502, "Instance quest"),
            quest_template_row(503, "Outside supported content"),
            quest_template_row(504, "Deathknell quest"),
            quest_template_row(505, "Valley of Trials quest"),
        )
    }

    fn quest_catalogue_plan(profile: &str) -> DumpPlan {
        let args = import_args([
            "--dump",
            "synthetic.sql",
            "--family",
            "quests",
            "--world-profile",
            profile,
        ])
        .unwrap();
        build_dump_plan(&quest_catalogue_fixture(), &args, &None, &None).unwrap()
    }

    #[test]
    fn canonical_profiles_share_supported_quest_definitions() {
        let mut catalogue = None;
        for profile in WorldImportProfile::NAMES.iter().filter(|name| {
            !WorldImportProfile::parse(name)
                .unwrap()
                .imports_all_templates()
        }) {
            let plan = quest_catalogue_plan(profile);
            let templates = plan
                .stmts
                .iter()
                .find(|s| s.starts_with("INSERT INTO game_quest_template"))
                .expect("quest catalogue on every Shard");
            for quest in [500, 501, 502, 504, 505] {
                assert!(
                    templates.contains(&format!("({quest},")),
                    "{profile} is missing quest {quest}: {templates}"
                );
            }
            assert!(!templates.contains("(503,"), "{profile}: {templates}");
            let objectives = plan
                .stmts
                .iter()
                .find(|s| s.starts_with("INSERT INTO game_quest_objective"))
                .expect("quest objectives on every Shard");
            assert!(
                objectives.contains(",500,0,0,100,2)"),
                "{profile}: {objectives}"
            );
            assert!(
                objectives.contains(",501,0,1,900,3)"),
                "{profile}: {objectives}"
            );
            for (table, expected) in [
                ("game_quest_cast_objective", ",500,0,456)"),
                ("game_quest_reward_item", ",501,901,2)"),
                ("game_quest_reward_choice", ",501,0,902,1)"),
            ] {
                let statement = plan
                    .stmts
                    .iter()
                    .find(|s| s.starts_with(&format!("INSERT INTO {table} (")))
                    .expect("shared quest data");
                assert!(statement.contains(expected), "{profile}: {statement}");
            }
            let shared: Vec<_> = plan
                .stmts
                .iter()
                .filter(|s| {
                    !s.contains("game_creature_quest") && !s.contains("game_gameobject_quest")
                })
                .cloned()
                .collect();
            if let Some(expected) = &catalogue {
                assert_eq!(&shared, expected, "{profile} quest definitions");
            } else {
                catalogue = Some(shared);
            }
        }
    }

    #[test]
    fn canonical_profiles_keep_quest_givers_local() {
        for profile in WorldImportProfile::NAMES {
            let plan = quest_catalogue_plan(profile);
            let relations = plan
                .stmts
                .iter()
                .filter(|s| s.starts_with("INSERT INTO game_creature_quest ("))
                .cloned()
                .collect::<Vec<_>>()
                .join(";");
            let go_relations = plan
                .stmts
                .iter()
                .filter(|s| s.starts_with("INSERT INTO game_gameobject_quest ("))
                .cloned()
                .collect::<Vec<_>>()
                .join(";");
            for (giver, quest, owner) in [
                (600, 504, "starting-eastern"),
                (700, 505, "starting-kalimdor"),
            ] {
                for role in [0, 1] {
                    assert_eq!(
                        relations.contains(&format!(",{giver},{quest},{role})")),
                        *profile == owner
                            || *profile == "full-world"
                            || (*profile == "full-eastern" && giver == 600)
                            || (*profile == "full-kalimdor" && giver == 700),
                        "{profile} Horde giver {giver}: {relations}"
                    );
                }
            }
            for role in [0, 1] {
                assert_eq!(
                    relations.contains(&format!(",100,500,{role})")),
                    matches!(
                        *profile,
                        "alliance-eastern"
                            | "starting-eastern"
                            | "alliance-single"
                            | "full-world"
                            | "full-eastern"
                    ),
                    "{profile} Eastern giver role {role}"
                );
                assert_eq!(
                    relations.contains(&format!(",400,502,{role})")),
                    matches!(
                        *profile,
                        "instances" | "alliance-single" | "full-world" | "full-instances"
                    ),
                    "{profile} Instance giver role {role}"
                );
                assert_eq!(
                    go_relations.contains(&format!(",300,501,{role})")),
                    matches!(
                        *profile,
                        "alliance-kalimdor"
                            | "starting-kalimdor"
                            | "alliance-single"
                            | "full-world"
                            | "full-kalimdor"
                    ),
                    "{profile} Kalimdor giver role {role}"
                );
            }
        }
    }

    #[test]
    fn full_profiles_share_the_complete_quest_catalogue() {
        let mut expected = None;
        for profile in [
            "full-world",
            "full-eastern",
            "full-kalimdor",
            "full-instances",
        ] {
            let plan = quest_catalogue_plan(profile);
            let shared: Vec<_> = plan
                .stmts
                .iter()
                .filter(|sql| {
                    !sql.contains("game_creature_quest") && !sql.contains("game_gameobject_quest")
                })
                .collect();
            assert!(
                shared
                    .iter()
                    .any(|sql| sql.starts_with("INSERT INTO game_quest_template")
                        && sql.contains("(503,")),
                "{profile} excludes an outlying quest"
            );
            let shared: Vec<String> = shared.into_iter().cloned().collect();
            if let Some(expected) = &expected {
                assert_eq!(&shared, expected, "{profile}");
            } else {
                expected = Some(shared);
            }
        }
    }

    #[test]
    fn full_profiles_import_unplaced_definitions_without_foreign_spawns() {
        let dump = format!(
            "INSERT INTO `creature_template` VALUES {},{},{}; \
             INSERT INTO `creature` VALUES \
             (1,100,0,1,-9000,0,80,0,300,300,0,0),\
             (2,200,1,1,10300,800,1326,0,300,300,0,0);",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            creature_template_row(300, 0),
        );
        let dump = format!("{dump} INSERT INTO `creature_ai_summons` (`id`, `position_x`, `position_y`, `position_z`, `orientation`, `spawntimesecs`) VALUES (700,1,2,3,4,5); INSERT INTO `creature_ai_scripts` VALUES {},{},{};",
            eventai_rule_row(10,100,0,0,100,0,[0,1,0,1,0,0],[[32,300,0,700],[0;4],[0;4]]),
            eventai_rule_row(20,200,0,0,100,0,[0,1,0,1,0,0],[[22,1,0,0],[0;4],[0;4]]),
            eventai_rule_row(30,300,0,0,100,0,[0,1,0,1,0,0],[[22,1,0,0],[0;4],[0;4]]));
        for (profile, expected_spawns) in [
            ("full-world", 2),
            ("full-eastern", 1),
            ("full-kalimdor", 1),
            ("full-instances", 0),
        ] {
            let args = import_args([
                "--dump",
                "synthetic.sql",
                "--family",
                "creatures",
                "--world-profile",
                profile,
            ])
            .unwrap();
            let plan = build_dump_plan(&dump, &args, &None, &None).unwrap();
            assert_eq!(plan.spawn_row_count, expected_spawns, "{profile}");
            let ai = plan.eventai_definition_batches.join("\n");
            assert_eq!(
                ai.contains("entry:100@"),
                matches!(profile, "full-world" | "full-eastern")
            );
            assert_eq!(
                ai.contains("entry:200@"),
                matches!(profile, "full-world" | "full-kalimdor")
            );
            assert_eq!(
                ai.contains("entry:300@"),
                matches!(profile, "full-world" | "full-eastern"),
                "summoned creature AI must follow its summoner"
            );

            let templates = plan
                .stmts
                .iter()
                .filter(|sql| sql.starts_with("INSERT INTO game_creature_template"))
                .cloned()
                .collect::<Vec<_>>()
                .join(";");
            for entry in [100, 200, 300] {
                assert!(
                    templates.contains(&format!("({entry},")),
                    "{profile} template {entry}"
                );
            }
        }
    }

    #[test]
    fn legacy_imports_keep_the_selected_quest_scope() {
        let mut args = test_args();
        args.family = Some("quests".to_owned());
        let plan = build_dump_plan(&quest_catalogue_fixture(), &args, &None, &None).unwrap();
        let templates = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_quest_template"))
            .expect("local quest catalogue");
        assert!(templates.contains("(500,"), "{templates}");
        for quest in [501, 502, 503] {
            assert!(!templates.contains(&format!("({quest},")), "{templates}");
        }
    }

    #[test]
    fn canonical_profiles_import_deeprun_tram_objects_only_on_the_serving_shard() {
        let dump = "INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`) VALUES \
            (500,5,100,'Station Prop',0,0,0,1,0,0,0,0),\
            (501,5,101,'Tunnel Prop',0,0,0,1,0,0,0,0),\
            (502,5,102,'Other Map Prop',0,0,0,1,0,0,0,0); \
            INSERT INTO `gameobject` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`, `rotation0`, `rotation1`, `rotation2`, `rotation3`) VALUES \
            (10,500,369,0,10,20,-5,0,0,0,0,1),\
            (11,501,369,0,10,2200,-5,0,0,0,0,1),\
            (12,502,370,0,10,20,-5,0,0,0,0,1);";

        for (profile, serves_tram) in [
            ("alliance-eastern", true),
            ("starting-eastern", true),
            ("alliance-single", true),
            ("alliance-kalimdor", false),
            ("starting-kalimdor", false),
            ("instances", false),
        ] {
            let args = import_args([
                "--dump",
                "synthetic.sql",
                "--family",
                "gameobjects",
                "--world-profile",
                profile,
            ])
            .expect("profile command");
            let plan = build_dump_plan(dump, &args, &None, &None).expect("profile dump plan");
            let expected_count = if serves_tram { 2 } else { 0 };
            assert_eq!(plan.go_row_count, expected_count, "{profile} spawn count");
            let spawns = plan.go_batches.join(";");
            let templates = plan.stmts.join(";");
            for (entry, display, name, y) in [
                (500, 100, "Station Prop", 20),
                (501, 101, "Tunnel Prop", 2200),
            ] {
                assert_eq!(
                    spawns.contains(&format!(",{entry},369,10,{y},-5,0,0,0,0,0,1")),
                    serves_tram,
                    "{profile} spawn {entry} keeps its map, position and rotation"
                );
                assert!(
                    templates.contains(&format!("({entry},5,{display},'{name}',")),
                    "{profile} includes template {entry}"
                );
            }
            assert!(templates.contains("'Other Map Prop'"), "{profile}");
        }
    }

    #[test]
    fn canonical_eastern_command_keeps_two_disjoint_map_zero_corridors_and_drops_the_gap() {
        let dump = format!(
            "INSERT INTO `creature` VALUES \
             (1,100,0,1,-9000,0,80,0,300,300,0,0),\
             (2,200,0,1,-6200,300,380,0,300,300,0,0),\
             (3,300,0,1,-7500,300,200,0,300,300,0,0),\
             (4,344,0,1,-12000,300,200,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{},{},{}; \
             INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`) VALUES \
             (400,0,1,'Human Door',0,0,0,0,0,0,0,0),\
             (500,0,1,'Dwarf Door',0,0,0,0,0,0,0,0),\
             (600,0,1,'Gap Door',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`, `rotation0`, `rotation1`, `rotation2`, `rotation3`) VALUES \
             (10,400,0,0,-9000,0,80,0,0,0,0,1),\
             (11,500,0,0,-6200,300,380,0,0,0,0,1),\
             (12,600,0,0,-7500,300,200,0,0,0,0,1);",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            creature_template_row(300, 0),
            creature_template_row(344, 0),
        );
        let args = import_args([
            "--dump",
            "synthetic.sql",
            "--world-profile",
            "alliance-eastern",
        ])
        .expect("profile command");
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("profile dump plan");

        let creatures = plan.spawn_batches.join(";");
        assert!(creatures
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("100")));
        assert!(creatures
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("200")));
        assert!(!creatures
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("300")));
        assert!(
            creatures
                .split(';')
                .any(|row| row.split(',').nth(1) == Some("344")),
            "the profile catalogue retains its out-of-slice quest dependency"
        );

        let gameobjects = plan.go_batches.join(";");
        assert!(gameobjects
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("400")));
        assert!(gameobjects
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("500")));
        assert!(!gameobjects
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("600")));
    }

    #[test]
    fn canonical_single_command_keeps_bounded_maps_and_whole_instance_but_fences_forced_rows() {
        let dump = format!(
            "INSERT INTO `creature` VALUES \
             (1,100,0,1,-9000,0,80,0,300,300,0,0),\
             (2,200,1,1,10311,831,1326,0,300,300,0,0),\
             (3,300,36,1,-16,-383,-33,0,300,300,0,0),\
             (4,400,2,1,0,0,0,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{},{},{}; \
             INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`) VALUES \
             (500,0,1,'Eastern Door',0,0,0,0,0,0,0,0),\
             (600,0,1,'Kalimdor Door',0,0,0,0,0,0,0,0),\
             (700,0,1,'Instance Door',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`, `rotation0`, `rotation1`, `rotation2`, `rotation3`) VALUES \
             (10,500,0,0,-9000,0,80,0,0,0,0,1),\
             (11,600,1,0,10311,831,1326,0,0,0,0,1),\
             (12,700,36,0,90000,-90000,-33,0,0,0,0,1);",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            creature_template_row(300, 0),
            creature_template_row(400, 0),
        );
        let args = import_args([
            "--dump",
            "synthetic.sql",
            "--world-profile",
            "alliance-single",
            "--include-creatures",
            "400",
        ])
        .expect("profile command");
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("profile dump plan");

        let creatures = plan.spawn_batches.join(";");
        for entry in ["100", "200", "300"] {
            assert!(
                creatures
                    .split(';')
                    .any(|row| row.split(',').nth(1) == Some(entry)),
                "entry {entry} retained: {creatures}"
            );
        }
        assert!(
            !creatures
                .split(';')
                .any(|row| row.split(',').nth(1) == Some("400")),
            "forced creature on an unplanned map is fenced: {creatures}"
        );

        let gameobjects = plan.go_batches.join(";");
        for entry in ["500", "600", "700"] {
            assert!(
                gameobjects
                    .split(';')
                    .any(|row| row.split(',').nth(1) == Some(entry)),
                "gameobject {entry} retained: {gameobjects}"
            );
        }
    }

    #[test]
    fn trainer_type_and_class_import_from_their_own_columns() {
        let mut paladin = crate::dump::FixtureRow::new("creature_template");
        paladin[ct::ENTRY] = "925".to_string();
        paladin[ct::TRAINER_TYPE] = "0".to_string();
        paladin[ct::TRAINER_CLASS] = "2".to_string();
        paladin["MovementType"] = "66".to_string();
        paladin["TrainerSpell"] = "7".to_string();
        paladin["TrainerRace"] = "9".to_string();
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (1,925,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (2,11867,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (3,620,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES ({}),{},{}; y",
            paladin.join(","),
            creature_template_row_with_trainer(11867, 0, 2, 0),
            creature_template_row_with_trainer(620, 0, 0, 0),
        );
        let args = test_args();
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_template"))
            .unwrap_or_else(|| panic!("template insert present: {:?}", plan.stmts));
        assert!(
            insert.contains("skin_loot_id,trainer_type,trainer_class"),
            "trainer columns are named and end-appended: {insert}"
        );
        let values = insert
            .split_once(" VALUES ")
            .expect("the insert names its columns then VALUES")
            .1;
        let trailing: std::collections::HashMap<&str, (&str, &str)> = values
            .split("),(")
            .map(|t| {
                let t = t
                    .trim_start_matches('(')
                    .trim_end_matches(';')
                    .trim_end_matches(')');
                let f: Vec<&str> = t.split(',').collect();
                (f[0], (f[f.len() - 2], f[f.len() - 1]))
            })
            .collect();
        assert_eq!(
            trailing.get("925"),
            Some(&("0", "2")),
            "the Paladin trainer imports type 0 / class 2 — a slipped index would read 66, 7 or 9: {insert}"
        );
        assert_eq!(
            trailing.get("11867"),
            Some(&("2", "0")),
            "the weapon master imports TRADESKILLS with class 0 (serves everyone): {insert}"
        );
        assert_eq!(
            trailing.get("620"),
            Some(&("0", "0")),
            "an ordinary creature imports 0/0 — the fail-open default: {insert}"
        );
    }

    #[test]
    fn npc_trainer_armor_wrapper_row_imports_verbatim_required_level_and_cost() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (1,925,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `npc_trainer` (`entry`, `spell`, `spellcost`, `reqskill`, `reqskillvalue`, `reqlevel`) VALUES (925,7109,22000,0,0,40); y",
            creature_template_row(925, 0),
        );
        let args = test_args();
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_trainer_spell"))
            .unwrap_or_else(|| panic!("trainer_spell insert present: {:?}", plan.stmts));
        assert!(
            insert.contains(",925,7109,22000,40,0,75)"),
            "wrapper row keeps req level 40 and verbatim cost: {insert}"
        );
    }

    #[test]
    fn include_map_spawns_import_whole_map_with_their_own_map_and_verbatim_z() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (2,644,36,1,-16.4,-383.07,-33.35,1.5,7200,7200,0,0),\
             (3,999,1,1,-16.4,-383.07,-33.35,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{},{}; \
             INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`) VALUES (101,0,446,'Deadmines Door',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`, `rotation0`, `rotation1`, `rotation2`, `rotation3`) VALUES (50,101,36,0,-95.0,-345.0,-30.5,2.0,0,0,0,0); y",
            creature_template_row(100, 0),
            creature_template_row(644, 0),
            creature_template_row(999, 0),
        );
        let args = import_args(["--dump", "synthetic.sql", "--include-map", "36"]).unwrap();
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let spawn_payload = plan.spawn_batches.join(";");
        let rows: Vec<&str> = spawn_payload.split(';').collect();
        let dm_row = rows
            .iter()
            .find(|r| r.split(',').nth(1) == Some("644"))
            .unwrap_or_else(|| panic!("map-36 spawn imported: {spawn_payload}"));
        let f: Vec<&str> = dm_row.split(',').collect();
        assert_eq!(
            f[2], "36",
            "the packed row carries the spawn's OWN map: {dm_row}"
        );
        assert_eq!(
            f[5], "-33.35",
            "the dump Z is verbatim — no snap, no rounding: {dm_row}"
        );
        assert!(
            rows.iter().any(|r| {
                let f: Vec<&str> = r.split(',').collect();
                f.get(1) == Some(&"100") && f.get(2) == Some(&"0")
            }),
            "in-box map-0 spawn still imports: {spawn_payload}"
        );
        assert!(
            !rows.iter().any(|r| r.split(',').nth(1) == Some("999")),
            "a map neither primary nor included must be dropped: {spawn_payload}"
        );
        let go_payload = plan.go_batches.join(";");
        let go_row = go_payload
            .split(';')
            .find(|r| r.split(',').nth(1) == Some("101"))
            .unwrap_or_else(|| panic!("map-36 GO imported: {go_payload}"));
        assert_eq!(
            go_row.split(',').nth(2),
            Some("36"),
            "GO packed row carries map 36: {go_row}"
        );
    }

    #[test]
    fn gameobject_spawn_quaternion_rides_through_the_packed_row_verbatim() {
        let dump =
            "INSERT INTO `gameobject_template` (`entry`, `type`, `displayId`, `name`, `faction`, `flags`, `ExtraFlags`, `size`, `data0`, `data1`, `data2`, `data3`) VALUES (446,0,259,'Wooden Bench',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` (`guid`, `id`, `map`, `spawnMask`, `position_x`, `position_y`, `position_z`, `orientation`, `rotation0`, `rotation1`, `rotation2`, `rotation3`) VALUES \
             (60,446,0,0,-8949.95,-132.493,83.5312,1.57,0.1,0.2,0.70710678,0.70710678);"
                .to_string();
        let args = test_args();
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let go_row = plan
            .go_batches
            .join(";")
            .split(';')
            .find(|r| r.split(',').nth(1) == Some("446"))
            .map(|r| r.to_string())
            .unwrap_or_else(|| panic!("the bench GO must import"));
        let f: Vec<&str> = go_row.split(',').collect();
        assert_eq!(f.len(), 12, "packed row carries all 12 fields: {go_row}");
        assert_eq!(f[6], "1.57", "orientation rides through: {go_row}");
        assert_eq!(f[8], "0.1", "rot0 rides through: {go_row}");
        assert_eq!(f[9], "0.2", "rot1 rides through: {go_row}");
        assert_eq!(f[10], "0.70710678", "rot2 rides through: {go_row}");
        assert_eq!(f[11], "0.70710678", "rot3 rides through: {go_row}");
    }

    #[test]
    fn creature_movement_template_expands_onto_waypoint_spawns_without_direct_rows() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (10,644,36,1,-16.4,-383.07,-33.35,0,300,300,0,2),\
             (11,644,36,1,-20.0,-390.0,-33.35,0,300,300,0,2),\
             (12,645,36,1,-50.0,-400.0,-33.0,0,300,300,0,2); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `creature_movement` (`Id`, `Point`, `PositionX`, `PositionY`, `PositionZ`, `Orientation`, `WaitTime`, `ScriptId`) VALUES (12,1,-51,-401,-33,0,0,0); \
             INSERT INTO `creature_movement_template` (`Entry`, `PathId`, `Point`, `PositionX`, `PositionY`, `PositionZ`, `Orientation`, `WaitTime`, `ScriptId`) VALUES \
             (644,0,2,-18,-385,-33.35,0,0,0),\
             (644,0,1,-17,-384,-33.35,0,0,0),\
             (644,1,1,-99,-99,-99,0,0,0),\
             (645,0,1,-77,-77,-77,0,0,0); y",
            creature_template_row(644, 0),
            creature_template_row(645, 0),
        );
        let mut args = import_args(["--dump", "synthetic.sql", "--include-map", "36"]).unwrap();
        args.family = Some("creatures".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let wp_insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_waypoint"))
            .unwrap_or_else(|| panic!("waypoint insert present: {:?}", plan.stmts));
        assert_eq!(
            wp_insert.matches("-17,-384,-33.35").count(),
            2,
            "point 1 lands on both spawns: {wp_insert}"
        );
        assert_eq!(
            wp_insert.matches("-18,-385,-33.35").count(),
            2,
            "point 2 lands on both spawns: {wp_insert}"
        );
        let g10 = world_guid(644, 10).to_string();
        let p1 = wp_insert
            .find(&format!("{g10},-17,-384"))
            .expect("spawn 10 point 1");
        let p2 = wp_insert
            .find(&format!("{g10},-18,-385"))
            .expect("spawn 10 point 2");
        assert!(p1 < p2, "route order follows the Point column: {wp_insert}");
        assert!(
            !wp_insert.contains("-99,-99"),
            "PathId!=0 must be skipped: {wp_insert}"
        );
        assert!(
            !wp_insert.contains("-77,-77"),
            "direct creature_movement wins over the template: {wp_insert}"
        );
        assert!(
            wp_insert.contains("-51,-401,-33"),
            "the direct 645 row itself imports: {wp_insert}"
        );
    }

    #[test]
    #[ignore = "requires pinned ClassicDB, owned client data, and local SpacetimeDB 2.7.1"]
    fn full_world_source_loads_every_definition_and_spawn_locally() {
        let path = std::env::var("LYRACORE_CLASSIC_DB_SQL").unwrap();
        let client = std::env::var("LYRACORE_TEST_DBC").unwrap();
        let dump = read_dump(&path).unwrap();
        let args = import_args([
            "--dump",
            path.as_str(),
            "--dbc",
            client.as_str(),
            "--world-profile",
            "full-world",
        ])
        .unwrap();
        let display_scales = Some(dbc::load_display_scales(&client).unwrap());
        let tiers = Some(dbc::profession_tier_values(&client).unwrap());
        let plan = build_dump_plan(&dump, &args, &display_scales, &tiers).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(plan.eventai_manifest.as_deref().unwrap()).unwrap();
        assert_eq!(manifest["apply_ready"], true, "{manifest:#}");
        assert_eq!(manifest["counts"]["dropped_rules"], 0);
        assert_eq!(manifest["counts"]["unapproved_result_groups"], 0);
        assert_eq!(plan.spawn_row_count, 66_310);
        assert_eq!(
            plan.stamps
                .iter()
                .find(|(family, _)| *family == "creatures")
                .unwrap()
                .1,
            10_384
        );
        let mut node = standalone_support::Standalone::start("full-world-source");
        node.publish_module();
        node.assert_call("claim_operator", &[]);
        node.assert_call("install_guid_range", &["0"]);
        for statement in dbc::catalogue_statements(&client, &dump).unwrap() {
            node.assert_sql(&statement);
        }
        for statement in spell::build_spell_sql(std::path::Path::new(&client), &[], &[])
            .unwrap()
            .0
        {
            node.assert_sql(&statement);
        }
        for (index, statement) in plan.stmts.iter().enumerate() {
            node.assert_sql(statement);
            if index % 100 == 0 {
                eprintln!(
                    "full-world static catalogue: {index}/{} statements",
                    plan.stmts.len()
                );
            }
        }
        for (index, (reducer, batch)) in plan.reducer_batches(&args).unwrap().iter().enumerate() {
            if index % 50 == 0 {
                eprintln!("full-world reducer batch {index}: {reducer}");
            }
            node.assert_call(reducer, &[&serde_json::to_string(batch).unwrap()]);
        }
        assert_eq!(
            node.query_rows("SELECT entry FROM game_creature_template")
                .len(),
            10_384
        );
        let sources = node.query_rows("SELECT guid, map_id FROM game_creature_spawn WHERE source_guid = 0 AND instance_id = 0");
        assert_eq!(sources.len(), 66_310);
        let maps: std::collections::BTreeSet<_> = sources
            .iter()
            .map(|row| row["map_id"].parse::<u32>().unwrap())
            .collect();
        assert_eq!(maps.len(), 34);
        for map in lyracore_shared::instance::DUNGEON_MAPS
            .iter()
            .chain(lyracore_shared::instance::BATTLEGROUND_MAPS)
        {
            assert!(node
                .query_rows(&format!(
                    "SELECT guid FROM game_world_entity WHERE map_id = {map} AND instance_id = 0"
                ))
                .is_empty());
        }
        assert!(
            standalone_support::poll_until(std::time::Duration::from_secs(180), || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let live = node
                    .query_rows("SELECT guid FROM game_world_entity WHERE instance_id = 0")
                    .len();
                eprintln!("full-world live population: {live}");
                live > 45_000
            }),
            "shared-world population did not materialize within three minutes"
        );
        eprintln!("full-world reimport: resetting the populated roster");
        for (index, batch) in plan.spawn_batches.iter().enumerate() {
            let reducer = if index == 0 {
                "import_creature_spawns"
            } else {
                "import_creature_spawns_append"
            };
            node.assert_call(reducer, &[&serde_json::to_string(batch).unwrap()]);
        }
        assert_eq!(
            node.query_rows(
                "SELECT guid FROM game_creature_spawn WHERE source_guid = 0 AND instance_id = 0"
            )
            .len(),
            66_310
        );
        eprintln!("full-world source verified: 10,384 templates, 66,310 positions, 34 maps");
    }

    #[test]
    #[ignore = "requires pinned LYRACORE_CLASSIC_DB_SQL and build 5875 LYRACORE_TEST_DBC"]
    fn pinned_items_plan_counts_templates_and_property_catalogues() {
        let path = std::env::var("LYRACORE_CLASSIC_DB_SQL").unwrap();
        let dbc = std::env::var("LYRACORE_TEST_DBC").unwrap();
        let dump = read_dump(&path).unwrap();
        let args = import_args([
            "--dump",
            path.as_str(),
            "--dbc",
            dbc.as_str(),
            "--world-profile",
            "alliance-eastern",
            "--family",
            "items",
        ])
        .unwrap();
        let plan = build_dump_plan(&dump, &args, &None, &None).unwrap();
        let (templates, properties, enchantments, pool_members) = (17_718, 2_012, 1_685, 27_892);
        assert_eq!(
            plan.stamps,
            vec![(
                "items",
                templates + properties + enchantments + pool_members
            )]
        );
    }

    #[test]
    #[ignore = "requires pinned LYRACORE_CLASSIC_DB_SQL and build 5875 LYRACORE_TEST_DBC"]
    fn starting_profiles_are_apply_ready_with_owned_inputs() {
        let path = std::env::var("LYRACORE_CLASSIC_DB_SQL").unwrap();
        let dbc = std::env::var("LYRACORE_TEST_DBC").unwrap();
        let dump = read_dump(&path).unwrap();
        for profile in ["starting-eastern", "starting-kalimdor"] {
            let args = import_args([
                "--dump",
                path.as_str(),
                "--dbc",
                dbc.as_str(),
                "--world-profile",
                profile,
            ])
            .unwrap();
            let plan = build_dump_plan(&dump, &args, &None, &None).unwrap();
            let manifest: serde_json::Value =
                serde_json::from_str(plan.eventai_manifest.as_deref().unwrap()).unwrap();
            assert_eq!(manifest["apply_ready"], true, "{profile}: {manifest:#}");
            assert_eq!(manifest["findings"], serde_json::json!([]));
            assert_eq!(manifest["counts"]["unapproved_result_groups"], 0);
            assert_eq!(manifest["counts"]["dropped_rules"], 0);
        }
    }

    #[test]
    #[ignore = "requires pinned LYRACORE_CLASSIC_DB_SQL and build 5875 LYRACORE_TEST_DBC"]
    fn pinned_alliance_single_plan_is_apply_ready_with_relay_static_dependencies() {
        let path = std::env::var("LYRACORE_CLASSIC_DB_SQL").unwrap();
        let dbc = std::env::var("LYRACORE_TEST_DBC").unwrap();
        let dump = read_dump(&path).unwrap();
        let args = import_args([
            "--dump",
            path.as_str(),
            "--dbc",
            dbc.as_str(),
            "--world-profile",
            "alliance-single",
            "--eventai-profile",
            eventai::SOURCE_PROFILE_NAME,
        ])
        .unwrap();
        let plan = build_dump_plan(&dump, &args, &None, &None).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(plan.eventai_manifest.as_deref().unwrap()).unwrap();

        assert_eq!(manifest["apply_ready"], true, "{manifest:#}");
        assert_eq!(manifest["findings"], serde_json::json!([]));
        assert_eq!(manifest["counts"]["unapproved_result_groups"], 0);
        assert_eq!(manifest["counts"]["dropped_rules"], 0);
        assert_eq!(manifest["counts"]["ticket"]["relay_root_rules"], 141);
        assert_eq!(
            manifest["counts"]["ticket"]["relay_accepted_root_rules"],
            141
        );
        assert_eq!(manifest["counts"]["ticket"]["relay_definitions"], 121);
        assert_eq!(
            manifest["counts"]["ticket"]["relay_structural_definitions"],
            120
        );
        assert_eq!(manifest["counts"]["ticket"]["relay_rows"], 451);
        assert_eq!(manifest["counts"]["ticket"]["relay_structural_rows"], 447);
        assert_eq!(manifest["counts"]["ticket"]["relay_emitted_steps"], 449);
        assert_eq!(manifest["counts"]["ticket"]["relay_loader_skipped_rows"], 2);
        assert_eq!(
            manifest["counts"]["ticket"]["text_emote_dependency_occurrences"],
            307
        );
        let result = |action: u32, classification: &str, reason: &str| {
            let raw_value = action.to_string();
            manifest["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|result| {
                    result["dimension"] == "action"
                        && result["raw_value"].as_str() == Some(raw_value.as_str())
                        && result["classification"] == classification
                        && result["reason"] == reason
                })
                .unwrap_or_else(|| {
                    panic!("missing action result {action}/{classification}/{reason}")
                })
        };
        for (action, rules, occurrences) in [
            (10, 3, 3),
            (12, 1, 3),
            (28, 3, 3),
            (41, 6, 6),
            (45, 1, 1),
            (47, 2, 2),
            (50, 3, 3),
            (56, 15, 15),
        ] {
            let emitted = result(action, "emitted", "emitted");
            assert_eq!(emitted["source_rule_ids"].as_array().unwrap().len(), rules);
            assert_eq!(emitted["occurrences"], occurrences);
        }
        let text_new_occurrences = result(54, "emitted", "emitted")["occurrences"]
            .as_u64()
            .unwrap();
        assert_eq!(text_new_occurrences, 62);
        let text_templates = manifest["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|result| {
                result["dimension"] == "dependency"
                    && result["raw_value"] == "eventai_text_template"
                    && result["classification"] == "emitted"
                    && result["reason"] == "resolved"
            })
            .unwrap();
        let text_template_occurrences = text_templates["occurrences"].as_u64().unwrap();
        assert_eq!(text_template_occurrences, 56);
        assert_eq!(text_new_occurrences - text_template_occurrences, 6);
        let sound = result(4, "excluded", "unsupported_sound_playback");
        assert_eq!(
            sound["source_rule_ids"],
            serde_json::json!([
                6901, 29901, 83401, 128401, 192201, 278401, 361601, 793701, 793702, 799901, 1171102
            ])
        );
        assert_eq!(plan.eventai_relay_definition_count, 27);
        assert!(plan.stmts.iter().any(|statement| {
            statement.starts_with("INSERT INTO game_gameobject_trap (entry,spell_id,cooldown_secs)")
                && statement.contains("(180391,24425,0)")
        }));
        assert!(plan.stmts.iter().any(|statement| {
            statement.starts_with("INSERT INTO game_gossip_menu_profile (menu_id,text_id)")
                && ["(6623,", "(6687,", "(6688,", "(6949,"]
                    .iter()
                    .all(|row| statement.contains(row))
        }));
    }

    #[test]
    fn creature_template_import_preserves_source_faction_templates() {
        let dump = format!(
            "INSERT INTO `creature` VALUES \\
             (1,299,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\\
             (2,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \\
             INSERT INTO `creature_template` VALUES {},{};",
            creature_template_row_with_faction(299, 0, 32, 1, 0, 0),
            creature_template_row_with_faction(100, 0, 14, 0, 0, 0),
        );
        let plan = build_dump_plan(&dump, &test_args(), &None, &None).expect("build_dump_plan");
        let insert = plan
            .stmts
            .iter()
            .find(|stmt| stmt.starts_with("INSERT INTO game_creature_template"))
            .expect("creature template insert");
        let factions: std::collections::HashMap<&str, &str> = insert
            .split_once(" VALUES ")
            .expect("template insert values")
            .1
            .split("),(")
            .map(|tuple| {
                let fields: Vec<&str> = tuple
                    .trim_start_matches('(')
                    .trim_end_matches(';')
                    .trim_end_matches(')')
                    .split(',')
                    .collect();
                (fields[0], fields[6])
            })
            .collect();

        assert_eq!(factions.get("299"), Some(&"32"));
        assert_eq!(factions.get("100"), Some(&"14"));
    }

    #[test]
    fn binding_quests_import_source_items_objectives_and_learned_rewards() {
        let mut quest_rows = Vec::new();
        let mut wrapper_rows = Vec::new();
        let mut item_rows = Vec::new();
        for (quest, item, objective, wrapper, learned) in [
            (1689, 6928, 5676, 11520, 697),
            (1739, 6913, 5677, 11519, 712),
        ] {
            let mut row = crate::dump::FixtureRow::new("quest_template");
            row["entry"] = quest.to_string();
            row["Title"] = "'The Binding'".to_string();
            row["SrcItemId"] = item.to_string();
            row["SrcItemCount"] = "1".to_string();
            row["ReqCreatureOrGOId1"] = objective.to_string();
            row["ReqCreatureOrGOCount1"] = "1".to_string();
            row["RewSpell"] = learned.to_string();
            row["RewSpellCast"] = wrapper.to_string();
            quest_rows.push(format!("({})", row.join(",")));
            let mut spell = crate::dump::FixtureRow::new("spell_template");
            spell["Id"] = wrapper.to_string();
            spell["Effect1"] = "36".to_string();
            spell["EffectTriggerSpell1"] = learned.to_string();
            wrapper_rows.push(format!("({})", spell.join(",")));
            let mut item_row = crate::dump::FixtureRow::new("item_template");
            item_row[it::ENTRY] = item.to_string();
            item_row[it::NAME] = "'Binding item'".to_string();
            item_row[it::SPELLID_1] = if item == 6928 { "7728" } else { "8674" }.to_string();
            item_rows.push(format!("({})", item_row.join(",")));
        }
        let dump =
            format!(
            "INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
            INSERT INTO `creature_template` VALUES {}; \
            INSERT INTO `creature_questrelation` VALUES (100,1689),(100,1739); \
            INSERT INTO `quest_template` VALUES {}; \
            INSERT INTO `spell_template` VALUES {}; INSERT INTO `item_template` VALUES {};",
            [100, 5676, 5677, 1863].map(|entry| creature_template_row(entry, 0)).join(","),
            quest_rows.join(","),
            wrapper_rows.join(","),
            item_rows.join(",")
        );
        let plan = build_dump_plan(&dump, &test_args(), &None, &None).unwrap();
        let rewards = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_quest_reward_spell"))
            .unwrap();
        assert!(rewards.contains("(1689,697)"));
        assert!(rewards.contains("(1739,712)"));
        assert!(!rewards.contains("11520"));
        assert!(!rewards.contains("11519"));
        let objectives = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_quest_objective"))
            .unwrap();
        assert!(objectives.contains(",1689,0,0,5676,1)"));
        assert!(objectives.contains(",1739,0,0,5677,1)"));
        let items = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_item_template"))
            .unwrap();
        assert!(items.contains("(6928,"));
        assert!(items.contains("(6913,"));
        assert!(items.contains(",7728,0,"));
        assert!(items.contains(",8674,0,"));
        let creatures = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_template"))
            .unwrap();
        for entry in [5676, 5677, 1863] {
            assert!(creatures.contains(&format!("({entry},")));
        }
        let focus = go_template_row(5_091_040, 8, 1, "Focus", 83, 10, 1.0).0;
        assert!(focus.contains(",83,10,0,0,0,0,1)"));
    }

    #[test]
    fn wisp_visual_template_imports_without_a_world_spawn() {
        let mut row = crate::dump::FixtureRow::new("creature_template");
        row[ct::ENTRY] = "12861".to_string();
        row[ct::NAME] = "'Wisp visual'".to_string();
        row[ct::MODEL_ID1] = "1825".to_string();
        let dump = format!(
            "INSERT INTO `creature_template` VALUES ({});",
            row.join(",")
        );
        let plan = build_dump_plan(&dump, &test_args(), &None, &None).unwrap();
        let templates = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_template"))
            .expect("the wisp body must import without any placed creature");
        assert!(
            templates.contains("(12861,'Wisp visual','',1825,"),
            "{templates}"
        );
    }

    #[test]
    fn quest_reward_cast_precedence_and_single_learn_effect_are_preserved() {
        let mut quests = Vec::new();
        for (quest, reward, cast) in [
            (5091041, 50941, 0),
            (5091042, 50941, 50942),
            (5091043, 50941, 50943),
        ] {
            let mut row = crate::dump::FixtureRow::new("quest_template");
            row["entry"] = quest.to_string();
            row["Title"] = "'Reward fixture'".into();
            row["RewSpell"] = reward.to_string();
            row["RewSpellCast"] = cast.to_string();
            quests.push(format!("({})", row.join(",")));
        }
        let mut spells = Vec::new();
        for (spell, learned, other_effect) in
            [(50941, 50944, 0), (50942, 50945, 0), (50943, 50946, 2)]
        {
            let mut row = crate::dump::FixtureRow::new("spell_template");
            row["Id"] = spell.to_string();
            row["Effect1"] = "36".into();
            row["Effect2"] = other_effect.to_string();
            row["EffectTriggerSpell1"] = learned.to_string();
            spells.push(format!("({})", row.join(",")));
        }
        let dump = format!("INSERT INTO `creature_questrelation` VALUES (51940,5091041),(51940,5091042),(51940,5091043); INSERT INTO `quest_template` VALUES {}; INSERT INTO `spell_template` VALUES {};", quests.join(","), spells.join(","));
        let empty = std::collections::HashSet::new();
        let quests = build_quests(
            &dump,
            &std::collections::HashSet::from([51940]),
            &empty,
            &empty,
            &empty,
            None,
        )
        .unwrap();
        assert_eq!(quests.reward_spells, ["(5091041,50944)", "(5091042,50945)"]);
    }

    fn quest_template_row(entry: u64, title: &str) -> String {
        let mut cols = crate::dump::FixtureRow::new("quest_template");
        cols[qt::ENTRY] = entry.to_string();
        cols[qt::TITLE] = format!("'{title}'");
        format!("({})", cols.join(","))
    }

    fn quest_template_row_with_cast_objective(
        entry: u64,
        creature_entry: u32,
        spell_id: u32,
    ) -> String {
        let mut cols = crate::dump::FixtureRow::new("quest_template");
        cols[qt::ENTRY] = entry.to_string();
        cols[qt::TITLE] = "'Cast objective'".to_string();
        cols[qt::REQ_CREATURE_OR_GO_ID1] = creature_entry.to_string();
        cols[qt::REQ_CREATURE_OR_GO_COUNT1] = "2".to_string();
        cols[qt::REQ_SPELL_CAST1] = spell_id.to_string();
        format!("({})", cols.join(","))
    }

    fn quest_template_row_requiring_event(entry: u64) -> String {
        const QUEST_SPECIAL_FLAG_EXPLORATION_OR_EVENT: u32 = 0x2;
        let mut cols = crate::dump::FixtureRow::new("quest_template");
        cols[qt::ENTRY] = entry.to_string();
        cols[qt::TITLE] = "'Event requirement'".to_string();
        cols[qt::SPECIAL_FLAGS] = QUEST_SPECIAL_FLAG_EXPLORATION_OR_EVENT.to_string();
        format!("({})", cols.join(","))
    }

    fn quest_template_row_with_reward_mail(
        entry: u64,
        title: &str,
        mail_template: u32,
        mail_delay: u32,
    ) -> String {
        let mut cols = crate::dump::FixtureRow::new("quest_template");
        cols["entry"] = entry.to_string();
        cols["Title"] = format!("'{title}'");
        cols["RewMailTemplateId"] = mail_template.to_string();
        cols["RewMailDelaySecs"] = mail_delay.to_string();
        format!("({})", cols.join(","))
    }

    fn dbscripts_send_mail_row(
        quest_entry: u64,
        mail_template: u32,
        sender: u32,
        money: u32,
        delay_secs: u32,
    ) -> String {
        format!(
            "({quest_entry},0,0,38,{mail_template},{sender},{money},0,0,0,{delay_secs},0,0,0,0,0,0,0,0,0,0,'test')"
        )
    }

    #[test]
    fn quest_reward_mail_imports_from_quest_template_and_from_dbscripts() {
        let dump = format!(
            "INSERT INTO `creature_questrelation` VALUES (100,3645),(200,8728); \
             INSERT INTO `quest_template` VALUES {},{}; \
             INSERT INTO `dbscripts_on_quest_end` VALUES {}; \
             INSERT INTO `mail_loot_template` VALUES (99,11423,100,0,1,1,0,'');",
            quest_template_row_with_reward_mail(3645, "Membership Card Renewal", 99, 86400),
            quest_template_row(8728, "The Path to Ahn Qiraj"),
            dbscripts_send_mail_row(8728, 123, 11811, 1_000_000, 129_600),
        );
        let entries = std::collections::HashSet::from([100u64, 200u64]);
        let empty = std::collections::HashSet::new();
        let quests = build_quests(&dump, &entries, &empty, &empty, &empty, None).unwrap();
        assert_eq!(quests.reward_mail.len(), 2);
        assert!(
            quests
                .reward_mail
                .contains(&"(3645,99,86400,0,0)".to_string()),
            "{:?}",
            quests.reward_mail
        );
        assert!(
            quests
                .reward_mail
                .contains(&"(8728,123,129600,11811,1000000)".to_string()),
            "{:?}",
            quests.reward_mail
        );
        assert_eq!(
            quests.reward_mail_template_ids,
            std::collections::HashSet::from([99, 123])
        );

        let (mail_loot_rows, mail_loot_items) =
            build_mail_loot(&dump, &quests.reward_mail_template_ids).unwrap();
        assert_eq!(mail_loot_rows, ["(99,11423,1)"]);
        assert_eq!(mail_loot_items, std::collections::HashSet::from([11423u64]));
    }

    #[test]
    fn quest_reward_mail_from_both_sources_is_refused() {
        let dump = format!(
            "INSERT INTO `creature_questrelation` VALUES (100,500); \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `dbscripts_on_quest_end` VALUES {};",
            quest_template_row_with_reward_mail(500, "Conflict", 7, 3600),
            dbscripts_send_mail_row(500, 8, 0, 0, 3600),
        );
        let entries = std::collections::HashSet::from([100u64]);
        let empty = std::collections::HashSet::new();
        let err = build_quests(&dump, &entries, &empty, &empty, &empty, None)
            .err()
            .unwrap();
        assert!(err.to_string().contains("500"), "{err}");
    }

    #[test]
    fn dbscripts_reward_mail_is_scoped_to_in_scope_quests_like_every_other_quest_table() {
        let dump = format!(
            "INSERT INTO `dbscripts_on_quest_end` VALUES {};",
            dbscripts_send_mail_row(9999, 7, 0, 0, 3600),
        );
        let empty = std::collections::HashSet::new();
        let quests = build_quests(&dump, &empty, &empty, &empty, &empty, None).unwrap();
        assert!(quests.reward_mail.is_empty(), "{:?}", quests.reward_mail);
    }

    #[test]
    fn mail_loot_template_skips_a_row_whose_template_no_reward_mail_names() {
        let dump = "INSERT INTO `mail_loot_template` VALUES (99,11423,100,0,1,1,0,'');";
        let (rows, items) = build_mail_loot(dump, &std::collections::HashSet::new()).unwrap();
        assert!(rows.is_empty());
        assert!(items.is_empty());
    }

    #[test]
    fn mail_loot_template_fail_loud_shapes_name_the_table_and_the_row() {
        let ids = std::collections::HashSet::from([99u64]);
        for (dump, expect) in [
            (
                "INSERT INTO `mail_loot_template` VALUES (99,11423,50,0,1,1,0,'');",
                "chance",
            ),
            (
                "INSERT INTO `mail_loot_template` VALUES (99,11423,100,1,1,1,0,'');",
                "group",
            ),
            (
                "INSERT INTO `mail_loot_template` VALUES (99,11423,100,0,-5,1,0,'');",
                "references another loot group",
            ),
            (
                "INSERT INTO `mail_loot_template` VALUES (99,11423,100,0,1,2,0,'');",
                "count other than 1",
            ),
            (
                "INSERT INTO `mail_loot_template` VALUES (99,11423,100,0,1,1,7,'');",
                "condition",
            ),
            (
                "INSERT INTO `mail_loot_template` VALUES (99,11423,100,0,1,1,0,''),(99,17685,100,0,1,1,0,'');",
                "more than one loot row",
            ),
        ] {
            let err = build_mail_loot(dump, &ids).unwrap_err().to_string();
            assert!(err.contains("mail_loot_template"), "{err}");
            assert!(err.contains("99"), "{err}");
            assert!(err.contains(expect), "{err}");
        }
    }

    #[test]
    fn build_items_and_loot_fails_loud_when_a_mail_loot_item_has_no_template() {
        let mail_items = std::collections::HashSet::from([11423u64]);
        let err = build_items_and_loot(
            "",
            &std::collections::HashMap::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mail_items,
        )
        .unwrap_err();
        assert!(err.to_string().contains("11423"), "{err}");
    }

    #[test]
    fn quest_import_keeps_spell_gated_objective_metadata_private() {
        let dump = format!(
            "INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES (100,500);",
            creature_template_row(100, 0),
            quest_template_row_with_cast_objective(500, 12297, 456),
        );
        let plan = build_dump_plan(&dump, &test_args(), &None, &None).unwrap();
        let objectives = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_quest_objective"))
            .unwrap();
        assert!(objectives.contains("(1,500,0,0,12297,2)"), "{objectives}");
        let casts = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_quest_cast_objective"))
            .unwrap();
        assert!(casts.contains("(1,500,0,456)"), "{casts}");
    }

    #[test]
    fn quest_import_does_not_treat_every_special_flag_event_as_eventai_credit() {
        let dump = format!(
            "INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES (100,500);",
            creature_template_row(100, 0),
            quest_template_row_requiring_event(500),
        );
        let plan = build_dump_plan(&dump, &test_args(), &None, &None).unwrap();
        assert!(plan
            .stmts
            .iter()
            .all(|statement| !statement.starts_with("INSERT INTO game_quest_event_requirement")));
    }

    #[test]
    fn eventai_import_owns_requirements_for_accepted_quest_events() {
        let dump = format!(
            "INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES (100,500); \
             INSERT INTO `creature_ai_scripts` VALUES {};",
            creature_template_row(100, 0),
            quest_template_row_requiring_event(500),
            eventai_rule_row(
                10,
                100,
                4,
                0,
                100,
                0,
                [0; 6],
                [[15, 500, 6, 0], [0; 4], [0; 4]],
            ),
        );
        let mut args = test_args();
        args.family = Some("creature-ai".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).unwrap();
        let requirements = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_quest_event_requirement"))
            .expect("EventAI event requirement row");
        assert!(requirements.contains("(1,500)"), "{requirements}");
    }

    fn quest_template_row_chained_timed(
        entry: u64,
        title: &str,
        next_quest: u64,
        limit_time: u32,
    ) -> String {
        let mut cols = crate::dump::FixtureRow::new("quest_template");
        cols[qt::ENTRY] = entry.to_string();
        cols[qt::TITLE] = format!("'{title}'");
        cols[qt::NEXT_QUEST_IN_CHAIN] = next_quest.to_string();
        cols[qt::LIMIT_TIME] = limit_time.to_string();
        cols[qt::REW_MONEY_MAX_LEVEL] = "777".to_string();
        format!("({})", cols.join(","))
    }

    fn classicdb_quest_783_row() -> String {
        let mut cols = crate::dump::FixtureRow::new("quest_template");
        for (index, value) in [
            ("entry", "783"),
            ("Method", "2"),
            ("ZoneOrSort", "9"),
            ("MinLevel", "1"),
            ("MaxLevel", "255"),
            ("QuestLevel", "1"),
            ("RequiredRaces", "77"),
            ("QuestFlags", "8"),
            ("BreadcrumbForQuestId", "0"),
            ("NextQuestInChain", "7"),
            ("Title", "'A Threat Within'"),
        ] {
            cols[index] = value.to_string();
        }
        format!("({})", cols.join(","))
    }

    fn tiny_quest_dump(creature_entry: u64, quest_entry: u64) -> String {
        format!(
            "x INSERT INTO `creature` VALUES (1,{creature_entry},0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES ({creature_entry},{quest_entry}); y",
            creature_template_row(creature_entry, 0),
            quest_template_row(quest_entry, "Test Quest"),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn eventai_rule_row(
        id: u64,
        subject: i64,
        event: u32,
        inverse_phase_mask: u32,
        chance: u32,
        flags: u32,
        params: [u32; 6],
        actions: [[i64; 4]; 3],
    ) -> String {
        let mut row = crate::dump::FixtureRow::new("creature_ai_scripts");
        for (name, value) in [
            ("id", id.to_string()),
            ("creature_id", subject.to_string()),
            ("event_type", event.to_string()),
            ("event_inverse_phase_mask", inverse_phase_mask.to_string()),
            ("event_chance", chance.to_string()),
            ("event_flags", flags.to_string()),
        ] {
            row[name] = value;
        }
        for (index, value) in params.into_iter().enumerate() {
            row[&format!("event_param{}", index + 1)] = value.to_string();
        }
        for (index, action) in actions.into_iter().enumerate() {
            row[&format!("action{}_type", index + 1)] = action[0].to_string();
            for (param, value) in action[1..].iter().enumerate() {
                row[&format!("action{}_param{}", index + 1, param + 1)] = value.to_string();
            }
        }
        format!("({})", row.join(","))
    }

    fn creature_template_row_with_unit_flags(entry: u64, unit_flags: u32) -> String {
        let mut cols = crate::dump::FixtureRow::new("creature_template");
        cols[ct::ENTRY] = entry.to_string();
        cols[ct::UNIT_FLAGS] = unit_flags.to_string();
        format!("({})", cols.join(","))
    }

    fn eventai_broadcast_row(id: u32, male: &str, female: &str) -> String {
        format!("({id},'{male}','{female}',1,0,0,0,0,0,0,5,6,7,8,9,10,0)")
    }

    fn eventai_fixture_dump() -> String {
        let mut rules = vec![
            eventai_rule_row(
                10,
                100,
                0,
                64,
                100,
                0x421,
                [1_000, 2_000, 3_000, 4_000, 0, 0],
                [[1, 900, 901, 902], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                11,
                100,
                2,
                0,
                75,
                0,
                [80, 20, 100, 200, 0, 0],
                [[11, 42, 1, 0xA21], [5, 7, 0, 0], [25, 0, 0, 0]],
            ),
            eventai_rule_row(
                12,
                100,
                14,
                0,
                100,
                0,
                [500, 30, 100, 200, 0, 0],
                [[32, 200, 17, 700], [11, 42, 12, 0], [39, 40, 0, 0]],
            ),
            eventai_rule_row(
                13,
                100,
                11,
                0,
                100,
                0,
                [0; 6],
                [[22, 3, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                14,
                100,
                9,
                0,
                100,
                0,
                [5, 20, 100, 200, 0, 0],
                [[29, 15, -45, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                15,
                100,
                4,
                0,
                100,
                0,
                [0; 6],
                [[54, 900, 1, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                16,
                -2,
                6,
                0,
                100,
                0,
                [0; 6],
                [[1, -1, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                17,
                200,
                11,
                0,
                100,
                0,
                [0; 6],
                [[1, 901, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
        ];
        for (offset, target) in [0, 2, 4, 6, 8, 12, 18].into_iter().enumerate() {
            rules.push(eventai_rule_row(
                20 + offset as u64,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, target, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ));
        }
        let second_insert = rules.split_off(8);
        format!(
            "x INSERT INTO `creature` VALUES \
             (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (2,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `spell_template` (`Id`) VALUES (42); \
             INSERT INTO `broadcast_text` VALUES {},{},{},{}; \
             INSERT INTO `script_texts` (`entry`, `content_default`, `content_loc1`, `content_loc2`, `content_loc3`, `content_loc4`, `content_loc5`, `content_loc6`, `content_loc7`, `content_loc8`, `sound`, `type`, `language`, `emote`, `broadcast_text_id`) VALUES (-1,'Old\\'s','','','','','','','','',0,1,0,2,0); \
             INSERT INTO `creature_ai_summons` (`id`, `position_x`, `position_y`, `position_z`, `orientation`, `spawntimesecs`) VALUES (700,1,2,3,4,5); \
             INSERT INTO `creature_ai_scripts` VALUES {}; \
             INSERT INTO `creature_ai_scripts` VALUES {}; y",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            eventai_broadcast_row(900, "Hogger\\'s", "Hoggerette\\'s"),
            eventai_broadcast_row(901, "One", "One"),
            eventai_broadcast_row(902, "Two", "Two"),
            eventai_broadcast_row(903, "Unused", "Unused"),
            rules.join(","),
            second_insert.join(","),
        )
    }

    #[test]
    fn eventai_plan_keeps_source_rule_groups_and_native_mappings() {
        let dump = eventai_fixture_dump();
        let mut args = test_args();
        args.family = Some("creature-ai".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("EventAI plan");
        assert_eq!(plan.stamps, vec![("creature-ai", 8)]);
        assert_eq!(plan.eventai_definition_count, 3);
        assert_eq!(plan.eventai_instruction_count, 17);
        assert_eq!(plan.stmts.len(), 6, "{:?}", plan.stmts);
        assert_eq!(
            &plan.stmts[..4],
            [
                "DELETE FROM game_world_state_name WHERE variable_id >= 0",
                "DELETE FROM game_creature_ai_broadcast_text WHERE id > 0",
                "DELETE FROM game_creature_ai_summon WHERE id > 0",
                "DELETE FROM game_quest_event_requirement WHERE id > 0",
            ],
            "the family replaces its dependency catalogues"
        );
        assert!(plan.stmts.iter().all(|statement| {
            statement.contains("game_world_state_name")
                || statement.contains("game_creature_ai_broadcast_text")
                || statement.contains("game_creature_ai_summon")
                || statement.contains("game_quest_event_requirement")
        }));
        assert!(!plan.stmts.iter().any(|statement| {
            statement.contains("game_creature_ai_event")
                || statement.contains("game_creature_ai_definition")
                || statement.contains("game_creature_ai_state")
                || statement.contains("game_creature_ai_rule_state")
        }));

        let definitions = plan.eventai_definition_batches.join("\n");
        assert!(
            definitions.contains("entry:100@")
                && definitions.contains(&format!("guid:{}@", world_guid(100, 2)))
                && definitions.contains("entry:200@"),
            "{definitions}"
        );
        assert!(
            definitions.contains(
                "10,timer-combat:1000:2000,100,4294967231,repeat:3000:4000,random,combat,any-posture,speak:yell:self:900.901.902"
            ),
            "{definitions}"
        );
        assert!(
            definitions.contains(
                "11,health:20:80:0,75,4294967295,once,all,ordinary,any-posture,cast:42:opponent:1:direct:actor:selected:1:1:1:0:0+emote:7:self+flee"
            ),
            "{definitions}"
        );
        assert!(
            definitions.contains(
                "14,range:5:20,100,4294967295,once,all,ordinary,any-posture,posture:15:-45"
            ),
            "{definitions}"
        );
        assert!(
            definitions.contains(
                "15,aggro,100,4294967295,once,all,ordinary,any-posture,speak:yell:opponent:900"
            ),
            "{definitions}"
        );
        let texts = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_creature_ai_broadcast_text"))
            .expect("EventAI texts");
        assert!(texts.contains("Hogger''s"), "{texts}");
        assert!(texts.contains("Old''s"), "{texts}");
        assert!(!texts.contains("Unused"), "{texts}");
        assert!(texts.contains(",8,5,9,6,10,7)"), "{texts}");
        let manifest = plan.eventai_manifest.as_deref().expect("manifest");
        assert!(manifest.contains("\"source_rules\": 15"), "{manifest}");
        assert!(
            manifest.contains("\"emitted_instructions\": 17"),
            "{manifest}"
        );
        assert!(manifest.contains("\"apply_ready\": false"), "{manifest}");

        let again = build_dump_plan(&dump, &args, &None, &None).expect("repeat EventAI plan");
        assert_eq!(plan.stmts, again.stmts);
        assert_eq!(
            plan.eventai_definition_batches,
            again.eventai_definition_batches
        );
        assert_eq!(plan.eventai_manifest, again.eventai_manifest);
    }

    #[test]
    fn eventai_definition_uses_entry_or_remapped_guid_subject() {
        let dump = eventai_fixture_dump();
        let mut args = test_args();
        args.family = Some("creature-ai".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("EventAI plan");
        let definitions = plan.eventai_definition_batches.join("\n");

        assert!(
            definitions
                .lines()
                .any(|definition| definition.starts_with(&format!("guid:{}@", world_guid(100, 2)))),
            "{definitions}"
        );
        assert!(
            definitions
                .lines()
                .any(|definition| definition.starts_with("entry:200@")),
            "{definitions}"
        );
    }

    #[test]
    fn eventai_apply_refuses_during_pure_plan_construction() {
        let dump = eventai_fixture_dump();
        let mut args = test_args();
        args.family = Some("creature-ai".to_string());
        args.apply = true;

        let error = build_dump_plan(&dump, &args, &None, &None)
            .err()
            .expect("the compact dump cannot satisfy the full source profile")
            .to_string();

        assert!(
            error.contains("compatibility manifest is not approved"),
            "{error}"
        );
        assert!(error.contains("sql_sha256 expected="), "{error}");
        assert!(
            error.contains("source_rule_count expected=10843 observed=15"),
            "{error}"
        );
    }

    #[test]
    fn nonselectable_creatures_keep_their_spawns_flags_and_eventai_rules() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `broadcast_text` VALUES {}; \
             INSERT INTO `creature_ai_scripts` VALUES {}; y",
            creature_template_row_with_unit_flags(100, 0x0200_0000),
            eventai_broadcast_row(900, "Trigger", "Trigger"),
            eventai_rule_row(
                10,
                100,
                4,
                0,
                100,
                0,
                [0; 6],
                [[1, 900, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
        );
        let plan = build_dump_plan(&dump, &test_args(), &None, &None).expect("full plan");
        assert!(
            plan.stmts
                .iter()
                .any(|statement| statement.starts_with("INSERT INTO game_creature_template")),
            "{:?}",
            plan.stmts
        );
        assert_eq!(plan.spawn_row_count, 1);
        assert!(plan.stmts.iter().any(|sql| sql.contains(",33554432,")));
        assert!(
            plan.eventai_definition_batches
                .iter()
                .any(|batch| batch.contains("entry:100@")),
            "{:?}",
            plan.eventai_definition_batches
        );
    }

    #[test]
    fn eventai_inactive_family_does_not_extend_quest_scope() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `quest_template` VALUES {},{}; \
             INSERT INTO `creature_questrelation` VALUES (100,500),(200,501); \
             INSERT INTO `creature_ai_summons` (`id`, `position_x`, `position_y`, `position_z`, `orientation`, `spawntimesecs`) VALUES (700,1,2,3,4,5); \
             INSERT INTO `creature_ai_scripts` VALUES {}; y",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            quest_template_row(500, "In scope"),
            quest_template_row(501, "Summon only"),
            eventai_rule_row(
                10,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[32, 200, 0, 700], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
        );
        let mut args = test_args();
        args.family = Some("quests".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("quest-only plan");
        let quests = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_quest_template"))
            .expect("quest rows");
        assert!(quests.contains("(500,"), "{quests}");
        assert!(!quests.contains("(501,"), "{quests}");
        assert_eq!(plan.stamps, vec![("quests", 1)]);
    }

    #[test]
    fn eventai_drops_invalid_rules_into_explicit_buckets() {
        let mut dump = eventai_fixture_dump().replacen(
            "(903,'Unused','Unused',1,0,0,0,0,0,0,5,6,7,8,9,10,0)",
            "(903,'Unused','Unused',2,0,0,0,0,0,0,5,6,7,8,9,10,0)",
            1,
        );
        let invalid = [
            eventai_rule_row(40, 100, 99, 0, 100, 0, [0; 6], [[1, 900, 0, 0]; 3]),
            eventai_rule_row(41, 100, 0, 0, 0, 0, [0, 1, 0, 1, 0, 0], [[1, 900, 0, 0]; 3]),
            eventai_rule_row(
                42,
                100,
                0,
                u32::MAX,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 900, 0, 0]; 3],
            ),
            eventai_rule_row(
                43,
                100,
                0,
                0,
                100,
                64,
                [0, 1, 0, 1, 0, 0],
                [[1, 900, 0, 0]; 3],
            ),
            eventai_rule_row(
                44,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[99, 0, 0, 0]; 3],
            ),
            eventai_rule_row(
                45,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 999, 0, 0]; 3],
            ),
            eventai_rule_row(
                46,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 0, 0, 0]; 3],
            ),
            eventai_rule_row(
                47,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, 99, 0]; 3],
            ),
            eventai_rule_row(
                48,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[32, 999, 0, 700]; 3],
            ),
            eventai_rule_row(
                49,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[32, 200, 0, 999]; 3],
            ),
            eventai_rule_row(
                50,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 4_294_967_296, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                51,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[54, 900, 0, 77], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                52,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 903, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                53,
                100,
                14,
                0,
                100,
                0,
                [1, 30, 0, 1, 0, 0],
                [[11, 42, 10, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                54,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, 1, 0x2], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                55,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, 1, 0x4], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
        ];
        let mut invalid_id = crate::dump::FixtureRow::new("creature_ai_scripts");
        invalid_id["id"] = "-1".into();
        dump = dump.replacen(
            "; y",
            &format!(",{},({}); y", invalid.join(","), invalid_id.join(",")),
            1,
        );
        let entries = [100].into_iter().collect();
        let guids = [(1, 100), (2, 100)].into_iter().collect();
        let templates = [100, 200].into_iter().collect();
        let plan = eventai::parse(&dump)
            .unwrap()
            .assemble(&entries, &guids, &templates);
        for (reason, value) in [
            ("unsupported_event", 99),
            ("invalid_chance", 0),
            ("empty_phase_mask", u32::MAX as u64),
            ("unsupported_flag", 64),
            ("unsupported_action", 99),
            ("missing_broadcast_text", 999),
            ("invalid_spell", 0),
            ("unsupported_target", 99),
            ("missing_summon_creature", 999),
            ("missing_summon_location", 999),
            ("invalid_numeric", 0),
        ] {
            assert_eq!(plan.dropped(reason, value), 1, "{reason}/{value}");
        }
        assert!(plan
            .definition_rows
            .iter()
            .any(|row| row.contains("no-effect:missing-text-template:77")));
        assert!(plan
            .definition_rows
            .iter()
            .any(|row| row.contains("speak:emote:self:903")));
        assert_eq!(plan.event_counts(99), (1, 0, 1, 0));
        assert_eq!(plan.action_counts(99), (3, 0, 3, 0));
    }

    #[test]
    fn family_quests_filter_produces_only_quest_family_stmts() {
        let dump = tiny_quest_dump(100, 500);
        let mut args = test_args();
        args.family = Some("quests".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");

        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("DELETE FROM game_quest_template")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_quest_template") && s.contains("500")),
            "{:?}",
            plan.stmts
        );
        let non_quest_markers = [
            "game_creature_template",
            "game_creature_waypoint",
            "game_item_template",
            "game_creature_loot",
            "game_npc_vendor",
            "game_pickpocket_loot",
            "game_skinning_loot",
            "game_gameobject_loot",
            "game_fishing_loot",
            "game_gossip_menu",
            "game_npc_text",
            "game_gossip_option",
            "game_gameobject_template",
            "game_trainer_spell",
            "game_creature_cast",
            "game_creature_spell",
            "game_level_stats",
            "game_start_position",
            "game_graveyard_zone",
            "game_createinfo_spell",
            "game_createinfo_action",
            "game_spell_chain",
            "game_spell_learn",
            "game_spell_proc_event",
        ];
        for marker in non_quest_markers {
            assert!(
                !plan.stmts.iter().any(|s| s.contains(marker)),
                "family=quests must not touch {marker}: {:?}",
                plan.stmts
            );
        }
        assert_eq!(plan.stamps, vec![("quests", 1u64)], "{:?}", plan.stamps);
    }

    #[test]
    fn chained_and_timed_quest_columns_thread_through_the_full_import() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES (100,500); y",
            creature_template_row(100, 0),
            quest_template_row_chained_timed(500, "Chained Timed Quest", 501, 3600),
        );
        let mut args = test_args();
        args.family = Some("quests".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_quest_template") && s.contains("500"))
            .unwrap_or_else(|| panic!("quest 500 row present: {:?}", plan.stmts));
        assert!(insert.contains(",501,3600,777"), "next_quest_id=501, limit_time=3600, reward_money_max_level=777 must thread through, in order, as END-appended columns: {insert}");
    }

    #[test]
    fn classicdb_quest_783_uses_next_quest_in_chain_after_breadcrumb() {
        let dump = format!(
            "INSERT INTO `creature` VALUES (1,823,0,1,-8933.54,-136.523,83.4466,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES (823,783);",
            creature_template_row(823, 0),
            classicdb_quest_783_row(),
        );
        let mut args = test_args();
        args.family = Some("quests".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("quest import plan");
        let insert = plan
            .stmts
            .iter()
            .find(|statement| {
                statement.starts_with("INSERT INTO game_quest_template")
                    && statement.contains("(783,")
            })
            .expect("quest 783 insert");
        assert!(
            insert.contains(",false,7,0,"),
            "quest 783 must retain its forward link to quest 7: {insert}"
        );
    }

    #[test]
    fn quest_type_column_reads_into_quest_type_and_counts_raid_quests() {
        let normal_row = {
            let mut row = crate::dump::FixtureRow::new("quest_template");
            row["entry"] = "500".to_string();
            row["Title"] = "'Normal quest'".to_string();
            format!("({})", row.join(","))
        };
        let raid_row = {
            let mut row = crate::dump::FixtureRow::new("quest_template");
            row["entry"] = "501".to_string();
            row["Title"] = "'Raid quest'".to_string();
            row["Type"] = "62".to_string();
            format!("({})", row.join(","))
        };
        let dump = format!(
            "INSERT INTO `creature_questrelation` VALUES (51940,500),(51940,501); \
             INSERT INTO `quest_template` VALUES {normal_row},{raid_row};"
        );
        let entries = std::collections::HashSet::from([51940u64]);
        let empty = std::collections::HashSet::new();
        let quests = build_quests(&dump, &entries, &empty, &empty, &empty, None).unwrap();
        assert!(
            quests
                .templates
                .iter()
                .any(|row| row.starts_with("(500,") && row.ends_with(",0,0)")),
            "normal quest must carry quest_type 0: {:?}",
            quests.templates
        );
        assert!(
            quests
                .templates
                .iter()
                .any(|row| row.starts_with("(501,") && row.ends_with(",0,62)")),
            "type-62 quest must import as quest_type 62: {:?}",
            quests.templates
        );
        assert_eq!(quests.raid_quest_count, 1);
    }

    fn classicdb_quest_3645_row() -> String {
        let mut cols = crate::dump::FixtureRow::new("quest_template");
        for (index, value) in [
            ("entry", "3645"),
            ("Title", "'Membership Card Renewal'"),
            ("RewMailTemplateId", "99"),
            ("RewMailDelaySecs", "86400"),
        ] {
            cols[index] = value.to_string();
        }
        format!("({})", cols.join(","))
    }

    #[test]
    fn classicdb_quest_3645_reward_mail_lands_on_the_real_dump_columns() {
        let dump = format!(
            "INSERT INTO `creature_questrelation` VALUES (100,3645); \
             INSERT INTO `quest_template` VALUES {};",
            classicdb_quest_3645_row(),
        );
        let entries = std::collections::HashSet::from([100u64]);
        let empty = std::collections::HashSet::new();
        let quests = build_quests(&dump, &entries, &empty, &empty, &empty, None).unwrap();
        assert_eq!(quests.reward_mail, ["(3645,99,86400,0,0)"]);
    }

    #[test]
    fn family_absent_full_run_is_the_union_of_every_single_family_run() {
        let dump = tiny_quest_dump(101, 501);
        let mut full_args = test_args();
        full_args.family = None;
        let full_plan = build_dump_plan(&dump, &full_args, &None, &None).expect("full run");
        let full_set: std::collections::HashSet<&String> = full_plan.stmts.iter().collect();

        for &family in FAMILIES {
            let mut args = test_args();
            args.family = Some(family.to_string());
            let plan = build_dump_plan(&dump, &args, &None, &None)
                .unwrap_or_else(|e| panic!("family {family} failed: {e}"));
            for s in &plan.stmts {
                assert!(
                    full_set.contains(s),
                    "full run is missing a {family} statement: {s}"
                );
            }
            assert_eq!(
                plan.stamps.len(),
                1,
                "family {family} plan.stamps: {:?}",
                plan.stamps
            );
            assert_eq!(plan.stamps[0].0, family);
        }
        let stamped_families: Vec<&str> = full_plan.stamps.iter().map(|(f, _)| *f).collect();
        assert_eq!(
            stamped_families,
            FAMILIES.to_vec(),
            "{:?}",
            full_plan.stamps
        );

        let full_plan_again =
            build_dump_plan(&dump, &full_args, &None, &None).expect("full run again");
        assert_eq!(
            full_plan.stmts, full_plan_again.stmts,
            "build_dump_plan must be deterministic"
        );
    }
    fn tuple_count(insert: &str) -> usize {
        insert.matches("),(").count() + 1
    }

    #[test]
    fn stats_refuse_non_numeric_keys_before_emitting_sql() {
        for (dump, column) in [
            ("INSERT INTO `player_classlevelstats` VALUES (1,1,20,0),('warrior',2,29,0);", "player_classlevelstats.class"),
            ("INSERT INTO `player_classlevelstats` VALUES (1,'two',29,0);", "player_classlevelstats.level"),
            ("INSERT INTO `player_levelstats` VALUES (1,1,1,23,20,22,20,21),(1,1,'x',1,1,1,1,1);", "player_levelstats.level"),
        ] {
            assert!(build_stats_sql(dump).unwrap_err().to_string().contains(column));
        }
    }

    #[test]
    fn stats_refuse_rows_missing_value_columns() {
        let dump = "INSERT INTO `player_classlevelstats` VALUES (1,1);";
        assert!(dump.rows("player_classlevelstats").is_err());
    }

    #[test]
    fn build_stats_sql_empty_dump_yields_deletes_only_no_insert() {
        let stmts = build_stats_sql("").unwrap();
        assert_eq!(
            stmts,
            vec![
                "DELETE FROM game_class_level_stats WHERE class_level >= 0".to_string(),
                "DELETE FROM game_level_stats WHERE race_class_level >= 0".to_string(),
            ],
            "an empty dump must yield the two DELETEs and nothing else"
        );
    }

    #[test]
    fn graveyard_links_refuse_a_non_numeric_zone() {
        let dump = "CREATE TABLE `game_graveyard_zone` (\n`id` int,\n`ghost_loc` int,\n`link_kind` int,\n`faction` int\n); INSERT INTO `game_graveyard_zone` VALUES (105,'zone',0,469);";
        let error = build_graveyard_zone_sql(dump).unwrap_err();
        assert!(error.to_string().contains("game_graveyard_zone.ghost_loc"));
    }

    #[test]
    fn build_graveyard_zone_sql_empty_dump_yields_delete_only_no_insert() {
        let stmts = build_graveyard_zone_sql("").unwrap();
        assert_eq!(
            stmts,
            vec!["DELETE FROM game_graveyard_zone WHERE row_id >= 0".to_string()],
            "an empty dump must wipe and reload nothing, not emit an empty INSERT"
        );
    }

    #[test]
    fn spell_chain_drops_zero_ids_and_out_of_range_ranks() {
        let dump =
            "INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(145,133,133,300,0),(0,0,0,1,0);";
        let stmts = build_spell_chain_sql(dump, &[]).unwrap();
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_chain "))
            .expect("the one well-formed chain row must still import");
        assert_eq!(
            tuple_count(insert),
            1,
            "out-of-range ranks and zero spell ids drop: {insert}"
        );
        assert!(insert.contains("(133,0,133,1,0)"), "{insert}");
    }

    #[test]
    fn createinfo_spells_refuse_non_numeric_values_before_emitting_sql() {
        for (values, column) in [
            ("(0,1,78),('bad',0,0)", "race"),
            ("(0,1,78),(3,1,'fireball')", "Spell"),
        ] {
            let dump = format!(
                "INSERT INTO `playercreateinfo_spell` (`race`, `class`, `Spell`) VALUES {values};"
            );
            assert!(build_createinfo_spell_sql(&dump)
                .unwrap_err()
                .to_string()
                .contains(&format!("playercreateinfo_spell.{column}")));
        }
    }

    #[test]
    fn spell_chain_refuses_non_numeric_values_before_emitting_sql() {
        for (values, column) in [
            ("(133,0,133,1,0),(143,133,133,'bad',0)", "rank"),
            ("(133,0,133,1,0),(147,133,133,2,'x')", "req_spell"),
        ] {
            let dump = format!("INSERT INTO `spell_chain` VALUES {values};");
            assert!(build_spell_chain_sql(&dump, &[])
                .unwrap_err()
                .to_string()
                .contains(&format!("spell_chain.{column}")));
        }
    }

    #[test]
    fn build_createinfo_spell_sql_empty_dump_yields_delete_only_no_insert() {
        let stmts = build_createinfo_spell_sql("").unwrap();
        assert_eq!(
            stmts,
            vec!["DELETE FROM game_createinfo_spell WHERE id >= 0".to_string()]
        );
    }
    #[test]
    fn vmangos_family_help_names_every_supported_family() {
        let source = crate::world_source::sqlite_fixture(&[]).unwrap();
        let mut args = import_args([
            "--dump",
            "unused",
            "--source",
            "vmangos",
            "--family",
            "creature-ai",
        ])
        .unwrap();
        args.family = Some("unknown".into());
        let error = build_vmangos_plan(&source, &args)
            .err()
            .unwrap()
            .to_string();
        for family in [
            "creatures",
            "casts",
            "creature-ai",
            "dbc",
            "globals",
            "items",
            "loot",
            "gameobjects",
            "trainers",
            "quests",
            "gossip",
            "spell",
            "spellmeta",
        ] {
            assert!(
                error
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .any(|word| word == family),
                "{error} omits {family}"
            );
        }
    }
}
