use std::io::Read;
use std::process::Command;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;

pub(crate) mod ct {
    pub const ENTRY: usize = 0;
    pub const NAME: usize = 1;
    pub const SUBNAME: usize = 2;
    pub const MIN_LEVEL: usize = 3;
    pub const MAX_LEVEL: usize = 4;
    pub const MODEL_ID1: usize = 5;
    pub const FACTION: usize = 9;
    pub const SCALE: usize = 10;
    pub const FAMILY: usize = 11;
    pub const CREATURE_TYPE: usize = 12;
    pub const NPC_FLAGS: usize = 16;
    pub const UNIT_FLAGS: usize = 17;
    pub const CREATURE_TYPE_FLAGS: usize = 20;
    pub const RANK: usize = 33;
    pub const MIN_LEVEL_HEALTH: usize = 40;
    pub const MAX_LEVEL_HEALTH: usize = 41;
    pub const MIN_MELEE_DMG: usize = 44;
    pub const MAX_MELEE_DMG: usize = 45;
    pub const ARMOR: usize = 48;
    pub const MELEE_BASE_ATTACK_TIME: usize = 51;
    pub const MIN_LOOT_GOLD: usize = 54;
    pub const MAX_LOOT_GOLD: usize = 55;
    pub const LOOT_ID: usize = 56;
    pub const PICKPOCKET_LOOT_ID: usize = 57;
    pub const SKIN_LOOT_ID: usize = 58;
    pub const TRAINER_TYPE: usize = 71;
    pub const TRAINER_CLASS: usize = 73; // Class id, not a mask; zero serves every class.
    pub const GOSSIP_MENU_ID: usize = 77;
}

pub(crate) mod gm {
    pub const ENTRY: usize = 0;
    pub const TEXT_ID: usize = 1;
    pub const CONDITION_ID: usize = 3;
}

pub(crate) mod npct {
    // Legacy npc_text layout: ID plus eight 10-column male/female/language/probability/emote slots.
    pub const ID: usize = 0;
    pub const NCOLS_FULL: usize = 81;
    // BroadcastText npc_text layout: ID, eight probabilities, eight BroadcastText ids.
    pub const NCOLS_SHORT: usize = 17;
    pub const fn full_slot_base(slot: usize) -> usize {
        1 + slot * 10
    }
    pub const FULL_SLOT_MALE_OFFSET: usize = 0;
    pub const FULL_SLOT_FEMALE_OFFSET: usize = 1;
    pub const FULL_SLOT_PROB_OFFSET: usize = 3;
    pub const fn short_slot_prob(slot: usize) -> usize {
        1 + slot
    }
    pub const fn short_slot_bcast_id(slot: usize) -> usize {
        9 + slot
    }
}

pub(crate) mod bt {
    pub const ID: usize = 0;
    pub const TEXT: usize = 1;
    pub const TEXT1: usize = 2;
    pub const CHAT_TYPE: usize = 3;
    pub const LANGUAGE: usize = 4;
}

pub(crate) mod gmo {
    pub const MENU_ID: usize = 0;
    pub const ID: usize = 1;
    pub const OPTION_ICON: usize = 2;
    pub const OPTION_TEXT: usize = 3;
    pub const OPTION_TYPE: usize = 5;
    pub const ACTION_MENU_ID: usize = 7;
    // The 15-column layout ends with a conditions foreign key.
    pub const NCOLS_15_CONDITION_FK: usize = 15;
    pub const CONDITION_ID_15: usize = 14;
    // Legacy layouts use a foreign key at column 10 or inline conditions at columns 11..13.
    pub const NCOLS_WITH_CONDITION_FK: usize = 11;
    pub const CONDITION_ID: usize = 10;
    pub const NCOLS_WITH_INLINE_COND: usize = 14;
    pub const INLINE_COND_TYPE: usize = 11;
    pub const INLINE_COND_VALUE1: usize = 12;
    pub const INLINE_COND_VALUE2: usize = 13;
}

pub(crate) mod cts {
    pub const ENTRY: usize = 0;
    pub const SET_ID: usize = 1;
    pub const SPELL1: usize = 2;
}

pub(crate) mod cr {
    pub const GUID: usize = 0;
    pub const ID: usize = 1;
    pub const MAP: usize = 2;
    pub const X: usize = 4;
    pub const Y: usize = 5;
    pub const Z: usize = 6;
    pub const O: usize = 7;
    pub const SPAWN_TIME_SECS_MIN: usize = 8;
    pub const MOVEMENT_TYPE: usize = 11;
}

