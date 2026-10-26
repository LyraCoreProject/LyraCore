use std::borrow::Cow;
use std::io::Read;
use std::process::Command;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;

pub(crate) mod ct {
    pub const ENTRY: &str = "Entry";
    pub const NAME: &str = "Name";
    pub const SUBNAME: &str = "SubName";
    pub const MIN_LEVEL: &str = "MinLevel";
    pub const MAX_LEVEL: &str = "MaxLevel";
    pub const MODEL_ID1: &str = "ModelId1";
    pub const FACTION: &str = "Faction";
    pub const SCALE: &str = "Scale";
    pub const FAMILY: &str = "Family";
    pub const CREATURE_TYPE: &str = "CreatureType";
    pub const NPC_FLAGS: &str = "NpcFlags";
    pub const UNIT_FLAGS: &str = "UnitFlags";
    pub const CREATURE_TYPE_FLAGS: &str = "CreatureTypeFlags";
    pub const RANK: &str = "Rank";
    pub const MIN_LEVEL_HEALTH: &str = "MinLevelHealth";
    pub const MAX_LEVEL_HEALTH: &str = "MaxLevelHealth";
    pub const MIN_MELEE_DMG: &str = "MinMeleeDmg";
    pub const MAX_MELEE_DMG: &str = "MaxMeleeDmg";
    pub const ARMOR: &str = "Armor";
    pub const MELEE_BASE_ATTACK_TIME: &str = "MeleeBaseAttackTime";
    pub const MIN_LOOT_GOLD: &str = "MinLootGold";
    pub const MAX_LOOT_GOLD: &str = "MaxLootGold";
    pub const LOOT_ID: &str = "LootId";
    pub const PICKPOCKET_LOOT_ID: &str = "PickpocketLootId";
    pub const SKIN_LOOT_ID: &str = "SkinningLootId";
    pub const TRAINER_TYPE: &str = "TrainerType";
    // Class id, not a mask; zero serves every class.
    pub const TRAINER_CLASS: &str = "TrainerClass";
    pub const GOSSIP_MENU_ID: &str = "GossipMenuId";
}

pub(crate) mod gm {
    pub const ENTRY: &str = "entry";
    pub const TEXT_ID: &str = "text_id";
    pub const CONDITION_ID: &str = "condition_id";
}

pub(crate) mod npct {
    pub const ID: &str = "ID";
}

pub(crate) mod bt {
    pub const ID: &str = "Id";
    pub const TEXT: &str = "Text";
    pub const TEXT1: &str = "Text1";
    pub const CHAT_TYPE: &str = "ChatTypeID";
    pub const LANGUAGE: &str = "LanguageID";
}

pub(crate) mod gmo {
    pub const MENU_ID: &str = "menu_id";
    pub const ID: &str = "id";
    pub const OPTION_ICON: &str = "option_icon";
    pub const OPTION_TEXT: &str = "option_text";
    pub const OPTION_TYPE: &str = "option_id";
    pub const ACTION_MENU_ID: &str = "action_menu_id";
    pub const CONDITION_ID: &str = "condition_id";
}

pub(crate) mod cts {
    pub const ENTRY: &str = "entry";
    pub const SET_ID: &str = "setId";
    pub const SPELL1: &str = "spell1";
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
    pub const MOVEMENT_TYPE: &str = "MovementType";
}

pub(crate) mod cm {
    pub const ID: &str = "Id";
    pub const X: &str = "PositionX";
    pub const Y: &str = "PositionY";
    pub const Z: &str = "PositionZ";
}

