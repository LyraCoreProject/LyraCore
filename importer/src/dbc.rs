//! Stream 1 (client DBC) — extract `DBFilesClient/*.dbc` from the operator's OWN client MPQ archives
//! (`wow-mpq`) and parse them (gtker `wow_dbc`), emitting only derived `game_*` values.
//!
//! LICENSING FIREWALL (the firewall is this code's behavior, not just `.gitignore`): Blizzard client
//! data is copyrighted. The invariant is **never REDISTRIBUTE Blizzard data** — we hold extracted bytes
//! IN MEMORY ONLY and NEVER write a `.dbc` (or any Blizzard file) to the repo tree. The READ path here
//! only reads. The client-patch packager (`pack_client.rs`) DOES use `wow-mpq`'s builder, but only to
//! emit a patch MPQ into the operator's OWN client install (a local build artifact, never committed and
//! never shipped by us), built from the operator's own base bytes + our additions. The operator's MPQs
//! live outside the repo (their client install). Native-only, like the whole importer.
//!
//! `wow_dbc::T::read` hard-asserts the build-5875 `record_size`/`field_count` for each table, so a
//! wrong-version / heavily-patched client surfaces as a clear parse error here (the version guard).

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use anyhow::{bail, Context, Result};
use wow_dbc::vanilla_tables::area_table::AreaTable as DbcAreaTable;
use wow_dbc::vanilla_tables::area_trigger::AreaTrigger as DbcAreaTrigger;
use wow_dbc::vanilla_tables::char_base_info::CharBaseInfo as DbcCharBaseInfo;
use wow_dbc::vanilla_tables::char_start_outfit::CharStartOutfit as DbcCharStartOutfit;
use wow_dbc::vanilla_tables::chr_races::{ChrRaces, ChrRacesKey};
use wow_dbc::vanilla_tables::creature_display_info::{CreatureDisplayInfo, CreatureDisplayInfoKey};
use wow_dbc::vanilla_tables::creature_family::CreatureFamily as DbcCreatureFamily;
use wow_dbc::vanilla_tables::faction::Faction as DbcFaction;
use wow_dbc::vanilla_tables::faction_template::FactionTemplate as DbcFactionTemplate;
use wow_dbc::vanilla_tables::lock::Lock as DbcLock;
use wow_dbc::vanilla_tables::skill_line::SkillLine as DbcSkillLine;
use wow_dbc::vanilla_tables::skill_line_ability::SkillLineAbility as DbcSkillLineAbility;
use wow_dbc::vanilla_tables::skill_race_class_info::SkillRaceClassInfo as DbcSkillRaceClassInfo;
use wow_dbc::vanilla_tables::skill_tiers::SkillTiers as DbcSkillTiers;
use wow_dbc::vanilla_tables::world_safe_locs::WorldSafeLocs as DbcWorldSafeLocs;
use wow_dbc::{DbcTable, Indexable};
use wow_mpq::PatchChain;
use wow_world_base::vanilla::LockType;

use crate::{push_insert, sql_text, Args};

