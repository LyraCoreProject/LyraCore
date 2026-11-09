//! Before changing Character Migration, read docs/character-migration.md.

use std::collections::{HashMap, HashSet};

use spacetimedb::{reducer, table, Identity, ReducerContext, SpacetimeType, Table, Timestamp};

use crate::auth::{account_has_room, check_new_character, next_character_guid};
use crate::helpers::require_operator;
use crate::{
    game_account, game_area, game_character, game_character_explored, game_character_talent,
    game_character_taxi_node, game_creature_template, game_faction, game_hunter_pet,
    game_item_template, game_map, game_player_action, game_quest_objective, game_quest_template,
    game_skill_line, game_spell, game_talent, game_taxi_node, Account, Character,
    CharacterExplored, CharacterQuestEventCredit, CharacterTalent, CharacterTaxiNode, HunterPet,
    ItemInstance, PlayerAction, PlayerReputation, PlayerSkill,
};

#[table(accessor = game_character_migration)]
pub struct CharacterMigrationRecord {
    #[primary_key]
    pub source_key: String,
    pub source: String,
    pub source_guid: u64,
    pub character_guid: u64,
    pub migrated_at: Timestamp,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedAccount {
    pub username: String,
    // Little-endian SRP6 values, as the logon proof uses them.
    pub salt: Vec<u8>,
    pub verifier: Vec<u8>,
    pub banned: bool,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct MigratedPosition {
    pub map_id: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigratedBagSlot {
    pub bag_slot: u8,
    pub slot: u8,
}

// Vanilla addresses: an inventory slot, or a slot inside the bag equipped at `bag_slot`.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigratedItemLocation {
    Slot(u8),
    Bag(MigratedBagSlot),
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedItem {
    pub entry: u32,
    pub location: MigratedItemLocation,
    pub stack_count: u32,
    pub durability: u32,
    pub enchant_id: u32,
    pub random_property_id: u32,
    pub soulbound: bool,
    // The Item Text of a Letter Copy.
    pub text: Option<String>,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedAttachment {
    pub entry: u32,
    pub stack_count: u32,
    pub durability: u32,
    pub enchant_id: u32,
    pub random_property_id: u32,
    pub soulbound: bool,
    pub text: Option<String>,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct LetterMigration {
    pub source: String,
    pub source_mail_id: u64,
    // Zero, or the attachment's source item guid when one source Mail becomes several letters.
    pub source_item_guid: u64,
    pub recipient_source_guid: u64,
    // MailMessageType: 0 Character, 2 auction house, 3 creature, 4 GameObject.
    pub sender_kind: u8,
    pub sender_source_guid: u64,
    pub sender_entry: u32,
    pub subject: String,
    pub body: String,
    pub attachment: Option<MigratedAttachment>,
    pub money: u32,
    pub cod: u32,
    // The source MailCheckMask.
    pub checked: u32,
    pub mail_template_id: u32,
    pub expire_secs: u64,
    pub deliver_secs: u64,
}

#[table(accessor = game_mail_migration)]
pub struct MailMigrationRecord {
    #[primary_key]
    pub source_key: String,
    pub mail_id: u64,
    pub migrated_at: Timestamp,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedHunterPet {
    pub creature_entry: u32,
    pub name: String,
    pub rename_allowed: bool,
    pub level: u32,
    pub xp: u32,
    pub loyalty_level: u8,
    pub loyalty_progress: u32,
    pub happiness: u32,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigratedSkill {
    pub skill_line: u32,
    pub current: u16,
    pub max_rank: u16,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigratedReputation {
    pub faction_id: u32,
    // Reputation earned above the faction's base standing.
    pub earned_standing: i32,
    pub at_war: bool,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigratedAction {
    pub button: u8,
    pub action: u32,
    pub action_type: u8,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedQuestLogEntry {
    pub event_credit: bool,
    pub failed: bool,
    pub deadline_secs: u64,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedQuest {
    pub quest_entry: u32,
    pub rewarded: bool,
    pub log: Option<MigratedQuestLogEntry>,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub struct CharacterMigration {
    pub source: String,
    pub source_guid: u64,
    pub account: MigratedAccount,
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub level: u8,
    pub xp: u32,
    pub money: u32,
    // None, or a position off a continent, places the Character at its home bind.
    pub position: Option<MigratedPosition>,
    pub home: MigratedPosition,
    pub played_total_secs: u32,
    pub rested_xp: u32,
    pub logout_resting: bool,
    pub last_logout_secs: u64,
    pub bank_bag_slots: u8,
    pub watched_faction_index: i32,
    pub items: Vec<MigratedItem>,
    pub spells: Vec<u32>,
    pub skills: Vec<MigratedSkill>,
    pub reputations: Vec<MigratedReputation>,
    pub actions: Vec<MigratedAction>,
    pub quests: Vec<MigratedQuest>,
    // PLAYER_EXPLORED_ZONES words: bit n marks the area whose area_bit is n.
    pub explored_zones: Vec<u32>,
    // TaxiNodes.dbc ids.
    pub taxi_nodes: Vec<u32>,
    pub hunter_pet: Option<MigratedHunterPet>,
}

const SOURCE_LABEL_MAX_LEN: usize = 64;
const ACTION_BUTTONS: u8 = 120;
const MAX_WATCHED_FACTION_INDEX: i32 = 63;
const CONTINENT_MAP_TYPE: u32 = 0;
const HUNTER_CLASS: u8 = 3;
const MAX_PET_LOYALTY_LEVEL: u8 = 6;

#[reducer]
pub fn migrate_account(ctx: &ReducerContext, account: MigratedAccount) -> Result<(), String> {
    require_operator(ctx)?;
    require_single_shard(ctx)?;
    migrated_account(ctx, account).map(|_| ())
}

#[reducer]
pub fn migrate_character(
    ctx: &ReducerContext,
    migration: CharacterMigration,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_single_shard(ctx)?;
    let source_key = source_key(&migration.source, migration.source_guid)?;
    if let Some(record) = ctx
        .db
        .game_character_migration()
        .source_key()
        .find(&source_key)
    {
        spacetimedb::log::info!(
            "migrate_character: {source_key} is already Character {}",
            record.character_guid
        );
        return Ok(());
    }
    let account = migrated_account(ctx, migration.account.clone())?;
    let character_guid = insert_character(ctx, &account, &migration)?;
    let owner = account.identity.unwrap_or(Identity::ZERO);
    let character = Arrival {
        ctx,
        guid: character_guid,
        owner,
    };
    character.items(&migration.items)?;
    character.skills(&migration.skills)?;
    character.spells_and_talents(&migration.spells, migration.level)?;
    character.reputations(&migration.reputations)?;
    character.actions(&migration.actions)?;
    character.quests(&migration.quests)?;
    character.explored(&migration.explored_zones)?;
    character.taxi_nodes(&migration.taxi_nodes)?;
    if let Some(pet) = &migration.hunter_pet {
        character.hunter_pet(pet, migration.class, migration.level)?;
    }
    crate::native::directory::changed(ctx, account.id, character_guid, true);
    ctx.db
        .game_character_migration()
        .insert(CharacterMigrationRecord {
            source_key,
            source: migration.source,
            source_guid: migration.source_guid,
            character_guid,
            migrated_at: ctx.timestamp,
        });
    Ok(())
}

// Each letter is one transaction. Its recipient must already be a migrated Character.
#[reducer]
pub fn migrate_letter(ctx: &ReducerContext, letter: LetterMigration) -> Result<(), String> {
    require_operator(ctx)?;
    require_single_shard(ctx)?;
    let source_key = format!(
        "{}/{}",
        source_key(&letter.source, letter.source_mail_id)?,
        letter.source_item_guid
    );
    if ctx
        .db
        .game_mail_migration()
        .source_key()
        .find(&source_key)
        .is_some()
    {
        return Ok(());
    }
    let recipient_guid = migrated_character(ctx, &letter.source, letter.recipient_source_guid)
        .map_err(|refusal| format!("letter {source_key}: {refusal}"))?
        .guid;
    let (sender_guid, sender_entry) = match letter.sender_kind {
        lyracore_shared::mail::SENDER_KIND_CHARACTER => (
            migrated_guid(ctx, &letter.source, letter.sender_source_guid)?.unwrap_or(0),
            0,
        ),
        lyracore_shared::mail::SENDER_KIND_AUCTION
        | lyracore_shared::mail::SENDER_KIND_CREATURE
        | lyracore_shared::mail::SENDER_KIND_GAMEOBJECT => (0, letter.sender_entry),
        other => {
            return Err(format!(
                "letter {source_key}: sender kind {other} is not supported"
            ))
        }
    };
    if sender_kind_is_character(letter.sender_kind) && sender_guid == 0 && letter.cod > 0 {
        return Err(format!(
            "letter {source_key}: cash on delivery needs a migrated sender to receive the copper"
        ));
    }
    let item = match &letter.attachment {
        Some(attachment) => {
            checked_template(ctx, attachment.entry, attachment.stack_count)?;
            Some((attachment, letter_text(ctx, attachment.text.clone())?))
        }
        None => None,
    };
    let expire_secs = i64::try_from(letter.expire_secs)
        .map_err(|_| format!("letter {source_key}: expiry is out of range"))?;
    let lifetime_secs = lyracore_shared::mail::expires_at_secs(0, 0, letter.cod);
    let created_micros = expire_secs
        .saturating_sub(lifetime_secs)
        .saturating_mul(1_000_000);
    let deliver_micros = i64::try_from(letter.deliver_secs.saturating_mul(1_000_000))
        .map_err(|_| format!("letter {source_key}: delivery time is out of range"))?;
    let read_flag = lyracore_shared::mail::CHECK_MASK_READ;
    let mail = crate::mail::receive_letter(
        ctx,
        crate::Mail {
            id: 0,
            recipient_guid,
            sender_guid,
            subject: letter.subject,
            body: letter.body,
            item_entry: item.map_or(0, |(item, _)| item.entry),
            item_stack_count: item.map_or(0, |(item, _)| item.stack_count),
            item_durability: item.map_or(0, |(item, _)| item.durability),
            item_enchant_id: item.map_or(0, |(item, _)| item.enchant_id),
            item_soulbound: item.is_some_and(|(item, _)| item.soulbound),
            random_property_id: item.map_or(0, |(item, _)| item.random_property_id),
            item_text_id: item.map_or(0, |(_, text_id)| text_id),
            money: letter.money,
            cod: letter.cod,
            was_read: letter.checked & read_flag != 0,
            created_at: Timestamp::from_micros_since_unix_epoch(created_micros),
            sender_kind: letter.sender_kind,
            sender_entry,
            check_flags: letter.checked & lyracore_shared::mail::CLIENT_CHECK_MASK & !read_flag,
            mail_template_id: letter.mail_template_id,
            deliver_micros,
        },
    );
    ctx.db.game_mail_migration().insert(MailMigrationRecord {
        source_key,
        mail_id: mail.id,
        migrated_at: ctx.timestamp,
    });
    Ok(())
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigratedContact {
    pub target_source_guid: u64,
    pub ignore: bool,
}

#[table(accessor = game_contact_migration)]
pub struct ContactMigrationRecord {
    #[primary_key]
    pub source_key: String,
    pub migrated_at: Timestamp,
}

// One Character's friends and ignores, once; targets that are not migrated Characters are skipped.
#[reducer]
pub fn migrate_contacts(
    ctx: &ReducerContext,
    source: String,
    source_guid: u64,
    contacts: Vec<MigratedContact>,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_single_shard(ctx)?;
    let source_key = source_key(&source, source_guid)?;
    if ctx
        .db
        .game_contact_migration()
        .source_key()
        .find(&source_key)
        .is_some()
    {
        return Ok(());
    }
    let owner = migrated_character(ctx, &source, source_guid)?;
    let owner = crate::chat::ContactOwner {
        guid: owner.guid,
        identity: owner.owner_identity,
        race: owner.race,
    };
    for contact in contacts {
        let Ok(target) = migrated_character(ctx, &source, contact.target_source_guid) else {
            spacetimedb::log::info!(
                "migrate_contacts: {source_key} skips {}, which is not a migrated Character",
                contact.target_source_guid
            );
            continue;
        };
        match crate::chat::insert_contact(
            ctx,
            owner,
            target.guid,
            contact.ignore,
            Some(target.race),
        ) {
            Ok(()) | Err(lyracore_shared::social::ContactRefusal::AlreadyOnList) => {}
            Err(refusal) => {
                return Err(format!(
                    "contact {} of {source_key}: {}",
                    contact.target_source_guid,
                    refusal.as_tag()
                ))
            }
        }
    }
    ctx.db
        .game_contact_migration()
        .insert(ContactMigrationRecord {
            source_key,
            migrated_at: ctx.timestamp,
        });
    Ok(())
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedGuildRank {
    pub rank_id: u32,
    pub name: String,
    pub rights: u32,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct MigratedGuildMember {
    pub source_guid: u64,
    pub rank_id: u32,
    pub public_note: String,
    pub officer_note: String,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildMigration {
    pub source: String,
    pub source_guild_id: u64,
    pub name: String,
    pub leader_source_guid: u64,
    pub motd: String,
    pub info: String,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
    pub background_color: u32,
    // Rank 0 is the Guild Master; ids run from 0 without gaps.
    pub ranks: Vec<MigratedGuildRank>,
    pub members: Vec<MigratedGuildMember>,
}

#[table(accessor = game_guild_migration)]
pub struct GuildMigrationRecord {
    #[primary_key]
    pub source_key: String,
    pub guild_id: u32,
    pub migrated_at: Timestamp,
}

// Founds one Guild of migrated Characters through the normal founding and membership rules.
#[reducer]
pub fn migrate_guild(ctx: &ReducerContext, guild: GuildMigration) -> Result<(), String> {
    use crate::{game_guild, game_guild_rank};
    use lyracore_shared::guild::{
        fits_length, MAX_INFO, MAX_MOTD, MAX_NOTE, MAX_RANKS, MAX_RANK_NAME, MIN_RANKS,
    };
    require_operator(ctx)?;
    require_single_shard(ctx)?;
    let source_key = source_key(&guild.source, guild.source_guild_id)?;
    if ctx
        .db
        .game_guild_migration()
        .source_key()
        .find(&source_key)
        .is_some()
    {
        return Ok(());
    }
    let refuse = |reason: &str| format!("guild {}: {reason}", guild.name);
    if !(MIN_RANKS..=MAX_RANKS).contains(&guild.ranks.len())
        || guild
            .ranks
            .iter()
            .map(|rank| rank.rank_id)
            .ne(0..guild.ranks.len() as u32)
    {
        return Err(refuse("ranks must be 5 to 10 ids from 0 without gaps"));
    }
    if !fits_length(&guild.motd, MAX_MOTD)
        || !fits_length(&guild.info, MAX_INFO)
        || guild
            .ranks
            .iter()
            .any(|rank| !fits_length(&rank.name, MAX_RANK_NAME))
        || guild.members.iter().any(|member| {
            !fits_length(&member.public_note, MAX_NOTE)
                || !fits_length(&member.officer_note, MAX_NOTE)
        })
    {
        return Err(refuse("text is outside protocol limits"));
    }
    let leader = migrated_character(ctx, &guild.source, guild.leader_source_guid)?;
    let team = lyracore_shared::faction::team_for_race(leader.race);
    let guild_id = crate::guild::create_guild(
        ctx,
        leader.guid,
        &leader.name,
        team,
        leader.account_id,
        &guild.name,
    )
    .map_err(|refusal| refuse(refusal.as_tag()))?;
    let guilds = ctx.db.game_guild();
    let mut row = guilds
        .guild_id()
        .find(guild_id)
        .ok_or_else(|| refuse("the founded Guild is missing"))?;
    row.motd = guild.motd.clone();
    row.info = guild.info.clone();
    row.emblem_style = guild.emblem_style;
    row.emblem_color = guild.emblem_color;
    row.border_style = guild.border_style;
    row.border_color = guild.border_color;
    row.background_color = guild.background_color;
    guilds.guild_id().update(row);
    let ranks = ctx.db.game_guild_rank();
    for rank in ranks.by_guild().filter(&guild_id).collect::<Vec<_>>() {
        ranks.id().delete(rank.id);
    }
    for rank in &guild.ranks {
        ranks.insert(crate::GuildRank {
            id: 0,
            guild_id,
            rank_id: rank.rank_id,
            name: rank.name.clone(),
            rights: rank.rights,
        });
    }
    for member in &guild.members {
        let character = migrated_character(ctx, &guild.source, member.source_guid)?;
        if character.guid != leader.guid {
            if member.rank_id == 0 || member.rank_id as usize >= guild.ranks.len() {
                return Err(refuse(
                    "only the leader holds rank 0, and every rank must exist",
                ));
            }
            if lyracore_shared::faction::team_for_race(character.race) != team {
                return Err(refuse("every member must be on the leader's team"));
            }
            crate::guild::add_member(
                ctx,
                guild_id,
                character.guid,
                &character.name,
                member.rank_id,
                character.account_id,
            )
            .map_err(|refusal| refuse(refusal.as_tag()))?;
        }
        let mut row = crate::guild::member(ctx, character.guid)
            .ok_or_else(|| refuse("a member is missing after joining"))?;
        row.public_note = member.public_note.clone();
        row.officer_note = member.officer_note.clone();
        row.update(ctx);
    }
    ctx.db.game_guild_migration().insert(GuildMigrationRecord {
        source_key,
        guild_id,
        migrated_at: ctx.timestamp,
    });
    Ok(())
}

fn migrated_character(
    ctx: &ReducerContext,
    source: &str,
    source_guid: u64,
) -> Result<Character, String> {
    migrated_guid(ctx, source, source_guid)?
        .and_then(|guid| crate::helpers::character_by_guid(ctx, guid))
        .ok_or_else(|| format!("source Character {source_guid} is not a migrated Character"))
}

fn migrated_guid(
    ctx: &ReducerContext,
    source: &str,
    source_guid: u64,
) -> Result<Option<u64>, String> {
    Ok(ctx
        .db
        .game_character_migration()
        .source_key()
        .find(source_key(source, source_guid)?)
        .map(|record| record.character_guid))
}

fn sender_kind_is_character(kind: u8) -> bool {
    kind == lyracore_shared::mail::SENDER_KIND_CHARACTER
}

fn checked_template(
    ctx: &ReducerContext,
    entry: u32,
    stack_count: u32,
) -> Result<crate::ItemTemplate, String> {
    let template = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry)
        .ok_or_else(|| format!("Item entry {entry} is not imported"))?;
    if stack_count == 0 || stack_count > template.max_stack.max(1) {
        return Err(format!(
            "Item {entry}: stack of {stack_count} exceeds {}",
            template.max_stack
        ));
    }
    Ok(template)
}

// Each Letter Copy carries its own Item Text row.
fn letter_text(ctx: &ReducerContext, text: Option<String>) -> Result<u32, String> {
    text.map_or(Ok(0), |text| crate::mail_text::import_text(ctx, text, 1))
}

fn require_single_shard(ctx: &ReducerContext) -> Result<(), String> {
    if crate::peer::role(ctx).is_some() {
        return Err("Character Migration requires a single-Shard Realm".to_string());
    }
    Ok(())
}

fn source_key(source: &str, source_guid: u64) -> Result<String, String> {
    if source.is_empty()
        || source.len() > SOURCE_LABEL_MAX_LEN
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!(
            "Migration Source label {source:?} must be 1 to {SOURCE_LABEL_MAX_LEN} ASCII letters, \
             digits, hyphens or underscores"
        ));
    }
    Ok(format!("{source}/{source_guid}"))
}

// An existing Account is the same credential holder only when its salt and verifier match.
fn migrated_account(ctx: &ReducerContext, account: MigratedAccount) -> Result<Account, String> {
    let username = lyracore_protocol::srp::normalize(&account.username).map_err(|_| {
        format!(
            "Account name {:?} is not a valid SRP name",
            account.username
        )
    })?;
    if account.salt.len() != 32 || account.verifier.len() != 32 {
        return Err(format!(
            "Account {username}: salt and verifier must be 32 bytes each"
        ));
    }
    let accounts = ctx.db.game_account();
    match accounts.username().find(&username) {
        Some(existing)
            if existing.salt == account.salt && existing.verifier == account.verifier =>
        {
            if account.banned && !existing.banned {
                return Ok(accounts.id().update(Account {
                    banned: true,
                    ..existing
                }));
            }
            Ok(existing)
        }
        Some(_) => Err(format!(
            "Account {username} already exists with other credentials"
        )),
        None => Ok(crate::auth::insert_provisioned_account(
            ctx,
            username,
            account.salt,
            account.verifier,
            account.banned,
        )),
    }
}

fn insert_character(
    ctx: &ReducerContext,
    account: &Account,
    migration: &CharacterMigration,
) -> Result<u64, String> {
    check_new_character(ctx, &migration.name, migration.race, migration.class)
        .map_err(|refusal| format!("Character {}: {refusal}", migration.name))?;
    if !account_has_room(ctx, account.id) {
        return Err(format!(
            "Account {} has no room for Character {}",
            account.username, migration.name
        ));
    }
    if !(1..=crate::xp::LEVEL_CAP).contains(&u32::from(migration.level)) {
        return Err(format!(
            "Character {}: level {} is outside 1 to {}",
            migration.name,
            migration.level,
            crate::xp::LEVEL_CAP
        ));
    }
    let next_level_xp = crate::xp::xp_to_next_level(u32::from(migration.level));
    let xp = if next_level_xp == 0 { 0 } else { migration.xp };
    if next_level_xp != 0 && xp >= next_level_xp {
        return Err(format!(
            "Character {}: {xp} XP reaches the next level",
            migration.name
        ));
    }
    if migration.bank_bag_slots > lyracore_shared::bank::MAX_BANK_BAG_SLOTS {
        return Err(format!(
            "Character {}: {} bank bag slots exceed {}",
            migration.name,
            migration.bank_bag_slots,
            lyracore_shared::bank::MAX_BANK_BAG_SLOTS
        ));
    }
    if !(-1..=MAX_WATCHED_FACTION_INDEX).contains(&migration.watched_faction_index) {
        return Err(format!(
            "Character {}: watched faction index {} is outside -1 to {MAX_WATCHED_FACTION_INDEX}",
            migration.name, migration.watched_faction_index
        ));
    }
    let home = migration.home;
    require_map(ctx, &migration.name, home.map_id)?;
    let position = match migration.position {
        Some(position) if is_continent(ctx, position.map_id) => position,
        _ => home,
    };
    require_map(ctx, &migration.name, position.map_id)?;
    let guid = next_character_guid(ctx);
    ctx.db.game_character().insert(Character {
        guid,
        account_id: account.id,
        owner_identity: account.identity.unwrap_or(Identity::ZERO),
        name: migration.name.clone(),
        race: migration.race,
        class: migration.class,
        gender: migration.gender,
        skin: migration.skin,
        face: migration.face,
        hair_style: migration.hair_style,
        hair_color: migration.hair_color,
        facial_hair: migration.facial_hair,
        level: migration.level,
        xp,
        next_level_xp,
        map_id: position.map_id,
        zone_id: position.zone_id,
        x: position.x,
        y: position.y,
        z: position.z,
        orientation: position.orientation,
        first_login: false,
        online: false,
        money: migration.money,
        rested_xp: migration.rested_xp,
        last_logout_micros: migration.last_logout_secs.saturating_mul(1_000_000),
        home_map: home.map_id,
        home_zone: home.zone_id,
        home_x: home.x,
        home_y: home.y,
        home_z: home.z,
        played_total_secs: migration.played_total_secs,
        session_start_micros: 0,
        health: 0,
        power: 0,
        respec_count: 0,
        death_expire_micros: 0,
        pending_instance_id: 0,
        gm_level: 0,
        pending_ghost: false,
        resting: migration.logout_resting,
        rested_since_micros: 0,
        pending_godmode: false,
        pending_run_speed_mult_bp: crate::world::RUN_SPEED_BP_1X,
        bank_bag_slots: migration.bank_bag_slots,
        watched_faction_index: migration.watched_faction_index,
        rest_state: lyracore_shared::packing::REST_STATE_NORMAL,
    });
    Ok(guid)
}

fn is_continent(ctx: &ReducerContext, map_id: u32) -> bool {
    ctx.db
        .game_map()
        .id()
        .find(map_id)
        .is_some_and(|map| map.map_type == CONTINENT_MAP_TYPE)
}

fn require_map(ctx: &ReducerContext, name: &str, map_id: u32) -> Result<(), String> {
    if ctx.db.game_map().id().find(map_id).is_none() {
        return Err(format!("Character {name}: map {map_id} is not imported"));
    }
    if !crate::native::directory::owns_map(ctx, map_id) {
        return Err(format!(
            "Character {name}: this Shard does not own map {map_id}"
        ));
    }
    Ok(())
}

struct Arrival<'a> {
    ctx: &'a ReducerContext,
    guid: u64,
    owner: Identity,
}

impl Arrival<'_> {
    fn items(&self, items: &[MigratedItem]) -> Result<(), String> {
        let mut occupied = HashSet::new();
        let mut displaced = Vec::new();
        let (outside_bags, inside_bags): (Vec<_>, Vec<_>) = items
            .iter()
            .partition(|item| matches!(item.location, MigratedItemLocation::Slot(_)));
        for item in outside_bags.into_iter().chain(inside_bags) {
            match addressed_slot(item.location)? {
                Some(slot) => {
                    if !occupied.insert(slot) {
                        return Err(format!("two Items arrive in slot {slot}"));
                    }
                    let row = self.item(item, slot)?;
                    if matches!(item.location, MigratedItemLocation::Bag(_)) {
                        crate::items::validate_bag_dest_slot(self.ctx, self.guid, slot).map_err(
                            |refusal| {
                                format!(
                                    "Item {} at {:?}: {}",
                                    item.entry,
                                    item.location,
                                    refusal.as_tag()
                                )
                            },
                        )?;
                    }
                    ItemInstance::insert_split(self.ctx, row);
                }
                None => displaced.push(item),
            }
        }
        for item in displaced {
            let slot =
                crate::items::first_free_storage_slot(self.ctx, self.guid).ok_or_else(|| {
                    format!(
                        "Item {} from a bank bag has no free bank or bag slot",
                        item.entry
                    )
                })?;
            let row = self.item(item, slot)?;
            ItemInstance::insert_split(self.ctx, row);
        }
        Ok(())
    }

    fn item(&self, item: &MigratedItem, slot: u8) -> Result<ItemInstance, String> {
        let template = checked_template(self.ctx, item.entry, item.stack_count)?;
        Ok(ItemInstance {
            guid: crate::items::next_item_guid(self.ctx)?,
            entry: item.entry,
            owner_identity: self.owner,
            owner_guid: self.guid,
            slot,
            stack_count: item.stack_count,
            durability: item.durability.min(template.max_durability),
            created_at: self.ctx.timestamp,
            enchant_id: item.enchant_id,
            soulbound: item.soulbound,
            random_property_id: item.random_property_id,
            item_text_id: letter_text(self.ctx, item.text.clone())?,
        })
    }

    // Talent ranks arrive as rank spells; world entry relearns them from the talent rows.
    fn spells_and_talents(&self, spells: &[u32], level: u8) -> Result<(), String> {
        let mut talent_ranks: HashMap<u32, (u32, u8)> = HashMap::new();
        for talent in self.ctx.db.game_talent().iter() {
            for rank in 1..=talent.max_rank {
                let spell = crate::talent::talent_rank_spell(&talent, rank);
                if spell != 0 {
                    talent_ranks.insert(spell, (talent.talent_id, rank));
                }
            }
        }
        let mut learned_talents: HashMap<u32, u8> = HashMap::new();
        for &spell_id in spells {
            if let Some(&(talent_id, rank)) = talent_ranks.get(&spell_id) {
                let learned = learned_talents.entry(talent_id).or_default();
                *learned = (*learned).max(rank);
                continue;
            }
            if self.ctx.db.game_spell().spell_id().find(spell_id).is_none() {
                return Err(format!("spell {spell_id} is not imported"));
            }
            crate::spell::learn_spell(self.ctx, self.guid, self.owner, spell_id);
        }
        let spent: u32 = learned_talents.values().map(|&rank| u32::from(rank)).sum();
        let earned = crate::talent::talent_points_available(u32::from(level), 0);
        if spent > earned {
            return Err(format!(
                "{spent} talent points exceed the {earned} a level {level} Character has"
            ));
        }
        for (talent_id, rank) in learned_talents {
            self.ctx.db.game_character_talent().insert(CharacterTalent {
                id: 0,
                character_guid: self.guid,
                owner_identity: self.owner,
                talent_id,
                rank,
            });
        }
        Ok(())
    }

    fn skills(&self, skills: &[MigratedSkill]) -> Result<(), String> {
        let mut seen = HashSet::new();
        for skill in skills {
            if !seen.insert(skill.skill_line) {
                return Err(format!("skill {} arrives twice", skill.skill_line));
            }
            if self
                .ctx
                .db
                .game_skill_line()
                .skill_line()
                .find(skill.skill_line)
                .is_none()
            {
                return Err(format!("skill {} is not imported", skill.skill_line));
            }
            PlayerSkill::insert(
                self.ctx,
                PlayerSkill {
                    id: 0,
                    character_guid: self.guid,
                    owner_identity: self.owner,
                    skill_line: skill.skill_line,
                    current: skill.current.min(skill.max_rank),
                    max_rank: skill.max_rank,
                },
            );
        }
        Ok(())
    }

    fn reputations(&self, reputations: &[MigratedReputation]) -> Result<(), String> {
        let mut seen = HashSet::new();
        for reputation in reputations {
            if !seen.insert(reputation.faction_id) {
                return Err(format!("faction {} arrives twice", reputation.faction_id));
            }
            let faction = self
                .ctx
                .db
                .game_faction()
                .faction_id()
                .find(reputation.faction_id)
                .filter(|faction| faction.reputation_index >= 0)
                .ok_or_else(|| format!("faction {} has no reputation", reputation.faction_id))?;
            PlayerReputation::insert(
                self.ctx,
                PlayerReputation {
                    id: 0,
                    character_guid: self.guid,
                    owner_identity: self.owner,
                    faction_id: reputation.faction_id,
                    standing: crate::reputation::clamped_standing(
                        faction
                            .base_standing
                            .saturating_add(reputation.earned_standing),
                    ),
                    reputation_index: faction.reputation_index,
                    at_war: reputation.at_war,
                },
            );
        }
        Ok(())
    }

    fn actions(&self, actions: &[MigratedAction]) -> Result<(), String> {
        let mut seen = HashSet::new();
        for action in actions {
            if action.button >= ACTION_BUTTONS || !seen.insert(action.button) {
                return Err(format!(
                    "action button {} is invalid or repeated",
                    action.button
                ));
            }
            self.ctx.db.game_player_action().insert(PlayerAction {
                id: 0,
                character_guid: self.guid,
                owner_identity: self.owner,
                button: action.button,
                action: action.action,
                action_type: action.action_type,
            });
        }
        Ok(())
    }

    fn quests(&self, quests: &[MigratedQuest]) -> Result<(), String> {
        let mut seen = HashSet::new();
        let mut logged = 0;
        for quest in quests {
            if !seen.insert(quest.quest_entry) {
                return Err(format!("quest {} arrives twice", quest.quest_entry));
            }
            let template = self
                .ctx
                .db
                .game_quest_template()
                .entry()
                .find(quest.quest_entry)
                .ok_or_else(|| format!("quest {} is not imported", quest.quest_entry))?;
            // A rewarded quest returns to the log only when it can be taken again.
            let log = quest
                .log
                .as_ref()
                .filter(|_| !quest.rewarded || template.repeatable);
            let (counts, rewarded, failed, deadline_micros) = match log {
                Some(entry) => {
                    logged += 1;
                    if logged > crate::quest::MAX_QUEST_LOG_SIZE {
                        return Err(format!(
                            "more than {} quests arrive in the quest log",
                            crate::quest::MAX_QUEST_LOG_SIZE
                        ));
                    }
                    if entry.event_credit {
                        CharacterQuestEventCredit::insert(
                            self.ctx,
                            CharacterQuestEventCredit {
                                id: 0,
                                character_guid: self.guid,
                                quest_entry: quest.quest_entry,
                            },
                        );
                    }
                    // MaNGOS saves a timer for every quest; only a timed quest has a deadline.
                    let deadline_secs = if template.limit_time > 0 {
                        entry.deadline_secs
                    } else {
                        0
                    };
                    let deadline_micros = i64::try_from(deadline_secs.saturating_mul(1_000_000))
                        .map_err(|_| {
                            format!("quest {} deadline is out of range", quest.quest_entry)
                        })?;
                    (
                        self.objective_counts(quest.quest_entry),
                        false,
                        entry.failed,
                        deadline_micros,
                    )
                }
                None if quest.rewarded => (Vec::new(), true, false, 0),
                None => continue,
            };
            crate::quest::CharacterQuest::insert(
                self.ctx,
                crate::quest::CharacterQuest {
                    id: 0,
                    character_guid: self.guid,
                    owner_identity: self.owner,
                    quest_entry: quest.quest_entry,
                    counts,
                    rewarded,
                    deadline_micros,
                    failed,
                },
            );
        }
        Ok(())
    }

    // Kill and GameObject counters restart: the catalogue does not keep source objective slots.
    fn objective_counts(&self, quest_entry: u32) -> Vec<u32> {
        let objectives = self
            .ctx
            .db
            .game_quest_objective()
            .by_quest()
            .filter(&quest_entry)
            .count();
        vec![0; objectives.min(crate::quest::MAX_OBJECTIVES)]
    }

    fn explored(&self, explored_zones: &[u32]) -> Result<(), String> {
        let areas: HashMap<i32, u32> = self
            .ctx
            .db
            .game_area()
            .iter()
            .filter(|area| area.area_bit >= 0)
            .map(|area| (area.area_bit, area.id))
            .collect();
        for (word_index, word) in explored_zones.iter().enumerate() {
            for bit in 0..32 {
                if word & (1 << bit) == 0 {
                    continue;
                }
                let area_bit = i32::try_from(word_index * 32 + bit)
                    .map_err(|_| "explored zones are out of range".to_string())?;
                let area_id = *areas
                    .get(&area_bit)
                    .ok_or_else(|| format!("explored area bit {area_bit} is not imported"))?;
                self.ctx
                    .db
                    .game_character_explored()
                    .insert(CharacterExplored {
                        id: 0,
                        character_guid: self.guid,
                        area_bit,
                        area_id,
                        experience: 0,
                    });
            }
        }
        Ok(())
    }

    // A Hunter pet's identity is keyed by its owner's guid; calling the pet materializes it.
    fn hunter_pet(&self, pet: &MigratedHunterPet, class: u8, level: u8) -> Result<(), String> {
        if class != HUNTER_CLASS {
            return Err("only a Hunter arrives with a Hunter pet".to_string());
        }
        let template = self
            .ctx
            .db
            .game_creature_template()
            .entry()
            .find(pet.creature_entry)
            .filter(|template| template.creature_family != 0)
            .ok_or_else(|| {
                format!(
                    "pet creature {} is not an imported beast",
                    pet.creature_entry
                )
            })?;
        if !(1..=u32::from(level)).contains(&pet.level)
            || !(1..=MAX_PET_LOYALTY_LEVEL).contains(&pet.loyalty_level)
        {
            return Err(format!(
                "pet level {} or loyalty level {} is out of range",
                pet.level, pet.loyalty_level
            ));
        }
        self.ctx.db.game_hunter_pet().insert(HunterPet {
            pet_id: self.guid,
            owner_guid: self.guid,
            creature_entry: pet.creature_entry,
            creature_family: u32::from(template.creature_family),
            name: pet.name.clone(),
            rename_allowed: pet.rename_allowed,
            level: pet.level,
            pet_xp: pet.xp,
            loyalty_level: pet.loyalty_level,
            loyalty_progress: pet.loyalty_progress,
            happiness: pet.happiness.min(crate::creatures::MAX_HAPPINESS),
            created_at: self.ctx.timestamp,
            care_updated_at: self.ctx.timestamp,
            loyalty_updated_at: self.ctx.timestamp,
            name_timestamp: 0,
        });
        Ok(())
    }

    fn taxi_nodes(&self, client_node_ids: &[u32]) -> Result<(), String> {
        let mut seen = HashSet::new();
        for &client_node_id in client_node_ids {
            let node = self
                .ctx
                .db
                .game_taxi_node()
                .client_node_id()
                .find(client_node_id)
                .ok_or_else(|| format!("taxi node {client_node_id} is not imported"))?;
            if seen.insert(node.id) {
                self.ctx
                    .db
                    .game_character_taxi_node()
                    .insert(CharacterTaxiNode {
                        id: 0,
                        character_guid: self.guid,
                        node_id: node.id,
                    });
            }
        }
        Ok(())
    }
}

const BANK_BAG_SLOTS: std::ops::RangeInclusive<u8> = 63..=68;
const KEYRING_SLOTS: std::ops::RangeInclusive<u8> = 81..=112;
const EQUIPPED_BAG_SLOTS: std::ops::RangeInclusive<u8> = 19..=22;
const LAST_BANK_SLOT: u8 = 62;

// Returns None for an Item that this model cannot hold where the source kept it.
fn addressed_slot(location: MigratedItemLocation) -> Result<Option<u8>, String> {
    match location {
        MigratedItemLocation::Slot(slot)
            if slot <= LAST_BANK_SLOT || KEYRING_SLOTS.contains(&slot) =>
        {
            Ok(Some(slot))
        }
        MigratedItemLocation::Slot(slot) if BANK_BAG_SLOTS.contains(&slot) => Ok(None),
        MigratedItemLocation::Bag(bag) if BANK_BAG_SLOTS.contains(&bag.bag_slot) => Ok(None),
        MigratedItemLocation::Bag(bag) if EQUIPPED_BAG_SLOTS.contains(&bag.bag_slot) => {
            Ok(crate::items::bag_content_slot(bag.bag_slot, bag.slot))
        }
        MigratedItemLocation::Bag(bag) => Err(format!("bag position {bag:?} has no address")),
        MigratedItemLocation::Slot(slot) => Err(format!("slot {slot} cannot hold an Item")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_bank_and_keyring_slots_keep_their_vanilla_address() {
        for slot in [0, 18, 19, 22, 23, 38, 39, 62, 81, 112] {
            assert_eq!(
                addressed_slot(MigratedItemLocation::Slot(slot)),
                Ok(Some(slot))
            );
        }
    }

    #[test]
    fn bank_bags_and_their_contents_are_displaced() {
        assert_eq!(addressed_slot(MigratedItemLocation::Slot(63)), Ok(None));
        assert_eq!(
            addressed_slot(MigratedItemLocation::Bag(MigratedBagSlot {
                bag_slot: 68,
                slot: 3
            })),
            Ok(None)
        );
    }

    #[test]
    fn a_slot_in_the_second_bag_uses_the_flat_bag_content_address() {
        assert_eq!(
            addressed_slot(MigratedItemLocation::Bag(MigratedBagSlot {
                bag_slot: 20,
                slot: 5
            })),
            Ok(Some(120 + 18 + 5))
        );
    }

    #[test]
    fn a_bag_slot_past_the_flat_model_is_displaced() {
        assert_eq!(
            addressed_slot(MigratedItemLocation::Bag(MigratedBagSlot {
                bag_slot: 22,
                slot: 20
            })),
            Ok(None)
        );
    }

    #[test]
    fn buyback_and_unknown_slots_are_refused() {
        for slot in [69, 80, 113, 255] {
            assert!(addressed_slot(MigratedItemLocation::Slot(slot)).is_err());
        }
        assert!(addressed_slot(MigratedItemLocation::Bag(MigratedBagSlot {
            bag_slot: 23,
            slot: 0
        }))
        .is_err());
    }

    #[test]
    fn source_labels_name_one_server() {
        assert_eq!(
            source_key("vmangos-main", 42),
            Ok("vmangos-main/42".to_string())
        );
        assert!(source_key("", 1).is_err());
        assert!(source_key("a/b", 1).is_err());
    }
}