pub(crate) mod cmt {
    // Template paths use creature entries; creature_movement uses spawn guids.
    pub const ENTRY: &str = "Entry";
    pub const PATH_ID: &str = "PathId";
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
    pub const SPELL: &str = "Spell";
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
    pub const DISPLAY_ID: &str = "displayid";
    pub const QUALITY: &str = "Quality";
    pub const FLAGS: &str = "Flags";
    pub const BUY_COUNT: &str = "BuyCount";
    pub const BUY_PRICE: &str = "BuyPrice";
    pub const SELL_PRICE: &str = "SellPrice";
    pub const INVENTORY_TYPE: &str = "InventoryType";
    // ClassicDB uses -1 for an unrestricted allowable-class or allowable-race mask.
    pub const ALLOWED_CLASS: &str = "AllowableClass";
    pub const ALLOWED_RACE: &str = "AllowableRace";
    pub const REQUIRED_SKILL: &str = "RequiredSkill";
    pub const REQUIRED_SKILL_RANK: &str = "RequiredSkillRank";
    pub const REQUIRED_REPUTATION_FACTION: &str = "RequiredReputationFaction";
    pub const REQUIRED_REPUTATION_RANK: &str = "RequiredReputationRank";
    pub const MAX_COUNT: &str = "maxcount";
    pub const ITEM_LEVEL: &str = "ItemLevel";
    pub const REQUIRED_LEVEL: &str = "RequiredLevel";
    pub const STACKABLE: &str = "stackable";
    pub const CONTAINER_SLOTS: &str = "ContainerSlots";
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
    pub const BONDING: &str = "bonding";
    pub const PAGE_TEXT: &str = "PageText";
    pub const START_QUEST: &str = "startquest";
    pub const SHEATH: &str = "sheath";
    pub const RANDOM_PROPERTY: &str = "RandomProperty";
    pub const BLOCK: &str = "block";
    pub const MAX_DURABILITY: &str = "MaxDurability";
    pub const BAG_FAMILY: &str = "BagFamily";
    pub const FOOD_TYPE: &str = "FoodType";

    pub(crate) fn read<'a>(
        row: &'a crate::world_source::Row,
        kind: crate::world_source::SourceKind,
        column: &str,
    ) -> anyhow::Result<&'a str> {
        let name = if kind == crate::world_source::SourceKind::Vmangos {
            match column {
                DISPLAY_ID => "display_id",
                QUALITY => "quality",
                FLAGS => "flags",
                BUY_COUNT => "buy_count",
                BUY_PRICE => "buy_price",
                SELL_PRICE => "sell_price",
                INVENTORY_TYPE => "inventory_type",
                ALLOWED_CLASS => "allowable_class",
                ALLOWED_RACE => "allowable_race",
                REQUIRED_SKILL => "required_skill",
                REQUIRED_SKILL_RANK => "required_skill_rank",
                REQUIRED_REPUTATION_FACTION => "required_reputation_faction",
                REQUIRED_REPUTATION_RANK => "required_reputation_rank",
                MAX_COUNT => "max_count",
                ITEM_LEVEL => "item_level",
                REQUIRED_LEVEL => "required_level",
                CONTAINER_SLOTS => "container_slots",
                PAGE_TEXT => "page_text",
                START_QUEST => "start_quest",
                RANDOM_PROPERTY => "random_property",
                MAX_DURABILITY => "max_durability",
                BAG_FAMILY => "bag_family",
                FOOD_TYPE => "food_type",
                other => other,
            }
        } else {
            column
        };
        if column == NAME {
            row.text(name).map(str::trim)
        } else {
            row.numeric_text(name)
        }
    }
}

pub(crate) mod clt {
    pub const ENTRY: &str = "entry";
    pub const ITEM: &str = "item";
    // Negative ChanceOrQuestChance marks a quest-only drop.
    pub const CHANCE: &str = "ChanceOrQuestChance";
    pub const GROUPID: &str = "groupid"; // 0 = independent; >0 = pick one.
                                         // Negative mincountOrRef refers to a shared loot pool.
    pub const MINCOUNT_OR_REF: &str = "mincountOrRef";
    pub const MAXCOUNT: &str = "maxcount";
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

pub(crate) mod dse {
    pub const ID: &str = "id";
    pub const COMMAND: &str = "command";
    pub const DATALONG: &str = "datalong";
    pub const DATALONG2: &str = "datalong2";
    pub const DATALONG3: &str = "datalong3"; // Copper.
    pub const DATAINT: &str = "dataint"; // Seconds.
    pub const SEND_MAIL: u32 = 38;
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

pub(crate) mod nt {
    pub const ENTRY: &str = "entry";
    pub const SPELL: &str = "spell";
    pub const COST: &str = "spellcost";
    pub const REQ_SKILL: &str = "reqskill";
    pub const REQ_LEVEL: &str = "reqlevel";
}

pub(crate) mod sc {
    pub const SPELL_ID: &str = "spell_id";
    pub const PREV_SPELL: &str = "prev_spell";
    pub const FIRST_SPELL: &str = "first_spell";
    pub const RANK: &str = "rank";
    pub const REQ_SPELL: &str = "req_spell";
}

pub(crate) mod sls {