pub(crate) mod cm {
    pub const ID: usize = 0;
    pub const X: usize = 2;
    pub const Y: usize = 3;
    pub const Z: usize = 4;
}

pub(crate) mod cmt {
    // Template paths are keyed by entry; direct creature_movement paths are keyed by spawn guid.
    pub const ENTRY: usize = 0;
    pub const NCOLS_WITH_PATH_ID: usize = 9;
    pub const PATH_ID: usize = 1;
    pub const fn pos_indices(ncols: usize) -> (usize, usize, usize, usize) {
        if ncols >= NCOLS_WITH_PATH_ID {
            (2, 3, 4, 5)
        } else {
            (1, 2, 3, 4)
        }
    }
}

pub(crate) mod cls {
    pub const CLASS: usize = 0;
    pub const LEVEL: usize = 1;
    pub const BASEHP: usize = 2;
    pub const BASEMANA: usize = 3;
}

pub(crate) mod lvl {
    pub const RACE: usize = 0;
    pub const CLASS: usize = 1;
    pub const LEVEL: usize = 2;
    pub const STR: usize = 3;
    pub const AGI: usize = 4;
    pub const STA: usize = 5;
    pub const INTE: usize = 6;
    pub const SPI: usize = 7;
}

pub(crate) mod pc {
    pub const RACE: usize = 0;
    pub const CLASS: usize = 1;
    pub const MAP: usize = 2;
    pub const ZONE: usize = 3;
    pub const X: usize = 4;
    pub const Y: usize = 5;
    pub const Z: usize = 6;
    pub const O: usize = 7;
}

pub(crate) mod pcs {
    pub const RACE: usize = 0;
    pub const CLASS: usize = 1;
    pub const SPELL: usize = 2;
}

pub(crate) mod pca {
    pub const RACE: usize = 0;
    pub const CLASS: usize = 1;
    pub const BUTTON: usize = 2;
    pub const ACTION: usize = 3;
    pub const TYPE: usize = 4;
}

pub(crate) mod pci {
    pub const RACE: usize = 0;
    pub const CLASS: usize = 1;
    pub const ITEM: usize = 2;
    #[allow(dead_code)]
    pub const AMOUNT: usize = 3;
}

pub(crate) mod gz {
    pub const ID: usize = 0;
    pub const GHOST_ZONE: usize = 1;
    pub const FACTION: usize = 2;
}

pub(crate) mod at {
    pub const ID: usize = 0;
    pub const NAME: usize = 1;
    pub const TARGET_MAP: usize = 6;
    pub const TARGET_X: usize = 7;
    pub const TARGET_Y: usize = 8;
    pub const TARGET_Z: usize = 9;
    pub const TARGET_O: usize = 10;
}

pub(crate) mod it {
    pub const ENTRY: usize = 0;
    pub const CLASS: usize = 1;
    pub const SUBCLASS: usize = 2;
    pub const NAME: usize = 3;
    pub const DISPLAY_ID: usize = 4;
    pub const QUALITY: usize = 5;
    pub const FLAGS: usize = 6;
    pub const BUY_COUNT: usize = 7;
    pub const BUY_PRICE: usize = 8;
    pub const SELL_PRICE: usize = 9;
    pub const INVENTORY_TYPE: usize = 10;
    // ClassicDB uses -1 for an unrestricted allowable-class or allowable-race mask.
    pub const ALLOWED_CLASS: usize = 11;
    pub const ALLOWED_RACE: usize = 12;
    pub const REQUIRED_SKILL: usize = 15;
    pub const REQUIRED_SKILL_RANK: usize = 16;
    pub const REQUIRED_REPUTATION_FACTION: usize = 20;
    pub const REQUIRED_REPUTATION_RANK: usize = 21;
    pub const MAX_COUNT: usize = 22;
    pub const ITEM_LEVEL: usize = 13;
    pub const REQUIRED_LEVEL: usize = 14;
    pub const STACKABLE: usize = 23;
    pub const CONTAINER_SLOTS: usize = 24;
    pub const STAT_TYPE1: usize = 25;
    pub const DMG_MIN1: usize = 45;
    pub const DMG_MAX1: usize = 46;
    pub const ARMOR: usize = 60;
    pub const HOLY_RES: usize = 61;
    pub const FIRE_RES: usize = 62;
    pub const NATURE_RES: usize = 63;
    pub const FROST_RES: usize = 64;
    pub const SHADOW_RES: usize = 65;
    pub const ARCANE_RES: usize = 66;
    pub const DELAY: usize = 67;
    pub const SPELLID_1: usize = 70;
    pub const SPELLTRIGGER_1: usize = 71;
    pub const SPELLID_2: usize = 77;
    pub const SPELLTRIGGER_2: usize = 78;
    pub const SPELLID_3: usize = 84;
    pub const SPELLTRIGGER_3: usize = 85;
    pub const SPELLID_4: usize = 91;
    pub const SPELLTRIGGER_4: usize = 92;
    pub const SPELLID_5: usize = 98;
    pub const SPELLTRIGGER_5: usize = 99;
    pub const BONDING: usize = 105;
    pub const PAGE_TEXT: usize = 107;
    pub const START_QUEST: usize = 110;
    pub const SHEATH: usize = 113;
    pub const RANDOM_PROPERTY: usize = 114;
    pub const BLOCK: usize = 115;
    pub const MAX_DURABILITY: usize = 117;
    pub const BAG_FAMILY: usize = 120;
    pub const FOOD_TYPE: usize = 123;
}

