use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::read::{Ban, LegacyAccount, LegacyCharacter, LegacyItem, LegacyPosition};

pub(crate) const PLAN_FORMAT_VERSION: u32 = 1;
// Linux limits one argv string to 128 KiB; `spacetime call` passes each argument as one.
const MAX_REDUCER_ARGUMENT_BYTES: usize = 120_000;
const SOULBOUND_FLAG: u32 = 0x1;
const AT_WAR_FLAG: u32 = 0x2;
const ACTION_BUTTONS: u8 = 120;
const MAX_BANK_BAG_SLOTS: u8 = 6;
const QUEST_STATUS_COMPLETE: u32 = 1;
const QUEST_STATUS_INCOMPLETE: u32 = 3;
const QUEST_STATUS_FAILED: u32 = 5;
const ENCHANTMENT_WORDS_PER_SLOT: usize = 3;
const TEMPORARY_ENCHANTMENT_SLOT: usize = 1;

// SATS JSON for Option: {"some": value} or {"none": []}.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum SatsOption<T> {
    #[serde(rename = "some")]
    Some(T),
    #[serde(rename = "none")]
    None([(); 0]),
}

impl<T> From<Option<T>> for SatsOption<T> {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::None([]), Self::Some)
    }
}

// Field names and order match module/src/character_migration.rs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct MigratedAccount {
    pub(crate) username: String,
    pub(crate) salt: String,
    pub(crate) verifier: String,
    pub(crate) banned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct MigratedPosition {
    pub(crate) map_id: u32,
    pub(crate) zone_id: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) z: f32,
    pub(crate) orientation: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MigratedBagSlot {
    pub(crate) bag_slot: u8,
    pub(crate) slot: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MigratedItemLocation {
    Slot(u8),
    Bag(MigratedBagSlot),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MigratedItem {
    pub(crate) entry: u32,
    pub(crate) location: MigratedItemLocation,
    pub(crate) stack_count: u32,
    pub(crate) durability: u32,
    pub(crate) enchant_id: u32,
    pub(crate) random_property_id: u32,
    pub(crate) soulbound: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MigratedSkill {
    pub(crate) skill_line: u32,
    pub(crate) current: u16,
    pub(crate) max_rank: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MigratedReputation {
    pub(crate) faction_id: u32,
    pub(crate) earned_standing: i32,
    pub(crate) at_war: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MigratedAction {
    pub(crate) button: u8,
    pub(crate) action: u32,
    pub(crate) action_type: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MigratedQuestLogEntry {
    pub(crate) event_credit: bool,
    pub(crate) failed: bool,
    pub(crate) deadline_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct MigratedQuest {
    pub(crate) quest_entry: u32,
    pub(crate) rewarded: bool,
    pub(crate) log: SatsOption<MigratedQuestLogEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CharacterMigration {
    pub(crate) source: String,
    pub(crate) source_guid: u64,
    pub(crate) account: MigratedAccount,
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
    pub(crate) position: SatsOption<MigratedPosition>,
    pub(crate) home: MigratedPosition,
    pub(crate) played_total_secs: u32,
    pub(crate) rested_xp: u32,
    pub(crate) logout_resting: bool,
    pub(crate) last_logout_secs: u64,
    pub(crate) bank_bag_slots: u8,
    pub(crate) watched_faction_index: i32,
    pub(crate) items: Vec<MigratedItem>,
    pub(crate) spells: Vec<u32>,
    pub(crate) skills: Vec<MigratedSkill>,
    pub(crate) reputations: Vec<MigratedReputation>,
    pub(crate) actions: Vec<MigratedAction>,
    pub(crate) quests: Vec<MigratedQuest>,
    pub(crate) explored_zones: Vec<u32>,
    pub(crate) taxi_nodes: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Refused,
    Skipped,
    Dropped,
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    pub(crate) subject: String,
    pub(crate) outcome: Outcome,
    pub(crate) detail: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub(crate) format_version: u32,
    pub(crate) source: String,
    pub(crate) accounts: Vec<MigratedAccount>,
    pub(crate) characters: Vec<CharacterMigration>,
    pub(crate) findings: Vec<Finding>,
}

// What the target Shard already holds; plans refer only to content it has.
#[derive(Debug, Default)]
pub(crate) struct Catalogue {
    pub(crate) items: HashSet<u32>,
    pub(crate) spells: HashSet<u32>,
    pub(crate) skills: HashSet<u32>,
    pub(crate) reputation_factions: HashSet<u32>,
    pub(crate) quests: HashSet<u32>,
    pub(crate) area_bits: HashSet<u32>,
    pub(crate) taxi_nodes: HashSet<u32>,
    pub(crate) maps: HashSet<u32>,
    pub(crate) continents: HashSet<u32>,
    pub(crate) character_names: HashSet<String>,
    // Upper-case username to little-endian salt and verifier hex.
    pub(crate) accounts: HashMap<String, (String, String)>,
    pub(crate) migrated: HashSet<String>,
}

// realmd stores big-endian hex without leading zeros; the Module wants 32 little-endian bytes.
pub(crate) fn little_endian_hex(big_endian_hex: &str) -> Option<String> {
    let digits = big_endian_hex.trim();
    if digits.is_empty() || digits.len() > 64 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let padded = format!("{digits:0>64}").to_ascii_lowercase();
    let bytes: Vec<&str> = (0..32).map(|i| &padded[i * 2..i * 2 + 2]).collect();
    let little_endian: String = bytes.into_iter().rev().collect();
    (little_endian.bytes().any(|b| b != b'0')).then_some(little_endian)
}

struct Findings<'a>(&'a mut Vec<Finding>);

impl Findings<'_> {
    fn add(&mut self, subject: &str, outcome: Outcome, detail: impl Into<String>) {
        self.0.push(Finding {
            subject: subject.to_string(),
            outcome,
            detail: detail.into(),
        });
    }

    fn dropped_ids(&mut self, subject: &str, what: &str, ids: &[u32]) {
        self.dropped_ids_because(subject, what, "not imported", ids);
    }

    fn dropped_ids_because(&mut self, subject: &str, what: &str, why: &str, ids: &[u32]) {
        if !ids.is_empty() {
            let list: Vec<String> = ids.iter().map(u32::to_string).collect();
            self.add(
                subject,
                Outcome::Dropped,
                format!("{} {what} {why}: {}", ids.len(), list.join(", ")),
            );
        }
    }
}

pub(crate) fn build(
    source: &str,
    catalogue: &Catalogue,
    accounts: &[LegacyAccount],
    characters: &[LegacyCharacter],
) -> Plan {
    let mut findings = Vec::new();
    let mut report = Findings(&mut findings);
    let mut migrated_accounts: HashMap<u64, MigratedAccount> = HashMap::new();
    for account in accounts {
        let subject = format!("Account {}", account.username);
        if let Some(migrated) = migrated_account(catalogue, account, &subject, &mut report) {
            migrated_accounts.insert(account.id, migrated);
        }
    }
    let mut planned = Vec::new();
    let mut names = HashSet::new();
    for character in characters {
        let subject = format!("Character {} ({})", character.name, character.guid);
        if catalogue
            .migrated
            .contains(&format!("{source}/{}", character.guid))
        {
            report.add(&subject, Outcome::Skipped, "already migrated");
            continue;
        }
        let Some(account) = migrated_accounts.get(&character.account) else {
            report.add(&subject, Outcome::Refused, "its Account is refused");
            continue;
        };
        if !names.insert(character.name.to_lowercase()) {
            report.add(
                &subject,
                Outcome::Refused,
                "another source Character has this name",
            );
            continue;
        }
        let mut character_findings = Vec::new();
        let outcome = migrated_character(
            source,
            catalogue,
            account,
            character,
            &subject,
            &mut Findings(&mut character_findings),
        );
        match outcome {
            Ok(migration) => {
                planned.push(migration);
                report.0.append(&mut character_findings);
            }
            Err(reason) => report.add(&subject, Outcome::Refused, reason.to_string()),
        }
    }
    let mut accounts: Vec<_> = migrated_accounts.into_values().collect();
    accounts.sort_by(|a, b| a.username.cmp(&b.username));
    Plan {
        format_version: PLAN_FORMAT_VERSION,
        source: source.to_string(),
        accounts,
        characters: planned,
        findings,
    }
}

fn migrated_account(
    catalogue: &Catalogue,
    account: &LegacyAccount,
    subject: &str,
    report: &mut Findings,
) -> Option<MigratedAccount> {
    let username = account.username.to_ascii_uppercase();
    let (Some(salt), Some(verifier)) = (
        little_endian_hex(&account.salt_hex),
        little_endian_hex(&account.verifier_hex),
    ) else {
        report.add(
            subject,
            Outcome::Refused,
            "has no SRP salt and verifier; its player must set a password on the source first",
        );
        return None;
    };
    if let Some((existing_salt, existing_verifier)) = catalogue.accounts.get(&username) {
        if *existing_salt != salt || *existing_verifier != verifier {
            report.add(
                subject,
                Outcome::Refused,
                "the Shard already has this Account name with other credentials",
            );
            return None;
        }
    }
    if account.gm_level > 0 {
        report.add(
            subject,
            Outcome::Changed,
            format!("GM level {} does not migrate", account.gm_level),
        );
    }
    if let Some(Ban::Until(expires)) = account.ban {
        report.add(
            subject,
            Outcome::Changed,
            format!("the ban that ends at Unix time {expires} becomes permanent"),
        );
    }
    Some(MigratedAccount {
        username,
        salt,
        verifier,
        banned: account.ban.is_some(),
    })
}

fn migrated_character(
    source: &str,
    catalogue: &Catalogue,
    account: &MigratedAccount,
    character: &LegacyCharacter,
    subject: &str,
    report: &mut Findings,
) -> Result<CharacterMigration> {
    if character.online {
        bail!("is online at the source; stop the source server first");
    }
    if catalogue
        .character_names
        .contains(&character.name.to_lowercase())
    {
        bail!("the Shard already has a Character with this name");
    }
    let home = character
        .home
        .as_ref()
        .ok_or_else(|| anyhow!("has no home bind"))?;
    if !catalogue.maps.contains(&home.map) {
        bail!("its home bind map {} is not imported", home.map);
    }
    let away = if character.on_transport {
        Some("was on a Transport")
    } else if !catalogue.maps.contains(&character.position.map) {
        Some("was on a map that is not imported")
    } else if !catalogue.continents.contains(&character.position.map) {
        Some("was in an Instance")
    } else {
        None
    };
    let position = match away {
        Some(reason) => {
            report.add(
                subject,
                Outcome::Changed,
                format!("{reason}; arrives at its home bind"),
            );
            None
        }
        None => Some(position_of(&character.position)),
    };
    let migration = CharacterMigration {
        source: source.to_string(),
        source_guid: character.guid,
        account: account.clone(),
        name: character.name.clone(),
        race: character.race,
        class: character.class,
        gender: character.gender,
        skin: character.skin,
        face: character.face,
        hair_style: character.hair_style,
        hair_color: character.hair_color,
        facial_hair: character.facial_hair,
        level: character.level,
        xp: character.xp,
        money: character.money,
        position: position.into(),
        home: MigratedPosition {
            orientation: 0.0,
            ..position_of(home)
        },
        played_total_secs: character.played_total_secs,
        rested_xp: character.rest_bonus.max(0.0) as u32,
        logout_resting: character.logout_resting,
        last_logout_secs: character.logout_secs,
        bank_bag_slots: character.bank_bag_slots.min(MAX_BANK_BAG_SLOTS),
        watched_faction_index: i32::try_from(character.watched_faction)
            .ok()
            .filter(|index| (0..64).contains(index))
            .unwrap_or(-1),
        items: items(catalogue, &character.items, subject, report)?,
        spells: kept(
            &character.spells,
            &catalogue.spells,
            |spell| *spell,
            subject,
            "spells",
            report,
        ),
        skills: kept(
            &character.skills,
            &catalogue.skills,
            |skill| skill.skill,
            subject,
            "skills",
            report,
        )
        .into_iter()
        .map(|skill| MigratedSkill {
            skill_line: skill.skill,
            current: skill.value,
            max_rank: skill.max,
        })
        .collect(),
        reputations: kept(
            &character.reputations,
            &catalogue.reputation_factions,
            |reputation| reputation.faction,
            subject,
            "factions",
            report,
        )
        .into_iter()
        .map(|reputation| MigratedReputation {
            faction_id: reputation.faction,
            earned_standing: reputation.standing,
            at_war: reputation.flags & AT_WAR_FLAG != 0,
        })
        .collect(),
        actions: {
            let buttons: Vec<u32> = character
                .actions
                .iter()
                .filter(|action| action.button >= ACTION_BUTTONS)
                .map(|action| u32::from(action.button))
                .collect();
            report.dropped_ids_because(
                subject,
                "action buttons",
                "outside the action bar",
                &buttons,
            );
            character
                .actions
                .iter()
                .filter(|action| action.button < ACTION_BUTTONS)
                .map(|action| MigratedAction {
                    button: action.button,
                    action: action.action,
                    action_type: action.action_type,
                })
                .collect()
        },
        quests: kept(
            &character.quests,
            &catalogue.quests,
            |quest| quest.quest,
            subject,
            "quests",
            report,
        )
        .into_iter()
        .filter_map(|quest| {
            // MaNGOS keeps a rewarded quest at COMPLETE; only a repeat in progress is in the log.
            let in_log = match quest.status {
                QUEST_STATUS_INCOMPLETE | QUEST_STATUS_FAILED => true,
                QUEST_STATUS_COMPLETE => !quest.rewarded,
                _ => false,
            };
            if in_log && quest.mob_counts.iter().any(|count| *count != 0) {
                report.add(
                    subject,
                    Outcome::Changed,
                    format!("kill and object counters of quest {} restart", quest.quest),
                );
            }
            (in_log || quest.rewarded).then(|| MigratedQuest {
                quest_entry: quest.quest,
                rewarded: quest.rewarded,
                log: in_log
                    .then_some(MigratedQuestLogEntry {
                        event_credit: quest.explored,
                        failed: quest.status == QUEST_STATUS_FAILED,
                        deadline_secs: quest.timer_secs,
                    })
                    .into(),
            })
        })
        .collect(),
        explored_zones: masked_words(
            &character.explored_zones,
            &catalogue.area_bits,
            0,
            subject,
            "explored areas",
            report,
        ),
        taxi_nodes: {
            let known = masked_words(
                &character.taxi_mask,
                &catalogue.taxi_nodes,
                1,
                subject,
                "taxi nodes",
                report,
            );
            set_bits(&known, 1)
        },
    };
    fits_one_call(&migration)?;
    Ok(migration)
}

fn fits_one_call(argument: &impl Serialize) -> Result<()> {
    let size = serde_json::to_string(argument)?.len();
    if size > MAX_REDUCER_ARGUMENT_BYTES {
        bail!("its plan is {size} bytes, more than one reducer call carries");
    }
    Ok(())
}

fn position_of(position: &LegacyPosition) -> MigratedPosition {
    MigratedPosition {
        map_id: position.map,
        zone_id: position.zone,
        x: position.x,
        y: position.y,
        z: position.z,
        orientation: position.orientation,
    }
}

fn kept<T: Copy>(
    rows: &[T],
    catalogue: &HashSet<u32>,
    id: impl Fn(&T) -> u32,
    subject: &str,
    what: &str,
    report: &mut Findings,
) -> Vec<T> {
    let (kept, dropped): (Vec<T>, Vec<T>) =
        rows.iter().partition(|row| catalogue.contains(&id(row)));
    let mut dropped: Vec<u32> = dropped.iter().map(&id).collect();
    dropped.sort_unstable();
    dropped.dedup();
    report.dropped_ids(subject, what, &dropped);
    kept
}

// Bit n of the words is id n + first_id.
fn set_bits(words: &[u32], first_id: u32) -> Vec<u32> {
    let mut ids = Vec::new();
    for (index, word) in words.iter().enumerate() {
        for bit in 0..32 {
            if word & (1 << bit) != 0 {
                ids.push(index as u32 * 32 + bit + first_id);
            }
        }
    }
    ids
}

fn masked_words(
    words: &[u32],
    catalogue: &HashSet<u32>,
    first_id: u32,
    subject: &str,
    what: &str,
    report: &mut Findings,
) -> Vec<u32> {
    let mut kept = words.to_vec();
    let mut dropped = Vec::new();
    for id in set_bits(words, first_id) {
        if !catalogue.contains(&id) {
            let bit = id - first_id;
            kept[(bit / 32) as usize] &= !(1 << (bit % 32));
            dropped.push(id);
        }
    }
    report.dropped_ids(subject, what, &dropped);
    kept
}

fn items(
    catalogue: &Catalogue,
    items: &[LegacyItem],
    subject: &str,
    report: &mut Findings,
) -> Result<Vec<MigratedItem>> {
    let bags: HashMap<u64, u8> = items
        .iter()
        .filter(|item| item.bag_guid == 0)
        .map(|item| (item.item_guid, item.slot))
        .collect();
    let mut migrated = Vec::new();
    for item in items {
        if !catalogue.items.contains(&item.entry) {
            bail!("Item entry {} is not imported", item.entry);
        }
        let location = if item.bag_guid == 0 {
            MigratedItemLocation::Slot(item.slot)
        } else {
            let bag_slot = *bags.get(&item.bag_guid).ok_or_else(|| {
                anyhow!(
                    "Item {} is in bag {}, which it does not carry",
                    item.item_guid,
                    item.bag_guid
                )
            })?;
            MigratedItemLocation::Bag(MigratedBagSlot {
                bag_slot,
                slot: item.slot,
            })
        };
        let random_property_id = u32::try_from(item.random_property_id).map_err(|_| {
            anyhow!(
                "Item {} has random suffix {}, which 1.12 does not have",
                item.item_guid,
                item.random_property_id
            )
        })?;
        let enchantment = |slot: usize| {
            item.enchantments
                .get(slot * ENCHANTMENT_WORDS_PER_SLOT)
                .copied()
                .unwrap_or(0)
        };
        if enchantment(TEMPORARY_ENCHANTMENT_SLOT) != 0 {
            report.add(
                subject,
                Outcome::Dropped,
                format!("temporary enchantment on Item {}", item.item_guid),
            );
        }
        if item.text_id != 0 {
            report.add(
                subject,
                Outcome::Dropped,
                format!("text {} of Item {}", item.text_id, item.item_guid),
            );
        }
        migrated.push(MigratedItem {
            entry: item.entry,
            location,
            stack_count: item.count.max(1),
            durability: item.durability,
            enchant_id: enchantment(0),
            random_property_id,
            soulbound: item.flags & SOULBOUND_FLAG != 0,
        });
    }
    Ok(migrated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::{LegacyQuest, LegacyReputation};

    fn account() -> LegacyAccount {
        LegacyAccount {
            id: 1,
            username: "migrant".to_string(),
            salt_hex: "AB".to_string(),
            verifier_hex: "0102".to_string(),
            gm_level: 0,
            ban: None,
        }
    }

    fn here(map: u32) -> LegacyPosition {
        LegacyPosition {
            map,
            zone: 12,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            orientation: 0.5,
        }
    }

    fn character() -> LegacyCharacter {
        LegacyCharacter {
            guid: 10,
            account: 1,
            name: "Migrant".to_string(),
            race: 1,
            class: 1,
            gender: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            level: 20,
            xp: 100,
            money: 5,
            position: here(0),
            on_transport: false,
            online: false,
            played_total_secs: 60,
            logout_secs: 1_700_000_000,
            logout_resting: false,
            rest_bonus: 12.7,
            bank_bag_slots: 1,
            watched_faction: -1,
            taxi_mask: vec![0b110],
            explored_zones: vec![0b11],
            home: Some(here(0)),
            items: Vec::new(),
            spells: vec![100, 200],
            skills: Vec::new(),
            reputations: Vec::new(),
            actions: Vec::new(),
            quests: Vec::new(),
        }
    }

    fn catalogue() -> Catalogue {
        Catalogue {
            items: [2589, 4500].into(),
            spells: [100].into(),
            reputation_factions: [72].into(),
            quests: [7, 8].into(),
            area_bits: [0].into(),
            taxi_nodes: [2].into(),
            maps: [0, 36].into(),
            continents: [0].into(),
            ..Catalogue::default()
        }
    }

    fn plan_one(character: LegacyCharacter) -> Plan {
        build("vmangos-main", &catalogue(), &[account()], &[character])
    }

    #[test]
    fn realmd_hex_becomes_32_little_endian_bytes() {
        assert_eq!(
            little_endian_hex("0A0B").as_deref(),
            Some(format!("0b0a{}", "00".repeat(30)).as_str())
        );
        assert_eq!(little_endian_hex(""), None);
        assert_eq!(little_endian_hex("0"), None);
        assert_eq!(little_endian_hex("xyz"), None);
        assert_eq!(little_endian_hex(&"1".repeat(65)), None);
    }

    #[test]
    fn a_migrated_verifier_accepts_the_source_password() {
        use lyracore_protocol::srp;
        use wow_srp::{
            client::SrpClientChallenge, normalized_string::NormalizedString, server::SrpVerifier,
            PublicKey,
        };

        let username = NormalizedString::new("MIGRANT").unwrap();
        let password = NormalizedString::new("SECRET").unwrap();
        let source = SrpVerifier::from_username_and_password(username.clone(), password.clone());
        // MaNGOS BigNumber::AsHexStr writes the number big-endian without leading zeros.
        let realmd_hex = |bytes: &[u8; 32]| {
            let hex: String = bytes
                .iter()
                .rev()
                .map(|byte| format!("{byte:02X}"))
                .collect();
            hex.trim_start_matches('0').to_string()
        };
        let bytes = |hex: String| -> [u8; 32] {
            std::array::from_fn(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
        };
        let salt = bytes(little_endian_hex(&realmd_hex(source.salt())).unwrap());
        let verifier = bytes(little_endian_hex(&realmd_hex(source.password_verifier())).unwrap());

        let private = [0x42; 32];
        let public = srp::server_public_key(&verifier, &private).unwrap();
        let client = SrpClientChallenge::new(
            username,
            password,
            srp::GENERATOR,
            srp::PRIME,
            PublicKey::from_le_bytes(public).unwrap(),
            salt,
        );
        srp::verify(
            "migrant",
            &salt,
            &verifier,
            &private,
            client.client_public_key(),
            client.client_proof(),
        )
        .unwrap();
    }

    #[test]
    fn content_the_shard_lacks_is_dropped_and_reported() {
        let plan = plan_one(character());
        let migration = &plan.characters[0];
        assert_eq!(migration.spells, [100]);
        assert_eq!(migration.explored_zones, [0b01]);
        assert_eq!(migration.taxi_nodes, [2]);
        let details: Vec<_> = plan.findings.iter().map(|f| f.detail.as_str()).collect();
        assert!(
            details.contains(&"1 spells not imported: 200"),
            "{details:?}"
        );
        assert!(
            details.contains(&"1 explored areas not imported: 1"),
            "{details:?}"
        );
        assert!(
            details.contains(&"1 taxi nodes not imported: 3"),
            "{details:?}"
        );
    }

    #[test]
    fn an_item_in_a_bag_is_addressed_by_the_bag_slot() {
        let mut source = character();
        let item = |item_guid, bag_guid, slot, entry| LegacyItem {
            item_guid,
            bag_guid,
            slot,
            entry,
            count: 1,
            flags: SOULBOUND_FLAG,
            enchantments: vec![15, 0, 0, 0, 0, 0],
            random_property_id: 0,
            durability: 20,
            text_id: 0,
        };
        source.items = vec![item(500, 0, 20, 4500), item(501, 500, 3, 2589)];
        let items = &plan_one(source).characters[0].items;
        assert_eq!(items[0].location, MigratedItemLocation::Slot(20));
        assert_eq!(
            items[1].location,
            MigratedItemLocation::Bag(MigratedBagSlot {
                bag_slot: 20,
                slot: 3
            })
        );
        assert_eq!(items[1].enchant_id, 15);
        assert!(items[1].soulbound);
    }

    #[test]
    fn an_unknown_item_refuses_the_character() {
        let mut source = character();
        source.items = vec![LegacyItem {
            item_guid: 1,
            bag_guid: 0,
            slot: 23,
            entry: 99_999,
            count: 1,
            flags: 0,
            enchantments: Vec::new(),
            random_property_id: 0,
            durability: 0,
            text_id: 0,
        }];
        let plan = plan_one(source);
        assert!(plan.characters.is_empty());
        assert_eq!(plan.findings[0].outcome, Outcome::Refused);
        assert_eq!(plan.findings[0].detail, "Item entry 99999 is not imported");
    }

    #[test]
    fn an_online_character_is_refused() {
        let mut source = character();
        source.online = true;
        let plan = plan_one(source);
        assert!(plan.characters.is_empty());
        assert!(plan.findings[0].detail.contains("online at the source"));
    }

    #[test]
    fn a_character_away_from_a_continent_arrives_at_home() {
        for map in [533, 36] {
            let mut source = character();
            source.position = here(map);
            let plan = plan_one(source);
            assert_eq!(plan.characters[0].position, SatsOption::None([]));
            assert_eq!(plan.findings[0].outcome, Outcome::Changed);
        }
    }

    #[test]
    fn a_refused_character_reports_only_its_refusal() {
        let mut source = character();
        source.position = here(533);
        source.home = None;
        let plan = plan_one(source);
        assert_eq!(plan.findings.len(), 1);
        assert_eq!(plan.findings[0].detail, "has no home bind");
    }

    #[test]
    fn quest_log_rewarded_and_failed_quests_keep_their_state() {
        let mut source = character();
        let quest = |quest, status, rewarded| LegacyQuest {
            quest,
            status,
            rewarded,
            explored: true,
            timer_secs: 0,
            mob_counts: [2, 0, 0, 0],
        };
        source.quests = vec![
            quest(7, QUEST_STATUS_FAILED, false),
            quest(8, QUEST_STATUS_COMPLETE, true),
        ];
        let quests = &plan_one(source).characters[0].quests;
        let SatsOption::Some(log) = &quests[0].log else {
            panic!("quest 7 is in the log");
        };
        assert!(log.failed);
        assert!(log.event_credit);
        assert_eq!(quests[1].log, SatsOption::None([]));
        assert!(quests[1].rewarded);
    }

    #[test]
    fn an_account_with_other_credentials_on_the_shard_is_refused() {
        let mut catalogue = catalogue();
        catalogue
            .accounts
            .insert("MIGRANT".to_string(), ("00".repeat(32), "00".repeat(32)));
        let plan = build("vmangos-main", &catalogue, &[account()], &[character()]);
        assert!(plan.accounts.is_empty());
        assert!(plan.characters.is_empty());
        assert_eq!(plan.findings.len(), 2);
    }

    #[test]
    fn reputation_at_war_comes_from_the_source_flags() {
        let mut source = character();
        source.reputations = vec![LegacyReputation {
            faction: 72,
            standing: 3000,
            flags: AT_WAR_FLAG | 1,
        }];
        let reputations = &plan_one(source).characters[0].reputations;
        assert_eq!(
            reputations,
            &[MigratedReputation {
                faction_id: 72,
                earned_standing: 3000,
                at_war: true
            }]
        );
    }

    #[test]
    fn a_cmangos_unwatched_faction_arrives_as_no_watched_faction() {
        let mut source = character();
        source.watched_faction = i64::from(u32::MAX);
        assert_eq!(plan_one(source).characters[0].watched_faction_index, -1);
    }

    #[test]
    fn quest_counters_that_restart_are_reported() {
        let mut source = character();
        source.quests = vec![LegacyQuest {
            quest: 7,
            status: QUEST_STATUS_INCOMPLETE,
            rewarded: false,
            explored: false,
            timer_secs: 0,
            mob_counts: [2, 0, 0, 0],
        }];
        let plan = plan_one(source);
        assert!(plan
            .findings
            .iter()
            .any(|f| f.detail == "kill and object counters of quest 7 restart"));
    }

    #[test]
    fn the_reducer_argument_uses_sats_json() {
        let mut source = character();
        source.position = here(533);
        let json = serde_json::to_value(&plan_one(source).characters[0]).unwrap();
        assert_eq!(json["position"], serde_json::json!({"none": []}));
        assert_eq!(json["account"]["username"], "MIGRANT");
        assert_eq!(json["account"]["salt"], format!("ab{}", "00".repeat(31)));
        assert_eq!(json["rested_xp"], 12);
    }
}