/// Open the operator's DBC patch chain in client load order — base `dbc.MPQ` < `patch.MPQ` <
/// `patch-2.MPQ` (higher priority wins, so a table re-shipped in a patch overrides the base). Only
/// archives that actually exist are added (a fresh install may lack `patch-2.MPQ`). READ-ONLY.
pub(crate) fn open_chain(data_dir: &Path) -> Result<PatchChain> {
    let mut chain = PatchChain::new();
    let mut added = 0u32;
    // (filename, priority) — higher priority overrides lower.
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

/// Read + parse one DBC table by its `DbcTable::FILENAME`, resolved through the patch chain. The
/// archive stores it at `DBFilesClient\<name>` (backslash path; MPQ lookup is by name-hash, so the
/// hash-encrypted filename table is a non-issue — we request the known path). Bytes are decompressed
/// transparently and held in memory only — never written to disk.
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

/// `--dbc <Data dir>` mode. P1a (proof-of-toolchain): confirm `wow-mpq` opens the operator's real
/// `dbc.MPQ`, the known-path lookup works, `wow_dbc` accepts the 5875 schema, and a foreign-key chase
/// resolves — by printing Human's male display (must be 49) and the demo chicken's display-304 scale.
/// Writes NO game data and NO `.dbc` file; it only reads + prints, so it's safe to run any time.
pub fn run(data_dir: &str, args: &Args) -> Result<()> {
    let dir = Path::new(data_dir);
    let mut chain = open_chain(dir)?;
    eprintln!("dbc: opened MPQ chain from {}", dir.display());

    // ChrRaces: race 1 = Human; its male_display is a CreatureDisplayInfo key (== 49 for human male).
    let races: ChrRaces = read_table(&mut chain)?;
    let human = races
        .get(ChrRacesKey::new(1))
        .context("ChrRaces has no race 1 (Human) — truncated extract?")?;
    println!(
        "ChrRaces: {} races; Human male_display = {} (expect 49)",
        races.rows().len(),
        human.male_display.id
    );

    // CreatureDisplayInfo: the demo chicken uses display 304; its creature_model_scale feeds entity scale.
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

    // Load game_faction_template (hostility data) + game_faction (reputation metadata) + game_race_info
    // (P1: per-race display + faction). All no-Timestamp → plain SQL.
    let (faction_stmts, fn_count) = faction_template_sql(&mut chain)?;
    let (gf_stmts, gf_count) = faction_sql(&mut chain)?;
    let (cbi_stmts, cbi_count) = char_base_info_sql(&mut chain)?;
    let (race_stmts, race_count) = race_info_sql(&races);
    let (si_stmts, si_count) = start_item_sql(&mut chain)?;

    // Areas / area triggers / graveyards (work-item 209): AreaTable.dbc → game_area, AreaTrigger.dbc
    // → game_area_trigger, WorldSafeLocs.dbc → game_graveyard. All small tables — load ALL rows (no
    // map filtering here; that's a verify-time concern, see import-world.sh). All no-Timestamp →
    // plain SQL, same clear+reload shape as the blocks above.
    let area_table: DbcAreaTable = read_table(&mut chain)?;
    let (area_stmts, area_count) = area_sql(&area_table);
    let area_triggers: DbcAreaTrigger = read_table(&mut chain)?;
    let (trigger_stmts, trigger_count) = area_trigger_sql(&area_triggers);
    let safe_locs: DbcWorldSafeLocs = read_table(&mut chain)?;
    let (graveyard_stmts, graveyard_count) = graveyard_sql(&safe_locs);

    // Creature families (work-item 214): CreatureFamily.dbc → game_creature_family. Small table (no
    // map filtering — same "load ALL rows" convention as the other DBC-backed lookups above). No
    // Timestamp → plain SQL, same clear+reload shape.
    let creature_families: DbcCreatureFamily = read_table(&mut chain)?;
    let (family_stmts, family_count) = creature_family_sql(&creature_families);
    let tameable_family_count = creature_families
        .rows()
        .iter()
        .filter(|r| r.pet_talent_type != -1)
        .count();

    // Locks (work-item 211): Lock.dbc → game_lock, the DATA half of open-lock (119 wires enforcement).
    // Small table (a few hundred rows in vanilla) — load ALL, same "load ALL rows" convention as the
    // other DBC-backed lookups above. No Timestamp → plain SQL, same clear+reload shape.
    let locks: DbcLock = read_table(&mut chain)?;
    let (lock_stmts, lock_count, lock_unmapped) = lock_sql(&locks);

    // Load game_skill_line + game_skill_ability + game_skill_availability from SkillLine.dbc /
    // SkillLineAbility.dbc / SkillRaceClassInfo.dbc (work-item 208: the skill fabric as data — see
    // module/src/skilldata.rs). All no-Timestamp → plain SQL, same clear+reload shape as the blocks
    // above. `SkillTiers.dbc` is NOT loaded here (no `game_*` table of its own) — it only feeds
    // `profession_tier_values` below, which `importer/src/main.rs` uses for the profession trainer
    // tier-cap parity guard.
    let skill_lines: DbcSkillLine = read_table(&mut chain)?;
    let skill_abilities: DbcSkillLineAbility = read_table(&mut chain)?;
    let skill_availability: DbcSkillRaceClassInfo = read_table(&mut chain)?;
    let autolearn_count = skill_abilities
        .rows()
        .iter()
        // real AcquireMethod lives in the mis-named `trivial_skill_line_rank_high` (282, see skill_ability_sql).
        .filter(|r| r.trivial_skill_line_rank_high == AUTOLEARN_ACQUIRE_METHOD)
        .count();
    let (sl_stmts, sl_count) = skill_line_sql(&skill_lines);
    let (sa_stmts, sa_count) = skill_ability_sql(&skill_abilities);
    let (sav_stmts, sav_count) = skill_availability_sql(&skill_availability);

    if args.apply {
        crate::run_sql_statements(args, &faction_stmts, "faction")?;
        eprintln!("dbc: loaded {fn_count} faction templates into game_faction_template.");
        crate::run_sql_statements(args, &gf_stmts, "game_faction")?;
        eprintln!("dbc: loaded {gf_count} factions into game_faction.");
        crate::run_sql_statements(args, &cbi_stmts, "charbaseinfo")?;
        eprintln!("dbc: loaded {cbi_count} (race,class) combos into game_char_base_info.");
        crate::run_sql_statements(args, &race_stmts, "race")?;
        eprintln!("dbc: loaded {race_count} races into game_race_info.");
        crate::run_sql_statements(args, &si_stmts, "startitem")?;
        eprintln!("dbc: loaded {si_count} starting items into game_start_item.");
        crate::run_sql_statements(args, &sl_stmts, "skillline")?;
        eprintln!("dbc: loaded {sl_count} skill lines into game_skill_line.");
        crate::run_sql_statements(args, &sa_stmts, "skillability")?;
        eprintln!("dbc: loaded {sa_count} skill abilities into game_skill_ability.");
        crate::run_sql_statements(args, &sav_stmts, "skillavailability")?;
        eprintln!("dbc: loaded {sav_count} availability rows into game_skill_availability.");
        crate::run_sql_statements(args, &area_stmts, "area")?;
        eprintln!("dbc: loaded {area_count} areas into game_area.");
        crate::run_sql_statements(args, &trigger_stmts, "areatrigger")?;
        eprintln!("dbc: loaded {trigger_count} area triggers into game_area_trigger.");
        crate::run_sql_statements(args, &graveyard_stmts, "graveyard")?;
        eprintln!("dbc: loaded {graveyard_count} graveyards into game_graveyard.");
        crate::run_sql_statements(args, &family_stmts, "creaturefamily")?;
        eprintln!("dbc: loaded {family_count} creature families into game_creature_family.");
        crate::run_sql_statements(args, &lock_stmts, "lock")?;
        eprintln!("dbc: loaded {lock_count} lock indices into game_lock.");
    } else {
        println!("-- DRY RUN: load {fn_count} faction templates + {gf_count} factions + {cbi_count} (race,class) combos + {race_count} races:");
        println!("{};", faction_stmts[0]);
        if let Some(ins) = faction_stmts.get(1) {
            println!(
                "{}… ; -- (+{} faction INSERT chunk(s))",
                &ins[..ins.len().min(90)],
                faction_stmts.len() - 1
            );
        }
        for s in cbi_stmts.iter().chain(race_stmts.iter()) {
            println!("{}…", &s[..s.len().min(110)]);
        }
        eprintln!("dbc: dry-run — re-run with --apply to load.");
    }

    // Skill-import coverage prints (always printed — informational, not gated on --apply like the
    // SQL-preview lines above).
    println!("SkillLine: {sl_count} lines");
    println!("SkillLineAbility: {sa_count} abilities ({autolearn_count} autolearn)");
    println!("SkillRaceClassInfo: {sav_count} availability rows");
    // Work-item 209 coverage prints (always printed, like the skill lines above).
    println!("AreaTable: {area_count} areas");
    println!("AreaTrigger: {trigger_count} triggers");
    println!("WorldSafeLocs: {graveyard_count} graveyards");
    // Work-item 214 coverage print (always printed, like the ones above).
    println!("CreatureFamily: {family_count} families ({tameable_family_count} tameable)");
    // Work-item 211 coverage print (always printed, like the ones above). Loud about any Lock.dbc
    // LocktypeReference id `LOCKTYPE_TO_SKILL_LINE` doesn't cover — see that const's doc comment.
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
    Ok(())
}

/// Clear+reload SQL for `game_faction` from `Faction.dbc` (Elwynn faction system): each parent faction's
/// reputation index + the player's starting standing. `reputation_index` 0xFFFFFFFF (no rep bar) → -1.
/// `base_standing` = the primary race-group's base reputation (group 0), reinterpreted as signed (a Hated
/// faction stores a wrapped negative like 4294925296 == -42000); correct for our Human test character.
/// Per-race resolution via `reputation_race_mask` is a follow-up. No Timestamp → plain SQL.
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

/// Clear+reload SQL for `game_char_base_info` from `CharBaseInfo.dbc` (importer P1): the legal
/// (race, class) combos. No PK in the DBC — iterate. No Timestamp → plain SQL. PK packs `(race<<8)|class`.
fn char_base_info_sql(chain: &mut PatchChain) -> Result<(Vec<String>, usize)> {
    let cbi: DbcCharBaseInfo = read_table(chain)?;
    let rows: Vec<String> = cbi
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{})",
                ((r.race.id) << 8) | r.class.id,
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

/// The `CharStartOutfit.dbc` (race_class, item) set — shared by `start_item_sql` (the outfit's own
/// clear+reload) AND `main.rs`'s `build_createinfo_item_sql` (work-item 212: the cmangos dump's
/// `playercreateinfo_item` EXTRAS are deduped against this SAME set, so an item CharStartOutfit
/// already grants for a (race, class) is never double-listed). `race == 0 || class == 0` rows are
/// skipped (not a real playable combo) and `item == 0` slots (empty) are skipped, matching
/// `start_item_sql`'s own filtering exactly.
pub(crate) fn outfit_item_set(
    chain: &mut PatchChain,
) -> Result<std::collections::HashSet<(u32, i32)>> {
    let outfits: DbcCharStartOutfit = read_table(chain)?;
    let mut seen: std::collections::HashSet<(u32, i32)> = std::collections::HashSet::new();
    for o in outfits.rows().iter() {
        let (race, class) = (o.race.id, o.class.id);
        if race == 0 || class == 0 {
            continue;
        }
        let race_class = (race << 8) | class;
        for &item in o.item_id.iter() {
            if item > 0 {
                seen.insert((race_class, item));
            }
        }
    }
    Ok(seen)
}

/// Clear+reload SQL for `game_start_item` from `CharStartOutfit.dbc` — the per-(race,class) creation
/// loadout (the cmangos dump's `playercreateinfo_item` used to be treated as EMPTY; work-item 212 now
/// imports its EXTRAS separately — see `main.rs::build_createinfo_item_sql`, deduped against the SAME
/// (race_class, item) pairs this function writes via `outfit_item_set`). One row per non-empty item
/// slot, DEDUPED on (race_class, item) so the male+female outfits (identical item ids, differing only
/// in display) don't double-grant. Keyed by race_class = (race<<8)|class, matching game_start_position.
/// Ids are assigned densely from 1 — `build_createinfo_item_sql` uses a disjoint high id range
/// (`CREATEINFO_ITEM_ID_FLOOR`) so the two writers' DELETEs never touch each other's rows, regardless
/// of which pass runs first. No Timestamp → plain SQL.
fn start_item_sql(chain: &mut PatchChain) -> Result<(Vec<String>, usize)> {
    let outfits: DbcCharStartOutfit = read_table(chain)?;
    let mut seen: std::collections::HashSet<(u32, i32)> = std::collections::HashSet::new();
    let mut rows: Vec<String> = Vec::new();
    let mut id: u64 = 1;
    for o in outfits.rows().iter() {
        let (race, class) = (o.race.id, o.class.id);
        if race == 0 || class == 0 {
            continue;
        }
        let race_class = (race << 8) | class;
        for &item in o.item_id.iter() {
            if item > 0 && seen.insert((race_class, item)) {
                rows.push(format!("({id},{race_class},{item})"));
                id += 1;
            }
        }
    }
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_start_item WHERE id >= 0 AND id < 1000000".to_string()];
    crate::push_insert(
        &mut stmts,
        "game_start_item",
        "id,race_class,item_entry",
        &rows,
    );
    Ok((stmts, n))
}

/// `SkillLineAbility.dbc`'s `acquire_method` value meaning "automatically learned once the
/// race/class/skill conditions are met" (as opposed to 2 = learned from a trainer, 0 = not learnable
/// this way). The DBC carries no strings for this enum, so this is sourced from community DBC
/// documentation (wowdev.wiki), not something verified against our own client dump — it's informational
/// only HERE (it only drives the "N autolearn" coverage-print count); a future AUTOLEARN consumer
/// (work-item 208's "auto-learned spells at thresholds" follow-up) is what needs this value truly
/// load-bearing-correct, and can re-derive/correct it against a real client at that point.
const AUTOLEARN_ACQUIRE_METHOD: i32 = 1;

/// The profession `SkillLine.dbc` ids whose Apprentice→Artisan tier caps `profession_tier_values`
/// resolves — mirrors `module/src/skill.rs::skill_line`'s profession consts. Duplicated here, not
/// shared, because the wasm `module` crate is never a dependency of this native importer (same
/// reasoning as `spell.rs`'s taxonomy duplication) — keep the two lists in lockstep. NOTE: this is 12
/// lines, not 13 — `module/src/skill.rs`'s own doc comment says "completing the 13" but only ever
/// defines 12 profession consts (a pre-existing, harmless comment miscount noticed while wiring this
/// item; out of scope to fix here).
const PROFESSION_SKILL_LINES: &[u32] = &[
    186, // Mining
    182, // Herbalism
    393, // Skinning
    185, // Cooking
    129, // First Aid
    171, // Alchemy
    164, // Blacksmithing
    197, // Tailoring
    165, // Leatherworking
    202, // Engineering
    333, // Enchanting
    356, // Fishing
];

/// Clear+reload SQL for `game_skill_line` from `SkillLine.dbc` — one row per skill line (weapon lines,
/// Defense, the 12 professions, and a long tail of NPC-only/deprecated lines the client never shows a
/// player; ~135 total in vanilla). `category` is `SkillLineCategory.dbc`'s raw id (not separately
/// imported, kept as the plain foreign key — same "store the raw key" convention as
/// `race_info_sql`/`faction_sql`). Name via `.en_gb` + `sql_text` (apostrophe-escaped), matching
/// `talent.rs`'s `LocalizedString` convention. No Timestamp → plain SQL.
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

/// Clear+reload SQL for `game_skill_ability` from `SkillLineAbility.dbc` — a `(spell, skill_line)`
/// pairing plus its race/class gate, `acquire_method` (0=trained/learned-from-item, 1/2=autolearn
/// at a skill threshold), and recipe skill-up band `green`(TrivialLow)..`gray`(TrivialHigh) (~9-10k
/// rows in vanilla). ⚠ 282: wow_dbc 0.3's `SkillLineAbilityRow` MIS-NAMES every field from position
/// 8 (it omits the real `MinSkillLineRank` column), so this reads by CONTENT, not by wow_dbc's field
/// names — real `AcquireMethod` lives in `trivial_skill_line_rank_high`, real `TrivialHigh` in
/// `trivial_skill_line_rank_low`, real `TrivialLow` in `character_points[0]`, real `MinSkillLineRank`
/// in `superseded_by`. Ids are assigned sequentially (1-based) — the DBC's own `id` isn't reused
/// (mirrors `start_item_sql`). No Timestamp → plain SQL.
fn skill_ability_sql(table: &DbcSkillLineAbility) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // ⚠ wow_dbc 0.3 SkillLineAbilityRow MIS-NAMES every field from position 8 (work-item
            // 282): it omits the real `MinSkillLineRank` column, so each field's NAME lags the real
            // column by one while the byte OFFSET stays correct. CONFIRMED against the live import:
            // the field wow_dbc calls `acquire_method` holds SupercededBySpell SPELL IDS (thousands),
            // and the real 0/1/2 `AcquireMethod` enum sits in `trivial_skill_line_rank_high`. Remap
            // by CONTENT (same off-by-one class as the Spell.dbc InterruptFlags bug in spell.rs):
            let min_skill = r.superseded_by.id; // real MinSkillLineRank
            let acquire_method = r.trivial_skill_line_rank_high; // real AcquireMethod (0/1/2)
            let gray = r.trivial_skill_line_rank_low; // real TrivialSkillLineRankHigh
            let green = r.character_points[0]; // real TrivialSkillLineRankLow
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

/// Clear+reload SQL for `game_skill_availability` from `SkillRaceClassInfo.dbc` — which (race, class)
/// combos a skill line is available to, its availability flags, and the minimum character level to
/// pick it up (~700-800 rows in vanilla). Ids assigned sequentially, same convention as
/// `skill_ability_sql`. No Timestamp → plain SQL.
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

/// Clear+reload SQL for `game_race_info` from `ChrRaces.dbc` (importer P1): per-race male/female
/// display models + faction. No Timestamp → plain SQL. Skips race 0 (the "none" row). Returns
/// (statements, row_count). Keys (`.id`) expose the raw u32.
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

/// Clear+reload SQL for `game_faction_template` from `FactionTemplate.dbc`. No Timestamp columns, so
/// plain `spacetime sql` (the Timestamp-reducer rule doesn't apply). Chunked under a conservative row
/// count per INSERT. Returns (statements, row_count). Keys expose their raw id via `.id`.
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
    // `id >= 0` matches every u32 → clears the whole table before reload (clear+reload is idempotent).
    let mut stmts = vec!["DELETE FROM game_faction_template WHERE id >= 0".to_string()];
    for chunk in rows.chunks(500) {
        stmts.push(format!(
            "INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES {}",
            chunk.join(",")
        ));
    }
    Ok((stmts, n))
}

