use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use lyracore_package_delta::packed_race_class_id;
use wow_dbc::vanilla_tables::area_table::AreaTable as DbcAreaTable;
use wow_dbc::vanilla_tables::area_trigger::AreaTrigger as DbcAreaTrigger;
use wow_dbc::vanilla_tables::auction_house::AuctionHouse as DbcAuctionHouse;
use wow_dbc::vanilla_tables::char_base_info::CharBaseInfo as DbcCharBaseInfo;
use wow_dbc::vanilla_tables::char_start_outfit::CharStartOutfit as DbcCharStartOutfit;
use wow_dbc::vanilla_tables::chr_races::{ChrRaces, ChrRacesKey};
use wow_dbc::vanilla_tables::creature_display_info::{CreatureDisplayInfo, CreatureDisplayInfoKey};
use wow_dbc::vanilla_tables::creature_family::CreatureFamily as DbcCreatureFamily;
use wow_dbc::vanilla_tables::faction::Faction as DbcFaction;
use wow_dbc::vanilla_tables::faction_template::FactionTemplate as DbcFactionTemplate;
use wow_dbc::vanilla_tables::lock::Lock as DbcLock;
use wow_dbc::vanilla_tables::mail_template::MailTemplate as DbcMailTemplate;
use wow_dbc::vanilla_tables::skill_line::SkillLine as DbcSkillLine;
use wow_dbc::vanilla_tables::skill_line_ability::SkillLineAbility as DbcSkillLineAbility;
use wow_dbc::vanilla_tables::skill_race_class_info::SkillRaceClassInfo as DbcSkillRaceClassInfo;
use wow_dbc::vanilla_tables::skill_tiers::SkillTiers as DbcSkillTiers;
use wow_dbc::vanilla_tables::taxi_nodes::TaxiNodes as DbcTaxiNodes;
use wow_dbc::vanilla_tables::taxi_path::TaxiPath as DbcTaxiPath;
use wow_dbc::vanilla_tables::taxi_path_node::TaxiPathNode as DbcTaxiPathNode;
use wow_dbc::vanilla_tables::world_safe_locs::WorldSafeLocs as DbcWorldSafeLocs;
use wow_dbc::{DbcTable, Indexable};
use wow_mpq::PatchChain;
use wow_world_base::vanilla::LockType;

use crate::dump::{push_insert, sql_text};
use crate::shard::Shard;

pub(crate) fn open_chain(data_dir: &Path) -> Result<PatchChain> {
    let mut chain = PatchChain::new();
    let mut added = 0u32;
    for (name, prio) in [("dbc.MPQ", 0i32), ("patch.MPQ", 100), ("patch-2.MPQ", 200)] {
        let path = data_dir.join(name);
        if path.exists() {
            chain
                .add_archive(&path, prio)
                .with_context(|| format!("open MPQ {}", path.display()))?;
            added += 1;
        }
    }
    if added == 0 {
        bail!(
            "no client MPQ archives in {} (expected dbc.MPQ — point --dbc at the client's Data/ dir)",
            data_dir.display()
        );
    }
    Ok(chain)
}

pub(crate) fn ui_baseline_archives(data_dir: &Path) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![data_dir.join("interface.MPQ")];
    if let Some(locale) = client_locale(data_dir) {
        let dir = data_dir.join(&locale);
        candidates.push(dir.join(format!("locale-{locale}.MPQ")));
        candidates.push(data_dir.join("patch.MPQ"));
        candidates.push(data_dir.join("patch-2.MPQ"));
        candidates.push(dir.join(format!("patch-{locale}.MPQ")));
        candidates.push(dir.join(format!("patch-2-{locale}.MPQ")));
    } else {
        candidates.push(data_dir.join("patch.MPQ"));
        candidates.push(data_dir.join("patch-2.MPQ"));
    }
    candidates.retain(|p| p.exists());
    candidates
}

fn client_locale(data_dir: &Path) -> Option<String> {
    let mut found: Vec<String> = fs::read_dir(data_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            data_dir
                .join(name)
                .join(format!("locale-{name}.MPQ"))
                .is_file()
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

pub(crate) fn open_ui_baseline_chain(data_dir: &Path) -> Result<PatchChain> {
    let archives = ui_baseline_archives(data_dir);
    if archives.is_empty() {
        bail!(
            "no client UI archives in {} (expected interface.MPQ or a locale archive under Data/<locale>/)",
            data_dir.display()
        );
    }
    let mut chain = PatchChain::new();
    for (prio, path) in archives.iter().enumerate() {
        chain
            .add_archive(path, prio as i32)
            .with_context(|| format!("open MPQ {}", path.display()))?;
    }
    Ok(chain)
}

pub(crate) fn read_table<T: DbcTable>(chain: &mut PatchChain) -> Result<T> {
    let in_archive = format!("DBFilesClient\\{}", T::FILENAME);
    let bytes = chain
        .read_file(&in_archive)
        .with_context(|| format!("read {in_archive} from the client MPQs"))?;
    T::read(&mut Cursor::new(bytes)).with_context(|| {
        format!(
            "parse {} (wrong client version? expected build-5875 schema)",
            T::FILENAME
        )
    })
}

pub fn run(data_dir: &str, taxi_mounts: Option<&str>, apply: bool, shard: &Shard) -> Result<()> {
    let mount_dump = crate::dump::read_dump(taxi_mounts.context(
        "standalone --dbc requires --taxi-mounts <classic-db .sql[.gz]> to resolve taxi mount displays and transports",
    )?)?;
    run_from_dump(data_dir, &mount_dump, apply, shard)
}

pub(crate) fn run_from_dump(
    data_dir: &str,
    mount_dump: &str,
    apply: bool,
    shard: &Shard,
) -> Result<()> {
    let statements = catalogue_statements(data_dir, mount_dump)?;
    if apply {
        shard.run_sql_statements(&statements, "dbc")?;
        eprintln!(
            "dbc: loaded client catalogues ({} SQL statements)",
            statements.len()
        );
    } else {
        println!("-- DBC catalogue: {} SQL statements", statements.len());
        for statement in &statements {
            println!("{statement};");
        }
    }
    Ok(())
}

pub(crate) fn catalogue_statements(data_dir: &str, mount_dump: &str) -> Result<Vec<String>> {
    let dir = Path::new(data_dir);
    let mut chain = open_chain(dir)?;
    eprintln!("dbc: opened MPQ chain from {}", dir.display());

    let races: ChrRaces = read_table(&mut chain)?;
    let human = races
        .get(ChrRacesKey::new(1))
        .context("ChrRaces has no race 1 (Human) — truncated extract?")?;
    println!(
        "ChrRaces: {} races; Human male_display = {} (expect 49)",
        races.rows().len(),
        human.male_display.id
    );

    let cdi: CreatureDisplayInfo = read_table(&mut chain)?;
    match cdi.get(CreatureDisplayInfoKey::new(304)) {
        Some(row) => println!(
            "CreatureDisplayInfo: {} rows; display 304 (chicken) model_scale = {}",
            cdi.rows().len(),
            row.creature_model_scale
        ),
        None => println!(
            "CreatureDisplayInfo: {} rows; display 304 NOT found (truncated extract?)",
            cdi.rows().len()
        ),
    }

    eprintln!("dbc: toolchain OK — read + parsed real client DBCs, wrote no .dbc to the tree.");

    let (faction_stmts, fn_count) = faction_template_sql(&mut chain)?;
    let (gf_stmts, gf_count) = faction_sql(&mut chain)?;
    let auction_houses: DbcAuctionHouse = read_table(&mut chain)?;
    let (auction_house_stmts, auction_house_count) = auction_house_sql(&auction_houses)?;
    let mail_templates: DbcMailTemplate = read_table(&mut chain)?;
    let (mail_template_stmts, mail_template_count) = mail_template_sql(&mail_templates)?;
    let (cbi_stmts, cbi_count) = char_base_info_sql(&mut chain)?;
    let (race_stmts, race_count) = race_info_sql(&races);
    let (si_stmts, si_count) = start_item_sql(&mut chain)?;
    let (property_stmts, _, _) = crate::item_property::catalogue_sql(&mut chain)?;

    let area_table: DbcAreaTable = read_table(&mut chain)?;
    let (area_stmts, area_count) = area_sql(&area_table);
    let area_triggers: DbcAreaTrigger = read_table(&mut chain)?;
    let (trigger_stmts, trigger_count) = area_trigger_sql(&area_triggers);
    let safe_locs: DbcWorldSafeLocs = read_table(&mut chain)?;
    let (graveyard_stmts, graveyard_count) = graveyard_sql(&safe_locs);

    let creature_families: DbcCreatureFamily = read_table(&mut chain)?;
    let (family_stmts, family_count) = creature_family_sql(&creature_families);
    let tameable_family_count = creature_families
        .rows()
        .iter()
        .filter(|r| r.pet_talent_type != -1)
        .count();

    let locks: DbcLock = read_table(&mut chain)?;
    let (lock_stmts, lock_count, lock_unmapped) = lock_sql(&locks);

    let taxi_nodes: DbcTaxiNodes = read_table(&mut chain)?;
    let taxi_paths: DbcTaxiPath = read_table(&mut chain)?;
    let taxi_path_nodes: DbcTaxiPathNode = read_table(&mut chain)?;
    let TaxiCatalogueSql {
        statements: taxi_stmts,
        counts: taxi_counts,
        warnings: taxi_warnings,
    } = taxi_catalogue_sql(
        &taxi_nodes,
        &taxi_paths,
        &taxi_path_nodes,
        mount_dump,
        &cdi.rows().iter().map(|row| row.id.id).collect(),
    )?;
    for warning in taxi_warnings {
        eprintln!("{warning}");
    }
    let transports = crate::transport::catalogue_sql(&taxi_paths, &taxi_path_nodes, mount_dump)?;
    for warning in &transports.warnings {
        eprintln!("{warning}");
    }

    let skill_lines: DbcSkillLine = read_table(&mut chain)?;
    let skill_abilities: DbcSkillLineAbility = read_table(&mut chain)?;
    let skill_availability: DbcSkillRaceClassInfo = read_table(&mut chain)?;
    let autolearn_count = skill_abilities
        .rows()
        .iter()
        // wow_dbc misnames the real AcquireMethod column.
        .filter(|r| r.trivial_skill_line_rank_high == AUTOLEARN_ACQUIRE_METHOD)
        .count();
    let (sl_stmts, sl_count) = skill_line_sql(&skill_lines);
    let (sa_stmts, sa_count) = skill_ability_sql(&skill_abilities);
    let (sav_stmts, sav_count) = skill_availability_sql(&skill_availability);

    println!("FactionTemplate: {fn_count} templates; Faction: {gf_count} factions");
    println!("CharBaseInfo: {cbi_count} combinations; ChrRaces: {race_count} races; CharStartOutfit: {si_count} items");
    println!("SkillLine: {sl_count} lines");
    println!("SkillLineAbility: {sa_count} abilities ({autolearn_count} autolearn)");
    println!("SkillRaceClassInfo: {sav_count} availability rows");
    println!("AuctionHouse: {auction_house_count} houses");
    println!("MailTemplate: {mail_template_count} templates");
    println!("AreaTable: {area_count} areas");
    println!("AreaTrigger: {trigger_count} triggers");
    println!("WorldSafeLocs: {graveyard_count} graveyards");
    println!("CreatureFamily: {family_count} families ({tameable_family_count} tameable)");
    if lock_unmapped.is_empty() {
        println!("Lock: {lock_count} lock indices");
    } else {
        println!(
            "Lock: {lock_count} lock indices ({} distinct UNMAPPED LockType id(s) {:?} — extend \
             LOCKTYPE_TO_SKILL_LINE; those rows import with SkillLine 0)",
            lock_unmapped.len(),
            lock_unmapped
        );
    }
    println!(
        "TaxiNodes/TaxiPath/TaxiPathNode: {} nodes, {} directed paths, {} ordered points",
        taxi_counts.nodes, taxi_counts.paths, taxi_counts.path_nodes
    );
    println!("Transports: {} single-map routes", transports.count);
    Ok([
        property_stmts,
        faction_stmts,
        gf_stmts,
        auction_house_stmts,
        mail_template_stmts,
        cbi_stmts,
        race_stmts,
        si_stmts,
        sl_stmts,
        sa_stmts,
        sav_stmts,
        area_stmts,
        trigger_stmts,
        graveyard_stmts,
        family_stmts,
        lock_stmts,
        taxi_stmts,
        transports.statements,
    ]
    .into_iter()
    .flatten()
    .collect())
}

fn auction_house_sql(table: &DbcAuctionHouse) -> Result<(Vec<String>, usize)> {
    if table.rows().is_empty() {
        bail!("AuctionHouse.dbc contains no rows");
    }

    let mut seen_ids = HashSet::new();
    let mut seen_factions = HashSet::new();
    let mut rows = Vec::with_capacity(table.rows().len());
    for row in table.rows() {
        let id = row.id.id;
        if id == 0 {
            bail!("AuctionHouse.dbc contains invalid house id 0");
        }
        if !seen_ids.insert(id) {
            bail!("AuctionHouse.dbc contains duplicate house id {id}");
        }
        if row.faction.id == 0 {
            bail!("AuctionHouse.dbc house {id} has invalid parent faction 0");
        }
        if !seen_factions.insert(row.faction.id) {
            bail!(
                "AuctionHouse.dbc contains duplicate parent faction {}",
                row.faction.id
            );
        }
        if !(0..=100).contains(&row.deposit_rate) {
            bail!(
                "AuctionHouse.dbc house {id} has invalid deposit rate {} (expected 0..=100)",
                row.deposit_rate
            );
        }
        if !(0..=100).contains(&row.consignment_rate) {
            bail!(
                "AuctionHouse.dbc house {id} has invalid consignment rate {} (expected 0..=100)",
                row.consignment_rate
            );
        }
        rows.push((
            id,
            format!(
                "({id},{},{},{},{})",
                row.faction.id,
                row.deposit_rate,
                row.consignment_rate,
                sql_text(&row.name.en_gb)
            ),
        ));
    }
    rows.sort_unstable_by_key(|(id, _)| *id);

    let count = rows.len();
    let rows: Vec<String> = rows.into_iter().map(|(_, sql)| sql).collect();
    let mut stmts = vec!["DELETE FROM game_auction_house WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_auction_house",
        "id,faction,deposit_rate,consignment_rate,name",
        &rows,
    );
    Ok((stmts, count))
}

fn mail_template_sql(table: &DbcMailTemplate) -> Result<(Vec<String>, usize)> {
    if table.rows().is_empty() {
        bail!("MailTemplate.dbc contains no rows");
    }

    let mut seen_ids = HashSet::new();
    let mut rows = Vec::with_capacity(table.rows().len());
    for row in table.rows() {
        let id = row.id.id;
        if id == 0 {
            bail!("MailTemplate.dbc contains invalid template id 0");
        }
        if !seen_ids.insert(id) {
            bail!("MailTemplate.dbc contains duplicate template id {id}");
        }
        rows.push((id, format!("({id},{})", sql_text(&row.body.en_gb))));
    }
    rows.sort_unstable_by_key(|(id, _)| *id);

    let count = rows.len();
    let rows: Vec<String> = rows.into_iter().map(|(_, sql)| sql).collect();
    let mut stmts = vec!["DELETE FROM game_mail_template WHERE id >= 0".to_string()];
    push_insert(&mut stmts, "game_mail_template", "id,body", &rows);
    Ok((stmts, count))
}

fn faction_sql(chain: &mut PatchChain) -> Result<(Vec<String>, usize)> {
    let factions: DbcFaction = read_table(chain)?;
    let rows: Vec<String> = factions
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{})",
                r.id.id, r.reputation_index as i32, r.reputation_base[0] as i32
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_faction WHERE faction_id >= 0".to_string()];
    for chunk in rows.chunks(500) {
        stmts.push(format!(
            "INSERT INTO game_faction (faction_id,reputation_index,base_standing) VALUES {}",
            chunk.join(",")
        ));
    }
    Ok((stmts, n))
}

fn char_base_info_sql(chain: &mut PatchChain) -> Result<(Vec<String>, usize)> {
    let cbi: DbcCharBaseInfo = read_table(chain)?;
    let rows: Vec<String> = cbi
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{})",
                packed_race_class_id(r.race.id as u8, r.class.id as u8),
                r.race.id,
                r.class.id
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_char_base_info WHERE race_class >= 0".to_string()];
    if !rows.is_empty() {
        stmts.push(format!(
            "INSERT INTO game_char_base_info (race_class,race,class) VALUES {}",
            rows.join(",")
        ));
    }
    Ok((stmts, n))
}

