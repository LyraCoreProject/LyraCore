//! Stream 1 (client DBC) — `TalentTab.dbc` + `Talent.dbc` → `game_talent_tab` + `game_talent` importer.
//! Reads the operator's OWN `TalentTab.dbc`/`Talent.dbc` via the same `open_chain`/`read_table` path as
//! `dbc.rs`, and emits derived `game_*` rows as chunked clear+reload SQL (mirrors `spell.rs`'s shape).
//! This is the DATA half of work-item 031 (the real talent PANE is the wire/system half); a talent's
//! rank-1 spell (`spell_id`) may reference a `Spell.dbc` row that hasn't been imported yet — a missing
//! header is a graceful no-op at learn time, never a hard error (the rotation-latency doctrine — see
//! `module/src/talent.rs::do_learn_talent`).
//!
//! LICENSING FIREWALL: like the rest of `dbc.rs`, the client bytes stay in memory — NO `.dbc` (or any
//! Blizzard file) is ever written; only derived `game_talent_tab`/`game_talent` rows are emitted.
//!
//! Talent.dbc carries NO name field for a talent (the client shows the rank-1 spell's own name) — we
//! stamp a placeholder `"Talent {id}"` so the NOT NULL `game_talent.name` column stays populated; the
//! real display name is a client-side (Spell.dbc) concern the wire side already owns.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use wow_dbc::vanilla_tables::talent::Talent as DbcTalent;
use wow_dbc::vanilla_tables::talent_tab::TalentTab as DbcTalentTab;
use wow_dbc::DbcTable;

use crate::dbc::{open_chain, read_table};
use crate::{push_insert, run_sql_statements, sql_text, Args};

