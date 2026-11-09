use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};

use crate::source::{ColumnType, Filter, Row, SourceDatabase, SourceKind};

use ColumnType::{Float, Integer, Text};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ban {
    Permanent,
    Until(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LegacyAccount {
    pub(crate) id: u64,
    pub(crate) username: String,
    // realmd keeps SRP6 values as big-endian hexadecimal.
    pub(crate) salt_hex: String,
    pub(crate) verifier_hex: String,
    pub(crate) gm_level: u8,
    pub(crate) ban: Option<Ban>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LegacyPosition {
    pub(crate) map: u32,
    pub(crate) zone: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) z: f32,
    pub(crate) orientation: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LegacyItem {
    pub(crate) item_guid: u64,
    // Zero for an inventory slot, otherwise the item guid of the containing bag.
    pub(crate) bag_guid: u64,
    pub(crate) slot: u8,
    pub(crate) entry: u32,
    pub(crate) count: u32,
    pub(crate) flags: u32,
    pub(crate) enchantments: Vec<u32>,
    pub(crate) random_property_id: i64,
    pub(crate) durability: u32,
    pub(crate) text_id: u32,
    pub(crate) text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LegacyPet {
    pub(crate) id: u64,
    pub(crate) entry: u32,
    pub(crate) hunter: bool,
    pub(crate) slot: u32,
    pub(crate) name: String,
    pub(crate) renamed: bool,
    pub(crate) level: u32,
    pub(crate) xp: u32,
    pub(crate) loyalty: u8,
    pub(crate) loyalty_points: u32,
    pub(crate) happiness: u32,
    pub(crate) spells: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LegacyLetter {
    pub(crate) id: u64,
    pub(crate) message_type: u8,
    pub(crate) mail_template_id: u32,
    pub(crate) sender: u64,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) attachments: Vec<LegacyItem>,
    pub(crate) expire_secs: u64,
    pub(crate) deliver_secs: u64,
    pub(crate) money: u32,
    pub(crate) cod: u32,
    pub(crate) checked: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LegacySkill {
    pub(crate) skill: u32,
    pub(crate) value: u16,
    pub(crate) max: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LegacyReputation {
    pub(crate) faction: u32,
    pub(crate) standing: i32,
    pub(crate) flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LegacyAction {
    pub(crate) button: u8,
    pub(crate) action: u32,
    pub(crate) action_type: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LegacyQuest {
    pub(crate) quest: u32,
    pub(crate) status: u32,
    pub(crate) rewarded: bool,
    pub(crate) explored: bool,
    pub(crate) timer_secs: u64,
    pub(crate) mob_counts: [u32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LegacyCharacter {
    pub(crate) guid: u64,
    pub(crate) account: u64,
    pub(crate) name: String,
    pub(crate) race: u8,
    pub(crate) class: u8,
    pub(crate) gender: u8,
    pub(crate) skin: u8,
    pub(crate) face: u8,
    pub(crate) hair_style: u8,
    pub(crate) hair_color: u8,
    pub(crate) facial_hair: u8,
    pub(crate) level: u8,
    pub(crate) xp: u32,
    pub(crate) money: u32,
    pub(crate) position: LegacyPosition,
    pub(crate) on_transport: bool,
    pub(crate) online: bool,
    pub(crate) played_total_secs: u32,
    pub(crate) logout_secs: u64,
    pub(crate) logout_resting: bool,
    pub(crate) rest_bonus: f64,
    pub(crate) bank_bag_slots: u8,
    pub(crate) watched_faction: i64,
    pub(crate) taxi_mask: Vec<u32>,
    pub(crate) explored_zones: Vec<u32>,
    pub(crate) home: Option<LegacyPosition>,
    pub(crate) items: Vec<LegacyItem>,
    pub(crate) spells: Vec<u32>,
    pub(crate) skills: Vec<LegacySkill>,
    pub(crate) reputations: Vec<LegacyReputation>,
    pub(crate) actions: Vec<LegacyAction>,
    pub(crate) quests: Vec<LegacyQuest>,
    pub(crate) letters: Vec<LegacyLetter>,
    pub(crate) pets: Vec<LegacyPet>,
    pub(crate) contacts: Vec<LegacyContact>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LegacyContact {
    pub(crate) target: u64,
    pub(crate) flags: u32,
}

enum Appearance {
    Columns,
    // cmangos packs appearance and bank bag slots into the playerBytes descriptor fields.
    PlayerBytes,
}

pub(crate) struct Dialect {
    appearance: Appearance,
    taxi_mask: &'static str,
    played_total: &'static str,
    transport_guid: &'static str,
    watched_faction: &'static str,
    explored_zones: &'static str,
    inventory_item: &'static str,
    item_entry: &'static str,
    item_random_property: &'static str,
    item_text: &'static str,
    quest_mob_count: [&'static str; 4],
    ban_account: &'static str,
    ban_date: &'static str,
    ban_expiry: &'static str,
    mail_type: &'static str,
    mail_template: &'static str,
    mail_sender: &'static str,
    mail_receiver: &'static str,
    mail_text: &'static str,
    pet_owner: &'static str,
    pet_type: &'static str,
    pet_xp: &'static str,
    pet_loyalty_points: &'static str,
    pet_happiness: &'static str,
}

const VMANGOS: Dialect = Dialect {
    appearance: Appearance::Columns,
    taxi_mask: "known_taxi_mask",
    played_total: "played_time_total",
    transport_guid: "transport_guid",
    watched_faction: "watched_faction",
    explored_zones: "explored_zones",
    inventory_item: "item_guid",
    item_entry: "item_id",
    item_random_property: "random_property_id",
    item_text: "text",
    quest_mob_count: ["mob_count1", "mob_count2", "mob_count3", "mob_count4"],
    ban_account: "id",
    ban_date: "bandate",
    ban_expiry: "unbandate",
    mail_type: "message_type",
    mail_template: "mail_template_id",
    mail_sender: "sender_guid",
    mail_receiver: "receiver_guid",
    mail_text: "item_text_id",
    pet_owner: "owner_guid",
    pet_type: "pet_type",
    pet_xp: "xp",
    pet_loyalty_points: "loyalty_points",
    pet_happiness: "current_happiness",
};

const CMANGOS: Dialect = Dialect {
    appearance: Appearance::PlayerBytes,
    taxi_mask: "taximask",
    played_total: "totaltime",
    transport_guid: "transguid",
    watched_faction: "watchedFaction",
    explored_zones: "exploredZones",
    inventory_item: "item",
    item_entry: "itemEntry",
    item_random_property: "randomPropertyId",
    item_text: "itemTextId",
    quest_mob_count: ["mobcount1", "mobcount2", "mobcount3", "mobcount4"],
    ban_account: "account_id",
    ban_date: "banned_at",
    ban_expiry: "expires_at",
    mail_type: "messageType",
    mail_template: "mailTemplateId",
    mail_sender: "sender",
    mail_receiver: "receiver",
    mail_text: "itemTextId",
    pet_owner: "owner",
    pet_type: "PetType",
    pet_xp: "exp",
    pet_loyalty_points: "loyaltypoints",
    pet_happiness: "curhappiness",
};

impl Dialect {
    pub(crate) fn of(kind: SourceKind) -> &'static Self {
        match kind {
            SourceKind::Vmangos => &VMANGOS,
            SourceKind::Cmangos => &CMANGOS,
        }
    }

    fn realmd_tables(&self) -> Vec<(&'static str, Vec<(&'static str, ColumnType)>)> {
        vec![
            (
                "account",
                vec![
                    ("id", Integer),
                    ("username", Text),
                    ("gmlevel", Integer),
                    ("v", Text),
                    ("s", Text),
                ],
            ),
            (
                "account_banned",
                vec![
                    (self.ban_account, Integer),
                    (self.ban_date, Integer),
                    (self.ban_expiry, Integer),
                    ("active", Integer),
                ],
            ),
        ]
    }

    fn character_columns(&self) -> Vec<(&'static str, ColumnType)> {
        let mut columns = vec![
            ("guid", Integer),
            ("account", Integer),
            ("name", Text),
            ("race", Integer),
            ("class", Integer),
            ("gender", Integer),
            ("level", Integer),
            ("xp", Integer),
            ("money", Integer),
            ("map", Integer),
            ("zone", Integer),
            ("position_x", Float),
            ("position_y", Float),
            ("position_z", Float),
            ("orientation", Float),
            (self.transport_guid, Integer),
            ("online", Integer),
            (self.played_total, Integer),
            ("logout_time", Integer),
            ("is_logout_resting", Integer),
            ("rest_bonus", Float),
            (self.watched_faction, Integer),
            (self.taxi_mask, Text),
            (self.explored_zones, Text),
        ];
        match self.appearance {
            Appearance::Columns => columns.extend([
                ("skin", Integer),
                ("face", Integer),
                ("hair_style", Integer),
                ("hair_color", Integer),
                ("facial_hair", Integer),
                ("bank_bag_slots", Integer),
            ]),
            Appearance::PlayerBytes => {
                columns.extend([("playerBytes", Integer), ("playerBytes2", Integer)])
            }
        }
        columns
    }

    fn characters_tables(&self) -> Vec<(&'static str, Vec<(&'static str, ColumnType)>)> {
        let mut quest = vec![
            ("guid", Integer),
            ("quest", Integer),
            ("status", Integer),
            ("rewarded", Integer),
            ("explored", Integer),
            ("timer", Integer),
        ];
        quest.extend(self.quest_mob_count.iter().map(|column| (*column, Integer)));
        vec![
            ("characters", self.character_columns()),
            (
                "character_homebind",
                vec![
                    ("guid", Integer),
                    ("map", Integer),
                    ("zone", Integer),
                    ("position_x", Float),
                    ("position_y", Float),
                    ("position_z", Float),
                ],
            ),
            (
                "character_inventory",
                vec![
                    ("guid", Integer),
                    ("bag", Integer),
                    ("slot", Integer),
                    (self.inventory_item, Integer),
                ],
            ),
            (
                "item_instance",
                vec![
                    ("guid", Integer),
                    ("owner_guid", Integer),
                    (self.item_entry, Integer),
                    ("count", Integer),
                    ("flags", Integer),
                    ("enchantments", Text),
                    (self.item_random_property, Integer),
                    ("durability", Integer),
                    (self.item_text, Integer),
                ],
            ),
            (
                "character_spell",
                vec![("guid", Integer), ("spell", Integer), ("disabled", Integer)],
            ),
            (
                "character_skills",
                vec![
                    ("guid", Integer),
                    ("skill", Integer),
                    ("value", Integer),
                    ("max", Integer),
                ],
            ),
            (
                "character_reputation",
                vec![
                    ("guid", Integer),
                    ("faction", Integer),
                    ("standing", Integer),
                    ("flags", Integer),
                ],
            ),
            (
                "character_action",
                vec![
                    ("guid", Integer),
                    ("button", Integer),
                    ("action", Integer),
                    ("type", Integer),
                ],
            ),
            ("character_queststatus", quest),
            (
                "mail",
                vec![
                    ("id", Integer),
                    (self.mail_type, Integer),
                    (self.mail_template, Integer),
                    (self.mail_sender, Integer),
                    (self.mail_receiver, Integer),
                    ("subject", Text),
                    (self.mail_text, Integer),
                    ("expire_time", Integer),
                    ("deliver_time", Integer),
                    ("money", Integer),
                    ("cod", Integer),
                    ("checked", Integer),
                ],
            ),
            (
                "mail_items",
                vec![("mail_id", Integer), ("item_guid", Integer)],
            ),
            ("item_text", vec![("id", Integer), ("text", Text)]),
            ("character_pet", self.pet_columns()),
            ("pet_spell", vec![("guid", Integer), ("spell", Integer)]),
            (
                "character_social",
                vec![("guid", Integer), ("friend", Integer), ("flags", Integer)],
            ),
        ]
    }

    fn pet_columns(&self) -> Vec<(&'static str, ColumnType)> {
        vec![
            ("id", Integer),
            ("entry", Integer),
            (self.pet_owner, Integer),
            (self.pet_type, Integer),
            ("level", Integer),
            (self.pet_xp, Integer),
            (self.pet_loyalty_points, Integer),
            ("loyalty", Integer),
            ("name", Text),
            ("renamed", Integer),
            ("slot", Integer),
            (self.pet_happiness, Integer),
        ]
    }
}

// The Source Profile: every consumed column must exist with the expected type family.
fn check_profile(
    database: &mut dyn SourceDatabase,
    label: &str,
    tables: &[(&'static str, Vec<(&'static str, ColumnType)>)],
) -> Result<()> {
    let mut problems = Vec::new();
    for (table, columns) in tables {
        let found = database.column_types(table)?;
        for (column, expected) in columns {
            match found.get(*column) {
                Some(actual) if actual == expected => {}
                Some(actual) => {
                    problems.push(format!("{table}.{column} is {actual:?}, not {expected:?}"))
                }
                None => problems.push(format!("{table}.{column} is missing")),
            }
        }
    }
    if !problems.is_empty() {
        bail!(
            "the {label} database does not match the Source Profile: {}",
            problems.join("; ")
        );
    }
    Ok(())
}

pub(crate) fn read_accounts(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    now_secs: u64,
) -> Result<Vec<LegacyAccount>> {
    let tables = dialect.realmd_tables();
    check_profile(database, "realmd", &tables)?;
    let mut bans: HashMap<u64, Ban> = HashMap::new();
    for row in database.select(
        "account_banned",
        &[
            dialect.ban_account,
            dialect.ban_date,
            dialect.ban_expiry,
            "active",
        ],
        Filter::All,
    )? {
        if row.int("active")? == 0 {
            continue;
        }
        let banned_at: u64 = row.number(dialect.ban_date)?;
        let expires_at: u64 = row.number(dialect.ban_expiry)?;
        let ban = if expires_at <= banned_at {
            Ban::Permanent
        } else if expires_at > now_secs {
            Ban::Until(expires_at)
        } else {
            continue;
        };
        let account = row.number(dialect.ban_account)?;
        let stronger = match (bans.get(&account), &ban) {
            (Some(Ban::Permanent), _) => false,
            (Some(Ban::Until(old)), Ban::Until(new)) => new > old,
            _ => true,
        };
        if stronger {
            bans.insert(account, ban);
        }
    }
    database
        .select(
            "account",
            &["id", "username", "gmlevel", "v", "s"],
            Filter::All,
        )?
        .into_iter()
        .map(|row| {
            let id = row.number("id")?;
            Ok(LegacyAccount {
                id,
                username: row.text("username")?,
                salt_hex: row.text("s")?,
                verifier_hex: row.text("v")?,
                gm_level: row.number("gmlevel")?,
                ban: bans.remove(&id),
            })
        })
        .collect()
}

pub(crate) fn check_characters_profile(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
) -> Result<()> {
    check_profile(database, "characters", &dialect.characters_tables())
}

// Characters unlinked from their Account by a source deletion have account 0 and are skipped.
pub(crate) fn read_characters(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    accounts: &[u64],
) -> Result<Vec<LegacyCharacter>> {
    let columns: Vec<&str> = dialect
        .character_columns()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut characters = Vec::new();
    for &account in accounts {
        for row in database.select("characters", &columns, Filter::Equals("account", account))? {
            let guid: u64 = row.number("guid")?;
            characters.push(
                read_character(database, dialect, &row)
                    .with_context(|| format!("read source Character {guid}"))?,
            );
        }
    }
    Ok(characters)
}

fn read_character(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    row: &Row,
) -> Result<LegacyCharacter> {
    let guid: u64 = row.number("guid")?;
    let by_guid = Filter::Equals("guid", guid);
    let (skin, face, hair_style, hair_color, facial_hair, bank_bag_slots) = match dialect.appearance
    {
        Appearance::Columns => (
            row.number("skin")?,
            row.number("face")?,
            row.number("hair_style")?,
            row.number("hair_color")?,
            row.number("facial_hair")?,
            row.number("bank_bag_slots")?,
        ),
        Appearance::PlayerBytes => {
            let bytes = row.int("playerBytes")?.to_le_bytes();
            let bytes_2 = row.int("playerBytes2")?.to_le_bytes();
            (
                bytes[0], bytes[1], bytes[2], bytes[3], bytes_2[0], bytes_2[2],
            )
        }
    };
    let home = database
        .select(
            "character_homebind",
            &["map", "zone", "position_x", "position_y", "position_z"],
            by_guid,
        )?
        .first()
        .map(|home| -> Result<LegacyPosition> {
            Ok(LegacyPosition {
                map: home.number("map")?,
                zone: home.number("zone")?,
                x: home.float("position_x")? as f32,
                y: home.float("position_y")? as f32,
                z: home.float("position_z")? as f32,
                orientation: 0.0,
            })
        })
        .transpose()?;
    Ok(LegacyCharacter {
        guid,
        account: row.number("account")?,
        name: row.text("name")?,
        race: row.number("race")?,
        class: row.number("class")?,
        gender: row.number("gender")?,
        skin,
        face,
        hair_style,
        hair_color,
        facial_hair,
        level: row.number("level")?,
        xp: row.number("xp")?,
        money: row.number("money")?,
        position: LegacyPosition {
            map: row.number("map")?,
            zone: row.number("zone")?,
            x: row.float("position_x")? as f32,
            y: row.float("position_y")? as f32,
            z: row.float("position_z")? as f32,
            orientation: row.float("orientation")? as f32,
        },
        on_transport: row.int(dialect.transport_guid)? != 0,
        online: row.int("online")? != 0,
        played_total_secs: row.number(dialect.played_total)?,
        logout_secs: row.number("logout_time")?,
        logout_resting: row.int("is_logout_resting")? != 0,
        rest_bonus: row.float("rest_bonus")?,
        bank_bag_slots,
        watched_faction: row.int(dialect.watched_faction)?,
        taxi_mask: row.words(dialect.taxi_mask)?,
        explored_zones: row.words(dialect.explored_zones)?,
        home,
        items: read_items(database, dialect, guid)?,
        spells: database
            .select("character_spell", &["spell", "disabled"], by_guid)?
            .into_iter()
            .filter_map(|spell| match spell.int("disabled") {
                Ok(0) => Some(spell.number("spell")),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<_>>()?,
        skills: database
            .select("character_skills", &["skill", "value", "max"], by_guid)?
            .into_iter()
            .map(|skill| {
                Ok(LegacySkill {
                    skill: skill.number("skill")?,
                    value: skill.number("value")?,
                    max: skill.number("max")?,
                })
            })
            .collect::<Result<_>>()?,
        reputations: database
            .select(
                "character_reputation",
                &["faction", "standing", "flags"],
                by_guid,
            )?
            .into_iter()
            .map(|reputation| {
                Ok(LegacyReputation {
                    faction: reputation.number("faction")?,
                    standing: reputation.number("standing")?,
                    flags: reputation.number("flags")?,
                })
            })
            .collect::<Result<_>>()?,
        actions: database
            .select("character_action", &["button", "action", "type"], by_guid)?
            .into_iter()
            .map(|action| {
                Ok(LegacyAction {
                    button: action.number("button")?,
                    action: action.number("action")?,
                    action_type: action.number("type")?,
                })
            })
            .collect::<Result<_>>()?,
        quests: read_quests(database, dialect, guid)?,
        letters: read_letters(database, dialect, guid)?,
        pets: read_pets(database, dialect, guid)?,
        contacts: database
            .select("character_social", &["friend", "flags"], by_guid)?
            .into_iter()
            .map(|contact| {
                Ok(LegacyContact {
                    target: contact.number("friend")?,
                    flags: contact.number("flags")?,
                })
            })
            .collect::<Result<_>>()?,
    })
}

// MaNGOS pet_type 1 is a Hunter pet; 0 is a summoned pet such as a Warlock demon.
const HUNTER_PET_TYPE: i64 = 1;

fn read_pets(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    guid: u64,
) -> Result<Vec<LegacyPet>> {
    let columns: Vec<&str> = dialect
        .pet_columns()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let rows = database.select(
        "character_pet",
        &columns,
        Filter::Equals(dialect.pet_owner, guid),
    )?;
    let mut pets = Vec::new();
    for row in rows {
        let id: u64 = row.number("id")?;
        pets.push(LegacyPet {
            id,
            entry: row.number("entry")?,
            hunter: row.int(dialect.pet_type)? == HUNTER_PET_TYPE,
            slot: row.number("slot")?,
            name: row.text("name")?,
            renamed: row.int("renamed")? != 0,
            level: row.number("level")?,
            xp: row.number(dialect.pet_xp)?,
            loyalty: row.number("loyalty")?,
            loyalty_points: row.number(dialect.pet_loyalty_points)?,
            happiness: row.number(dialect.pet_happiness)?,
            spells: database
                .select("pet_spell", &["spell"], Filter::Equals("guid", id))?
                .len(),
        });
    }
    Ok(pets)
}

fn item_columns(dialect: &Dialect) -> [&'static str; 8] {
    [
        "guid",
        dialect.item_entry,
        "count",
        "flags",
        "enchantments",
        dialect.item_random_property,
        "durability",
        dialect.item_text,
    ]
}

fn legacy_item(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    instance: &Row,
    bag_guid: u64,
    slot: u8,
) -> Result<LegacyItem> {
    let text_id = instance.number(dialect.item_text)?;
    Ok(LegacyItem {
        item_guid: instance.number("guid")?,
        bag_guid,
        slot,
        entry: instance.number(dialect.item_entry)?,
        count: instance.number("count")?,
        flags: instance.number("flags")?,
        enchantments: instance.words("enchantments")?,
        random_property_id: instance.int(dialect.item_random_property)?,
        durability: instance.number("durability")?,
        text_id,
        text: item_text(database, text_id)?,
    })
}

fn item_text(database: &mut dyn SourceDatabase, text_id: u32) -> Result<Option<String>> {
    if text_id == 0 {
        return Ok(None);
    }
    database
        .select(
            "item_text",
            &["text"],
            Filter::Equals("id", u64::from(text_id)),
        )?
        .first()
        .map(|row| row.text("text"))
        .transpose()
}

fn read_letters(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    guid: u64,
) -> Result<Vec<LegacyLetter>> {
    let mail = database.select(
        "mail",
        &[
            "id",
            dialect.mail_type,
            dialect.mail_template,
            dialect.mail_sender,
            "subject",
            dialect.mail_text,
            "expire_time",
            "deliver_time",
            "money",
            "cod",
            "checked",
        ],
        Filter::Equals(dialect.mail_receiver, guid),
    )?;
    let mut letters = Vec::new();
    for row in mail {
        let id: u64 = row.number("id")?;
        let mut attachments = Vec::new();
        for attachment in
            database.select("mail_items", &["item_guid"], Filter::Equals("mail_id", id))?
        {
            let item_guid: u64 = attachment.number("item_guid")?;
            let instance = database
                .select(
                    "item_instance",
                    &item_columns(dialect),
                    Filter::Equals("guid", item_guid),
                )?
                .into_iter()
                .next()
                .with_context(|| {
                    format!("mail {id} attaches item {item_guid}, which has no item_instance row")
                })?;
            attachments.push(legacy_item(database, dialect, &instance, 0, 0)?);
        }
        letters.push(LegacyLetter {
            id,
            message_type: row.number(dialect.mail_type)?,
            mail_template_id: row.number(dialect.mail_template)?,
            sender: row.number(dialect.mail_sender)?,
            subject: row.text("subject")?,
            body: item_text(database, row.number(dialect.mail_text)?)?.unwrap_or_default(),
            attachments,
            expire_secs: row.number("expire_time")?,
            deliver_secs: row.number("deliver_time")?,
            money: row.number("money")?,
            cod: row.number("cod")?,
            checked: row.number("checked")?,
        });
    }
    Ok(letters)
}

fn read_items(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    guid: u64,
) -> Result<Vec<LegacyItem>> {
    let instances: BTreeMap<u64, Row> = database
        .select(
            "item_instance",
            &item_columns(dialect),
            Filter::Equals("owner_guid", guid),
        )?
        .into_iter()
        .map(|row| Ok((row.number("guid")?, row)))
        .collect::<Result<_>>()?;
    let slots = database.select(
        "character_inventory",
        &["bag", "slot", dialect.inventory_item],
        Filter::Equals("guid", guid),
    )?;
    slots
        .into_iter()
        .map(|slot| {
            let item_guid: u64 = slot.number(dialect.inventory_item)?;
            let instance = instances.get(&item_guid).with_context(|| {
                format!("inventory names item {item_guid}, which has no item_instance row")
            })?;
            legacy_item(
                database,
                dialect,
                instance,
                slot.number("bag")?,
                slot.number("slot")?,
            )
        })
        .collect()
}

fn read_quests(
    database: &mut dyn SourceDatabase,
    dialect: &Dialect,
    guid: u64,
) -> Result<Vec<LegacyQuest>> {
    let mut columns = vec!["quest", "status", "rewarded", "explored", "timer"];
    columns.extend(dialect.quest_mob_count);
    database
        .select(
            "character_queststatus",
            &columns,
            Filter::Equals("guid", guid),
        )?
        .into_iter()
        .map(|quest| {
            let mut mob_counts = [0; 4];
            for (count, column) in mob_counts.iter_mut().zip(dialect.quest_mob_count) {
                *count = quest.number(column)?;
            }
            Ok(LegacyQuest {
                quest: quest.number("quest")?,
                status: quest.number("status")?,
                rewarded: quest.int("rewarded")? != 0,
                explored: quest.int("explored")? != 0,
                timer_secs: quest.number("timer")?,
                mob_counts,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::source::fake::FakeDatabase;
    use crate::source::Value;

    pub(crate) fn source(kind: SourceKind) -> FakeDatabase {
        let dialect = Dialect::of(kind);
        let mut database = FakeDatabase::default();
        for (table, columns) in dialect
            .realmd_tables()
            .into_iter()
            .chain(dialect.characters_tables())
        {
            database.table(table, &columns);
        }
        database
    }

    #[test]
    fn a_missing_source_column_stops_the_read_with_its_name() {
        let mut database = FakeDatabase::default();
        database.table("account", &[("id", Integer), ("username", Text)]);
        let error = read_accounts(&mut database, Dialect::of(SourceKind::Vmangos), 0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("account.gmlevel is missing"), "{error}");
        assert!(
            error.contains("account_banned.bandate is missing"),
            "{error}"
        );
    }

    #[test]
    fn active_unexpired_and_permanent_bans_carry_over() {
        let mut database = source(SourceKind::Vmangos);
        for id in 1..=4 {
            database.row("account", &[("id", Value::Int(id))]);
        }
        let ban = |id, bandate, unbandate, active| {
            vec![
                ("id", Value::Int(id)),
                ("bandate", Value::Int(bandate)),
                ("unbandate", Value::Int(unbandate)),
                ("active", Value::Int(active)),
            ]
        };
        database.row("account_banned", &ban(1, 100, 100, 1));
        database.row("account_banned", &ban(2, 100, 5000, 1));
        database.row("account_banned", &ban(3, 100, 500, 1));
        database.row("account_banned", &ban(4, 100, 100, 0));

        let bans: Vec<_> = read_accounts(&mut database, Dialect::of(SourceKind::Vmangos), 1000)
            .unwrap()
            .into_iter()
            .map(|account| account.ban)
            .collect();
        assert_eq!(
            bans,
            [Some(Ban::Permanent), Some(Ban::Until(5000)), None, None]
        );
    }

    #[test]
    fn cmangos_appearance_and_bank_slots_unpack_from_player_bytes() {
        let mut database = source(SourceKind::Cmangos);
        let player_bytes = i64::from(u32::from_le_bytes([1, 2, 3, 4]));
        let player_bytes_2 = i64::from(u32::from_le_bytes([5, 0, 6, 2]));
        database.row(
            "characters",
            &[
                ("guid", Value::Int(10)),
                ("account", Value::Int(1)),
                ("playerBytes", Value::Int(player_bytes)),
                ("playerBytes2", Value::Int(player_bytes_2)),
            ],
        );
        let character =
            &read_characters(&mut database, Dialect::of(SourceKind::Cmangos), &[1]).unwrap()[0];
        assert_eq!(
            (
                character.skin,
                character.face,
                character.hair_style,
                character.hair_color,
                character.facial_hair,
                character.bank_bag_slots
            ),
            (1, 2, 3, 4, 5, 6)
        );
    }

    #[test]
    fn an_inventory_row_joins_its_item_instance_and_bag() {
        let mut database = source(SourceKind::Vmangos);
        database.row(
            "characters",
            &[("guid", Value::Int(10)), ("account", Value::Int(1))],
        );
        database.row(
            "character_inventory",
            &[
                ("guid", Value::Int(10)),
                ("bag", Value::Int(500)),
                ("slot", Value::Int(3)),
                ("item_guid", Value::Int(501)),
            ],
        );
        database.row(
            "item_instance",
            &[
                ("guid", Value::Int(501)),
                ("owner_guid", Value::Int(10)),
                ("item_id", Value::Int(2589)),
                ("count", Value::Int(7)),
                ("flags", Value::Int(1)),
                ("enchantments", Value::Text("15 0 0 0 0 0".to_string())),
            ],
        );
        let character =
            &read_characters(&mut database, Dialect::of(SourceKind::Vmangos), &[1]).unwrap()[0];
        assert_eq!(
            character.items,
            [LegacyItem {
                item_guid: 501,
                bag_guid: 500,
                slot: 3,
                entry: 2589,
                count: 7,
                flags: 1,
                enchantments: vec![15, 0, 0, 0, 0, 0],
                random_property_id: 0,
                durability: 0,
                text_id: 0,
                text: None,
            }]
        );
    }
}