pub(crate) fn outfit_item_set(
    chain: &mut PatchChain,
) -> Result<std::collections::HashSet<(u64, i32)>> {
    let outfits: DbcCharStartOutfit = read_table(chain)?;
    let mut seen: std::collections::HashSet<(u64, i32)> = std::collections::HashSet::new();
    for o in outfits.rows().iter() {
        let (race, class) = (o.race.id, o.class.id);
        if race == 0 || class == 0 {
            continue;
        }
        let race_class = packed_race_class_id(race as u8, class as u8);
        for &item in o.item_id.iter() {
            if item > 0 {
                seen.insert((race_class, item));
            }
        }
    }
    Ok(seen)
}

fn start_item_sql(chain: &mut PatchChain) -> Result<(Vec<String>, usize)> {
    let outfits: DbcCharStartOutfit = read_table(chain)?;
    let mut seen: std::collections::HashSet<(u64, i32)> = std::collections::HashSet::new();
    let mut rows: Vec<String> = Vec::new();
    let mut id: u64 = 1;
    for o in outfits.rows().iter() {
        let (race, class) = (o.race.id, o.class.id);
        if race == 0 || class == 0 {
            continue;
        }
        let race_class = packed_race_class_id(race as u8, class as u8);
        for &item in o.item_id.iter() {
            if item > 0 && seen.insert((race_class, item)) {
                rows.push(format!("({id},{race_class},{item})"));
                id += 1;
            }
        }
    }
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_start_item WHERE id >= 0 AND id < 1000000".to_string()];
    push_insert(
        &mut stmts,
        "game_start_item",
        "id,race_class,item_entry",
        &rows,
    );
    Ok((stmts, n))
}

const AUTOLEARN_ACQUIRE_METHOD: i32 = 1;

const PROFESSION_SKILL_LINES: &[u32] =
    &[186, 182, 393, 185, 129, 171, 164, 197, 165, 202, 333, 356];

fn skill_line_sql(table: &DbcSkillLine) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{})",
                r.id.id,
                r.category.id,
                sql_text(&r.display_name.en_gb)
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_skill_line WHERE skill_line >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_skill_line",
        "skill_line,category,name",
        &rows,
    );
    (stmts, n)
}

fn skill_ability_sql(table: &DbcSkillLineAbility) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let min_skill = r.superseded_by.id; // Real MinSkillLineRank.
            let acquire_method = r.trivial_skill_line_rank_high; // Real AcquireMethod.
            let gray = r.trivial_skill_line_rank_low; // Real TrivialSkillLineRankHigh.
            let green = r.character_points[0]; // Real TrivialSkillLineRankLow.
            format!(
                "({},{},{},{},{},{},{},{},{})",
                i as u64 + 1,
                r.spell.id,
                r.skill_line.id,
                r.race_mask.id,
                r.class_mask.id,
                min_skill,
                acquire_method,
                gray,
                green,
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_skill_ability WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_skill_ability",
        "id,spell_id,skill_line,race_mask,class_mask,min_skill,acquire_method,gray,green",
        &rows,
    );
    (stmts, n)
}

// The pinned reader calls vanilla SupercededBySpell acquire_method.
pub(crate) fn read_spell_rank_links(data_dir: &str) -> Result<Vec<(u32, u32)>> {
    let mut chain = open_chain(Path::new(data_dir))?;
    let abilities: DbcSkillLineAbility = read_table(&mut chain)?;
    spell_rank_links(&abilities)
}

fn spell_rank_links(abilities: &DbcSkillLineAbility) -> Result<Vec<(u32, u32)>> {
    let mut links = Vec::new();
    for row in abilities.rows() {
        let next = u32::try_from(row.acquire_method)
            .context("SkillLineAbility has a negative successor spell")?;
        links.push((row.spell.id, next));
    }
    Ok(links)
}

fn skill_availability_sql(table: &DbcSkillRaceClassInfo) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "({},{},{},{},{},{})",
                i as u64 + 1,
                r.skill_line.id,
                r.race_mask.id,
                r.class_mask.id,
                r.flags,
                r.min_level,
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_skill_availability WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_skill_availability",
        "id,skill_line,race_mask,class_mask,flags,min_level",
        &rows,
    );
    (stmts, n)
}