/// Clear+reload SQL for `game_area` from `AreaTable.dbc` (work-item 209): every zone AND subzone row
/// (id, map, parent, area_bit, flags, exploration level, faction group, name). `flags` is the raw
/// `AreaFlags` bitmask reinterpreted as `u32` (`as_int()` widened) — undecoded here, a consumer
/// decodes what it needs (rest-state city detection is 196's job). Name via `.en_gb` + `sql_text`
/// (apostrophe-escaping), matching `skill_line_sql`'s convention. Loads ALL rows (small table — map
/// filtering is a verify-time concern, not an import-time one). No Timestamp → plain SQL.
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

/// Clear+reload SQL for `game_area_trigger` from `AreaTrigger.dbc` (work-item 209): the geometric
/// half of inn triggers (196), dungeon entrances (190), and quest explore objectives — a trigger
/// volume is either a sphere (`radius`) or a box (`box_length`/`box_width`/`box_height`/`box_yaw`);
/// the DBC carries both fields for every row regardless of which shape a given trigger actually
/// uses. Loads ALL rows (small table — map filtering is a verify-time concern). No Timestamp →
/// plain SQL.
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

/// Clear+reload SQL for `game_graveyard` from `WorldSafeLocs.dbc` (work-item 209): a graveyard's
/// fixed position, replacing the hand-coded `world::graveyard::{NORTHSHIRE, GOLDSHIRE, ...}` consts
/// as the primary data source (those consts — and `seed.rs`'s row-seeded twins — remain the
/// no-import fallback). No orientation column — the DBC carries none. Name via `.en_gb` +
/// `sql_text`. Loads ALL rows (small table). No Timestamp → plain SQL.
fn graveyard_sql(table: &DbcWorldSafeLocs) -> (Vec<String>, usize) {
    let rows: Vec<String> = table
        .rows()
        .iter()
        .map(|r| {
            format!(
                // o = 0.0: WorldSafeLocs.dbc carries no orientation — the seed rows are the only
                // source of a real facing (the Northshire 2.72271 case).
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

/// Clear+reload SQL for `game_creature_family` from `CreatureFamily.dbc` (work-item 214: the 188 pet
/// system's data half — `CreatureTemplate.creature_family`/`type_flags` already import via
/// `main.rs`'s `ct::FAMILY`/`ct::CREATURE_TYPE_FLAGS`, and the Wolf faction fixup already reads
/// `family_id` off the dump; this is the last missing piece, the family lookup table itself). Name
/// via `.en_gb` + `sql_text` (apostrophe-escaping), matching `skill_line_sql`'s convention.
/// `pet_food_mask`/`pet_talent_type`/`category` are the raw DBC `int32` fields, signed — see
/// `game_creature_family`'s doc comment (`module/src/creatures/spawn.rs`) for their meaning. Loads
/// ALL rows (small table, ~30-40 in vanilla). No Timestamp → plain SQL.
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

/// `LockType.dbc` id -> module SkillLine id, for a Lock.dbc index whose `ty == LocktypeReference`
/// (there `property` is a LockType.dbc id, NOT a SkillLine — see `lock_sql`). REPO-VERIFIED: 1
/// Lockpicking, 2 Herbalism, 3 Mining (cmangos's `SkillByLockType[]` is the live-check reference for
/// any OTHER LockType id — [V] not extended here; an id outside this list resolves to `None` and
/// `lock_sql` surfaces it as an unmapped count in the coverage print rather than guessing).
const LOCKTYPE_TO_SKILL_LINE: &[(u32, u32)] = &[
    (1, 633), // Lockpicking
    (2, 182), // Herbalism
    (3, 186), // Mining
];

fn locktype_skill_line(locktype_id: u32) -> Option<u32> {
    LOCKTYPE_TO_SKILL_LINE
        .iter()
        .find(|&&(id, _)| id == locktype_id)
        .map(|&(_, skill)| skill)
}

/// Clear+reload SQL for `game_lock` from `Lock.dbc` (work-item 211 — the DATA half of open-lock; 119
/// wires the enforcement). Each Lock.dbc row packs FOUR parallel `[;8]` arrays: `ty`/`property`/
/// `required_skill`/`action` — up to 8 ALTERNATIVE ways to open the SAME lock (e.g. "the right key OR
/// enough Lockpicking"). Only the non-`LockType::None` indices are real; a lock with 1 requirement
/// emits exactly 1 `game_lock` row (see the module's `GameLock` doc, `module/src/gameobject.rs`).
/// `ty == ItemRequired` → `kind` 1, `property` IS the required key item entry, verbatim. `ty ==
/// LocktypeReference` → `kind` 2, `property` is a LockType.dbc id (NOT a SkillLine) resolved via
/// `locktype_skill_line` at IMPORT time, so the module never needs LockType.dbc itself — an
/// unmapped id resolves to SkillLine 0 (unknown) and is counted in the returned unmapped-id set so
/// `run()`'s coverage print can surface it loudly instead of silently miscoding a lock. `required_skill`
/// is the DBC's own signed value, floored at 0 (never negative in a real table). No Timestamp → plain
/// SQL, same clear+reload shape as `creature_family_sql`. Returns (statements, row count, unmapped
/// LockType ids encountered).
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

/// `display_id -> creature_model_scale` from CreatureDisplayInfo.dbc. The cmangos creature ETL uses
/// this to resolve `creature_template.Scale == 0` (vanilla's "use the DBC display scale" sentinel) at
/// import time — otherwise those creatures load with `scale_x = 0` and the client renders them at
/// scale 0. In-memory only (no `.dbc` written). The map is built once; lookups are O(1).
pub fn load_display_scales(data_dir: &str) -> Result<HashMap<u32, f32>> {
    let mut chain = open_chain(Path::new(data_dir))?;
    let cdi: CreatureDisplayInfo = read_table(&mut chain)?;
    let mut scales = HashMap::with_capacity(cdi.rows().len());
    for row in cdi.rows() {
        scales.insert(row.id.id, row.creature_model_scale);
    }
    Ok(scales)
}

/// `skill_line -> [Apprentice, Journeyman, Expert, Artisan]` tier caps for the 12 profession lines
/// (`PROFESSION_SKILL_LINES`), resolved via `SkillRaceClassInfo.dbc` (the first row matching that
/// `skill_line`'s `skill_tier` foreign key) → `SkillTiers.dbc`'s `value[0..4]`. This is the DATA half
/// of `importer/src/main.rs`'s `PROFESSION_LEARN` byte-parity guard: the hardcoded 75/150/225/300
/// literals it always used must equal what the real client's tables actually carry — this function is
/// what lets `main.rs` check that. In-memory only, like the rest of `dbc.rs` (no `.dbc` written).
pub fn profession_tier_values(data_dir: &str) -> Result<HashMap<u32, [u16; 4]>> {
    let mut chain = open_chain(Path::new(data_dir))?;
    let availability: DbcSkillRaceClassInfo = read_table(&mut chain)?;
    let tiers: DbcSkillTiers = read_table(&mut chain)?;
    Ok(resolve_profession_tier_values(&availability, &tiers))
}

/// Chain-free core of `profession_tier_values` — fixture-tested directly (constructed
/// `SkillRaceClassInfoRow`/`SkillTiersRow`s, no MPQ needed). A profession line absent from
/// `availability` (shouldn't happen with a real client) is silently skipped; the caller's parity guard
/// (`main.rs`'s `assert_profession_tier_parity`) catches the gap by failing to find it in the map.
fn resolve_profession_tier_values(
    availability: &DbcSkillRaceClassInfo,
    tiers: &DbcSkillTiers,
) -> HashMap<u32, [u16; 4]> {
    let mut out = HashMap::new();
    for &line in PROFESSION_SKILL_LINES {
        // A line can carry SEVERAL SkillRaceClassInfo rows (race/class variants, trainer-visibility
        // flag rows) and not all of them carry the tier FK — take the first row whose skill_tier
        // RESOLVES, not the first row that merely matches the line (first-match spuriously failed
        // the parity guard when a non-tier variant row sorted first — review catch).
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

// ===========================================================================================
//  Tests
// ===========================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use wow_dbc::vanilla_tables::area_table::{AreaTableKey, AreaTableRow};
    use wow_dbc::vanilla_tables::area_trigger::{AreaTriggerKey, AreaTriggerRow};
    use wow_dbc::vanilla_tables::chr_classes::ChrClassesKey;
    use wow_dbc::vanilla_tables::chr_races::ChrRacesKey as SkillChrRacesKey;
    use wow_dbc::vanilla_tables::creature_family::{CreatureFamilyKey, CreatureFamilyRow};
    use wow_dbc::vanilla_tables::faction_group::FactionGroupKey;
    use wow_dbc::vanilla_tables::light::LightKey;
    use wow_dbc::vanilla_tables::liquid_type::LiquidTypeKey;
    use wow_dbc::vanilla_tables::lock::{LockKey, LockRow};
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
    use wow_dbc::vanilla_tables::world_safe_locs::{WorldSafeLocsKey, WorldSafeLocsRow};
    use wow_dbc::vanilla_tables::zone_intro_music_table::ZoneIntroMusicTableKey;
    use wow_dbc::vanilla_tables::zone_music::ZoneMusicKey;
    use wow_dbc::LocalizedString;
    use wow_world_base::vanilla::AreaFlags;

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
        // id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name
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
        ); // apostrophe escaping
    }

    #[test]
    fn area_trigger_sql_shape_goldshire_inn_fixture() {
        // A CONSTRUCTED fixture (not client-verified — this sandbox has no real client MPQ to read a
        // real trigger id from) shaped like Goldshire's Lion's Pride Inn entrance trigger, proving
        // the emitted tuple's column order — id,map_id,x,y,z,radius,box_length,box_width,box_height,
        // box_yaw — is exactly what work-item 196's rest-state system will read a real inn
        // AreaTrigger row as.
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
                creature_family_row(1, "Wolf", 1, 0, 8), // tameable pet family
                creature_family_row(3, "O'Ryan's Beast", 6, -1, 8), // non-pet family + apostrophe escaping
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
        // family_id,name,pet_food_mask,pet_talent_type,category
        assert!(insert.contains("(1,'Wolf',1,0,8)"), "{insert}");
        assert!(insert.contains("(3,'O''Ryan''s Beast',6,-1,8)"), "{insert}"); // apostrophe escaping
                                                                               // tameable = pet_talent_type != -1 (mirrors run()'s coverage-print filter).
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

    /// Build a `LockRow` with `entries` filling indices `0..entries.len()` (each `(LockType, property,
    /// required_skill)`); the remaining indices up to 8 are `LockType::None` (padding, matching a real
    /// Lock.dbc row that rarely uses all 8 alternatives).
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
                // Lock 1: index 0 = ITEM (a real key, item entry 12345); indices 1..8 unused (None,
                // skipped) — a lock with exactly ONE real requirement emits exactly ONE row.
                lock_row(1, &[(LockType::ItemRequired, 12345, 0)]),
                // Lock 2: index 0 = SKILL via a MAPPED LockType id (1 = Lockpicking -> SkillLine 633),
                // required value 100 (a hard pick). index 1 = SKILL via LockType 3 (Mining -> 186),
                // required value 1.
                lock_row(
                    2,
                    &[
                        (LockType::LocktypeReference, 1, 100),
                        (LockType::LocktypeReference, 3, 1),
                    ],
                ),
                // Lock 3: an UNMAPPED LockType id (999) — must NOT be guessed; resolves to SkillLine 0
                // and is surfaced via the returned unmapped-id set.
                lock_row(3, &[(LockType::LocktypeReference, 999, 50)]),
            ],
        };
        let (stmts, n, unmapped) = lock_sql(&table);
        // 1 (lock 1) + 2 (lock 2) + 1 (lock 3) = 4 rows total; NONE of the padding (None) indices emit.
        assert_eq!(n, 4);
        assert_eq!(stmts[0], "DELETE FROM game_lock WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_lock "))
            .unwrap();
        // id,lock_id,index,kind,property,required_skill
        assert!(insert.contains("(1,1,0,1,12345,0)"), "{insert}"); // item lock, kind 1, property=item entry
        assert!(insert.contains("(2,2,0,2,633,100)"), "{insert}"); // Lockpicking (LockType 1 -> SkillLine 633)
        assert!(insert.contains("(3,2,1,2,186,1)"), "{insert}"); // Mining (LockType 3 -> SkillLine 186)
        assert!(insert.contains("(4,3,0,2,0,50)"), "{insert}"); // unmapped LockType 999 -> property 0
        assert_eq!(
            unmapped,
            std::collections::BTreeSet::from([999u32]),
            "only the truly-unmapped id is flagged"
        );
    }

    #[test]
    fn lock_sql_all_none_lock_emits_no_rows() {
        // A lock with every index LockType::None (shouldn't happen in a real table, but the parser
        // must not crash or emit a bogus row) contributes 0 rows and 0 unmapped ids.
        let table = DbcLock {
            rows: vec![lock_row(9, &[])],
        };
        let (_, n, unmapped) = lock_sql(&table);
        assert_eq!(n, 0);
        assert!(unmapped.is_empty());
    }

    #[test]
    fn locktype_skill_line_covers_the_three_repo_verified_ids_only() {
        assert_eq!(locktype_skill_line(1), Some(633)); // Lockpicking
        assert_eq!(locktype_skill_line(2), Some(182)); // Herbalism
        assert_eq!(locktype_skill_line(3), Some(186)); // Mining
        assert_eq!(locktype_skill_line(4), None); // [V] not yet mapped — must NOT silently guess
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

    // Params are REAL SkillLineAbility semantics; they're placed into the wow_dbc fields the
    // corrected `skill_ability_sql` reads for each (see the off-by-one remap there, 282): real
    // min_skill → superseded_by, real acquire_method → trivial_high, real gray → trivial_low,
    // real green → character_points[0].
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
            superseded_by: SpellKey::new(min_skill), // corrected reads real MinSkillLineRank here
            acquire_method: 0,                       // real SupercededBySpell (unused by us)
            trivial_skill_line_rank_high: acquire_method, // corrected reads real AcquireMethod here
            trivial_skill_line_rank_low: gray,       // corrected reads real TrivialHigh here
            character_points: [green, 0],            // corrected reads real TrivialLow here
            num_skills_up: 1,
            unknown_padding: 0,
        }
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
                skill_line_row(999, 0, "O'Ryan's Line"), // apostrophe escaping
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
        // Params are REAL semantics (min_skill, acquire_method, gray, green); the builder places
        // them into the mis-named wow_dbc fields the corrected importer reads (282). Row 1: a
        // trained recipe (min 25, acquire 0, gray 75, green 30). Row 2: an autolearn ability
        // (acquire 1) with a trained-with-skill min.
        let table = DbcSkillLineAbility {
            rows: vec![
                skill_ability_row(1, 185, 2550, 0xFFFF_FFFF, 0xFFFF_FFFF, 25, 0, 75, 30),
                skill_ability_row(2, 333, 13262, 0xFFFF_FFFF, 0xFFFF_FFFF, 0, 1, 0, 0), // autolearn (disenchant-shaped)
            ],
        };
        let (stmts, n) = skill_ability_sql(&table);
        assert_eq!(n, 2);
        assert_eq!(stmts[0], "DELETE FROM game_skill_ability WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_skill_ability "))
            .unwrap();
        // id,spell_id,skill_line,race_mask,class_mask,min_skill,acquire_method,gray,green
        assert!(
            insert.contains("(1,2550,185,4294967295,4294967295,25,0,75,30)"),
            "{insert}"
        );
        assert!(
            insert.contains("(2,13262,333,4294967295,4294967295,0,1,0,0)"),
            "{insert}"
        );
        // autolearn count reads the CORRECTED field (real AcquireMethod in trivial_high).
        let autolearn = table
            .rows()
            .iter()
            .filter(|r| r.trivial_skill_line_rank_high == AUTOLEARN_ACQUIRE_METHOD)
            .count();
        assert_eq!(autolearn, 1);
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
                186, /* Mining */
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
        // Real SkillRaceClassInfo carries multiple rows per line (race/class variants, visibility
        // flag rows) and not all carry the tier FK. A non-resolving row sorted FIRST must not sink
        // the line — the resolver scans until a skill_tier resolves (the review's spurious-bail case).
        let availability = DbcSkillRaceClassInfo {
            rows: vec![
                skill_availability_row(1, 186 /* Mining */, 0x1, 0x1, 0, 1, 0), // variant: tier FK 0 → unresolvable
                skill_availability_row(2, 186, 0xFFFF_FFFF, 0xFFFF_FFFF, 0, 1, 42), // the real tier row
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
        // No SkillRaceClassInfo row at all for line 356 (Fishing) — resolves to nothing, not a panic.
        let availability = DbcSkillRaceClassInfo { rows: vec![] };
        let tiers = DbcSkillTiers { rows: vec![] };
        let map = resolve_profession_tier_values(&availability, &tiers);
        assert!(map.is_empty());
    }

    /// The byte-parity proof (in-sandbox, no real client available): construct a `SkillRaceClassInfo`/
    /// `SkillTiers` fixture for all 12 profession lines carrying the well-known 75/150/225/300
    /// progression, resolve it, and check it against BOTH `main.rs`'s `LEGACY_TIER_CAPS` (via the exact
    /// `PROFESSION_LEARN` tier_index each entry uses) AND `main.rs`'s own
    /// `assert_profession_tier_parity` guard directly — proving the plumbing a real client run would
    /// exercise is wired correctly, even though we can't check it against Blizzard's actual bytes here.
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
}