    pub const ENTRY: &str = "entry";
    pub const SPELL_ID: &str = "SpellID";
    pub const ACTIVE: &str = "Active";
}

pub(crate) mod spe {
    pub const ENTRY: &str = "entry";
    pub const SCHOOL_MASK: &str = "SchoolMask";
    pub const FAMILY_NAME: &str = "SpellFamilyName";
    pub const PROC_FLAGS: &str = "procFlags";
    pub const PROC_EX: &str = "procEx";
    pub const PPM_RATE: &str = "ppmRate";
    pub const CUSTOM_CHANCE: &str = "CustomChance";
    // Source cooldown is seconds; the Module stores milliseconds.
    pub const COOLDOWN: &str = "Cooldown";
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

fn named_columns(dump: &str, table: &str) -> Result<Vec<(String, String)>> {
    let marker = format!("CREATE TABLE `{table}` (");
    let start = dump
        .find(&marker)
        .with_context(|| format!("{table}: missing CREATE TABLE declaration"))?
        + marker.len();
    let end = dump[start..]
        .find("\n)")
        .context("unterminated CREATE TABLE declaration")?
        + start;
    let columns = dump[start..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix('`')?;
            let (name, declaration) = line.split_once('`')?;
            let kind = declaration.split_whitespace().next()?.to_ascii_lowercase();
            Some((name.to_string(), kind))
        })
        .collect::<Vec<_>>();
    let unique = columns
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != columns.len() {
        bail!("{table}: duplicate column declaration");
    }
    if columns.is_empty() {
        bail!("{table}: no column declarations");
    }
    Ok(columns)
}

fn numeric_column(kind: &str) -> bool {
    kind.contains("int")
        || kind.starts_with("float")
        || kind.starts_with("double")
        || kind.starts_with("decimal")
}

fn source_cell(
    table: &str,
    name: &str,
    number: bool,
    value: String,
) -> Result<crate::world_source::Cell> {
    use crate::world_source::Cell;
    if value.trim() == "NULL" {
        Ok(Cell::Null)
    } else if number {
        if !value.trim().parse::<f64>().is_ok_and(f64::is_finite) {
            bail!("{table}.{name}: invalid numeric value");
        }
        Ok(Cell::Number(value.into()))
    } else {
        Ok(Cell::Text(value.into()))
    }
}

pub(crate) fn named_table(dump: &str, table: &str) -> Result<Vec<crate::world_source::Row>> {
    use crate::world_source::TableBuilder;
    let columns = named_columns(dump, table)?;
    let mut output = TableBuilder::new(table, columns.iter().map(|(name, _)| name.clone()));
    let complete = visit_inserts(dump, &format!("INSERT INTO `{table}` VALUES "), |values| {
        if values.len() != columns.len() {
            bail!(
                "{table}: tuple has {} values, CREATE TABLE has {} columns",
                values.len(),
                columns.len()
            );
        }
        let cells = columns
            .iter()
            .zip(values)
            .map(|((name, kind), value)| source_cell(table, name, numeric_column(kind), value))
            .collect::<Result<Vec<_>>>()?;
        output.push(cells)
    })?;
    if !complete {
        bail!("unterminated {table} INSERT in dump");
    }
    Ok(output.finish())
}

#[derive(Debug)]
pub(crate) struct ParsedDump {
    source_hash: String,
    tables: std::collections::BTreeMap<String, Vec<crate::world_source::Row>>,
}

pub(crate) trait TableRows {
    fn rows(&self, table: &str) -> Result<Cow<'_, [crate::world_source::Row]>>;
    fn source_hash(&self) -> Result<String>;
    // May retain extra columns; row selection and checked field access match rows().
    fn columns(
        &self,
        table: &str,
        _columns: &[&str],
    ) -> Result<Cow<'_, [crate::world_source::Row]>> {
        self.rows(table)
    }
    fn kind(&self) -> crate::world_source::SourceKind {
        crate::world_source::SourceKind::ClassicDb
    }
}

impl ParsedDump {
    pub(crate) fn parse(text: String) -> Result<Self> {
        let mut tables = std::collections::BTreeMap::new();
        for &(table, columns) in crate::classic_columns::TABLES {
            check_columns(&text, table, columns)?;
            let rows = named_table(&text, table)?;
            tables.insert(table.to_string(), rows);
        }
        Ok(Self {
            source_hash: sha256_hex(text.as_bytes())?,
            tables,
        })
    }
}

fn check_columns(text: &str, table: &str, columns: &[(&str, bool)]) -> Result<()> {
    let declarations = named_columns(text, table)?;
    for &(column, number) in columns {
        let (_, kind) = declarations
            .iter()
            .find(|(name, _)| name == column)
            .with_context(|| format!("{table}.{column}: missing column"))?;
        if numeric_column(kind) != number {
            bail!("{table}.{column}: source column type changed");
        }
    }
    Ok(())
}

impl TableRows for ParsedDump {
    fn rows(&self, table: &str) -> Result<Cow<'_, [crate::world_source::Row]>> {
        self.tables
            .get(table)
            .map(|rows| Cow::Borrowed(rows.as_slice()))
            .with_context(|| format!("{table}: table is not in the classic-db column contract"))
    }
    fn source_hash(&self) -> Result<String> {
        Ok(self.source_hash.clone())
    }
}