fn race_info_sql(races: &ChrRaces) -> (Vec<String>, usize) {
    let rows: Vec<String> = races
        .rows()
        .iter()
        .filter(|r| r.id.id != 0)
        .map(|r| {
            format!(
                "({},{},{},{})",
                r.id.id, r.male_display.id, r.female_display.id, r.faction.id
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_race_info WHERE race >= 0".to_string()];
    if !rows.is_empty() {
        stmts.push(format!(
            "INSERT INTO game_race_info (race,male_display,female_display,faction_template) VALUES {}",
            rows.join(",")
        ));
    }
    (stmts, n)
}

fn faction_template_sql(chain: &mut PatchChain) -> Result<(Vec<String>, usize)> {
    let ft: DbcFactionTemplate = read_table(chain)?;
    let rows: Vec<String> = ft
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{},{},{},{},{},{},{},{},{},{},{})",
                r.id.id,
                r.faction.id,
                r.faction_group.id,
                r.friend_group.id,
                r.enemy_group.id,
                r.enemies[0],
                r.enemies[1],
                r.enemies[2],
                r.enemies[3],
                r.friends[0],
                r.friends[1],
                r.friends[2],
                r.friends[3],
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_faction_template WHERE id >= 0".to_string()];
    for chunk in rows.chunks(500) {
        stmts.push(format!(
            "INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES {}",
            chunk.join(",")
        ));
    }
    Ok((stmts, n))
}

fn area_sql(table: &DbcAreaTable) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{},{},{},{},{},{})",
                r.id.id,
                r.map.id,
                r.parent_area_table.id,
                r.area_bit,
                r.flags.as_int() as u32,
                r.exploration_level,
                r.faction_group.id,
                sql_text(&r.area_name.en_gb),
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_area WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_area",
        "id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name",
        &rows,
    );
    (stmts, n)
}

fn area_trigger_sql(table: &DbcAreaTrigger) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{},{},{},{},{},{},{},{})",
                r.id.id,
                r.map.id,
                r.location_x,
                r.location_y,
                r.location_z,
                r.radius,
                r.box_length,
                r.box_width,
                r.box_height,
                r.box_yaw,
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_area_trigger WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_area_trigger",
        "id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw",
        &rows,
    );
    (stmts, n)
}

fn graveyard_sql(table: &DbcWorldSafeLocs) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .map(|r| {
            format!(
                // WorldSafeLocs.dbc has no orientation.
                "({},{},{},{},{},0,{})",
                r.id.id,
                r.map.id,
                r.location_x,
                r.location_y,
                r.location_z,
                sql_text(&r.area_name.en_gb),
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_graveyard WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_graveyard",
        "id,map_id,x,y,z,o,name",
        &rows,
    );
    (stmts, n)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaxiCounts {
    nodes: usize,
    paths: usize,
    path_nodes: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct TaxiCatalogueSql {
    statements: Vec<String>,
    counts: TaxiCounts,
    warnings: Vec<String>,
}

fn taxi_mount_displays(dump: &str, display_ids: &HashSet<u32>) -> Result<HashMap<u32, u32>> {
    let rows = crate::dump::parse_complete_table(dump, "creature_template")?;
    if rows.is_empty() {
        bail!("taxi mount dump has no creature_template rows");
    }
    let mut mounts = HashMap::with_capacity(rows.len());
    for row in rows {
        let entry: u32 = crate::dump::field(&row, crate::dump::ct::ENTRY)
            .parse()
            .context("taxi mount dump has an invalid creature entry")?;
        let mut display = 0;
        for index in crate::dump::ct::MODEL_ID1..crate::dump::ct::MODEL_ID1 + 4 {
            let model: u32 = row
                .get(index)
                .with_context(|| {
                    format!("taxi mount creature {entry} is missing model column {index}")
                })?
                .trim()
                .parse()
                .with_context(|| format!("taxi mount creature {entry} has an invalid model"))?;
            if display == 0 {
                display = model;
            }
        }
        if !display_ids.contains(&display) {
            display = 0;
        }
        if mounts.insert(entry, display).is_some() {
            bail!("taxi mount dump has duplicate creature entry {entry}");
        }
    }
    Ok(mounts)
}

#[allow(clippy::too_many_lines)]
fn taxi_catalogue_sql(
    nodes: &DbcTaxiNodes,
    paths: &DbcTaxiPath,
    path_nodes: &DbcTaxiPathNode,
    mount_dump: &str,
    display_ids: &HashSet<u32>,
) -> Result<TaxiCatalogueSql> {
    use lyracore_shared::constants::taxi_fixture as fixture;
    use lyracore_shared::constants::taxi_protocol;

    if nodes.rows().is_empty() {
        bail!("taxi DBC catalogue is incomplete: TaxiNodes=0 rows");
    }

    let mounts = taxi_mount_displays(mount_dump, display_ids)?;
    let mut warnings = Vec::new();
    let mut node_ids = HashSet::with_capacity(nodes.rows().len());
    let mut node_rows = Vec::with_capacity(nodes.rows().len() + 2);
    for row in nodes.rows() {
        let id = row.id.id;
        if id >= fixture::STORAGE_ID_FLOOR {
            bail!(
                "TaxiNodes.dbc id {id} enters LyraCore's reserved fixture namespace (>= {})",
                fixture::STORAGE_ID_FLOOR
            );
        }
        if !(taxi_protocol::CLIENT_NODE_ID_MIN..=taxi_protocol::CLIENT_NODE_ID_MAX).contains(&id) {
            bail!(
                "TaxiNodes.dbc id {id} is outside the vanilla client taxi-mask range {}..={}",
                taxi_protocol::CLIENT_NODE_ID_MIN,
                taxi_protocol::CLIENT_NODE_ID_MAX,
            );
        }
        if !node_ids.insert(id) {
            bail!("TaxiNodes.dbc contains duplicate node id {id}");
        }
        if ![row.location_x, row.location_y, row.location_z]
            .into_iter()
            .all(f32::is_finite)
        {
            bail!("TaxiNodes.dbc node {id} has a non-finite position");
        }
        // Vanilla TaxiNodes stores creature-template entries despite the reader field name.
        let entries = row.mount_creature_display_info;
        let displays = [0, 1].map(|faction| {
            let entry = if entries[faction] == 0 {
                entries[1 - faction]
            } else {
                entries[faction]
            };
            let display = if entry == 0 {
                0
            } else {
                mounts.get(&entry).copied().unwrap_or(0)
            };
            if entry != 0 && display == 0 {
                warnings.push(format!(
                    "dbc: WARN TaxiNodes.dbc node {id} faction slot {faction}: creature {entry} has no valid mount display; flights unavailable"
                ));
            }
            display
        });
        node_rows.push((
            id,
            format!(
                "({},{},{},{},{},{},{},{},{})",
                id,
                id,
                row.map.id,
                row.location_x,
                row.location_y,
                row.location_z,
                sql_text(&row.name.en_gb),
                displays[0],
                displays[1],
            ),
        ));
    }
    for client_node_id in [
        fixture::SOURCE_CLIENT_NODE_ID,
        fixture::DESTINATION_CLIENT_NODE_ID,
    ] {
        if node_ids.contains(&client_node_id) {
            bail!(
                "TaxiNodes.dbc node {client_node_id} collides with a reserved fixture client node id"
            );
        }
    }
    if paths.rows().is_empty() {
        bail!("taxi DBC catalogue is incomplete: TaxiPath=0 rows");
    }

    let mut path_ids = HashSet::with_capacity(paths.rows().len());
    let mut route_keys = HashSet::with_capacity(paths.rows().len());
    let mut path_rows = Vec::with_capacity(paths.rows().len() + 1);
    for row in paths.rows() {
        let id = row.id.id;
        let source = row.source_taxi_node.id;
        let destination = row.destination_taxi_node.id;
        if id >= fixture::STORAGE_ID_FLOOR {
            bail!(
                "TaxiPath.dbc id {id} enters LyraCore's reserved fixture namespace (>= {})",
                fixture::STORAGE_ID_FLOOR
            );
        }
        if !path_ids.insert(id) {
            bail!("TaxiPath.dbc contains duplicate path id {id}");
        }
        if !node_ids.contains(&source) {
            bail!("TaxiPath.dbc path {id} references missing source node {source}");
        }
        if !node_ids.contains(&destination) {
            bail!("TaxiPath.dbc path {id} references missing destination node {destination}");
        }
        if !route_keys.insert((source, destination)) {
            bail!(
                "TaxiPath.dbc contains more than one directed route from node {source} to node {destination}"
            );
        }
        let fare = u32::try_from(row.cost)
            .with_context(|| format!("TaxiPath.dbc path {id} has negative fare {}", row.cost))?;
        path_rows.push((id, format!("({id},{source},{destination},{fare})")));
    }
    if path_nodes.rows().is_empty() {
        bail!("taxi DBC catalogue is incomplete: TaxiPathNode=0 rows");
    }

    let mut point_ids = HashSet::with_capacity(path_nodes.rows().len());
    let mut point_ordinals = HashSet::with_capacity(path_nodes.rows().len());
    let mut paths_with_points = HashSet::with_capacity(paths.rows().len());
    let mut point_rows = Vec::with_capacity(path_nodes.rows().len() + fixture::POINT_IDS.len());
    let mut dangling_points = Vec::new();
    for row in path_nodes.rows() {
        let id = row.id.id;
        let path_id = row.taxi_path.id;
        if id >= fixture::STORAGE_ID_FLOOR {
            bail!(
                "TaxiPathNode.dbc id {id} enters LyraCore's reserved fixture namespace (>= {})",
                fixture::STORAGE_ID_FLOOR
            );
        }
        if !point_ids.insert(id) {
            bail!("TaxiPathNode.dbc contains duplicate point id {id}");
        }
        if ![row.location_x, row.location_y, row.location_z]
            .into_iter()
            .all(f32::is_finite)
        {
            bail!("TaxiPathNode.dbc point {id} has a non-finite position");
        }
        let node_index = u32::try_from(row.node_index).with_context(|| {
            format!(
                "TaxiPathNode.dbc point {id} has negative node index {}",
                row.node_index
            )
        })?;
        if !point_ordinals.insert((path_id, node_index)) {
            bail!("TaxiPathNode.dbc repeats node index {node_index} on path {path_id}");
        }
        if row.delay < 0 {
            bail!(
                "TaxiPathNode.dbc point {id} has negative delay {}",
                row.delay
            );
        }
        // TaxiPathNode.dbc stores the wait in seconds.
        let delay_ms = row.delay.checked_mul(1000).with_context(|| {
            format!(
                "TaxiPathNode.dbc point {id} has delay {} out of range",
                row.delay
            )
        })?;
        if !path_ids.contains(&path_id) {
            dangling_points.push((id, path_id));
            continue;
        }
        paths_with_points.insert(path_id);
        point_rows.push((
            (path_id, node_index, id),
            format!(
                "({id},{path_id},{node_index},{},{},{},{},{},{})",
                row.map.id, row.location_x, row.location_y, row.location_z, row.flags, delay_ms,
            ),
        ));
    }
    let mut empty_paths: Vec<u32> = path_ids
        .iter()
        .copied()
        .filter(|id| !paths_with_points.contains(id))
        .collect();
    empty_paths.sort_unstable();
    path_rows.retain(|(id, _)| paths_with_points.contains(id));

    let counts = TaxiCounts {
        nodes: node_rows.len(),
        paths: path_rows.len(),
        path_nodes: point_rows.len(),
    };

    node_rows.extend([
        (
            fixture::SOURCE_NODE_STORAGE_ID,
            format!(
                "({},{},{},{},{},{},{},{},{})",
                fixture::SOURCE_NODE_STORAGE_ID,
                fixture::SOURCE_CLIENT_NODE_ID,
                fixture::MAP_ID,
                fixture::SOURCE_X,
                fixture::SOURCE_Y,
                fixture::SOURCE_Z,
                sql_text(fixture::SOURCE_NAME),
                fixture::MOUNT_DISPLAY_HORDE,
                fixture::MOUNT_DISPLAY_ALLIANCE,
            ),
        ),
        (
            fixture::DESTINATION_NODE_STORAGE_ID,
            format!(
                "({},{},{},{},{},{},{},{},{})",
                fixture::DESTINATION_NODE_STORAGE_ID,
                fixture::DESTINATION_CLIENT_NODE_ID,
                fixture::MAP_ID,
                fixture::DESTINATION_X,
                fixture::DESTINATION_Y,
                fixture::DESTINATION_Z,
                sql_text(fixture::DESTINATION_NAME),
                fixture::MOUNT_DISPLAY_HORDE,
                fixture::MOUNT_DISPLAY_ALLIANCE,
            ),
        ),
    ]);
    path_rows.push((
        fixture::PATH_ID,
        format!(
            "({},{},{},{})",
            fixture::PATH_ID,
            fixture::SOURCE_NODE_STORAGE_ID,
            fixture::DESTINATION_NODE_STORAGE_ID,
            fixture::FARE,
        ),
    ));
    for (id, node_index, x, y, z) in [
        (
            fixture::POINT_IDS[0],
            0,
            fixture::SOURCE_X,
            fixture::SOURCE_Y,
            fixture::SOURCE_Z,
        ),
        (
            fixture::POINT_IDS[1],
            1,
            fixture::MIDPOINT_X,
            fixture::MIDPOINT_Y,
            fixture::MIDPOINT_Z,
        ),
        (
            fixture::POINT_IDS[2],
            2,
            fixture::DESTINATION_X,
            fixture::DESTINATION_Y,
            fixture::DESTINATION_Z,
        ),
    ] {
        point_rows.push((
            (fixture::PATH_ID, node_index, id),
            format!(
                "({id},{},{node_index},{},{x},{y},{z},0,0)",
                fixture::PATH_ID,
                fixture::MAP_ID,
            ),
        ));
    }

    node_rows.sort_by_key(|(id, _)| *id);
    path_rows.sort_by_key(|(id, _)| *id);
    point_rows.sort_by_key(|(key, _)| *key);
    let node_rows: Vec<String> = node_rows.into_iter().map(|(_, sql)| sql).collect();
    let path_rows: Vec<String> = path_rows.into_iter().map(|(_, sql)| sql).collect();
    let point_rows: Vec<String> = point_rows.into_iter().map(|(_, sql)| sql).collect();

    let mut stmts = vec![
        "DELETE FROM game_taxi_path_node WHERE id >= 0".to_string(),
        "DELETE FROM game_taxi_path WHERE id >= 0".to_string(),
        "DELETE FROM game_taxi_node WHERE id >= 0".to_string(),
    ];
    push_insert(
        &mut stmts,
        "game_taxi_node",
        "id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance",
        &node_rows,
    );
    push_insert(
        &mut stmts,
        "game_taxi_path",
        "id,source_node_id,destination_node_id,fare",
        &path_rows,
    );
    push_insert(
        &mut stmts,
        "game_taxi_path_node",
        "id,path_id,node_index,map_id,x,y,z,flags,delay_ms",
        &point_rows,
    );
    dangling_points.sort_unstable();
    warnings.sort_unstable();
    warnings.extend(dangling_points.into_iter().map(|(id, path_id)| {
        format!("dbc: WARN TaxiPathNode.dbc point {id} references missing path {path_id}; omitted")
    }));
    warnings.extend(empty_paths.into_iter().map(|path_id| {
        format!("dbc: WARN TaxiPath.dbc path {path_id} has no TaxiPathNode.dbc points; omitted")
    }));
    Ok(TaxiCatalogueSql {
        statements: stmts,
        counts,
        warnings,
    })
}

fn creature_family_sql(table: &DbcCreatureFamily) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{},{},{})",
                r.id.id,
                sql_text(&r.name.en_gb),
                r.pet_food_mask,
                r.pet_talent_type,
                r.category,
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_creature_family WHERE family_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_creature_family",
        "family_id,name,pet_food_mask,pet_talent_type,category",
        &rows,
    );
    (stmts, n)
}