pub(crate) mod clt {
    pub const ENTRY: usize = 0;
    pub const ITEM: usize = 1;
    pub const CHANCE: usize = 2; // Negative ChanceOrQuestChance marks a quest-only drop.
    pub const GROUPID: usize = 3; // 0 = independent; >0 = pick one.
    pub const MINCOUNT_OR_REF: usize = 4; // Negative mincountOrRef refers to a shared loot pool.
    pub const MAXCOUNT: usize = 5;
}

pub(crate) mod nv {
    pub const ENTRY: usize = 0;
    pub const ITEM: usize = 1;
    pub const MAXCOUNT: usize = 2;
    pub const SLOT: usize = 4;
}

pub(crate) mod qt {
    pub const ENTRY: usize = 0;
    pub const ZONE_OR_SORT: usize = 2;
    pub const MIN_LEVEL: usize = 3;
    pub const QUEST_LEVEL: usize = 5;
    pub const TYPE: usize = 6;
    pub const REQUIRED_CLASSES: usize = 7;
    pub const REQUIRED_RACES: usize = 8;
    // LimitTime is seconds after acceptance; zero means untimed.
    pub const LIMIT_TIME: usize = 19;
    pub const SPECIAL_FLAGS: usize = 21;
    pub const PREV_QUEST_ID: usize = 22;
    pub const NEXT_QUEST_IN_CHAIN: usize = 26;
    pub const SRC_ITEM_ID: usize = 27;
    pub const SRC_ITEM_COUNT: usize = 28; // SrcItemCount zero means one.
    pub const TITLE: usize = 30;
    pub const DETAILS: usize = 31;
    pub const OBJECTIVES: usize = 32;
    pub const OFFER_REWARD_TEXT: usize = 33;
    pub const REQUEST_ITEMS_TEXT: usize = 34;
    pub const REQ_ITEM_ID1: usize = 40;
    pub const REQ_ITEM_COUNT1: usize = 44;
    pub const REQ_CREATURE_OR_GO_ID1: usize = 56;
    pub const REQ_CREATURE_OR_GO_COUNT1: usize = 60;
    pub const REQ_SPELL_CAST1: usize = 64;
    pub const REW_CHOICE_ITEM_ID1: usize = 68;
    pub const REW_CHOICE_ITEM_COUNT1: usize = 74;
    pub const REW_ITEM_ID1: usize = 80;
    pub const REW_ITEM_COUNT1: usize = 84;
    pub const REW_REP_FACTION1: usize = 88;
    pub const REW_REP_VALUE1: usize = 93;
    pub const REW_OR_REQ_MONEY: usize = 98;
    pub const REW_SPELL: usize = 100;
    pub const REW_SPELL_CAST: usize = 101;
    pub const REW_MONEY_MAX_LEVEL: usize = 99;

    pub const REW_MAIL_TEMPLATE_ID: usize = 102;
    pub const REW_MAIL_DELAY_SECS: usize = 103;
}

pub(crate) mod dse {
    pub const ID: usize = 0;
    pub const COMMAND: usize = 3;
    pub const DATALONG: usize = 4;
    pub const DATALONG2: usize = 5;
    pub const DATALONG3: usize = 6; // Copper.
    pub const DATAINT: usize = 10; // Seconds.
    pub const SEND_MAIL: u32 = 38;
}

