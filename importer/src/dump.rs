use anyhow::{bail, Context, Result};
use std::borrow::Cow;
use std::process::Command;

pub(crate) mod gmo {
    pub const MENU_ID: &str = "menu_id";
    pub const ID: &str = "id";

    pub const CONDITION_ID: &str = "condition_id";
}

pub(crate) mod cr {
    pub const GUID: &str = "guid";
    pub const ID: &str = "id";
    pub const MAP: &str = "map";
    pub const X: &str = "position_x";
    pub const Y: &str = "position_y";
    pub const Z: &str = "position_z";
    pub const O: &str = "orientation";
    pub const SPAWN_TIME_SECS_MIN: &str = "spawntimesecsmin";
}

pub(crate) mod cls {
    pub const CLASS: &str = "class";
    pub const LEVEL: &str = "level";
    pub const BASEHP: &str = "basehp";
    pub const BASEMANA: &str = "basemana";
}

pub(crate) mod lvl {
    pub const RACE: &str = "race";
    pub const CLASS: &str = "class";
    pub const LEVEL: &str = "level";
    pub const STR: &str = "str";
    pub const AGI: &str = "agi";
    pub const STA: &str = "sta";
    pub const INTE: &str = "inte";
    pub const SPI: &str = "spi";
}

pub(crate) mod pc {
    pub const RACE: &str = "race";
    pub const CLASS: &str = "class";
    pub const MAP: &str = "map";
    pub const ZONE: &str = "zone";
    pub const X: &str = "position_x";
    pub const Y: &str = "position_y";
    pub const Z: &str = "position_z";
    pub const O: &str = "orientation";
}

pub(crate) mod pcs {
    pub const RACE: &str = "race";
    pub const CLASS: &str = "class";
}

pub(crate) mod pca {
    pub const RACE: &str = "race";
    pub const CLASS: &str = "class";
    pub const BUTTON: &str = "button";
    pub const ACTION: &str = "action";
    pub const TYPE: &str = "type";
}

pub(crate) mod pci {
    pub const RACE: &str = "race";
    pub const CLASS: &str = "class";
    pub const ITEM: &str = "itemid";
    #[allow(dead_code)]
    pub const AMOUNT: &str = "amount";
}

pub(crate) mod at {
    pub const ID: &str = "id";
    pub const NAME: &str = "name";
    pub const TARGET_MAP: &str = "target_map";
    pub const TARGET_X: &str = "target_position_x";
    pub const TARGET_Y: &str = "target_position_y";
    pub const TARGET_Z: &str = "target_position_z";
    pub const TARGET_O: &str = "target_orientation";
}

pub(crate) mod it {
    pub const ENTRY: &str = "entry";
    pub const CLASS: &str = "class";
    pub const SUBCLASS: &str = "subclass";
    pub const NAME: &str = "name";
    pub const DISPLAY_ID: &str = "display_id";
    pub const QUALITY: &str = "quality";
    pub const FLAGS: &str = "flags";
    pub const BUY_COUNT: &str = "buy_count";
    pub const BUY_PRICE: &str = "buy_price";
    pub const SELL_PRICE: &str = "sell_price";
    pub const INVENTORY_TYPE: &str = "inventory_type";
    // -1 means an unrestricted allowable-class or allowable-race mask.
    pub const ALLOWED_CLASS: &str = "allowable_class";
    pub const ALLOWED_RACE: &str = "allowable_race";
    pub const REQUIRED_SKILL: &str = "required_skill";
    pub const REQUIRED_SKILL_RANK: &str = "required_skill_rank";
    pub const REQUIRED_REPUTATION_FACTION: &str = "required_reputation_faction";
    pub const REQUIRED_REPUTATION_RANK: &str = "required_reputation_rank";
    pub const MAX_COUNT: &str = "max_count";
    pub const ITEM_LEVEL: &str = "item_level";
    pub const REQUIRED_LEVEL: &str = "required_level";
    pub const STACKABLE: &str = "stackable";
    pub const CONTAINER_SLOTS: &str = "container_slots";
    pub const DMG_MIN1: &str = "dmg_min1";
    pub const DMG_MAX1: &str = "dmg_max1";
    pub const ARMOR: &str = "armor";
    pub const HOLY_RES: &str = "holy_res";
    pub const FIRE_RES: &str = "fire_res";
    pub const NATURE_RES: &str = "nature_res";
    pub const FROST_RES: &str = "frost_res";
    pub const SHADOW_RES: &str = "shadow_res";
    pub const ARCANE_RES: &str = "arcane_res";
    pub const DELAY: &str = "delay";
    pub const SPELLID_1: &str = "spellid_1";
    pub const SPELLTRIGGER_1: &str = "spelltrigger_1";
    pub const SPELLID_2: &str = "spellid_2";
    pub const SPELLTRIGGER_2: &str = "spelltrigger_2";
    pub const SPELLID_3: &str = "spellid_3";
    pub const SPELLTRIGGER_3: &str = "spelltrigger_3";
    pub const SPELLID_4: &str = "spellid_4";
    pub const SPELLTRIGGER_4: &str = "spelltrigger_4";
    pub const SPELLID_5: &str = "spellid_5";
    pub const SPELLTRIGGER_5: &str = "spelltrigger_5";
    pub const SPELLPPMRATE_1: &str = "spellppmrate_1";
    pub const SPELLPPMRATE_2: &str = "spellppmrate_2";
    pub const SPELLPPMRATE_3: &str = "spellppmrate_3";
    pub const SPELLPPMRATE_4: &str = "spellppmrate_4";
    pub const SPELLPPMRATE_5: &str = "spellppmrate_5";
    pub const BONDING: &str = "bonding";
    pub const PAGE_TEXT: &str = "page_text";
    pub const START_QUEST: &str = "start_quest";
    pub const SHEATH: &str = "sheath";
    pub const RANDOM_PROPERTY: &str = "random_property";
    pub const BLOCK: &str = "block";
    pub const MAX_DURABILITY: &str = "max_durability";
    pub const BAG_FAMILY: &str = "bag_family";
    pub const FOOD_TYPE: &str = "food_type";