// LockType.dbc ids require conversion to SkillLine ids.
const LOCKTYPE_TO_SKILL_LINE: &[(u32, u32)] = &[(1, 633), (2, 182), (3, 186)];

fn locktype_skill_line(locktype_id: u32) -> Option<u32> {
    LOCKTYPE_TO_SKILL_LINE
        .iter()
        .find(|&&(id, _)| id == locktype_id)
        .map(|&(_, skill)| skill)
}

// The eight Lock.dbc indices are alternative ways to open one lock.
fn lock_sql(table: &DbcLock) -> (Vec<String>, usize, std::collections::BTreeSet<u32>) {
    let mut rows: Vec<String> = Vec::new();
    let mut row_id: u64 = 1;
    let mut unmapped_locktype_ids: std::collections::BTreeSet<u32> =
        std::collections::BTreeSet::new();
    for r in table.rows() {
        for i in 0..8usize {
            let (kind, property): (u8, u32) = match r.ty[i] {
                LockType::None => continue,
                LockType::ItemRequired => (1, r.property[i]),
                LockType::LocktypeReference => {
                    let locktype_id = r.property[i];
                    match locktype_skill_line(locktype_id) {
                        Some(skill) => (2, skill),
                        None => {
                            unmapped_locktype_ids.insert(locktype_id);
                            (2, 0)
                        }
                    }
                }
            };
            let required_skill = r.required_skill[i].max(0) as u32;
            rows.push(format!(
                "({row_id},{},{i},{kind},{property},{required_skill})",
                r.id.id
            ));
            row_id += 1;
        }
    }
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_lock WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_lock",
        "id,lock_id,index,kind,property,required_skill",
        &rows,
    );
    (stmts, n, unmapped_locktype_ids)
}

pub fn load_display_scales(data_dir: &str) -> Result<HashMap<u32, f32>> {
    let mut chain = open_chain(Path::new(data_dir))?;
    let cdi: CreatureDisplayInfo = read_table(&mut chain)?;
    let mut scales = HashMap::with_capacity(cdi.rows().len());
    for row in cdi.rows() {
        scales.insert(row.id.id, row.creature_model_scale);
    }
    Ok(scales)
}

pub fn profession_tier_values(data_dir: &str) -> Result<HashMap<u32, [u16; 4]>> {
    let mut chain = open_chain(Path::new(data_dir))?;
    let availability: DbcSkillRaceClassInfo = read_table(&mut chain)?;
    let tiers: DbcSkillTiers = read_table(&mut chain)?;
    Ok(resolve_profession_tier_values(&availability, &tiers))
}