pub(crate) mod mlt {
    pub const ENTRY: usize = 0;
    pub const ITEM: usize = 1;
    pub const CHANCE: usize = 2;
    pub const GROUP: usize = 3;
    pub const MINCOUNT_OR_REF: usize = 4;
    pub const MAXCOUNT: usize = 5;
    pub const CONDITION_ID: usize = 6;
}

pub(crate) mod qr {
    pub const CREATURE: usize = 0;
    pub const QUEST: usize = 1;
}

pub(crate) mod nt {
    pub const ENTRY: usize = 0;
    pub const SPELL: usize = 1;
    pub const COST: usize = 2;
    pub const REQ_SKILL: usize = 3;
    pub const REQ_LEVEL: usize = 5;
}

pub(crate) mod sc {
    pub const SPELL_ID: usize = 0;
    pub const PREV_SPELL: usize = 1;
    pub const FIRST_SPELL: usize = 2;
    pub const RANK: usize = 3;
    pub const REQ_SPELL: usize = 4;
}

pub(crate) mod sls {

    pub const ENTRY: usize = 0;
    pub const SPELL_ID: usize = 1;
    pub const ACTIVE: usize = 2;
}

pub(crate) mod spe {
    // Legacy proc rows have one family mask; current rows have one per effect.
    pub const ENTRY: usize = 0;
    pub const SCHOOL_MASK: usize = 1;
    pub const FAMILY_NAME: usize = 2;
    pub const FAMILY_MASK: usize = 3;
    pub const PROC_FLAGS: usize = 4;
    pub const PROC_EX: usize = 5;
    pub const PPM_RATE: usize = 6;
    pub const CUSTOM_CHANCE: usize = 7;
    // The dump cooldown uses seconds; durable rows use milliseconds.
    pub const COOLDOWN: usize = 8;
    pub const COLUMNS: usize = 9;
}

pub(crate) mod got {
    pub const ENTRY: usize = 0;
    pub const TYPE: usize = 1;
    pub const DISPLAY_ID: usize = 2;
    pub const NAME: usize = 3;
    pub const FLAGS: usize = 5;
    pub const SIZE: usize = 7;

    pub const DATA0: usize = 8;
    pub const DATA1: usize = 9;
    pub const DATA2: usize = 10;
    pub const DATA3: usize = 11;
    pub const DATA5: usize = 13;
}

pub(crate) mod go {
    pub const GUID: usize = 0;
    pub const ID: usize = 1;
    pub const MAP: usize = 2;
    pub const X: usize = 4;
    pub const Y: usize = 5;
    pub const Z: usize = 6;
    pub const O: usize = 7;
    // The spawn quaternion determines render orientation; orientation alone is movement facing.
    pub const ROT0: usize = 8;
    pub const ROT1: usize = 9;
    pub const ROT2: usize = 10;
    pub const ROT3: usize = 11;
}

pub(crate) mod ai {
    pub const TRIGGER: usize = 0;
    pub const QUEST: usize = 1;
}

pub(crate) fn parse_table(dump: &str, table: &str) -> Vec<Vec<String>> {
    parse_table_inserts(dump, table).0
}

pub(crate) fn parse_complete_table(dump: &str, table: &str) -> Result<Vec<Vec<String>>> {
    let (rows, complete) = parse_table_inserts(dump, table);
    if !complete {
        bail!("unterminated {table} INSERT in dump");
    }
    Ok(rows)
}

fn parse_table_inserts(dump: &str, table: &str) -> (Vec<Vec<String>>, bool) {
    let needle = format!("INSERT INTO `{table}` VALUES ");
    let mut rows = Vec::new();
    let mut search = 0;
    let mut complete = true;
    while let Some(rel) = dump[search..].find(&needle) {
        let mut i = search + rel + needle.len();
        let bytes = dump.as_bytes();
        let mut field = String::new();
        let mut tuple: Vec<String> = Vec::new();
        let mut in_tuple = false;
        let mut in_quote = false;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if in_quote {
                if c == '\\' && i + 1 < bytes.len() {
                    let n = bytes[i + 1] as char;
                    field.push(match n {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    });
                    i += 2;
                    continue;
                } else if c == '\'' {
                    in_quote = false;
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    '(' if !in_tuple => {
                        in_tuple = true;
                        tuple.clear();
                        field.clear();
                    }
                    '\'' if in_tuple => in_quote = true,
                    ',' if in_tuple => {
                        tuple.push(std::mem::take(&mut field));
                    }
                    ')' if in_tuple => {
                        tuple.push(std::mem::take(&mut field));
                        rows.push(std::mem::take(&mut tuple));
                        in_tuple = false;
                    }
                    ';' if !in_tuple => break,
                    _ if in_tuple => field.push(c),
                    _ => {}
                }
            }
            i += 1;
        }
        complete &= i < bytes.len() && !in_tuple && !in_quote;
        search = i;
    }
    (rows, complete)
}