/// Clear+reload SQL for `game_talent_tab` from `TalentTab.dbc` — one row per class talent-tree tab (27 in
/// vanilla: 9 classes × 3 tabs). `class_mask`/`race_mask` are the DBC's raw bitmasks; `order_index` is the
/// tab's 0-based position within its class. No Timestamp → plain SQL. Returns (statements, row_count).
fn map_talent_tabs(tabs: &DbcTalentTab) -> (Vec<String>, usize) {
    let rows: Vec<String> = tabs
        .rows()
        .iter()
        .map(|r| {
            format!(
                "({},{},{},{},{})",
                r.id.id,
                sql_text(&r.name.en_gb),
                r.class_mask.id,
                r.race_mask.id,
                r.order_index,
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_talent_tab WHERE tab_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_talent_tab",
        "tab_id,name,class_mask,race_mask,order_index",
        &rows,
    );
    (stmts, n)
}

/// Clear+reload SQL for `game_talent` from `Talent.dbc` — every class tree's real talents (tier/column/
/// prereqs/rank spells), replacing the `module/src/talent.rs` demo Warrior seed by id collision (the demo
/// ids 1-8 sit far below the real Talent.dbc id range, so a clear+reload never touches them — see the
/// module doc's KEEP decision). `order_map` resolves each talent's DBC `tab` id to that tab's
/// `order_index` (0/1/2), which becomes `tree_id` (mirrors the demo's Arms/Fury/Protection convention);
/// an unresolved tab (shouldn't happen with both DBCs from the same client) falls back to 0.
/// `max_rank` is derived by counting the nonzero entries in `spell_rank` (rank 1 lives in `spell_id`,
/// ranks 2-5 in `rank_spell_2..5`); `required_points_in_tree` derives from `tier * POINTS_PER_TIER` (5).
/// No Timestamp → plain SQL. Returns (statements, talent_count).
fn map_talents(talents: &DbcTalent, order_map: &HashMap<u32, u8>) -> (Vec<String>, usize) {
    let rows: Vec<String> = talents
        .rows()
        .iter()
        .map(|r| {
            let tree_id = order_map.get(&r.tab.id).copied().unwrap_or(0);
            let tier = r.tier.max(0) as u32;
            let column = r.column_index.max(0);
            // Clamp to the 5 stored slots (spell_id + rank_spell_2..5): the DBC array has 9, but
            // no vanilla talent exceeds 5 ranks — a hypothetical 6th would otherwise make max_rank
            // promise a rank the table can't resolve.
            let max_rank = (r.spell_rank.iter().filter(|&&s| s != 0).count() as u8).min(5);
            let required_talent_rank = r.prereq_ranks[0].max(0) as u8;
            format!(
                "({},{},{},{},{},{},{},{},{},0,{},{},{},{},{},{},{})",
                r.id.id,
                sql_text(&format!("Talent {}", r.id.id)),
                tree_id,
                tier,
                column,
                max_rank,
                r.spell_rank[0],
                r.prereq_talents[0],
                tier * 5, // required_points_in_tree = tier * POINTS_PER_TIER
                r.tab.id,
                r.spell_rank[1],
                r.spell_rank[2],
                r.spell_rank[3],
                r.spell_rank[4],
                required_talent_rank,
                r.required_spell.id,
            )
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_talent WHERE talent_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_talent",
        "talent_id,name,tree_id,tier,column,max_rank,spell_id,required_talent_id,\
         required_points_in_tree,grant_spell_id,tab_id,rank_spell_2,rank_spell_3,rank_spell_4,\
         rank_spell_5,required_talent_rank,required_spell_id",
        &rows,
    );
    (stmts, n)
}

/// Coverage accounting for the talent import — drives the coverage print.
#[derive(Default)]
struct Coverage {
    tabs: usize,
    talents: usize,
    classes: usize,
}

/// Print the coverage line + a warning when the shape doesn't match the expected vanilla layout (9
/// classes × 3 tabs = 27 tabs).
fn print_coverage(cov: &Coverage) {
    println!(
        "\ntalents: {} tabs, {} talents across {} classes",
        cov.tabs, cov.talents, cov.classes
    );
    if cov.tabs != 27 {
        println!(
            "  WARNING: expected 27 talent tabs (9 classes × 3 tabs), got {}",
            cov.tabs
        );
    }
    if cov.classes != 9 {
        println!("  WARNING: expected 9 classes, got {}", cov.classes);
    }
}

/// `--dbc <Data dir> --talents` mode: import `TalentTab.dbc` + `Talent.dbc` → `game_talent_tab` +
/// `game_talent`. Dry-run by default (parse + map + print coverage, write NOTHING); `--apply` runs the
/// SQL. Mirrors `spell::run_spells`'s shape.
pub fn run_talents(data_dir: &str, args: &Args) -> Result<()> {
    let dir = Path::new(data_dir);
    let mut chain = open_chain(dir)?;
    eprintln!("talents: opened MPQ chain from {}", dir.display());

    let tabs: DbcTalentTab = read_table(&mut chain)?;
    let talents: DbcTalent = read_table(&mut chain)?;
    eprintln!(
        "talents: parsed TalentTab({}) + Talent({})",
        tabs.rows().len(),
        talents.rows().len()
    );

    let order_map: HashMap<u32, u8> = tabs
        .rows()
        .iter()
        .map(|r| (r.id.id, r.order_index as u8))
        .collect();
    let classes: HashSet<u32> = tabs.rows().iter().map(|r| r.class_mask.id).collect();

    let (tab_stmts, tab_count) = map_talent_tabs(&tabs);
    let (talent_stmts, talent_count) = map_talents(&talents, &order_map);
    let cov = Coverage {
        tabs: tab_count,
        talents: talent_count,
        classes: classes.len(),
    };

    let mut stmts = tab_stmts;
    stmts.extend(talent_stmts);

    if args.apply {
        run_sql_statements(args, &stmts, "talents")?;
        eprintln!(
            "talents: loaded {} tabs + {} talents into game_talent_tab/game_talent.",
            cov.tabs, cov.talents
        );
    } else {
        println!(
            "-- DRY RUN: {} SQL statements (clear+reload game_talent_tab + game_talent), write NOTHING.",
            stmts.len()
        );
        for s in stmts.iter().take(2) {
            println!("{s};");
        }
        for s in stmts.iter().skip(2) {
            if s.starts_with("INSERT INTO game_talent_tab ") {
                println!("{}… ;", &s[..s.len().min(120)]);
                break;
            }
        }
        for s in stmts.iter().skip(2) {
            if s.starts_with("INSERT INTO game_talent ") {
                println!("{}… ;", &s[..s.len().min(120)]);
                break;
            }
        }
        eprintln!("talents: dry-run — re-run with --apply to load.");
    }

    print_coverage(&cov);
    Ok(())
}

// ===========================================================================================
//  Tests
// ===========================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use wow_dbc::vanilla_tables::chr_classes::ChrClassesKey;
    use wow_dbc::vanilla_tables::chr_races::ChrRacesKey;
    use wow_dbc::vanilla_tables::spell::SpellKey;
    use wow_dbc::vanilla_tables::spell_icon::SpellIconKey;
    use wow_dbc::vanilla_tables::talent::{TalentKey, TalentRow};
    use wow_dbc::vanilla_tables::talent_tab::{TalentTabKey, TalentTabRow};
    use wow_dbc::LocalizedString;

    fn tab_row(
        id: u32,
        name: &str,
        class_mask: u32,
        race_mask: u32,
        order_index: u32,
    ) -> TalentTabRow {
        TalentTabRow {
            id: TalentTabKey::new(id),
            name: LocalizedString {
                en_gb: name.to_string(),
                ..Default::default()
            },
            spell_icon: SpellIconKey::new(0),
            race_mask: ChrRacesKey::new(race_mask),
            class_mask: ChrClassesKey::new(class_mask),
            order_index,
            background_file: String::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn talent_row(
        id: u32,
        tab: u32,
        tier: i32,
        column_index: i32,
        spell_rank: [u32; 9],
        prereq_talents: [u32; 3],
        prereq_ranks: [i32; 3],
        required_spell: u32,
    ) -> TalentRow {
        TalentRow {
            id: TalentKey::new(id),
            tab: TalentTabKey::new(tab),
            tier,
            column_index,
            spell_rank,
            prereq_talents,
            prereq_ranks,
            flags: 0,
            required_spell: SpellKey::new(required_spell),
        }
    }

    #[test]
    fn talent_tabs_map_order_index_and_escape_names() {
        let tabs = DbcTalentTab {
            rows: vec![
                tab_row(161, "Arms", 1, 0, 0),
                tab_row(162, "Fury", 1, 0, 1),
                tab_row(163, "O'Ryan's Tab", 1, 0, 2), // apostrophe escaping
            ],
        };
        let (stmts, n) = map_talent_tabs(&tabs);
        assert_eq!(n, 3);
        assert_eq!(stmts[0], "DELETE FROM game_talent_tab WHERE tab_id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_talent_tab"))
            .unwrap();
        assert!(insert.contains("(161,'Arms',1,0,0)"), "{insert}");
        assert!(insert.contains("(162,'Fury',1,0,1)"), "{insert}");
        assert!(insert.contains("(163,'O''Ryan''s Tab',1,0,2)"), "{insert}");
    }

    #[test]
    fn talents_map_two_rank_with_prereq_and_tab_order() {
        // tab 161 is the FIRST tab of its class (order_index 0) → tree_id 0.
        let order_map: HashMap<u32, u8> = [(161u32, 0u8)].into_iter().collect();
        let talents = DbcTalent {
            rows: vec![talent_row(
                100,
                161,
                1,                                 // tier
                0,                                 // column_index
                [5001, 5002, 0, 0, 0, 0, 0, 0, 0], // rank 1 + rank 2 (2-rank talent)
                [99, 0, 0],                        // prereq talent 99
                [2, 0, 0],                         // must be learned to rank 2
                0,
            )],
        };
        let (stmts, n) = map_talents(&talents, &order_map);
        assert_eq!(n, 1);
        assert_eq!(stmts[0], "DELETE FROM game_talent WHERE talent_id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_talent "))
            .unwrap();
        // talent_id=100,name,tree_id=0,tier=1,column=0,max_rank=2,spell_id=5001,required_talent_id=99,
        // required_points_in_tree=5(tier*5),grant_spell_id=0,tab_id=161,rank_spell_2=5002,
        // rank_spell_3=0,rank_spell_4=0,rank_spell_5=0,required_talent_rank=2,required_spell_id=0
        assert!(
            insert.contains("(100,'Talent 100',0,1,0,2,5001,99,5,0,161,5002,0,0,0,2,0)"),
            "{insert}"
        );
    }

    #[test]
    fn talents_map_missing_tab_falls_back_to_tree_zero() {
        let order_map: HashMap<u32, u8> = HashMap::new(); // no TalentTab row for tab 999
        let talents = DbcTalent {
            rows: vec![talent_row(
                1,
                999,
                0,
                0,
                [42, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0],
                [0, 0, 0],
                0,
            )],
        };
        let (stmts, _n) = map_talents(&talents, &order_map);
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_talent "))
            .unwrap();
        assert!(
            insert.contains("(1,'Talent 1',0,0,0,1,42,0,0,0,999,0,0,0,0,0,0)"),
            "{insert}"
        );
    }
}