fn resolve_profession_tier_values(
    availability: &DbcSkillRaceClassInfo,
    tiers: &DbcSkillTiers,
) -> HashMap<u32, [u16; 4]> {
    let mut out = HashMap::new();
    for &line in PROFESSION_SKILL_LINES {
        let Some(tier) = availability
            .rows()
            .iter()
            .filter(|r| r.skill_line.id == line)
            .find_map(|r| tiers.get(r.skill_tier))
        else {
            continue;
        };
        let mut caps = [0u16; 4];
        for (i, slot) in caps.iter_mut().enumerate() {
            *slot = tier.value[i].max(0) as u16;
        }
        out.insert(line, caps);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_dbc::vanilla_tables::area_table::{AreaTableKey, AreaTableRow};
    use wow_dbc::vanilla_tables::area_trigger::{AreaTriggerKey, AreaTriggerRow};
    use wow_dbc::vanilla_tables::auction_house::{AuctionHouseKey, AuctionHouseRow};
    use wow_dbc::vanilla_tables::chr_classes::ChrClassesKey;
    use wow_dbc::vanilla_tables::chr_races::ChrRacesKey as SkillChrRacesKey;
    use wow_dbc::vanilla_tables::creature_family::{CreatureFamilyKey, CreatureFamilyRow};
    use wow_dbc::vanilla_tables::faction_group::FactionGroupKey;
    use wow_dbc::vanilla_tables::light::LightKey;
    use wow_dbc::vanilla_tables::liquid_type::LiquidTypeKey;
    use wow_dbc::vanilla_tables::lock::{LockKey, LockRow};
    use wow_dbc::vanilla_tables::mail_template::{MailTemplateKey, MailTemplateRow};
    use wow_dbc::vanilla_tables::map::MapKey;
    use wow_dbc::vanilla_tables::skill_costs_data::SkillCostsDataKey;
    use wow_dbc::vanilla_tables::skill_line::{SkillLineKey, SkillLineRow};
    use wow_dbc::vanilla_tables::skill_line_ability::{SkillLineAbilityKey, SkillLineAbilityRow};
    use wow_dbc::vanilla_tables::skill_line_category::SkillLineCategoryKey;
    use wow_dbc::vanilla_tables::skill_race_class_info::{
        SkillRaceClassInfoKey, SkillRaceClassInfoRow,
    };
    use wow_dbc::vanilla_tables::skill_tiers::{SkillTiersKey, SkillTiersRow};
    use wow_dbc::vanilla_tables::sound_ambience::SoundAmbienceKey;
    use wow_dbc::vanilla_tables::sound_provider_preferences::SoundProviderPreferencesKey;
    use wow_dbc::vanilla_tables::spell::SpellKey;
    use wow_dbc::vanilla_tables::spell_icon::SpellIconKey;
    use wow_dbc::vanilla_tables::taxi_nodes::{TaxiNodesKey, TaxiNodesRow};
    use wow_dbc::vanilla_tables::taxi_path::{TaxiPathKey, TaxiPathRow};
    use wow_dbc::vanilla_tables::taxi_path_node::{TaxiPathNodeKey, TaxiPathNodeRow};
    use wow_dbc::vanilla_tables::world_safe_locs::{WorldSafeLocsKey, WorldSafeLocsRow};
    use wow_dbc::vanilla_tables::zone_intro_music_table::ZoneIntroMusicTableKey;
    use wow_dbc::vanilla_tables::zone_music::ZoneMusicKey;
    use wow_dbc::LocalizedString;
    use wow_world_base::vanilla::AreaFlags;

    fn placeholder_data_dir(tag: &str, names: &[&str]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dbc-ui-baseline-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for name in names {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
        }
        dir
    }

    #[test]
    fn ui_baseline_chain_never_reads_the_packers_own_patch_3() {
        let dir = placeholder_data_dir(
            "no-patch-3",
            &[
                "dbc.MPQ",
                "interface.MPQ",
                "patch.MPQ",
                "patch-2.MPQ",
                "patch-3.MPQ",
                "enUS/locale-enUS.MPQ",
                "enUS/patch-enUS.MPQ",
                "enUS/patch-3-enUS.MPQ",
            ],
        );
        let listed: Vec<String> = ui_baseline_archives(&dir)
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
            .collect();
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(
            listed,
            [
                "interface.MPQ",
                "enUS/locale-enUS.MPQ",
                "patch.MPQ",
                "patch-2.MPQ",
                "enUS/patch-enUS.MPQ",
            ],
            "load order, and patch-3 in either spelling is our own output"
        );
    }

    #[test]
    fn ui_baseline_chain_skips_archives_a_client_does_not_have() {
        let dir = placeholder_data_dir("sparse", &["dbc.MPQ", "interface.MPQ"]);
        let listed = ui_baseline_archives(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert!(listed[0].ends_with("interface.MPQ"), "{listed:?}");
    }

    fn auction_house_row(
        id: u32,
        faction: u32,
        deposit_rate: i32,
        consignment_rate: i32,
        name: &str,
    ) -> AuctionHouseRow {
        AuctionHouseRow {
            id: AuctionHouseKey::new(id),
            faction: wow_dbc::vanilla_tables::faction::FactionKey::new(faction),
            deposit_rate,
            consignment_rate,
            name: LocalizedString {
                en_gb: name.to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn auction_house_sql_is_validated_deterministic_and_escapes_names() {
        let table = DbcAuctionHouse {
            rows: vec![
                auction_house_row(7, 120, 25, 15, "Booty Bay's Auction House"),
                auction_house_row(1, 12, 5, 5, "Stormwind Auction House"),
            ],
        };

        let (first, count) = auction_house_sql(&table).unwrap();
        assert_eq!(count, 2);
        assert_eq!(first[0], "DELETE FROM game_auction_house WHERE id >= 0");
        assert_eq!(
            first[1],
            "INSERT INTO game_auction_house (id,faction,deposit_rate,consignment_rate,name) VALUES (1,12,5,5,'Stormwind Auction House'),(7,120,25,15,'Booty Bay''s Auction House')"
        );

        let reversed = DbcAuctionHouse {
            rows: table.rows.into_iter().rev().collect(),
        };
        assert_eq!(auction_house_sql(&reversed).unwrap().0, first);

        for invalid in [
            DbcAuctionHouse {
                rows: vec![auction_house_row(1, 0, 5, 5, "missing faction")],
            },
            DbcAuctionHouse {
                rows: vec![auction_house_row(1, 12, -1, 5, "negative deposit")],
            },
            DbcAuctionHouse {
                rows: vec![auction_house_row(1, 12, 5, 101, "excessive cut")],
            },
            DbcAuctionHouse {
                rows: vec![
                    auction_house_row(1, 12, 5, 5, "first"),
                    auction_house_row(1, 29, 5, 5, "duplicate"),
                ],
            },
            DbcAuctionHouse {
                rows: vec![
                    auction_house_row(1, 12, 5, 5, "first"),
                    auction_house_row(2, 12, 5, 5, "ambiguous faction"),
                ],
            },
        ] {
            assert!(auction_house_sql(&invalid).is_err());
        }
    }

    fn mail_template_row(id: u32, body: &str) -> MailTemplateRow {
        MailTemplateRow {
            id: MailTemplateKey::new(id),
            body: LocalizedString {
                en_gb: body.to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn mail_template_sql_is_validated_deterministic_and_escapes_body() {
        let table = DbcMailTemplate {
            rows: vec![
                mail_template_row(99, "Here's your reward, $N."),
                mail_template_row(1, "$B$BEnjoy."),
            ],
        };

        let (first, count) = mail_template_sql(&table).unwrap();
        assert_eq!(count, 2);
        assert_eq!(first[0], "DELETE FROM game_mail_template WHERE id >= 0");
        assert_eq!(
            first[1],
            "INSERT INTO game_mail_template (id,body) VALUES (1,'$B$BEnjoy.'),(99,'Here''s your reward, $N.')"
        );

        let reversed = DbcMailTemplate {
            rows: table.rows.into_iter().rev().collect(),
        };
        assert_eq!(mail_template_sql(&reversed).unwrap().0, first);

        for invalid in [
            DbcMailTemplate { rows: vec![] },
            DbcMailTemplate {
                rows: vec![mail_template_row(0, "invalid id")],
            },
            DbcMailTemplate {
                rows: vec![
                    mail_template_row(1, "first"),
                    mail_template_row(1, "duplicate"),
                ],
            },
        ] {
            assert!(mail_template_sql(&invalid).is_err());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn area_table_row(
        id: u32,
        map: u32,
        parent: u32,
        area_bit: i32,
        flags: AreaFlags,
        exploration_level: i32,
        faction_group: u32,
        name: &str,
    ) -> AreaTableRow {
        AreaTableRow {
            id: AreaTableKey::new(id),
            map: MapKey::new(map),
            parent_area_table: AreaTableKey::new(parent),
            area_bit,
            flags,
            sound_preferences: SoundProviderPreferencesKey::new(0),
            sound_preferences_underwater: SoundProviderPreferencesKey::new(0),
            sound_ambience: SoundAmbienceKey::new(0),
            zone_music: ZoneMusicKey::new(0),
            zone_music_intro: ZoneIntroMusicTableKey::new(0),
            exploration_level,
            area_name: LocalizedString {
                en_gb: name.to_string(),
                ..Default::default()
            },
            faction_group: FactionGroupKey::new(faction_group),
            liquid_type: LiquidTypeKey::new(0),
            min_elevation: 0,
            ambient_multiplier: 0.0,
            light: LightKey::new(0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn area_trigger_row(
        id: u32,
        map: u32,
        x: f32,
        y: f32,
        z: f32,
        radius: f32,
        box_length: f32,
        box_width: f32,
        box_height: f32,
        box_yaw: f32,
    ) -> AreaTriggerRow {
        AreaTriggerRow {
            id: AreaTriggerKey::new(id),
            map: MapKey::new(map),
            location_x: x,
            location_y: y,
            location_z: z,
            radius,
            box_length,
            box_width,
            box_height,
            box_yaw,
        }
    }

    fn world_safe_locs_row(
        id: u32,
        map: u32,
        x: f32,
        y: f32,
        z: f32,
        name: &str,
    ) -> WorldSafeLocsRow {
        WorldSafeLocsRow {
            id: WorldSafeLocsKey::new(id),
            map: MapKey::new(map),
            location_x: x,
            location_y: y,
            location_z: z,
            area_name: LocalizedString {
                en_gb: name.to_string(),
                ..Default::default()
            },
        }
    }

    fn taxi_node_row(id: u32, name: &str, mounts: [u32; 2], x: f32) -> TaxiNodesRow {
        TaxiNodesRow {
            id: TaxiNodesKey::new(id),
            map: MapKey::new(0),
            location_x: x,
            location_y: 2.0,
            location_z: 3.0,
            name: LocalizedString {
                en_gb: name.to_string(),
                ..Default::default()
            },
            mount_creature_display_info: mounts,
        }
    }

    fn taxi_path_row(id: u32, source: u32, destination: u32, cost: i32) -> TaxiPathRow {
        TaxiPathRow {
            id: TaxiPathKey::new(id),
            source_taxi_node: TaxiNodesKey::new(source),
            destination_taxi_node: TaxiNodesKey::new(destination),
            cost,
        }
    }

    fn taxi_point_row(
        id: u32,
        path_id: u32,
        node_index: i32,
        x: f32,
        flags: i32,
        delay: i32,
    ) -> TaxiPathNodeRow {
        TaxiPathNodeRow {
            id: TaxiPathNodeKey::new(id),
            taxi_path: TaxiPathKey::new(path_id),
            node_index,
            map: MapKey::new(0),
            location_x: x,
            location_y: 5.0,
            location_z: 6.0,
            flags,
            delay,
        }
    }

    fn catalogue_with_mounts(
        nodes: &DbcTaxiNodes,
        paths: &DbcTaxiPath,
        points: &DbcTaxiPathNode,
    ) -> Result<TaxiCatalogueSql> {
        super::taxi_catalogue_sql(
            nodes,
            paths,
            points,
            "INSERT INTO `creature_template` VALUES \
                (1100,'Horde mount','',1,1,295,0,0,0),
                (1101,'Alliance mount','',1,1,1147,0,0,0),
                (2200,'Other Horde mount','',1,1,295,0,0,0),
                (2201,'Other Alliance mount','',1,1,1147,0,0,0);",
            &HashSet::from([295, 1147]),
        )
    }

    fn synthetic_taxi_catalogue() -> (DbcTaxiNodes, DbcTaxiPath, DbcTaxiPathNode) {
        (
            DbcTaxiNodes {
                rows: vec![
                    taxi_node_row(20, "Destination", [2200, 2201], 20.0),
                    taxi_node_row(10, "O'Ryan's Source", [1100, 1101], 10.0),
                ],
            },
            DbcTaxiPath {
                rows: vec![taxi_path_row(70, 10, 20, 125)],
            },
            DbcTaxiPathNode {
                rows: vec![
                    taxi_point_row(703, 70, 2, 30.0, i32::MIN, 900),
                    taxi_point_row(701, 70, 0, 10.0, 1, 0),
                    taxi_point_row(702, 70, 1, 20.0, 2, 450),
                ],
            },
        )
    }

    #[test]
    fn taxi_mount_fallback_requires_an_absent_faction_entry() {
        let (mut nodes, paths, points) = synthetic_taxi_catalogue();
        for (entries, expected, warning_count) in [
            ([0, 1101], [1147, 1147], 0),
            ([1100, 0], [295, 295], 0),
            ([0, 0], [0, 0], 0),
            ([9999, 1101], [0, 1147], 1),
            ([1100, 9999], [295, 0], 1),
        ] {
            nodes.rows[1].mount_creature_display_info = entries;
            let catalogue = catalogue_with_mounts(&nodes, &paths, &points).unwrap();
            let expected = format!(
                "(10,10,0,10,2,3,'O''Ryan''s Source',{},{})",
                expected[0], expected[1]
            );
            assert!(
                catalogue
                    .statements
                    .iter()
                    .any(|sql| sql.contains(&expected)),
                "{entries:?}"
            );
            assert_eq!(catalogue.warnings.len(), warning_count);
            if warning_count != 0 {
                assert!(catalogue.warnings[0].contains("creature 9999"));
            }
        }
    }

    #[test]
    fn taxi_mounts_use_the_first_nonzero_template_model() {
        let (nodes, paths, points) = synthetic_taxi_catalogue();
        let catalogue = super::taxi_catalogue_sql(
            &nodes,
            &paths,
            &points,
            "INSERT INTO `creature_template` VALUES \
                (1100,'Horde mount','',1,1, 0, 295, 1147, 0),
                (1101,'Alliance mount','',1,1,0,0,0,1147);",
            &HashSet::from([295, 1147]),
        )
        .unwrap();
        assert!(catalogue
            .statements
            .iter()
            .any(|sql| sql.contains("(10,10,0,10,2,3,'O''Ryan''s Source',295,1147)")));
    }

    #[test]
    fn taxi_mounts_without_a_client_display_stay_unavailable() {
        let (nodes, paths, points) = synthetic_taxi_catalogue();
        let catalogue = super::taxi_catalogue_sql(
            &nodes,
            &paths,
            &points,
            "INSERT INTO `creature_template` VALUES \
                (1100,'Unknown model','',1,1,9999,0,0,0),
                (1101,'No model','',1,1,0,0,0,0);",
            &HashSet::from([295, 1147]),
        )
        .unwrap();
        assert!(catalogue
            .statements
            .iter()
            .any(|sql| sql.contains("(10,10,0,10,2,3,'O''Ryan''s Source',0,0)")));
        assert!(catalogue
            .warnings
            .iter()
            .any(|warning| warning.contains("creature 1100")));
        assert!(catalogue
            .warnings
            .iter()
            .any(|warning| warning.contains("creature 1101")));
    }

    #[test]
    fn taxi_catalogue_refuses_unusable_template_dumps_before_returning_sql() {
        let (nodes, paths, points) = synthetic_taxi_catalogue();
        for dump in [
            "",
            "INSERT INTO `creature_template` VALUES (1100,'Short row');",
            "INSERT INTO `creature_template` VALUES (1100,'Bad model','',1,1,'bad',0,0,0);",
            "INSERT INTO `creature_template` VALUES ('bad','Bad entry','',1,1,295,0,0,0);",
            "INSERT INTO `creature_template` VALUES \
                (1100,'First','',1,1,295,0,0,0),(1100,'Duplicate','',1,1,1147,0,0,0);",
        ] {
            assert!(
                super::taxi_catalogue_sql(
                    &nodes,
                    &paths,
                    &points,
                    dump,
                    &HashSet::from([295, 1147])
                )
                .is_err(),
                "{dump}"
            );
        }
    }

    #[test]
    fn taxi_catalogue_refuses_truncated_template_inserts() {
        let (nodes, paths, points) = synthetic_taxi_catalogue();
        for tail in [",(1101,'Unfinished", ",(1101,'Mount',1", ""] {
            let dump = format!(
                "INSERT INTO `creature_template` VALUES (1100,'Mount','',1,1,295,0,0,0){tail}"
            );
            let error = super::taxi_catalogue_sql(
                &nodes,
                &paths,
                &points,
                &dump,
                &HashSet::from([295, 1147]),
            )
            .unwrap_err();
            assert!(error
                .to_string()
                .contains("unterminated creature_template INSERT"));
        }
    }

    #[test]
    fn taxi_catalogue_sql_preserves_direction_fare_mounts_and_point_order() {
        let (nodes, paths, points) = synthetic_taxi_catalogue();
        let TaxiCatalogueSql {
            statements: stmts,
            counts,
            warnings,
        } = catalogue_with_mounts(&nodes, &paths, &points).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(
            counts,
            TaxiCounts {
                nodes: 2,
                paths: 1,
                path_nodes: 3,
            }
        );
        assert_eq!(
            &stmts[..3],
            [
                "DELETE FROM game_taxi_path_node WHERE id >= 0",
                "DELETE FROM game_taxi_path WHERE id >= 0",
                "DELETE FROM game_taxi_node WHERE id >= 0",
            ],
            "stale geometry is removed before its parent routes and nodes"
        );

        let node_insert = stmts
            .iter()
            .find(|sql| sql.starts_with("INSERT INTO game_taxi_node "))
            .unwrap();
        assert!(
            node_insert.contains("(10,10,0,10,2,3,'O''Ryan''s Source',295,1147)"),
            "Horde/Alliance creature entries must resolve to displays: {node_insert}"
        );

        let path_insert = stmts
            .iter()
            .find(|sql| sql.starts_with("INSERT INTO game_taxi_path "))
            .unwrap();
        assert!(path_insert.contains("(70,10,20,125)"), "{path_insert}");
        assert!(
            !path_insert.contains("(70,20,10,125)"),
            "a directed path must not grow an inferred reverse: {path_insert}"
        );

        let point_insert = stmts
            .iter()
            .find(|sql| sql.starts_with("INSERT INTO game_taxi_path_node "))
            .unwrap();
        let p0 = point_insert.find("(701,70,0,0,10,5,6,1,0)").unwrap();
        let p1 = point_insert.find("(702,70,1,0,20,5,6,2,450000)").unwrap();
        let p2 = point_insert
            .find("(703,70,2,0,30,5,6,-2147483648,900000)")
            .unwrap();
        assert!(
            p0 < p1 && p1 < p2,
            "path points are not ordered: {point_insert}"
        );
    }

    #[test]
    fn taxi_catalogue_sql_keeps_valid_geometry_when_a_point_references_an_absent_path() {
        let (nodes, paths, mut points) = synthetic_taxi_catalogue();
        points.rows.push(taxi_point_row(5221, 248, 0, 40.0, 0, 0));

        let TaxiCatalogueSql {
            statements: stmts,
            counts,
            warnings,
        } = catalogue_with_mounts(&nodes, &paths, &points).unwrap();

        assert_eq!(counts.path_nodes, 3);
        assert_eq!(
            warnings,
            ["dbc: WARN TaxiPathNode.dbc point 5221 references missing path 248; omitted"]
        );
        let path_insert = stmts
            .iter()
            .find(|sql| sql.starts_with("INSERT INTO game_taxi_path "))
            .unwrap();
        assert!(path_insert.contains("(70,10,20,125)"), "{path_insert}");
        let point_insert = stmts
            .iter()
            .find(|sql| sql.starts_with("INSERT INTO game_taxi_path_node "))
            .unwrap();
        assert!(point_insert.contains("(701,70,0,"), "{point_insert}");
        assert!(!point_insert.contains("(5221,248,0,"), "{point_insert}");
    }

    #[test]
    fn taxi_catalogue_sql_keeps_valid_geometry_when_a_path_has_no_points() {
        let (nodes, mut paths, points) = synthetic_taxi_catalogue();
        paths.rows.push(taxi_path_row(472, 20, 10, 50));

        let TaxiCatalogueSql {
            statements: stmts,
            counts,
            warnings,
        } = catalogue_with_mounts(&nodes, &paths, &points).unwrap();

        assert_eq!(
            counts,
            TaxiCounts {
                nodes: 2,
                paths: 1,
                path_nodes: 3,
            }
        );
        assert_eq!(
            warnings,
            ["dbc: WARN TaxiPath.dbc path 472 has no TaxiPathNode.dbc points; omitted"]
        );
        let path_insert = stmts
            .iter()
            .find(|sql| sql.starts_with("INSERT INTO game_taxi_path "))
            .unwrap();
        assert!(path_insert.contains("(70,10,20,125)"), "{path_insert}");
        assert!(!path_insert.contains("(472,20,10,50)"), "{path_insert}");

        let point_insert = stmts
            .iter()
            .find(|sql| sql.starts_with("INSERT INTO game_taxi_path_node "))
            .unwrap();
        let p0 = point_insert.find("(701,70,0,").unwrap();
        let p1 = point_insert.find("(702,70,1,").unwrap();
        let p2 = point_insert.find("(703,70,2,").unwrap();
        assert!(p0 < p1 && p1 < p2, "{point_insert}");
    }

    #[test]
    fn taxi_catalogue_sql_is_deterministic_and_restores_the_reserved_fixture() {
        use lyracore_shared::constants::taxi_fixture as fixture;

        let (nodes, paths, points) = synthetic_taxi_catalogue();
        let first = catalogue_with_mounts(&nodes, &paths, &points)
            .unwrap()
            .statements;

        let mut nodes_reversed = nodes.clone();
        nodes_reversed.rows.reverse();
        let mut points_reversed = points.clone();
        points_reversed.rows.reverse();
        let second = catalogue_with_mounts(&nodes_reversed, &paths, &points_reversed)
            .unwrap()
            .statements;
        assert_eq!(
            first, second,
            "DBC record order must not change emitted SQL"
        );

        let all = first.join("\n");
        for id in [
            fixture::SOURCE_NODE_STORAGE_ID,
            fixture::DESTINATION_NODE_STORAGE_ID,
            fixture::PATH_ID,
            fixture::POINT_IDS[0],
            fixture::POINT_IDS[1],
            fixture::POINT_IDS[2],
        ] {
            assert!(
                all.contains(&id.to_string()),
                "successful replacement omitted reserved fixture id {id}"
            );
        }

        use lyracore_shared::constants::taxi_protocol;
        assert_eq!(taxi_protocol::CLIENT_NODE_ID_MAX, 8 * u32::BITS);
        assert_eq!(
            fixture::DESTINATION_CLIENT_NODE_ID,
            taxi_protocol::CLIENT_NODE_ID_MAX,
            "the fixture pins the final representable vanilla taxi-mask bit"
        );
        assert!(
            all.contains(&format!(
                "({},{},0,",
                fixture::SOURCE_NODE_STORAGE_ID,
                fixture::SOURCE_CLIENT_NODE_ID
            )) && all.contains(&format!(
                "({},{},0,",
                fixture::DESTINATION_NODE_STORAGE_ID,
                fixture::DESTINATION_CLIENT_NODE_ID
            )),
            "fixture storage ids and client ids must remain separate: {all}"
        );
    }

    #[test]
    fn taxi_catalogue_rejects_dangling_path_endpoints_before_emitting_clear_sql() {
        let (nodes, _, _) = synthetic_taxi_catalogue();
        let bad_path = DbcTaxiPath {
            rows: vec![taxi_path_row(70, 10, 999, 125)],
        };
        let no_points = DbcTaxiPathNode { rows: vec![] };
        let err = catalogue_with_mounts(&nodes, &bad_path, &no_points)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing destination node 999"), "{err}");
    }

    #[test]
    fn taxi_catalogue_rejects_an_empty_file_before_emitting_clear_sql() {
        let (nodes, paths, _) = synthetic_taxi_catalogue();
        let err = catalogue_with_mounts(&nodes, &paths, &DbcTaxiPathNode { rows: vec![] })
            .unwrap_err()
            .to_string();
        assert!(err.contains("TaxiPathNode=0 rows"), "{err}");
    }

    #[test]
    fn taxi_catalogue_rejects_negative_fares_indices_delays_and_id_collisions() {
        let (nodes, _, _) = synthetic_taxi_catalogue();
        let negative_fare = DbcTaxiPath {
            rows: vec![taxi_path_row(70, 10, 20, -1)],
        };
        let point = DbcTaxiPathNode {
            rows: vec![taxi_point_row(701, 70, 0, 10.0, 0, 0)],
        };
        assert!(catalogue_with_mounts(&nodes, &negative_fare, &point)
            .unwrap_err()
            .to_string()
            .contains("negative fare"));

        let path = DbcTaxiPath {
            rows: vec![taxi_path_row(70, 10, 20, 1)],
        };
        let negative_index = DbcTaxiPathNode {
            rows: vec![taxi_point_row(701, 70, -1, 10.0, 0, 0)],
        };
        assert!(catalogue_with_mounts(&nodes, &path, &negative_index)
            .unwrap_err()
            .to_string()
            .contains("negative node index"));

        let negative_delay = DbcTaxiPathNode {
            rows: vec![taxi_point_row(701, 70, 0, 10.0, 0, -1)],
        };
        assert!(catalogue_with_mounts(&nodes, &path, &negative_delay)
            .unwrap_err()
            .to_string()
            .contains("negative delay"));

        let reserved_nodes = DbcTaxiNodes {
            rows: vec![taxi_node_row(
                lyracore_shared::constants::taxi_fixture::STORAGE_ID_FLOOR,
                "collision",
                [1, 2],
                0.0,
            )],
        };
        assert!(catalogue_with_mounts(
            &reserved_nodes,
            &DbcTaxiPath { rows: vec![] },
            &DbcTaxiPathNode { rows: vec![] }
        )
        .unwrap_err()
        .to_string()
        .contains("reserved fixture namespace"));

        for id in [
            0,
            lyracore_shared::constants::taxi_protocol::CLIENT_NODE_ID_MAX + 1,
        ] {
            let out_of_range = DbcTaxiNodes {
                rows: vec![taxi_node_row(id, "unrepresentable", [1, 2], 0.0)],
            };
            assert!(catalogue_with_mounts(
                &out_of_range,
                &DbcTaxiPath { rows: vec![] },
                &DbcTaxiPathNode { rows: vec![] }
            )
            .unwrap_err()
            .to_string()
            .contains("outside the vanilla client taxi-mask range"));
        }

        let wire_collision = DbcTaxiNodes {
            rows: vec![taxi_node_row(
                lyracore_shared::constants::taxi_fixture::SOURCE_CLIENT_NODE_ID,
                "wire collision",
                [1, 2],
                0.0,
            )],
        };
        assert!(catalogue_with_mounts(
            &wire_collision,
            &DbcTaxiPath { rows: vec![] },
            &DbcTaxiPathNode { rows: vec![] }
        )
        .unwrap_err()
        .to_string()
        .contains("reserved fixture client node id"));
    }

    #[test]
    fn area_sql_shape_and_escaping() {
        let table = DbcAreaTable {
            rows: vec![
                area_table_row(12, 0, 0, 4479, AreaFlags::new(0x40), 1, 1, "Elwynn Forest"),
                area_table_row(9, 0, 12, 4480, AreaFlags::new(0), 1, 1, "Goldshire"),
                area_table_row(999, 0, 0, 1, AreaFlags::new(0), 0, 0, "O'Ryan's Rest"),
            ],
        };
        let (stmts, n) = area_sql(&table);
        assert_eq!(n, 3);
        assert_eq!(stmts[0], "DELETE FROM game_area WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_area "))
            .unwrap();
        assert!(
            insert.contains("(12,0,0,4479,64,1,1,'Elwynn Forest')"),
            "{insert}"
        );
        assert!(
            insert.contains("(9,0,12,4480,0,1,1,'Goldshire')"),
            "{insert}"
        );
        assert!(
            insert.contains("(999,0,0,1,0,0,0,'O''Ryan''s Rest')"),
            "{insert}"
        );
    }

    #[test]
    fn area_trigger_sql_shape_goldshire_inn_fixture() {
        let table = DbcAreaTrigger {
            rows: vec![area_trigger_row(
                1524, 0, -9454.5, 39.8, 64.0, 0.0, 6.0, 4.0, 6.0, 1.55,
            )],
        };
        let (stmts, n) = area_trigger_sql(&table);
        assert_eq!(n, 1);
        assert_eq!(stmts[0], "DELETE FROM game_area_trigger WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_area_trigger "))
            .unwrap();
        let expected = format!(
            "({},{},{},{},{},{},{},{},{},{})",
            1524, 0, -9454.5f32, 39.8f32, 64.0f32, 0.0f32, 6.0f32, 4.0f32, 6.0f32, 1.55f32,
        );
        assert!(insert.contains(&expected), "{insert}");
    }

    #[test]
    fn graveyard_sql_shape() {
        let table = DbcWorldSafeLocs {
            rows: vec![
                world_safe_locs_row(105, 0, -8935.33, -188.646, 80.4165, "Northshire Abbey"),
                world_safe_locs_row(80, 0, -10650.0, 1180.0, 34.0, "Sentinel Hill"),
            ],
        };
        let (stmts, n) = graveyard_sql(&table);
        assert_eq!(n, 2);
        assert_eq!(stmts[0], "DELETE FROM game_graveyard WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_graveyard "))
            .unwrap();
        let expected0 = format!(
            "({},{},{},{},{},0,{})",
            105, 0, -8935.33f32, -188.646f32, 80.4165f32, "'Northshire Abbey'"
        );
        assert!(insert.contains(&expected0), "{insert}");
        let expected1 = format!(
            "({},{},{},{},{},0,{})",
            80, 0, -10650.0f32, 1180.0f32, 34.0f32, "'Sentinel Hill'"
        );
        assert!(insert.contains(&expected1), "{insert}");
    }

    fn creature_family_row(
        id: u32,
        name: &str,
        pet_food_mask: i32,
        pet_talent_type: i32,
        category: i32,
    ) -> CreatureFamilyRow {
        CreatureFamilyRow {
            id: CreatureFamilyKey::new(id),
            min_scale: 0.0,
            min_scale_level: 0,
            max_scale: 0.0,
            max_scale_level: 0,
            pet_food_mask,
            pet_talent_type,
            category,
            name: LocalizedString {
                en_gb: name.to_string(),
                ..Default::default()
            },
            icon_path: String::new(),
        }
    }

    #[test]
    fn creature_family_sql_shape_escaping_and_tameable_count() {
        let table = DbcCreatureFamily {
            rows: vec![
                creature_family_row(1, "Wolf", 1, 0, 8),
                creature_family_row(3, "O'Ryan's Beast", 6, -1, 8),
            ],
        };
        let (stmts, n) = creature_family_sql(&table);
        assert_eq!(n, 2);
        assert_eq!(
            stmts[0],
            "DELETE FROM game_creature_family WHERE family_id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_family "))
            .unwrap();
        assert!(insert.contains("(1,'Wolf',1,0,8)"), "{insert}");
        assert!(insert.contains("(3,'O''Ryan''s Beast',6,-1,8)"), "{insert}");
        let tameable = table
            .rows()
            .iter()
            .filter(|r| r.pet_talent_type != -1)
            .count();
        assert_eq!(
            tameable, 1,
            "only the Wolf row (talent 0) is tameable; the -1 row is not"
        );
    }

    fn lock_row(id: u32, entries: &[(LockType, u32, i32)]) -> LockRow {
        let mut ty = [LockType::None; 8];
        let mut property = [0u32; 8];
        let mut required_skill = [0i32; 8];
        for (i, &(t, p, r)) in entries.iter().enumerate() {
            ty[i] = t;
            property[i] = p;
            required_skill[i] = r;
        }
        LockRow {
            id: LockKey::new(id),
            ty,
            property,
            required_skill,
            action: [0i32; 8],
        }
    }

    #[test]
    fn lock_sql_emits_only_non_none_indices_with_item_and_skill_kinds() {
        let table = DbcLock {
            rows: vec![
                lock_row(1, &[(LockType::ItemRequired, 12345, 0)]),
                lock_row(
                    2,
                    &[
                        (LockType::LocktypeReference, 1, 100),
                        (LockType::LocktypeReference, 3, 1),
                    ],
                ),
                lock_row(3, &[(LockType::LocktypeReference, 999, 50)]),
            ],
        };
        let (stmts, n, unmapped) = lock_sql(&table);
        assert_eq!(n, 4);
        assert_eq!(stmts[0], "DELETE FROM game_lock WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_lock "))
            .unwrap();
        assert!(insert.contains("(1,1,0,1,12345,0)"), "{insert}");
        assert!(insert.contains("(2,2,0,2,633,100)"), "{insert}");
        assert!(insert.contains("(3,2,1,2,186,1)"), "{insert}");
        assert!(insert.contains("(4,3,0,2,0,50)"), "{insert}");
        assert_eq!(
            unmapped,
            std::collections::BTreeSet::from([999u32]),
            "only the truly-unmapped id is flagged"
        );
    }

    #[test]
    fn lock_sql_all_none_lock_emits_no_rows() {
        let table = DbcLock {
            rows: vec![lock_row(9, &[])],
        };
        let (_, n, unmapped) = lock_sql(&table);
        assert_eq!(n, 0);
        assert!(unmapped.is_empty());
    }

    #[test]
    fn locktype_skill_line_covers_the_three_repo_verified_ids_only() {
        assert_eq!(locktype_skill_line(1), Some(633));
        assert_eq!(locktype_skill_line(2), Some(182));
        assert_eq!(locktype_skill_line(3), Some(186));
        assert_eq!(locktype_skill_line(4), None);
        assert_eq!(locktype_skill_line(0), None);
    }

    fn skill_line_row(id: u32, category: u32, name: &str) -> SkillLineRow {
        SkillLineRow {
            id: SkillLineKey::new(id),
            category: SkillLineCategoryKey::new(category),
            skill_costs: SkillCostsDataKey::new(0),
            display_name: LocalizedString {
                en_gb: name.to_string(),
                ..Default::default()
            },
            description: LocalizedString::default(),
            spell_icon: SpellIconKey::new(0),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn skill_ability_row(
        id: u32,
        skill_line: u32,
        spell: u32,
        race_mask: u32,
        class_mask: u32,
        min_skill: u32,
        acquire_method: i32,
        gray: i32,
        green: i32,
    ) -> SkillLineAbilityRow {
        SkillLineAbilityRow {
            id: SkillLineAbilityKey::new(id),
            skill_line: SkillLineKey::new(skill_line),
            spell: SpellKey::new(spell),
            race_mask: SkillChrRacesKey::new(race_mask),
            class_mask: ChrClassesKey::new(class_mask),
            exclude_race: SkillChrRacesKey::new(0),
            exclude_class: ChrClassesKey::new(0),
            superseded_by: SpellKey::new(min_skill),
            acquire_method: 0,
            trivial_skill_line_rank_high: acquire_method,
            trivial_skill_line_rank_low: gray,
            character_points: [green, 0],
            num_skills_up: 1,
            unknown_padding: 0,
        }
    }

    #[test]
    fn spell_rank_links_read_the_successor_column() {
        let mut row = skill_ability_row(1, 26, 78, 0, 1, 75, 2, 100, 50);
        row.acquire_method = 284;
        let table = DbcSkillLineAbility { rows: vec![row] };
        assert_eq!(spell_rank_links(&table).unwrap(), vec![(78, 284)]);
    }

    #[test]
    fn spell_rank_links_reject_negative_successors() {
        let mut row = skill_ability_row(1, 26, 78, 0, 1, 0, 0, 0, 0);
        row.acquire_method = -1;
        assert!(spell_rank_links(&DbcSkillLineAbility { rows: vec![row] }).is_err());
    }

    fn skill_availability_row(
        id: u32,
        skill_line: u32,
        race_mask: u32,
        class_mask: u32,
        flags: i32,
        min_level: i32,
        skill_tier: u32,
    ) -> SkillRaceClassInfoRow {
        SkillRaceClassInfoRow {
            id: SkillRaceClassInfoKey::new(id),
            skill_line: SkillLineKey::new(skill_line),
            race_mask: SkillChrRacesKey::new(race_mask),
            class_mask: ChrClassesKey::new(class_mask),
            flags,
            min_level,
            skill_tier: SkillTiersKey::new(skill_tier),
            skill_cost: SkillCostsDataKey::new(0),
        }
    }

    fn skill_tiers_row(id: u32, values: [i32; 4]) -> SkillTiersRow {
        let mut value = [0i32; 16];
        value[..4].copy_from_slice(&values);
        SkillTiersRow {
            id: SkillTiersKey::new(id),
            cost: [0; 16],
            value,
        }
    }

    #[test]
    fn skill_line_sql_shape_and_escaping() {
        let table = DbcSkillLine {
            rows: vec![
                skill_line_row(185, 11, "Cooking"),
                skill_line_row(44, 7, "Axes"),
                skill_line_row(999, 0, "O'Ryan's Line"),
            ],
        };
        let (stmts, n) = skill_line_sql(&table);
        assert_eq!(n, 3);
        assert_eq!(
            stmts[0],
            "DELETE FROM game_skill_line WHERE skill_line >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_skill_line "))
            .unwrap();
        assert!(insert.contains("(185,11,'Cooking')"), "{insert}");
        assert!(insert.contains("(44,7,'Axes')"), "{insert}");
        assert!(insert.contains("(999,0,'O''Ryan''s Line')"), "{insert}");
    }

    #[test]
    fn skill_ability_sql_remaps_off_by_one_dbc_fields() {
        let table = DbcSkillLineAbility {
            rows: vec![
                skill_ability_row(1, 185, 2550, 0xFFFF_FFFF, 0xFFFF_FFFF, 25, 0, 75, 30),
                skill_ability_row(2, 333, 13262, 0xFFFF_FFFF, 0xFFFF_FFFF, 0, 1, 0, 0),
            ],
        };
        let (stmts, n) = skill_ability_sql(&table);
        assert_eq!(n, 2);
        assert_eq!(stmts[0], "DELETE FROM game_skill_ability WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_skill_ability "))
            .unwrap();
        assert!(
            insert.contains("(1,2550,185,4294967295,4294967295,25,0,75,30)"),
            "{insert}"
        );
        assert!(
            insert.contains("(2,13262,333,4294967295,4294967295,0,1,0,0)"),
            "{insert}"
        );
        let autolearn = table
            .rows()
            .iter()
            .filter(|r| r.trivial_skill_line_rank_high == AUTOLEARN_ACQUIRE_METHOD)
            .count();
        assert_eq!(autolearn, 1);
    }

    #[test]
    fn skill_ability_sql_pairs_riding_mount_spells_with_the_skill_line_and_tiers() {
        const RIDING_SKILL_LINE: u32 = 762;
        const RACE_HUMAN: u32 = 0x1;
        const CLASS_ALL: u32 = 0xFFFF_FFFF;
        let table = DbcSkillLineAbility {
            rows: vec![
                skill_ability_row(
                    1,
                    RIDING_SKILL_LINE,
                    458,
                    RACE_HUMAN,
                    CLASS_ALL,
                    75,
                    0,
                    0,
                    0,
                ),
                skill_ability_row(
                    2,
                    RIDING_SKILL_LINE,
                    6648,
                    0xFFFF_FFFF,
                    CLASS_ALL,
                    150,
                    0,
                    0,
                    0,
                ),
            ],
        };
        let (stmts, n) = skill_ability_sql(&table);
        assert_eq!(n, 2);
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_skill_ability "))
            .unwrap();
        assert!(
            insert.contains(&format!(
                "(1,458,{RIDING_SKILL_LINE},1,{CLASS_ALL},75,0,0,0)"
            )),
            "{insert}"
        );
        assert!(
            insert.contains(&format!(
                "(2,6648,{RIDING_SKILL_LINE},4294967295,{CLASS_ALL},150,0,0,0)"
            )),
            "{insert}"
        );
    }

    #[test]
    fn skill_availability_sql_shape() {
        let table = DbcSkillRaceClassInfo {
            rows: vec![
                skill_availability_row(1, 185, 0xFFFF_FFFF, 0xFFFF_FFFF, 0, 1, 10),
                skill_availability_row(2, 44, 1, 1, 0, 1, 0),
            ],
        };
        let (stmts, n) = skill_availability_sql(&table);
        assert_eq!(n, 2);
        assert_eq!(
            stmts[0],
            "DELETE FROM game_skill_availability WHERE id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_skill_availability "))
            .unwrap();
        assert!(
            insert.contains("(1,185,4294967295,4294967295,0,1)"),
            "{insert}"
        );
        assert!(insert.contains("(2,44,1,1,0,1)"), "{insert}");
    }

    #[test]
    fn profession_tier_values_resolves_apprentice_through_artisan() {
        let availability = DbcSkillRaceClassInfo {
            rows: vec![skill_availability_row(
                1,
                186,
                0xFFFF_FFFF,
                0xFFFF_FFFF,
                0,
                1,
                42,
            )],
        };
        let tiers = DbcSkillTiers {
            rows: vec![skill_tiers_row(42, [75, 150, 225, 300])],
        };
        let map = resolve_profession_tier_values(&availability, &tiers);
        assert_eq!(map.get(&186), Some(&[75u16, 150, 225, 300]));
    }

    #[test]
    fn profession_tier_values_skips_past_a_non_tier_variant_row_to_the_resolving_one() {
        let availability = DbcSkillRaceClassInfo {
            rows: vec![
                skill_availability_row(1, 186, 0x1, 0x1, 0, 1, 0),
                skill_availability_row(2, 186, 0xFFFF_FFFF, 0xFFFF_FFFF, 0, 1, 42),
            ],
        };
        let tiers = DbcSkillTiers {
            rows: vec![skill_tiers_row(42, [75, 150, 225, 300])],
        };
        let map = resolve_profession_tier_values(&availability, &tiers);
        assert_eq!(map.get(&186), Some(&[75u16, 150, 225, 300]));
    }

    #[test]
    fn profession_tier_values_skips_an_unresolvable_line() {
        let availability = DbcSkillRaceClassInfo { rows: vec![] };
        let tiers = DbcSkillTiers { rows: vec![] };
        let map = resolve_profession_tier_values(&availability, &tiers);
        assert!(map.is_empty());
    }

    #[test]
    fn profession_tier_values_matches_the_legacy_literals_for_every_profession_learn_line() {
        let mut availability_rows = Vec::new();
        let mut tier_rows = Vec::new();
        for (i, &line) in PROFESSION_SKILL_LINES.iter().enumerate() {
            let tier_id = 100 + i as u32;
            availability_rows.push(skill_availability_row(
                i as u32 + 1,
                line,
                0xFFFF_FFFF,
                0xFFFF_FFFF,
                0,
                1,
                tier_id,
            ));
            tier_rows.push(skill_tiers_row(tier_id, [75, 150, 225, 300]));
        }
        let availability = DbcSkillRaceClassInfo {
            rows: availability_rows,
        };
        let tiers = DbcSkillTiers { rows: tier_rows };
        let map = resolve_profession_tier_values(&availability, &tiers);

        assert_eq!(map.len(), PROFESSION_SKILL_LINES.len());
        for &(line, entries) in crate::PROFESSION_LEARN {
            let caps = map
                .get(&line)
                .unwrap_or_else(|| panic!("no resolved tier caps for line {line}"));
            for &(_, tier_index, _) in entries {
                assert_eq!(
                    caps[tier_index as usize] as u32,
                    crate::LEGACY_TIER_CAPS[tier_index as usize],
                    "line {line} tier {tier_index}"
                );
            }
        }
        crate::assert_profession_tier_parity(&map)
            .expect("fixture data must satisfy the parity guard");
    }

    #[test]
    fn standalone_dbc_requires_mount_templates_before_opening_inputs() {
        let shard = Shard {
            server: "local".into(),
            db: "lyracore".into(),
        };
        let error = run("/nonexistent/client/Data", None, false, &shard).unwrap_err();
        assert!(error.to_string().contains("--taxi-mounts"));
    }
}