pub(crate) fn parse_table<'a>(
    dump: &'a (impl TableRows + ?Sized),
    table: &str,
) -> Result<Cow<'a, [crate::world_source::Row]>> {
    dump.rows(table)
}

#[cfg(test)]
fn parse_table_inserts(dump: &str, table: &str) -> (Vec<Vec<String>>, bool) {
    parse_inserts(dump, &format!("INSERT INTO `{table}` VALUES "))
}

#[cfg(test)]
fn parse_inserts(dump: &str, needle: &str) -> (Vec<Vec<String>>, bool) {
    let mut rows = Vec::new();
    let complete = visit_inserts(dump, needle, |row| {
        rows.push(row);
        Ok(())
    })
    .expect("collecting fixture tuples cannot fail");
    (rows, complete)
}

fn visit_inserts(
    dump: &str,
    needle: &str,
    mut visit: impl FnMut(Vec<String>) -> Result<()>,
) -> Result<bool> {
    let mut search = 0;
    let mut complete = true;
    while let Some(rel) = dump[search..].find(needle) {
        let mut i = search + rel + needle.len();
        let bytes = dump.as_bytes();
        let mut field = Vec::new();
        let mut tuple: Vec<String> = Vec::new();
        let mut in_tuple = false;
        let mut in_quote = false;
        while i < bytes.len() {
            let c = bytes[i];
            if in_quote {
                if c == b'\\' && i + 1 < bytes.len() {
                    let n = bytes[i + 1];
                    field.push(match n {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        other => other,
                    });
                    i += 2;
                    continue;
                } else if c == b'\'' {
                    in_quote = false;
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    b'(' if !in_tuple => {
                        in_tuple = true;
                        tuple.clear();
                        field.clear();
                    }
                    b'\'' if in_tuple => in_quote = true,
                    b',' if in_tuple => {
                        tuple.push(String::from_utf8(std::mem::take(&mut field))?);
                    }
                    b')' if in_tuple => {
                        tuple.push(String::from_utf8(std::mem::take(&mut field))?);
                        visit(std::mem::take(&mut tuple))?;
                        in_tuple = false;
                    }
                    b';' if !in_tuple => break,
                    _ if in_tuple => field.push(c),
                    _ => {}
                }
            }
            i += 1;
        }
        complete &= i < bytes.len() && !in_tuple && !in_quote;
        search = i;
    }
    Ok(complete)
}

