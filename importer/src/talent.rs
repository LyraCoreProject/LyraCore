use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use wow_dbc::vanilla_tables::talent::Talent as DbcTalent;
use wow_dbc::vanilla_tables::talent_tab::TalentTab as DbcTalentTab;
use wow_dbc::DbcTable;

use crate::dbc::{open_chain, read_table};
use crate::dump::{push_insert, sql_text};
use crate::Args;

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

fn map_talents(talents: &DbcTalent, order_map: &HashMap<u32, u8>) -> (Vec<String>, usize) {
    let rows: Vec<String> = talents
        .rows()
        .iter()
        .map(|r| {
            let tree_id = order_map.get(&r.tab.id).copied().unwrap_or(0);
            let tier = r.tier.max(0) as u32;
            let column = r.column_index.max(0);
            // The DBC has nine rank slots, but vanilla talents and durable rows use at most five.
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
                tier * 5,
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

#[derive(Default)]
struct Coverage {
    tabs: usize,
    talents: usize,
    classes: usize,
}

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
        args.shard.run_sql_statements(&stmts, "talents")?;
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
                tab_row(163, "O'Ryan's Tab", 1, 0, 2),
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
        let order_map: HashMap<u32, u8> = [(161u32, 0u8)].into_iter().collect();
        let talents = DbcTalent {
            rows: vec![talent_row(
                100,
                161,
                1,
                0,
                [5001, 5002, 0, 0, 0, 0, 0, 0, 0],
                [99, 0, 0],
                [2, 0, 0],
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
        assert!(
            insert.contains("(100,'Talent 100',0,1,0,2,5001,99,5,0,161,5002,0,0,0,2,0)"),
            "{insert}"
        );
    }

    #[test]
    fn talents_map_missing_tab_falls_back_to_tree_zero() {
        let order_map: HashMap<u32, u8> = HashMap::new();
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