    pub(crate) fn read<'a>(
        row: &'a crate::world_source::Row,
        column: &str,
    ) -> anyhow::Result<&'a str> {
        if column == NAME {
            row.text(column).map(str::trim)
        } else {
            row.numeric_text(column)
        }
    }
}

pub(crate) mod nv {
    pub const ENTRY: &str = "entry";
    pub const ITEM: &str = "item";
    pub const MAXCOUNT: &str = "maxcount";
    pub const SLOT: &str = "slot";
}

pub(crate) mod qt {
    pub const ENTRY: &str = "entry";
    pub const ZONE_OR_SORT: &str = "ZoneOrSort";
    pub const MIN_LEVEL: &str = "MinLevel";
    pub const QUEST_LEVEL: &str = "QuestLevel";
    pub const TYPE: &str = "Type";
    pub const REQUIRED_CLASSES: &str = "RequiredClasses";
    pub const REQUIRED_RACES: &str = "RequiredRaces";
    // LimitTime is seconds after acceptance; zero means untimed.
    pub const LIMIT_TIME: &str = "LimitTime";
    pub const SPECIAL_FLAGS: &str = "SpecialFlags";
    pub const PREV_QUEST_ID: &str = "PrevQuestId";
    pub const NEXT_QUEST_IN_CHAIN: &str = "NextQuestInChain";
    pub const SRC_ITEM_ID: &str = "SrcItemId";
    pub const SRC_ITEM_COUNT: &str = "SrcItemCount"; // SrcItemCount zero means one.
    pub const TITLE: &str = "Title";
    pub const DETAILS: &str = "Details";
    pub const OBJECTIVES: &str = "Objectives";
    pub const OFFER_REWARD_TEXT: &str = "OfferRewardText";
    pub const REQUEST_ITEMS_TEXT: &str = "RequestItemsText";
    pub const REW_REP_FACTION1: &str = "RewRepFaction1";
    pub const REW_REP_VALUE1: &str = "RewRepValue1";
    pub const REW_OR_REQ_MONEY: &str = "RewOrReqMoney";
    pub const REW_SPELL: &str = "RewSpell";
    pub const REW_SPELL_CAST: &str = "RewSpellCast";
    pub const REW_MONEY_MAX_LEVEL: &str = "RewMoneyMaxLevel";

    pub const REW_MAIL_TEMPLATE_ID: &str = "RewMailTemplateId";
    pub const REW_MAIL_DELAY_SECS: &str = "RewMailDelaySecs";
}

pub(crate) mod mlt {
    pub const ENTRY: &str = "entry";
    pub const ITEM: &str = "item";
    pub const CHANCE: &str = "ChanceOrQuestChance";
    pub const GROUP: &str = "groupid";
    pub const MINCOUNT_OR_REF: &str = "mincountOrRef";
    pub const MAXCOUNT: &str = "maxcount";
    pub const CONDITION_ID: &str = "condition_id";
}

pub(crate) mod qr {
    pub const CREATURE: &str = "id";
    pub const QUEST: &str = "quest";
}

pub(crate) mod got {
    pub const ENTRY: &str = "entry";
    pub const TYPE: &str = "type";
    pub const DISPLAY_ID: &str = "displayId";
    pub const NAME: &str = "name";
    pub const FLAGS: &str = "flags";
    pub const SIZE: &str = "size";

    pub const DATA0: &str = "data0";
    pub const DATA1: &str = "data1";
    pub const DATA2: &str = "data2";
    pub const DATA3: &str = "data3";
    pub const DATA5: &str = "data5";
}

pub(crate) mod go {
    pub const GUID: &str = "guid";
    pub const ID: &str = "id";
    pub const MAP: &str = "map";
    pub const X: &str = "position_x";
    pub const Y: &str = "position_y";
    pub const Z: &str = "position_z";
    pub const O: &str = "orientation";
    // The spawn quaternion determines render orientation; orientation alone is movement facing.
    pub const ROT0: &str = "rotation0";
    pub const ROT1: &str = "rotation1";
    pub const ROT2: &str = "rotation2";
    pub const ROT3: &str = "rotation3";
}

pub(crate) mod ai {
    pub const TRIGGER: &str = "id";
    pub const QUEST: &str = "quest";
}

pub(crate) trait TableRows {
    fn rows(&self, table: &str) -> Result<Cow<'_, [crate::world_source::Row]>>;
    fn source_hash(&self) -> Result<String>;
    fn source_profile_report(&self) -> Option<serde_json::Value> {
        None
    }
    // May retain extra columns; row selection and checked field access match rows().
    fn columns(
        &self,
        table: &str,
        _columns: &[&str],
    ) -> Result<Cow<'_, [crate::world_source::Row]>> {
        self.rows(table)
    }
}

pub(crate) fn parse_table<'a>(
    dump: &'a (impl TableRows + ?Sized),
    table: &str,
) -> Result<Cow<'a, [crate::world_source::Row]>> {
    dump.rows(table)
}

pub(crate) fn field<'a>(row: &'a crate::world_source::Row, column: &str) -> Result<&'a str> {
    row.raw(column)
}

pub(crate) fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
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

pub(crate) fn sql_text(s: &str) -> String {
    sql_str(s)
}