pub(crate) fn field<'a>(row: &'a crate::world_source::Row, column: &str) -> Result<&'a str> {
    row.raw(column)
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

pub(crate) fn read_dump(path: &str) -> Result<ParsedDump> {
    ParsedDump::parse(read_dump_text(path)?)
}

pub(crate) fn read_dump_text(path: &str) -> Result<String> {
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
impl TableRows for str {
    fn rows(&self, table: &str) -> Result<Cow<'_, [crate::world_source::Row]>> {
        use crate::world_source::{Cell, TableBuilder};
        let contract = crate::classic_columns::columns(table)
            .with_context(|| format!("{table}: no fixture column contract"))?;
        if self.contains(&format!("CREATE TABLE `{table}` (")) {
            check_columns(self, table, contract)?;
            return named_table(self, table).map(Cow::Owned);
        }
        let marker = format!("INSERT INTO `{table}`");
        let Some(start) = self.find(&marker) else {
            return Ok(Cow::Owned(Vec::new()));
        };
        let rest = &self[start + marker.len()..];
        let (declaration, _) = rest
            .split_once(" VALUES ")
            .context("fixture INSERT has no VALUES")?;
        let names: Vec<&str> = if declaration.trim().is_empty() {
            contract.iter().map(|(name, _)| *name).collect()
        } else {
            declaration
                .trim()
                .strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .context("fixture INSERT column list")?
                .split(',')
                .map(|name| name.trim().trim_matches('`'))
                .collect()
        };
        let needle = format!("{marker}{declaration} VALUES ");
        let (rows, terminated) = parse_inserts(self, &needle);
        if !terminated {
            bail!("unterminated {table} INSERT in dump");
        }
        let mut output = TableBuilder::new(table, contract.iter().map(|(name, _)| (*name).into()));
        for values in rows {
            if values.len() != names.len() {
                bail!(
                    "{table}: fixture has {} values, expected {}",
                    values.len(),
                    names.len()
                );
            }
            let mut cells = contract
                .iter()
                .map(|(_, number)| {
                    if *number {
                        Cell::Number("0".into())
                    } else {
                        Cell::Text("".into())
                    }
                })
                .collect::<Vec<_>>();
            for (name, value) in names.iter().zip(values) {
                let index = contract
                    .iter()
                    .position(|(column, _)| column == name)
                    .with_context(|| format!("{table}.{name}: unknown fixture column"))?;
                cells[index] = source_cell(table, name, contract[index].1, value)?;
            }
            output.push(cells)?;
        }
        Ok(Cow::Owned(output.finish()))
    }
    fn source_hash(&self) -> Result<String> {
        sha256_hex(self.as_bytes())
    }
}

#[cfg(test)]
impl TableRows for String {
    fn rows(&self, table: &str) -> Result<Cow<'_, [crate::world_source::Row]>> {
        self.as_str().rows(table)
    }
    fn source_hash(&self) -> Result<String> {
        self.as_str().source_hash()
    }
}

#[cfg(test)]
pub(crate) struct FixtureRow {
    columns: Vec<String>,
    values: Vec<String>,
}

#[cfg(test)]
impl FixtureRow {
    pub(crate) fn new(table: &str) -> Self {
        let contract = crate::classic_columns::columns(table).expect("fixture table");
        let columns = contract
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        let values = contract
            .iter()
            .map(|(_, number)| if *number { "0" } else { "''" }.to_string())
            .collect();
        Self { columns, values }
    }
    pub(crate) fn join(&self, separator: &str) -> String {
        self.values.join(separator)
    }
}

#[cfg(test)]
impl std::ops::Index<&str> for FixtureRow {
    type Output = String;
    fn index(&self, name: &str) -> &String {
        &self.values[self
            .columns
            .iter()
            .position(|column| column == name)
            .expect("fixture column")]
    }
}