pub(crate) fn field(row: &[String], idx: usize) -> &str {
    row.get(idx).map(|s| s.trim()).unwrap_or("")
}

pub(crate) fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// The tuple parser cannot distinguish quoted NULL text from an unquoted NULL token.
pub(crate) fn sql_text(s: &str) -> String {
    if s == "NULL" {
        "''".into()
    } else {
        sql_str(s)
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> Result<String> {
    use std::io::Write;
    let mut child = Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("spawn sha256sum (is it on PATH?)")?;
    child
        .stdin
        .take()
        .context("sha256sum: no stdin handle")?
        .write_all(bytes)
        .context("write dump bytes to sha256sum stdin")?;
    let out = child.wait_with_output().context("wait for sha256sum")?;
    if !out.status.success() {
        bail!("sha256sum failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let digest = stdout
        .split_whitespace()
        .next()
        .with_context(|| format!("sha256sum produced no output: {stdout:?}"))?;
    Ok(digest.to_string())
}

pub(crate) fn read_dump(path: &str) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {path}"))?;
    let raw = if path.ends_with(".gz") {
        let mut v = Vec::new();
        GzDecoder::new(&bytes[..])
            .read_to_end(&mut v)
            .context("gunzip dump")?;
        v
    } else {
        bytes
    };
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

pub(crate) fn insert_row_count(stmts: &[String]) -> u64 {
    stmts
        .iter()
        .filter(|s| s.starts_with("INSERT"))
        .map(|s| s.matches("),(").count() as u64 + 1)
        .sum()
}

pub(crate) fn push_insert(stmts: &mut Vec<String>, table: &str, cols: &str, rows: &[String]) {
    const MAX_BYTES: usize = 32 * 1024;
    let mut batch: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    let flush = |batch: &mut Vec<&str>, stmts: &mut Vec<String>| {
        if !batch.is_empty() {
            stmts.push(format!(
                "INSERT INTO {table} ({cols}) VALUES {}",
                batch.join(",")
            ));
            batch.clear();
        }
    };
    for row in rows {
        if !batch.is_empty() && (batch.len() >= 500 || bytes + row.len() + 1 > MAX_BYTES) {
            flush(&mut batch, stmts);
            bytes = 0;
        }
        batch.push(row);
        bytes += row.len() + 1;
    }
    flush(&mut batch, stmts);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn parses_quoted_names_with_commas_and_escapes() {
        let dump = "x INSERT INTO `creature_template` VALUES (1,'A, the Bold','Sub\\'s',5),(2,'B',NULL,7); y";
        let rows = parse_table(dump, "creature_template");
        assert_eq!(rows.len(), 2);
        assert_eq!(field(&rows[0], 0), "1");
        assert_eq!(field(&rows[0], 1), "A, the Bold");
        assert_eq!(field(&rows[0], 2), "Sub's");
        assert_eq!(field(&rows[0], 3), "5");
        assert_eq!(field(&rows[1], 2), "NULL");
    }

    #[test]
    fn sql_str_escapes_apostrophes() {
        assert_eq!(sql_str("Hogger's"), "'Hogger''s'");
    }

    #[test]
    fn sql_text_collapses_null_token_to_empty() {
        assert_eq!(sql_text("NULL"), "''");
        assert_eq!(sql_text("Weaponsmith"), "'Weaponsmith'");
    }

    #[test]
    fn sha256_hex_matches_known_test_vectors() {
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn insert_row_count_counts_values_tuples_not_statements() {
        let mut stmts = vec!["DELETE FROM game_x WHERE id >= 0".to_string()];
        push_insert(
            &mut stmts,
            "game_x",
            "a,b",
            &["(1,2)".into(), "(3,4)".into(), "(5,6)".into()],
        );
        assert_eq!(stmts.len(), 2, "one DELETE + one batched INSERT: {stmts:?}");
        assert_eq!(insert_row_count(&stmts), 3);
        assert_eq!(insert_row_count(&[]), 0);
    }

    struct ScratchFile(std::path::PathBuf);

    impl ScratchFile {
        fn new(tag: &str, bytes: &[u8]) -> Self {
            let p = std::env::temp_dir().join(format!(
                "lyracore-importer-test-{}-{tag}",
                std::process::id()
            ));
            std::fs::write(&p, bytes).expect("write scratch file");
            ScratchFile(p)
        }
        fn path(&self) -> &str {
            self.0.to_str().expect("scratch path is utf8")
        }
    }

    impl Drop for ScratchFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn read_dump_on_a_missing_path_errors_and_names_the_path() {
        for path in [
            "/nonexistent/lyracore/importer/path.sql",
            "/nonexistent/lyracore/importer/path.sql.gz",
        ] {
            let err = read_dump(path).expect_err("a missing dump must not read as an empty dump");
            let msg = format!("{err:#}");
            assert!(
                msg.contains(path),
                "the error must name the path the operator passed, got: {msg}"
            );
        }
    }

    #[test]
    fn read_dump_on_a_gz_path_holding_non_gzip_bytes_errors_at_the_gunzip_step() {
        let f = ScratchFile::new(
            "not-really-gzip.sql.gz",
            b"INSERT INTO `spell_chain` VALUES (1);",
        );
        let err = read_dump(f.path()).expect_err("non-gzip bytes behind a .gz name must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("gunzip dump"),
            "the error must point at the decompression step, got: {msg}"
        );
    }

    #[test]
    fn read_dump_decodes_invalid_utf8_lossily_instead_of_failing() {
        let f = ScratchFile::new("latin1.sql", b"INSERT INTO `t` VALUES (1,'Caf\xE9 Ale');");
        let text = read_dump(f.path()).expect("a stray non-UTF8 byte must not fail the import");
        assert!(
            text.starts_with("INSERT INTO `t` VALUES (1,'Caf") && text.ends_with(" Ale');"),
            "the ASCII around the bad byte must survive verbatim, got: {text:?}"
        );
        assert!(
            text.contains('\u{FFFD}'),
            "the bad byte must become the replacement char, not be dropped: {text:?}"
        );
        let rows = parse_table(&text, "t");
        assert_eq!(
            rows.len(),
            1,
            "the row around the bad byte must still parse"
        );
        assert_eq!(field(&rows[0], 0), "1");
    }

    #[test]
    fn parse_table_returns_no_rows_for_a_table_the_dump_never_mentions() {
        let dump = "x INSERT INTO `creature_template` VALUES (1,'A',NULL,5); y";
        assert!(
            parse_table(dump, "spell_chain").is_empty(),
            "an absent table must yield zero rows, not a panic"
        );
        assert!(parse_table("", "creature_template").is_empty());
        assert!(parse_table("-- comment only, no INSERTs\n", "creature").is_empty());
    }

    #[test]
    fn parse_table_name_match_is_exact_not_a_prefix_of_a_longer_table() {
        let templates = "x INSERT INTO `creature_template` VALUES (1,'A',NULL,5); y";
        assert!(
            parse_table(templates, "creature").is_empty(),
            "`creature` must NOT match `creature_template` rows"
        );
        let spawns = "x INSERT INTO `creature` VALUES (7,299,0,1.0); y";
        assert!(
            parse_table(spawns, "creature_template").is_empty(),
            "`creature_template` must NOT match `creature` rows"
        );
        let both = format!("{templates}\n{spawns}");
        assert_eq!(parse_table(&both, "creature_template").len(), 1);
        assert_eq!(parse_table(&both, "creature").len(), 1);
    }

    #[test]
    fn parse_table_keeps_complete_tuples_and_drops_an_unterminated_one() {
        let truncated = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133";
        let rows = parse_table(truncated, "spell_chain");
        assert_eq!(
            rows.len(),
            1,
            "the closed tuple survives, the truncated tail is dropped: {rows:?}"
        );
        assert_eq!(field(&rows[0], 0), "133");
        let no_semicolon = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133,2,0)";
        assert_eq!(parse_table(no_semicolon, "spell_chain").len(), 2);
    }

    #[test]
    fn parse_table_unterminated_quote_swallows_the_rest_of_the_dump() {
        let dump = "x INSERT INTO `creature_template` VALUES (1,'Good'),(2,'oops); \
                    INSERT INTO `creature_template` VALUES (3,'Later'); y";
        let rows = parse_table(dump, "creature_template");
        assert_eq!(
            rows.len(),
            1,
            "only the rows before the unbalanced quote import: {rows:?}"
        );
        assert_eq!(field(&rows[0], 1), "Good");
    }
}