#[cfg(test)]
impl std::ops::IndexMut<&str> for FixtureRow {
    fn index_mut(&mut self, name: &str) -> &mut String {
        let index = self
            .columns
            .iter()
            .position(|column| column == name)
            .expect("fixture column");
        &mut self.values[index]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn classic_contract_refuses_missing_or_retyped_columns_even_without_rows() {
        for (columns, expected) in [
            (
                "`id` int,\n`wrong` int",
                "areatrigger_involvedrelation.quest: missing column",
            ),
            (
                "`id` text,\n`quest` int",
                "areatrigger_involvedrelation.id: source column type changed",
            ),
        ] {
            let error = ParsedDump::parse(format!(
                "CREATE TABLE `areatrigger_involvedrelation` (\n{columns}\n);"
            ))
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn fixture_and_production_readers_refuse_the_same_numeric_text() {
        for value in ["'bad'", "'NaN'", "'inf'"] {
            let insert = format!("INSERT INTO `areatrigger_involvedrelation` VALUES (1,{value});");
            let declared = format!(
                "CREATE TABLE `areatrigger_involvedrelation` (\n`id` int,\n`quest` int\n);{insert}"
            );
            for error in [
                insert.rows("areatrigger_involvedrelation").unwrap_err(),
                declared.rows("areatrigger_involvedrelation").unwrap_err(),
                ParsedDump::parse(declared).unwrap_err(),
            ] {
                assert!(error
                    .to_string()
                    .contains("areatrigger_involvedrelation.quest: invalid numeric value"));
            }
        }
    }

    #[test]
    fn parsed_dump_borrows_tables_and_refuses_names_outside_the_contract() {
        let mut sql = String::new();
        for &(table, columns) in crate::classic_columns::TABLES {
            let columns = columns
                .iter()
                .map(|(name, number)| format!("`{name}` {}", if *number { "int" } else { "text" }))
                .collect::<Vec<_>>()
                .join(",\n");
            sql.push_str(&format!("CREATE TABLE `{table}` (\n{columns}\n);\n"));
        }
        sql.push_str("INSERT INTO `areatrigger_involvedrelation` VALUES (1,2);");
        let dump = ParsedDump::parse(sql).unwrap();
        let first = dump.rows("areatrigger_involvedrelation").unwrap();
        let second = dump.rows("areatrigger_involvedrelation").unwrap();
        assert!(matches!(first, Cow::Borrowed(_)));
        assert!(std::ptr::eq(&first[0], &second[0]));
        assert!(dump.rows("spell_chain").unwrap().is_empty());
        assert!(dump
            .rows("absent")
            .unwrap_err()
            .to_string()
            .contains("absent: table is not in the classic-db column contract"));
    }

    #[test]
    fn named_reader_refuses_duplicate_columns_bad_values_and_unterminated_rows() {
        assert!(named_table(
            "CREATE TABLE `example` (\n`id` int,\n`id` int\n);",
            "example"
        )
        .is_err());
        let schema = "CREATE TABLE `example` (\n`id` int,\n`label` text\n);";
        for insert in [
            "INSERT INTO `example` VALUES (1);",
            "INSERT INTO `example` VALUES ('bad','x');",
            "INSERT INTO `example` VALUES (1,'x')",
        ] {
            assert!(named_table(&format!("{schema}{insert}"), "example").is_err());
        }
    }

    #[test]
    fn named_reader_preserves_utf8_through_mysql_escapes() {
        let rows = named_table(
            "CREATE TABLE `example` (\n`id` int,\n`label` text\n); INSERT INTO `example` VALUES (1,'Café, 雪 \\'🔥\\'\\nété');",
            "example",
        )
        .unwrap();
        assert_eq!(rows[0].text("label").unwrap(), "Café, 雪 '🔥'\nété");
    }

    #[test]
    fn parses_quoted_names_with_commas_and_escapes() {
        let dump = "x INSERT INTO `creature_template` (`Entry`, `Name`, `SubName`, `MinLevel`) VALUES (1,'A, the Bold','Sub\\'s',5),(2,'B',NULL,7); y";
        let rows = parse_table(dump, "creature_template").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(field(&rows[0], "Entry").unwrap(), "1");
        assert_eq!(field(&rows[0], "Name").unwrap(), "A, the Bold");
        assert_eq!(field(&rows[0], "SubName").unwrap(), "Sub's");
        assert_eq!(field(&rows[0], "MinLevel").unwrap(), "5");
        assert_eq!(field(&rows[1], "SubName").unwrap(), "NULL");
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
            let err =
                read_dump_text(path).expect_err("a missing dump must not read as an empty dump");
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
            b"INSERT INTO `spell_chain` (`spell_id`) VALUES (1);",
        );
        let err =
            read_dump_text(f.path()).expect_err("non-gzip bytes behind a .gz name must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("gunzip dump"),
            "the error must point at the decompression step, got: {msg}"
        );
    }

    #[test]
    fn read_dump_decodes_invalid_utf8_lossily_instead_of_failing() {
        let f = ScratchFile::new("latin1.sql", b"INSERT INTO `t` VALUES (1,'Caf\xE9 Ale');");
        let text =
            read_dump_text(f.path()).expect("a stray non-UTF8 byte must not fail the import");
        assert!(
            text.starts_with("INSERT INTO `t` VALUES (1,'Caf") && text.ends_with(" Ale');"),
            "the ASCII around the bad byte must survive verbatim, got: {text:?}"
        );
        assert!(
            text.contains('\u{FFFD}'),
            "the bad byte must become the replacement char, not be dropped: {text:?}"
        );
        let (rows, complete) = parse_table_inserts(&text, "t");
        assert!(complete);
        assert_eq!(
            rows.len(),
            1,
            "the row around the bad byte must still parse"
        );
        assert_eq!(rows[0][0], "1");
    }

    #[test]
    fn parse_table_returns_no_rows_for_a_table_the_dump_never_mentions() {
        let dump = "x INSERT INTO `creature_template` (`Entry`, `Name`, `SubName`, `MinLevel`) VALUES (1,'A',NULL,5); y";
        assert!(
            parse_table(dump, "spell_chain").unwrap().is_empty(),
            "an absent table must yield zero rows, not a panic"
        );
        assert!(parse_table("", "creature_template").unwrap().is_empty());
        assert!(parse_table("-- comment only, no INSERTs\n", "creature")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parse_table_name_match_is_exact_not_a_prefix_of_a_longer_table() {
        let templates = "x INSERT INTO `creature_template` (`Entry`, `Name`, `SubName`, `MinLevel`) VALUES (1,'A',NULL,5); y";
        assert!(
            parse_table(templates, "creature").unwrap().is_empty(),
            "`creature` must NOT match `creature_template` rows"
        );
        let spawns =
            "x INSERT INTO `creature` (`guid`, `id`, `map`, `spawnMask`) VALUES (7,299,0,1.0); y";
        assert!(
            parse_table(spawns, "creature_template").unwrap().is_empty(),
            "`creature_template` must NOT match `creature` rows"
        );
        let both = format!("{templates}\n{spawns}");
        assert_eq!(parse_table(&both, "creature_template").unwrap().len(), 1);
        assert_eq!(parse_table(&both, "creature").unwrap().len(), 1);
    }

    #[test]
    fn incomplete_inserts_fail_instead_of_importing_a_partial_table() {
        for truncated in [
            "INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133",
            "INSERT INTO `spell_chain` VALUES (133,0,133,1,0)",
        ] {
            assert!(truncated.rows("spell_chain").is_err());
        }
    }

    #[test]
    fn an_unterminated_quote_fails_the_table_read() {
        let dump = "INSERT INTO `creature_template` (`Entry`, `Name`) VALUES (1,'Good'),(2,'oops); INSERT INTO `creature_template` VALUES (3,'Later');";
        assert!(dump.rows("creature_template").is_err());
    }
}
