use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::canonical;
use crate::error::DeltaError;
use crate::ids::{
    is_fixture_reserved_cast_id, is_fixture_reserved_creature_ai_id,
    is_fixture_reserved_creature_id, is_fixture_reserved_creature_spawn_id,
    is_fixture_reserved_gameobject_id, is_fixture_reserved_globals_id,
    is_fixture_reserved_gossip_id, is_fixture_reserved_item_id, is_fixture_reserved_loot_id,
    is_fixture_reserved_quest_id, is_fixture_reserved_spell_id, is_fixture_reserved_spellmeta_id,
    is_fixture_reserved_trainer_id, is_package_cast_id, is_package_creature_ai_id,
    is_package_creature_id, is_package_gameobject_id, is_package_globals_id, is_package_gossip_id,
    is_package_item_id, is_package_loot_id, is_package_quest_id, is_package_spell_id,
    is_package_spellmeta_id, is_package_trainer_id, packed_class_level_id,
    packed_creature_spawn_guid, packed_gameobject_spawn_guid, packed_quest_objective_id,
    packed_quest_reward_choice_id, packed_quest_reward_item_id, packed_race_class_id,
    packed_race_class_level_id, packed_spell_effect_id, MAX_CREATURE_GUID_COMPONENT,
    MAX_QUEST_OBJECTIVE_INDEX, MAX_QUEST_REWARD_CHOICE_INDEX, MAX_SPELL_EFFECT_INDEX,
    MAX_STATS_LEVEL,
};
use crate::schema::{FieldType, FieldValue, Table};

pub const DELTA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(String);

impl PackageId {
    pub fn new(id: impl Into<String>) -> Result<Self, DeltaError> {
        let id = id.into();
        let shaped = !id.is_empty()
            && id.len() <= 64
            && id.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
            });
        if shaped {
            Ok(Self(id))
        } else {
            Err(DeltaError::InvalidPackageId { found: id })
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceHash(String);

impl SourceHash {
    pub fn new(hex: impl Into<String>) -> Result<Self, DeltaError> {
        let hex = hex.into();
        let shaped = hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if shaped {
            Ok(Self(hex))
        } else {
            Err(DeltaError::InvalidSourceHash { found: hex })
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    Insert,
    Update,
}

impl Operation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
        }
    }

    pub fn parse(name: &str) -> Result<Self, DeltaError> {
        match name {
            "insert" => Ok(Self::Insert),
            "update" => Ok(Self::Update),
            "delete" => Err(DeltaError::DeleteNotSupported),
            other => Err(DeltaError::UnknownOperation {
                found: other.to_owned(),
            }),
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimaryKey {
    Spell {
        spell_id: u32,
    },
    SpellEffect {
        spell_id: u32,
        effect_index: u8,
    },
    Item {
        entry: u32,
    },
    Quest {
        entry: u32,
    },
    QuestText {
        quest_entry: u32,
    },
    QuestObjective {
        quest_entry: u32,
        obj_index: u8,
    },
    QuestCastObjective {
        quest_entry: u32,
        obj_index: u8,
    },
    QuestRewardItem {
        quest_entry: u32,
        item_entry: u32,
    },
    QuestRewardChoice {
        quest_entry: u32,
        choice_index: u8,
    },
    PickpocketLoot {
        id: u64,
    },
    GameobjectLoot {
        id: u64,
    },
    SkinningLoot {
        id: u64,
    },
    FishingLoot {
        id: u64,
    },
    CreatureCast {
        creature_entry: u32,
    },
    CreatureSpell {
        id: u64,
    },
    TrainerSpell {
        id: u64,
    },
    GossipMenu {
        entry: u32,
    },
    GossipMenuProfile {
        menu_id: u32,
    },
    GossipMenuProfileOption {
        row_id: u32,
    },
    GossipOption {
        row_id: u32,
    },
    NpcText {
        text_id: u32,
    },
    NpcTextSlot {
        id: u64,
    },
    ClassLevelStats {
        class: u8,
        level: u8,
    },
    LevelStats {
        race: u8,
        class: u8,
        level: u8,
    },
    StartPosition {
        race: u8,
        class: u8,
    },
    GraveyardZone {
        row_id: u64,
    },
    AreatriggerTeleport {
        trigger_id: u32,
    },
    CreateinfoSpell {
        id: u64,
    },
    CreateinfoAction {
        row_id: u64,
    },
    SpellChain {
        spell_id: u32,
    },
    SpellLearn {
        id: u64,
    },
    SpellProcEvent {
        spell_id: u32,
    },
    CreatureTemplate {
        entry: u32,
    },
    CreatureSpawn {
        map_id: u32,
        entry: u32,
        spawn_id: u32,
    },
    GameobjectTemplate {
        entry: u32,
    },
    GameobjectTrap {
        entry: u32,
    },
    GameobjectSpawn {
        map_id: u32,
        spawn_id: u32,
    },
    CreatureAiBroadcastText {
        id: u32,
    },
    CreatureAiSummon {
        id: u32,
    },
    QuestEventRequirement {
        id: u64,
    },
}

impl PrimaryKey {
    pub fn spell(spell_id: u32) -> Result<Self, DeltaError> {
        check_claimable_spell_id(Table::Spell, spell_id)?;
        Ok(Self::Spell { spell_id })
    }

    pub fn spell_effect(spell_id: u32, effect_index: u8) -> Result<Self, DeltaError> {
        check_claimable_spell_id(Table::SpellEffect, spell_id)?;
        if effect_index > MAX_SPELL_EFFECT_INDEX {
            return Err(DeltaError::EffectIndexOutOfRange { effect_index });
        }
        Ok(Self::SpellEffect {
            spell_id,
            effect_index,
        })
    }

    pub fn item(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_item_id(entry)?;
        Ok(Self::Item { entry })
    }

    pub fn quest(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::Quest, entry)?;
        Ok(Self::Quest { entry })
    }

    pub fn quest_text(quest_entry: u32) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::QuestText, quest_entry)?;
        Ok(Self::QuestText { quest_entry })
    }

    pub fn quest_objective(quest_entry: u32, obj_index: u8) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::QuestObjective, quest_entry)?;
        if obj_index > MAX_QUEST_OBJECTIVE_INDEX {
            return Err(DeltaError::QuestObjectiveIndexOutOfRange { obj_index });
        }
        Ok(Self::QuestObjective {
            quest_entry,
            obj_index,
        })
    }

    pub fn quest_cast_objective(quest_entry: u32, obj_index: u8) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::QuestCastObjective, quest_entry)?;
        if obj_index > MAX_QUEST_OBJECTIVE_INDEX {
            return Err(DeltaError::QuestObjectiveIndexOutOfRange { obj_index });
        }
        Ok(Self::QuestCastObjective {
            quest_entry,
            obj_index,
        })
    }

    pub fn quest_reward_item(quest_entry: u32, item_entry: u32) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::QuestRewardItem, quest_entry)?;
        Ok(Self::QuestRewardItem {
            quest_entry,
            item_entry,
        })
    }

    pub fn quest_reward_choice(quest_entry: u32, choice_index: u8) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::QuestRewardChoice, quest_entry)?;
        if choice_index > MAX_QUEST_REWARD_CHOICE_INDEX {
            return Err(DeltaError::QuestRewardChoiceIndexOutOfRange { choice_index });
        }
        Ok(Self::QuestRewardChoice {
            quest_entry,
            choice_index,
        })
    }

    pub fn pickpocket_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::PickpocketLoot, id)?;
        Ok(Self::PickpocketLoot { id })
    }

    pub fn gameobject_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::GameobjectLoot, id)?;
        Ok(Self::GameobjectLoot { id })
    }

    pub fn skinning_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::SkinningLoot, id)?;
        Ok(Self::SkinningLoot { id })
    }

    pub fn fishing_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::FishingLoot, id)?;
        Ok(Self::FishingLoot { id })
    }

    pub fn creature_cast(creature_entry: u32) -> Result<Self, DeltaError> {
        check_claimable_creature_entry(Table::CreatureCast, "creature_entry", creature_entry)?;
        Ok(Self::CreatureCast { creature_entry })
    }

    pub fn creature_spell(id: u64) -> Result<Self, DeltaError> {
        check_claimable_cast_id(id)?;
        Ok(Self::CreatureSpell { id })
    }

    pub fn trainer_spell(id: u64) -> Result<Self, DeltaError> {
        check_claimable_trainer_id(id)?;
        Ok(Self::TrainerSpell { id })
    }

    pub fn gossip_menu(entry: u32) -> Result<Self, DeltaError> {
        if entry == 0 {
            return Err(DeltaError::MalformedKey {
                table: Table::GossipMenu,
                detail: "`entry` 0 is not a creature".to_owned(),
            });
        }
        Ok(Self::GossipMenu { entry })
    }

    pub fn gossip_menu_profile(menu_id: u32) -> Result<Self, DeltaError> {
        check_claimable_gossip_id(Table::GossipMenuProfile, "menu_id", u64::from(menu_id))?;
        Ok(Self::GossipMenuProfile { menu_id })
    }

    pub fn gossip_menu_profile_option(row_id: u32) -> Result<Self, DeltaError> {
        check_claimable_gossip_id(Table::GossipMenuProfileOption, "row_id", u64::from(row_id))?;
        Ok(Self::GossipMenuProfileOption { row_id })
    }

    pub fn gossip_option(row_id: u32) -> Result<Self, DeltaError> {
        check_claimable_gossip_id(Table::GossipOption, "row_id", u64::from(row_id))?;
        Ok(Self::GossipOption { row_id })
    }

    pub fn npc_text(text_id: u32) -> Result<Self, DeltaError> {
        check_claimable_gossip_id(Table::NpcText, "text_id", u64::from(text_id))?;
        Ok(Self::NpcText { text_id })
    }

    pub fn npc_text_slot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_gossip_id(Table::NpcTextSlot, "id", id)?;
        Ok(Self::NpcTextSlot { id })
    }

    pub fn class_level_stats(class: u8, level: u8) -> Result<Self, DeltaError> {
        if class == 0 {
            return Err(DeltaError::MalformedKey {
                table: Table::ClassLevelStats,
                detail: "`class` 0 is not a class".to_owned(),
            });
        }
        check_stats_level(level)?;
        Ok(Self::ClassLevelStats { class, level })
    }

    pub fn level_stats(race: u8, class: u8, level: u8) -> Result<Self, DeltaError> {
        check_race_and_class(Table::LevelStats, race, class)?;
        check_stats_level(level)?;
        Ok(Self::LevelStats { race, class, level })
    }

    pub fn start_position(race: u8, class: u8) -> Result<Self, DeltaError> {
        check_race_and_class(Table::StartPosition, race, class)?;
        Ok(Self::StartPosition { race, class })
    }

    pub fn graveyard_zone(row_id: u64) -> Result<Self, DeltaError> {
        check_claimable_globals_id(Table::GraveyardZone, "row_id", row_id)?;
        Ok(Self::GraveyardZone { row_id })
    }

    pub fn areatrigger_teleport(trigger_id: u32) -> Result<Self, DeltaError> {
        if trigger_id == 0 {
            return Err(DeltaError::MalformedKey {
                table: Table::AreatriggerTeleport,
                detail: "`trigger_id` 0 is not a trigger".to_owned(),
            });
        }
        Ok(Self::AreatriggerTeleport { trigger_id })
    }

    pub fn createinfo_spell(id: u64) -> Result<Self, DeltaError> {
        check_claimable_globals_id(Table::CreateinfoSpell, "id", id)?;
        Ok(Self::CreateinfoSpell { id })
    }

    pub fn createinfo_action(row_id: u64) -> Result<Self, DeltaError> {
        check_claimable_globals_id(Table::CreateinfoAction, "row_id", row_id)?;
        Ok(Self::CreateinfoAction { row_id })
    }

    pub fn spell_chain(spell_id: u32) -> Result<Self, DeltaError> {
        check_claimable_spell_id(Table::SpellChain, spell_id)?;
        Ok(Self::SpellChain { spell_id })
    }

    pub fn spell_learn(id: u64) -> Result<Self, DeltaError> {
        if id == 0 {
            return Err(DeltaError::MalformedKey {
                table: Table::SpellLearn,
                detail: "`id` 0 is not a row".to_owned(),
            });
        }
        if is_fixture_reserved_spellmeta_id(id) {
            return Err(DeltaError::SpellmetaIdFixtureReserved { id });
        }
        Ok(Self::SpellLearn { id })
    }

    pub fn spell_proc_event(spell_id: u32) -> Result<Self, DeltaError> {
        check_claimable_spell_id(Table::SpellProcEvent, spell_id)?;
        Ok(Self::SpellProcEvent { spell_id })
    }

    pub fn creature_template(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_creature_entry(Table::CreatureTemplate, "entry", entry)?;
        Ok(Self::CreatureTemplate { entry })
    }

    pub fn creature_spawn(map_id: u32, entry: u32, spawn_id: u32) -> Result<Self, DeltaError> {
        check_guid_component(Table::CreatureSpawn, "entry", entry)?;
        check_claimable_creature_spawn_id(spawn_id)?;
        Ok(Self::CreatureSpawn {
            map_id,
            entry,
            spawn_id,
        })
    }

    pub fn gameobject_template(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_gameobject_id(Table::GameobjectTemplate, "entry", entry)?;
        Ok(Self::GameobjectTemplate { entry })
    }

    pub fn gameobject_trap(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_gameobject_id(Table::GameobjectTrap, "entry", entry)?;
        Ok(Self::GameobjectTrap { entry })
    }

    pub fn gameobject_spawn(map_id: u32, spawn_id: u32) -> Result<Self, DeltaError> {
        check_claimable_gameobject_id(Table::GameobjectSpawn, "spawn_id", spawn_id)?;
        Ok(Self::GameobjectSpawn { map_id, spawn_id })
    }

    pub fn creature_ai_broadcast_text(id: u32) -> Result<Self, DeltaError> {
        check_claimable_creature_ai_id(Table::CreatureAiBroadcastText, u64::from(id))?;
        Ok(Self::CreatureAiBroadcastText { id })
    }

    pub fn creature_ai_summon(id: u32) -> Result<Self, DeltaError> {
        check_claimable_creature_ai_id(Table::CreatureAiSummon, u64::from(id))?;
        Ok(Self::CreatureAiSummon { id })
    }

    pub fn quest_event_requirement(id: u64) -> Result<Self, DeltaError> {
        check_claimable_creature_ai_id(Table::QuestEventRequirement, id)?;
        Ok(Self::QuestEventRequirement { id })
    }

    #[must_use]
    pub const fn table(self) -> Table {
        match self {
            Self::Spell { .. } => Table::Spell,
            Self::SpellEffect { .. } => Table::SpellEffect,
            Self::Item { .. } => Table::Item,
            Self::Quest { .. } => Table::Quest,
            Self::QuestText { .. } => Table::QuestText,
            Self::QuestObjective { .. } => Table::QuestObjective,
            Self::QuestCastObjective { .. } => Table::QuestCastObjective,
            Self::QuestRewardItem { .. } => Table::QuestRewardItem,
            Self::QuestRewardChoice { .. } => Table::QuestRewardChoice,
            Self::PickpocketLoot { .. } => Table::PickpocketLoot,
            Self::GameobjectLoot { .. } => Table::GameobjectLoot,
            Self::SkinningLoot { .. } => Table::SkinningLoot,
            Self::FishingLoot { .. } => Table::FishingLoot,
            Self::CreatureCast { .. } => Table::CreatureCast,
            Self::CreatureSpell { .. } => Table::CreatureSpell,
            Self::TrainerSpell { .. } => Table::TrainerSpell,
            Self::GossipMenu { .. } => Table::GossipMenu,
            Self::GossipMenuProfile { .. } => Table::GossipMenuProfile,
            Self::GossipMenuProfileOption { .. } => Table::GossipMenuProfileOption,
            Self::GossipOption { .. } => Table::GossipOption,
            Self::NpcText { .. } => Table::NpcText,
            Self::NpcTextSlot { .. } => Table::NpcTextSlot,
            Self::ClassLevelStats { .. } => Table::ClassLevelStats,
            Self::LevelStats { .. } => Table::LevelStats,
            Self::StartPosition { .. } => Table::StartPosition,
            Self::GraveyardZone { .. } => Table::GraveyardZone,
            Self::AreatriggerTeleport { .. } => Table::AreatriggerTeleport,
            Self::CreateinfoSpell { .. } => Table::CreateinfoSpell,
            Self::CreateinfoAction { .. } => Table::CreateinfoAction,
            Self::SpellChain { .. } => Table::SpellChain,
            Self::SpellLearn { .. } => Table::SpellLearn,
            Self::SpellProcEvent { .. } => Table::SpellProcEvent,
            Self::CreatureTemplate { .. } => Table::CreatureTemplate,
            Self::CreatureSpawn { .. } => Table::CreatureSpawn,
            Self::GameobjectTemplate { .. } => Table::GameobjectTemplate,
            Self::GameobjectTrap { .. } => Table::GameobjectTrap,
            Self::GameobjectSpawn { .. } => Table::GameobjectSpawn,
            Self::CreatureAiBroadcastText { .. } => Table::CreatureAiBroadcastText,
            Self::CreatureAiSummon { .. } => Table::CreatureAiSummon,
            Self::QuestEventRequirement { .. } => Table::QuestEventRequirement,
        }
    }

    #[must_use]
    pub const fn map_id(self) -> Option<u32> {
        match self {
            Self::CreatureSpawn { map_id, .. } | Self::GameobjectSpawn { map_id, .. } => {
                Some(map_id)
            }
            Self::Spell { .. }
            | Self::SpellEffect { .. }
            | Self::Item { .. }
            | Self::Quest { .. }
            | Self::QuestText { .. }
            | Self::QuestObjective { .. }
            | Self::QuestCastObjective { .. }
            | Self::QuestRewardItem { .. }
            | Self::QuestRewardChoice { .. }
            | Self::PickpocketLoot { .. }
            | Self::GameobjectLoot { .. }
            | Self::SkinningLoot { .. }
            | Self::FishingLoot { .. }
            | Self::CreatureCast { .. }
            | Self::CreatureSpell { .. }
            | Self::TrainerSpell { .. }
            | Self::GossipMenu { .. }
            | Self::GossipMenuProfile { .. }
            | Self::GossipMenuProfileOption { .. }
            | Self::GossipOption { .. }
            | Self::NpcText { .. }
            | Self::NpcTextSlot { .. }
            | Self::ClassLevelStats { .. }
            | Self::LevelStats { .. }
            | Self::StartPosition { .. }
            | Self::GraveyardZone { .. }
            | Self::AreatriggerTeleport { .. }
            | Self::CreateinfoSpell { .. }
            | Self::CreateinfoAction { .. }
            | Self::SpellChain { .. }
            | Self::SpellLearn { .. }
            | Self::SpellProcEvent { .. }
            | Self::CreatureTemplate { .. }
            | Self::GameobjectTemplate { .. }
            | Self::GameobjectTrap { .. }
            | Self::CreatureAiBroadcastText { .. }
            | Self::CreatureAiSummon { .. }
            | Self::QuestEventRequirement { .. } => None,
        }
    }

    #[must_use]
    pub fn spell_id(self) -> u32 {
        match self {
            Self::Spell { spell_id } | Self::SpellEffect { spell_id, .. } => spell_id,
            _ => unreachable!(
                "`spell_id` is a spell-family accessor; a foreign-family key never reaches it"
            ),
        }
    }

    #[must_use]
    pub fn quest_entry(self) -> u32 {
        match self {
            Self::Quest { entry } => entry,
            Self::QuestText { quest_entry }
            | Self::QuestObjective { quest_entry, .. }
            | Self::QuestCastObjective { quest_entry, .. }
            | Self::QuestRewardItem { quest_entry, .. }
            | Self::QuestRewardChoice { quest_entry, .. } => quest_entry,
            _ => unreachable!(
                "`quest_entry` is a quest-family accessor; a foreign-family key never reaches it"
            ),
        }
    }

    #[must_use]
    pub const fn row_id(self) -> u64 {
        match self {
            Self::Spell { spell_id }
            | Self::SpellChain { spell_id }
            | Self::SpellProcEvent { spell_id } => spell_id as u64,
            Self::SpellEffect {
                spell_id,
                effect_index,
            } => packed_spell_effect_id(spell_id, effect_index),
            Self::Item { entry } | Self::Quest { entry } | Self::GossipMenu { entry } => {
                entry as u64
            }
            Self::QuestText { quest_entry } => quest_entry as u64,
            Self::QuestObjective {
                quest_entry,
                obj_index,
            }
            | Self::QuestCastObjective {
                quest_entry,
                obj_index,
            } => packed_quest_objective_id(quest_entry, obj_index),
            Self::QuestRewardItem {
                quest_entry,
                item_entry,
            } => packed_quest_reward_item_id(quest_entry, item_entry),
            Self::QuestRewardChoice {
                quest_entry,
                choice_index,
            } => packed_quest_reward_choice_id(quest_entry, choice_index),
            Self::PickpocketLoot { id }
            | Self::GameobjectLoot { id }
            | Self::SkinningLoot { id }
            | Self::FishingLoot { id }
            | Self::CreatureSpell { id }
            | Self::TrainerSpell { id }
            | Self::NpcTextSlot { id }
            | Self::CreateinfoSpell { id }
            | Self::SpellLearn { id }
            | Self::QuestEventRequirement { id } => id,
            Self::CreatureCast { creature_entry } => creature_entry as u64,
            Self::GossipMenuProfile { menu_id } => menu_id as u64,
            Self::GossipMenuProfileOption { row_id } | Self::GossipOption { row_id } => {
                row_id as u64
            }
            Self::NpcText { text_id } => text_id as u64,
            Self::ClassLevelStats { class, level } => packed_class_level_id(class, level),
            Self::LevelStats { race, class, level } => {
                packed_race_class_level_id(race, class, level)
            }
            Self::StartPosition { race, class } => packed_race_class_id(race, class),
            Self::GraveyardZone { row_id } | Self::CreateinfoAction { row_id } => row_id,
            Self::AreatriggerTeleport { trigger_id } => trigger_id as u64,
            Self::CreatureTemplate { entry }
            | Self::GameobjectTemplate { entry }
            | Self::GameobjectTrap { entry } => entry as u64,
            Self::CreatureSpawn {
                entry, spawn_id, ..
            } => packed_creature_spawn_guid(entry, spawn_id),
            Self::GameobjectSpawn { spawn_id, .. } => packed_gameobject_spawn_guid(spawn_id),
            Self::CreatureAiBroadcastText { id } | Self::CreatureAiSummon { id } => id as u64,
        }
    }
}

impl fmt::Display for PrimaryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spell { spell_id } => write!(f, "{{spell_id={spell_id}}}"),
            Self::SpellEffect {
                spell_id,
                effect_index,
            } => write!(f, "{{spell_id={spell_id}, effect_index={effect_index}}}"),
            Self::Item { entry } | Self::Quest { entry } | Self::GossipMenu { entry } => {
                write!(f, "{{entry={entry}}}")
            }
            Self::QuestText { quest_entry } => write!(f, "{{quest_entry={quest_entry}}}"),
            Self::QuestObjective {
                quest_entry,
                obj_index,
            }
            | Self::QuestCastObjective {
                quest_entry,
                obj_index,
            } => write!(f, "{{quest_entry={quest_entry}, obj_index={obj_index}}}"),
            Self::QuestRewardItem {
                quest_entry,
                item_entry,
            } => write!(f, "{{quest_entry={quest_entry}, item_entry={item_entry}}}"),
            Self::QuestRewardChoice {
                quest_entry,
                choice_index,
            } => write!(
                f,
                "{{quest_entry={quest_entry}, choice_index={choice_index}}}"
            ),
            Self::PickpocketLoot { id }
            | Self::GameobjectLoot { id }
            | Self::SkinningLoot { id }
            | Self::FishingLoot { id }
            | Self::CreatureSpell { id }
            | Self::TrainerSpell { id }
            | Self::NpcTextSlot { id }
            | Self::CreateinfoSpell { id }
            | Self::SpellLearn { id }
            | Self::QuestEventRequirement { id } => write!(f, "{{id={id}}}"),
            Self::CreatureCast { creature_entry } => {
                write!(f, "{{creature_entry={creature_entry}}}")
            }
            Self::GossipMenuProfile { menu_id } => write!(f, "{{menu_id={menu_id}}}"),
            Self::GossipMenuProfileOption { row_id } | Self::GossipOption { row_id } => {
                write!(f, "{{row_id={row_id}}}")
            }
            Self::NpcText { text_id } => write!(f, "{{text_id={text_id}}}"),
            Self::ClassLevelStats { class, level } => {
                write!(f, "{{class={class}, level={level}}}")
            }
            Self::LevelStats { race, class, level } => {
                write!(f, "{{race={race}, class={class}, level={level}}}")
            }
            Self::StartPosition { race, class } => write!(f, "{{race={race}, class={class}}}"),
            Self::GraveyardZone { row_id } | Self::CreateinfoAction { row_id } => {
                write!(f, "{{row_id={row_id}}}")
            }
            Self::AreatriggerTeleport { trigger_id } => write!(f, "{{trigger_id={trigger_id}}}"),
            Self::SpellChain { spell_id } | Self::SpellProcEvent { spell_id } => {
                write!(f, "{{spell_id={spell_id}}}")
            }
            Self::CreatureTemplate { entry }
            | Self::GameobjectTemplate { entry }
            | Self::GameobjectTrap { entry } => write!(f, "{{entry={entry}}}"),
            Self::CreatureSpawn {
                map_id,
                entry,
                spawn_id,
            } => write!(f, "{{map_id={map_id}, entry={entry}, spawn_id={spawn_id}}}"),
            Self::GameobjectSpawn { map_id, spawn_id } => {
                write!(f, "{{map_id={map_id}, spawn_id={spawn_id}}}")
            }
            Self::CreatureAiBroadcastText { id } | Self::CreatureAiSummon { id } => {
                write!(f, "{{id={id}}}")
            }
        }
    }
}

fn check_inventable(key: PrimaryKey) -> Result<(), DeltaError> {
    match key {
        PrimaryKey::Spell { spell_id } | PrimaryKey::SpellEffect { spell_id, .. } => {
            if is_package_spell_id(spell_id) {
                Ok(())
            } else {
                Err(DeltaError::SpellIdNotClientSafe { spell_id })
            }
        }
        PrimaryKey::Item { entry } => {
            if is_package_item_id(entry) {
                Ok(())
            } else {
                Err(DeltaError::ItemIdNotClientSafe { entry })
            }
        }
        PrimaryKey::Quest { entry } => {
            if is_package_quest_id(entry) {
                Ok(())
            } else {
                Err(DeltaError::QuestIdNotClientSafe { entry })
            }
        }
        PrimaryKey::QuestText { quest_entry }
        | PrimaryKey::QuestObjective { quest_entry, .. }
        | PrimaryKey::QuestCastObjective { quest_entry, .. }
        | PrimaryKey::QuestRewardItem { quest_entry, .. }
        | PrimaryKey::QuestRewardChoice { quest_entry, .. } => {
            if is_package_quest_id(quest_entry) {
                Ok(())
            } else {
                Err(DeltaError::QuestIdNotClientSafe { entry: quest_entry })
            }
        }
        PrimaryKey::PickpocketLoot { id }
        | PrimaryKey::GameobjectLoot { id }
        | PrimaryKey::SkinningLoot { id }
        | PrimaryKey::FishingLoot { id } => {
            if is_package_loot_id(id) {
                Ok(())
            } else {
                Err(DeltaError::LootIdNotClientSafe { id })
            }
        }
        PrimaryKey::CreatureCast { .. } => Err(DeltaError::InsertNotSupported {
            table: Table::CreatureCast,
        }),
        PrimaryKey::CreatureSpell { id } => {
            if is_package_cast_id(id) {
                Ok(())
            } else {
                Err(DeltaError::CastIdNotClientSafe { id })
            }
        }
        PrimaryKey::TrainerSpell { id } => {
            if is_package_trainer_id(id) {
                Ok(())
            } else {
                Err(DeltaError::TrainerIdNotClientSafe { id })
            }
        }
        PrimaryKey::GossipMenu { .. } => insert_refused(Table::GossipMenu),
        PrimaryKey::ClassLevelStats { .. } => insert_refused(Table::ClassLevelStats),
        PrimaryKey::LevelStats { .. } => insert_refused(Table::LevelStats),
        PrimaryKey::StartPosition { .. } => insert_refused(Table::StartPosition),
        PrimaryKey::AreatriggerTeleport { .. } => insert_refused(Table::AreatriggerTeleport),
        PrimaryKey::GossipMenuProfile { menu_id: id }
        | PrimaryKey::GossipMenuProfileOption { row_id: id }
        | PrimaryKey::GossipOption { row_id: id }
        | PrimaryKey::NpcText { text_id: id } => gossip_band(u64::from(id)),
        PrimaryKey::NpcTextSlot { id } => gossip_band(id),
        PrimaryKey::GraveyardZone { row_id: id }
        | PrimaryKey::CreateinfoSpell { id }
        | PrimaryKey::CreateinfoAction { row_id: id } => globals_band(id),
        PrimaryKey::SpellChain { spell_id } | PrimaryKey::SpellProcEvent { spell_id } => {
            check_band(
                is_package_spell_id(spell_id),
                DeltaError::SpellIdNotClientSafe { spell_id },
            )
        }
        PrimaryKey::SpellLearn { id } => check_band(
            is_package_spellmeta_id(id),
            DeltaError::SpellmetaIdNotClientSafe { id },
        ),
        PrimaryKey::CreatureTemplate { entry: id }
        | PrimaryKey::CreatureSpawn { spawn_id: id, .. } => check_band(
            is_package_creature_id(id),
            DeltaError::CreatureIdNotClientSafe { id },
        ),
        PrimaryKey::GameobjectTemplate { entry: id }
        | PrimaryKey::GameobjectTrap { entry: id }
        | PrimaryKey::GameobjectSpawn { spawn_id: id, .. } => check_band(
            is_package_gameobject_id(id),
            DeltaError::GameobjectIdNotClientSafe { id },
        ),
        PrimaryKey::CreatureAiBroadcastText { id } | PrimaryKey::CreatureAiSummon { id } => {
            creature_ai_band(u64::from(id))
        }
        PrimaryKey::QuestEventRequirement { id } => creature_ai_band(id),
    }
}

fn insert_refused(table: Table) -> Result<(), DeltaError> {
    Err(DeltaError::InsertNotSupported { table })
}

fn check_band(inside: bool, refusal: DeltaError) -> Result<(), DeltaError> {
    if inside {
        Ok(())
    } else {
        Err(refusal)
    }
}

fn gossip_band(id: u64) -> Result<(), DeltaError> {
    check_band(
        is_package_gossip_id(id),
        DeltaError::GossipIdNotClientSafe { id },
    )
}

fn globals_band(id: u64) -> Result<(), DeltaError> {
    check_band(
        is_package_globals_id(id),
        DeltaError::GlobalsIdNotClientSafe { id },
    )
}

fn check_claimable_spell_id(table: Table, spell_id: u32) -> Result<(), DeltaError> {
    if spell_id == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: "`spell_id` 0 is not a spell".to_owned(),
        });
    }
    if is_fixture_reserved_spell_id(spell_id) {
        return Err(DeltaError::SpellIdFixtureReserved { spell_id });
    }
    Ok(())
}

fn check_claimable_quest_id(table: Table, quest_entry: u32) -> Result<(), DeltaError> {
    if quest_entry == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: "`quest_entry` 0 is not a quest".to_owned(),
        });
    }
    if is_fixture_reserved_quest_id(quest_entry) {
        return Err(DeltaError::QuestIdFixtureReserved { entry: quest_entry });
    }
    Ok(())
}

fn check_claimable_loot_id(table: Table, id: u64) -> Result<(), DeltaError> {
    if id == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: "`id` 0 is not a row".to_owned(),
        });
    }
    if is_fixture_reserved_loot_id(id) {
        return Err(DeltaError::LootIdFixtureReserved { id });
    }
    Ok(())
}

fn check_claimable_cast_id(id: u64) -> Result<(), DeltaError> {
    if id == 0 {
        return Err(DeltaError::MalformedKey {
            table: Table::CreatureSpell,
            detail: "`id` 0 is not a row".to_owned(),
        });
    }
    if is_fixture_reserved_cast_id(id) {
        return Err(DeltaError::CastIdFixtureReserved { id });
    }
    Ok(())
}

fn check_claimable_trainer_id(id: u64) -> Result<(), DeltaError> {
    if id == 0 {
        return Err(DeltaError::MalformedKey {
            table: Table::TrainerSpell,
            detail: "`id` 0 is not a row".to_owned(),
        });
    }
    if is_fixture_reserved_trainer_id(id) {
        return Err(DeltaError::TrainerIdFixtureReserved { id });
    }
    Ok(())
}

fn check_claimable_gossip_id(table: Table, member: &str, id: u64) -> Result<(), DeltaError> {
    if id == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: format!("`{member}` 0 is not a row"),
        });
    }
    if is_fixture_reserved_gossip_id(id) {
        return Err(DeltaError::GossipIdFixtureReserved { id });
    }
    Ok(())
}

fn check_claimable_globals_id(table: Table, member: &str, id: u64) -> Result<(), DeltaError> {
    if id == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: format!("`{member}` 0 is not a row"),
        });
    }
    if is_fixture_reserved_globals_id(id) {
        return Err(DeltaError::GlobalsIdFixtureReserved { id });
    }
    Ok(())
}

fn check_stats_level(level: u8) -> Result<(), DeltaError> {
    if level == 0 || level > MAX_STATS_LEVEL {
        return Err(DeltaError::StatsLevelOutOfRange { level });
    }
    Ok(())
}

fn check_race_and_class(table: Table, race: u8, class: u8) -> Result<(), DeltaError> {
    if race == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: "`race` 0 is not a race".to_owned(),
        });
    }
    if class == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: "`class` 0 is not a class".to_owned(),
        });
    }
    Ok(())
}

fn check_claimable_creature_entry(
    table: Table,
    member: &str,
    entry: u32,
) -> Result<(), DeltaError> {
    check_guid_component(table, member, entry)?;
    if is_fixture_reserved_creature_id(entry) {
        return Err(DeltaError::CreatureIdFixtureReserved { id: entry });
    }
    Ok(())
}

fn check_claimable_creature_spawn_id(spawn_id: u32) -> Result<(), DeltaError> {
    check_guid_component(Table::CreatureSpawn, "spawn_id", spawn_id)?;
    if is_fixture_reserved_creature_spawn_id(spawn_id) {
        return Err(DeltaError::CreatureIdFixtureReserved { id: spawn_id });
    }
    Ok(())
}

// Creature spawn GUIDs allocate 24 bits to each component.
fn check_guid_component(table: Table, member: &str, value: u32) -> Result<(), DeltaError> {
    if value == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: format!("`{member}` 0 is not a row"),
        });
    }
    if value > MAX_CREATURE_GUID_COMPONENT {
        return Err(DeltaError::MalformedKey {
            table,
            detail: format!(
                "`{member}` {value} is wider than the creature guid's 24-bit field \
                 (1..={MAX_CREATURE_GUID_COMPONENT})"
            ),
        });
    }
    Ok(())
}

fn check_claimable_gameobject_id(table: Table, member: &str, id: u32) -> Result<(), DeltaError> {
    if id == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: format!("`{member}` 0 is not a row"),
        });
    }
    if is_fixture_reserved_gameobject_id(id) {
        return Err(DeltaError::GameobjectIdFixtureReserved { id });
    }
    Ok(())
}

fn creature_ai_band(id: u64) -> Result<(), DeltaError> {
    check_band(
        is_package_creature_ai_id(id),
        DeltaError::CreatureAiIdNotClientSafe { id },
    )
}

fn check_claimable_creature_ai_id(table: Table, id: u64) -> Result<(), DeltaError> {
    if id == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: "`id` 0 is not a row".to_owned(),
        });
    }
    if is_fixture_reserved_creature_ai_id(id) {
        return Err(DeltaError::CreatureAiIdFixtureReserved { id });
    }
    Ok(())
}

fn check_claimable_item_id(entry: u32) -> Result<(), DeltaError> {
    if entry == 0 {
        return Err(DeltaError::MalformedKey {
            table: Table::Item,
            detail: "`entry` 0 is not an item".to_owned(),
        });
    }
    if is_fixture_reserved_item_id(entry) {
        return Err(DeltaError::ItemIdFixtureReserved { entry });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    key: PrimaryKey,
    operation: Operation,
    fields: BTreeMap<String, FieldValue>,
}

impl Claim {
    pub fn new(
        key: PrimaryKey,
        operation: Operation,
        fields: BTreeMap<String, FieldValue>,
    ) -> Result<Self, DeltaError> {
        let table = key.table();
        if operation == Operation::Insert {
            check_inventable(key)?;
        }

        for (name, value) in &fields {
            let Some(expected) = table.column_type(name) else {
                return Err(if is_key_column(table, name) {
                    DeltaError::KeyColumnClaimed {
                        table,
                        field: name.clone(),
                    }
                } else {
                    DeltaError::UnknownField {
                        table,
                        field: name.clone(),
                    }
                });
            };
            let found = value.field_type();
            if found != expected {
                return Err(DeltaError::FieldTypeMismatch {
                    table,
                    field: name.clone(),
                    expected,
                    found,
                });
            }
        }

        if let (Table::CreatureSpell, Some(FieldValue::U32(creature_entry))) =
            (table, fields.get("creature_entry"))
        {
            check_claimable_creature_entry(table, "creature_entry", *creature_entry)?;
        }

        match operation {
            Operation::Insert => {
                let missing: Vec<String> = table
                    .columns()
                    .iter()
                    .filter(|column| {
                        !fields.contains_key(column.name) && !table.column_has_default(column.name)
                    })
                    .map(|column| column.name.to_owned())
                    .collect();
                if !missing.is_empty() {
                    return Err(DeltaError::IncompleteInsert { table, missing });
                }
            }
            Operation::Update => {
                if fields.is_empty() {
                    return Err(DeltaError::EmptyUpdate { table });
                }
            }
        }

        Ok(Self {
            key,
            operation,
            fields,
        })
    }

    #[must_use]
    pub const fn key(&self) -> PrimaryKey {
        self.key
    }

    #[must_use]
    pub const fn table(&self) -> Table {
        self.key.table()
    }

    #[must_use]
    pub const fn operation(&self) -> Operation {
        self.operation
    }

    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<String, FieldValue> {
        &self.fields
    }
}

fn is_key_column(table: Table, name: &str) -> bool {
    match table {
        Table::Spell | Table::SpellChain | Table::SpellProcEvent => name == "spell_id",
        Table::SpellEffect => matches!(name, "id" | "spell_id" | "effect_index"),
        Table::Item
        | Table::Quest
        | Table::GossipMenu
        | Table::CreatureTemplate
        | Table::GameobjectTemplate
        | Table::GameobjectTrap => name == "entry",
        Table::QuestText => name == "quest_entry",
        Table::QuestObjective | Table::QuestCastObjective => {
            matches!(name, "id" | "quest_entry" | "obj_index")
        }
        Table::QuestRewardItem => matches!(name, "id" | "quest_entry" | "item_entry"),
        Table::QuestRewardChoice => matches!(name, "id" | "quest_entry" | "choice_index"),
        Table::PickpocketLoot
        | Table::GameobjectLoot
        | Table::SkinningLoot
        | Table::FishingLoot
        | Table::CreatureSpell
        | Table::TrainerSpell
        | Table::NpcTextSlot
        | Table::CreateinfoSpell
        | Table::SpellLearn
        | Table::CreatureAiBroadcastText
        | Table::CreatureAiSummon
        | Table::QuestEventRequirement => name == "id",
        Table::CreatureCast => name == "creature_entry",
        Table::GossipMenuProfile => name == "menu_id",
        Table::GossipMenuProfileOption
        | Table::GossipOption
        | Table::GraveyardZone
        | Table::CreateinfoAction => name == "row_id",
        Table::NpcText => name == "text_id",
        Table::ClassLevelStats => name == "class" || name == "level",
        Table::LevelStats => name == "race" || name == "class" || name == "level",
        Table::StartPosition => name == "race" || name == "class",
        Table::AreatriggerTeleport => name == "trigger_id",
        Table::CreatureSpawn => matches!(name, "guid" | "map_id" | "entry" | "spawn_id"),
        Table::GameobjectSpawn => matches!(name, "guid" | "map_id" | "spawn_id"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackageDelta {
    package: PackageId,
    source_hash: SourceHash,
    claims: Vec<Claim>,
}

impl PackageDelta {
    pub fn new(
        package: PackageId,
        source_hash: SourceHash,
        mut claims: Vec<Claim>,
    ) -> Result<Self, DeltaError> {
        claims.sort_by_key(Claim::key);
        for pair in claims.windows(2) {
            if pair[0].key == pair[1].key {
                return Err(DeltaError::DuplicateClaim {
                    table: pair[0].table(),
                    key: pair[0].key.to_string(),
                });
            }
        }
        Ok(Self {
            package,
            source_hash,
            claims,
        })
    }

    pub fn parse(json: &str) -> Result<Self, DeltaError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| DeltaError::Malformed(e.to_string()))?;
        let root = object(&root, "")?;

        // Version-1 Package Deltas omit kind; check it before reporting unexpected members.
        if let Some(kind) = root.get("kind") {
            let found = match kind {
                Value::String(kind) if kind == crate::script::SCRIPT_ARTIFACT_KIND => {
                    crate::script::ArtifactKind::Script
                }
                Value::String(kind) => crate::script::ArtifactKind::Other(kind.clone()),
                _ => {
                    return Err(DeltaError::WrongJsonType {
                        path: "kind".to_owned(),
                        expected: "a string",
                    })
                }
            };
            return Err(DeltaError::WrongArtifactKind {
                expected: "a Package Delta",
                found: found.to_string(),
            });
        }

        expect_members(root, "", &["claims", "package", "source_hash", "version"])?;

        let version = member(root, "", "version")?;
        let version = version.as_u64().ok_or(DeltaError::WrongJsonType {
            path: "version".to_owned(),
            expected: "a version number",
        })?;
        if version != DELTA_VERSION {
            return Err(DeltaError::UnsupportedVersion {
                found: version.to_string(),
            });
        }

        let package = PackageId::new(string(member(root, "", "package")?, "package")?)?;
        let source_hash =
            SourceHash::new(string(member(root, "", "source_hash")?, "source_hash")?)?;

        let claims = member(root, "", "claims")?
            .as_array()
            .ok_or(DeltaError::WrongJsonType {
                path: "claims".to_owned(),
                expected: "an array of claims",
            })?;
        let claims = claims
            .iter()
            .enumerate()
            .map(|(index, claim)| parse_claim(claim, index))
            .collect::<Result<Vec<_>, _>>()?;

        Self::new(package, source_hash, claims)
    }

    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        canonical::write_delta(self)
    }

    #[must_use]
    pub const fn package(&self) -> &PackageId {
        &self.package
    }

    #[must_use]
    pub const fn source_hash(&self) -> &SourceHash {
        &self.source_hash
    }

    #[must_use]
    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }

    #[must_use]
    pub fn claim_counts(&self) -> ClaimCounts {
        let mut counts = ClaimCounts::default();
        for claim in &self.claims {
            match (claim.operation(), claim.table()) {
                (Operation::Update, _) => counts.updated_rows += 1,
                (Operation::Insert, Table::Spell) => {
                    counts.inserted_spells += 1;
                    counts.inserted_rows += 1;
                }
                (Operation::Insert, Table::SpellEffect) => {
                    counts.inserted_effects += 1;
                    counts.inserted_rows += 1;
                }
                (
                    Operation::Insert,
                    Table::Item
                    | Table::Quest
                    | Table::QuestText
                    | Table::QuestObjective
                    | Table::QuestCastObjective
                    | Table::QuestRewardItem
                    | Table::QuestRewardChoice
                    | Table::PickpocketLoot
                    | Table::GameobjectLoot
                    | Table::SkinningLoot
                    | Table::FishingLoot
                    | Table::CreatureCast
                    | Table::CreatureSpell
                    | Table::TrainerSpell
                    | Table::GossipMenuProfile
                    | Table::GossipMenuProfileOption
                    | Table::GossipOption
                    | Table::NpcText
                    | Table::NpcTextSlot
                    | Table::GraveyardZone
                    | Table::CreateinfoSpell
                    | Table::CreateinfoAction
                    | Table::SpellChain
                    | Table::SpellLearn
                    | Table::SpellProcEvent
                    | Table::CreatureTemplate
                    | Table::CreatureSpawn
                    | Table::GameobjectTemplate
                    | Table::GameobjectTrap
                    | Table::GameobjectSpawn
                    | Table::CreatureAiBroadcastText
                    | Table::CreatureAiSummon
                    | Table::QuestEventRequirement
                    | Table::GossipMenu
                    | Table::ClassLevelStats
                    | Table::LevelStats
                    | Table::StartPosition
                    | Table::AreatriggerTeleport,
                ) => counts.inserted_rows += 1,
            }
        }
        counts
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaimCounts {
    pub updated_rows: u64,
    pub inserted_spells: u64,
    pub inserted_effects: u64,
    pub inserted_rows: u64,
}

fn parse_claim(value: &Value, index: usize) -> Result<Claim, DeltaError> {
    let path = format!("claims[{index}]");
    let claim = object(value, &path)?;
    expect_members(claim, &path, &["fields", "key", "operation", "table"])?;

    let table_name = string(member(claim, &path, "table")?, &format!("{path}.table"))?;
    let table = Table::parse(table_name).ok_or_else(|| DeltaError::UnknownTable {
        found: table_name.to_owned(),
    })?;

    let operation = Operation::parse(string(
        member(claim, &path, "operation")?,
        &format!("{path}.operation"),
    )?)?;

    let key = parse_key(member(claim, &path, "key")?, table)?;

    let fields_path = format!("{path}.fields");
    let fields = object(member(claim, &path, "fields")?, &fields_path)?;
    let fields = fields
        .iter()
        .map(|(name, value)| {
            parse_field_value(value, &format!("{fields_path}.{name}"), name)
                .map(|value| (name.clone(), value))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    Claim::new(key, operation, fields)
}

fn expected_key_members(table: Table) -> &'static [&'static str] {
    match table {
        Table::Spell | Table::SpellChain | Table::SpellProcEvent => &["spell_id"],
        Table::SpellEffect => &["effect_index", "spell_id"],
        Table::Item
        | Table::Quest
        | Table::GossipMenu
        | Table::CreatureTemplate
        | Table::GameobjectTemplate
        | Table::GameobjectTrap => &["entry"],
        Table::QuestText => &["quest_entry"],
        Table::QuestObjective | Table::QuestCastObjective => &["obj_index", "quest_entry"],
        Table::QuestRewardItem => &["item_entry", "quest_entry"],
        Table::QuestRewardChoice => &["choice_index", "quest_entry"],
        Table::PickpocketLoot
        | Table::GameobjectLoot
        | Table::SkinningLoot
        | Table::FishingLoot
        | Table::CreatureSpell
        | Table::TrainerSpell
        | Table::NpcTextSlot
        | Table::CreateinfoSpell
        | Table::SpellLearn
        | Table::CreatureAiBroadcastText
        | Table::CreatureAiSummon
        | Table::QuestEventRequirement => &["id"],
        Table::CreatureCast => &["creature_entry"],
        Table::GossipMenuProfile => &["menu_id"],
        Table::GossipMenuProfileOption
        | Table::GossipOption
        | Table::GraveyardZone
        | Table::CreateinfoAction => &["row_id"],
        Table::NpcText => &["text_id"],
        Table::ClassLevelStats => &["class", "level"],
        Table::LevelStats => &["class", "level", "race"],
        Table::StartPosition => &["class", "race"],
        Table::AreatriggerTeleport => &["trigger_id"],
        Table::CreatureSpawn => &["entry", "map_id", "spawn_id"],
        Table::GameobjectSpawn => &["map_id", "spawn_id"],
    }
}

fn parse_key(value: &Value, table: Table) -> Result<PrimaryKey, DeltaError> {
    let key = value.as_object().ok_or(DeltaError::MalformedKey {
        table,
        detail: "expected an object".to_owned(),
    })?;
    let expected = expected_key_members(table);
    for name in key.keys() {
        if !expected.contains(&name.as_str()) {
            return Err(DeltaError::MalformedKey {
                table,
                detail: format!("unexpected member `{name}`"),
            });
        }
    }
    for name in expected {
        if !key.contains_key(*name) {
            return Err(DeltaError::MalformedKey {
                table,
                detail: format!("missing member `{name}`"),
            });
        }
    }

    build_key(key, table)
}

fn build_key(key: &Map<String, Value>, table: Table) -> Result<PrimaryKey, DeltaError> {
    match table {
        Table::Spell => PrimaryKey::spell(key_u32(key, table, "spell_id")?),
        Table::SpellEffect => PrimaryKey::spell_effect(
            key_u32(key, table, "spell_id")?,
            key_u8(key, table, "effect_index")?,
        ),
        Table::Item => PrimaryKey::item(key_u32(key, table, "entry")?),
        Table::Quest => PrimaryKey::quest(key_u32(key, table, "entry")?),
        Table::QuestText => PrimaryKey::quest_text(key_u32(key, table, "quest_entry")?),
        Table::QuestObjective => PrimaryKey::quest_objective(
            key_u32(key, table, "quest_entry")?,
            key_u8(key, table, "obj_index")?,
        ),
        Table::QuestCastObjective => PrimaryKey::quest_cast_objective(
            key_u32(key, table, "quest_entry")?,
            key_u8(key, table, "obj_index")?,
        ),
        Table::QuestRewardItem => PrimaryKey::quest_reward_item(
            key_u32(key, table, "quest_entry")?,
            key_u32(key, table, "item_entry")?,
        ),
        Table::QuestRewardChoice => PrimaryKey::quest_reward_choice(
            key_u32(key, table, "quest_entry")?,
            key_u8(key, table, "choice_index")?,
        ),
        Table::PickpocketLoot => PrimaryKey::pickpocket_loot(key_u64(key, table, "id")?),
        Table::GameobjectLoot => PrimaryKey::gameobject_loot(key_u64(key, table, "id")?),
        Table::SkinningLoot => PrimaryKey::skinning_loot(key_u64(key, table, "id")?),
        Table::FishingLoot => PrimaryKey::fishing_loot(key_u64(key, table, "id")?),
        Table::CreatureCast => PrimaryKey::creature_cast(key_u32(key, table, "creature_entry")?),
        Table::CreatureSpell => PrimaryKey::creature_spell(key_u64(key, table, "id")?),
        Table::TrainerSpell => PrimaryKey::trainer_spell(key_u64(key, table, "id")?),
        Table::GossipMenu => PrimaryKey::gossip_menu(key_u32(key, table, "entry")?),
        Table::GossipMenuProfile => {
            PrimaryKey::gossip_menu_profile(key_u32(key, table, "menu_id")?)
        }
        Table::GossipMenuProfileOption => {
            PrimaryKey::gossip_menu_profile_option(key_u32(key, table, "row_id")?)
        }
        Table::GossipOption => PrimaryKey::gossip_option(key_u32(key, table, "row_id")?),
        Table::NpcText => PrimaryKey::npc_text(key_u32(key, table, "text_id")?),
        Table::NpcTextSlot => PrimaryKey::npc_text_slot(key_u64(key, table, "id")?),
        Table::ClassLevelStats => PrimaryKey::class_level_stats(
            key_u8(key, table, "class")?,
            key_u8(key, table, "level")?,
        ),
        Table::LevelStats => PrimaryKey::level_stats(
            key_u8(key, table, "race")?,
            key_u8(key, table, "class")?,
            key_u8(key, table, "level")?,
        ),
        Table::StartPosition => {
            PrimaryKey::start_position(key_u8(key, table, "race")?, key_u8(key, table, "class")?)
        }
        Table::GraveyardZone => PrimaryKey::graveyard_zone(key_u64(key, table, "row_id")?),
        Table::AreatriggerTeleport => {
            PrimaryKey::areatrigger_teleport(key_u32(key, table, "trigger_id")?)
        }
        Table::CreateinfoSpell => PrimaryKey::createinfo_spell(key_u64(key, table, "id")?),
        Table::CreateinfoAction => PrimaryKey::createinfo_action(key_u64(key, table, "row_id")?),
        Table::SpellChain => PrimaryKey::spell_chain(key_u32(key, table, "spell_id")?),
        Table::SpellLearn => PrimaryKey::spell_learn(key_u64(key, table, "id")?),
        Table::SpellProcEvent => PrimaryKey::spell_proc_event(key_u32(key, table, "spell_id")?),
        Table::CreatureTemplate => PrimaryKey::creature_template(key_u32(key, table, "entry")?),
        Table::CreatureSpawn => PrimaryKey::creature_spawn(
            key_u32(key, table, "map_id")?,
            key_u32(key, table, "entry")?,
            key_u32(key, table, "spawn_id")?,
        ),
        Table::GameobjectTemplate => PrimaryKey::gameobject_template(key_u32(key, table, "entry")?),
        Table::GameobjectTrap => PrimaryKey::gameobject_trap(key_u32(key, table, "entry")?),
        Table::GameobjectSpawn => PrimaryKey::gameobject_spawn(
            key_u32(key, table, "map_id")?,
            key_u32(key, table, "spawn_id")?,
        ),
        Table::CreatureAiBroadcastText => {
            PrimaryKey::creature_ai_broadcast_text(key_u32(key, table, "id")?)
        }
        Table::CreatureAiSummon => PrimaryKey::creature_ai_summon(key_u32(key, table, "id")?),
        Table::QuestEventRequirement => {
            PrimaryKey::quest_event_requirement(key_u64(key, table, "id")?)
        }
    }
}

fn key_u8(key: &Map<String, Value>, table: Table, name: &str) -> Result<u8, DeltaError> {
    key_number(key, table, name, u64::from(u8::MAX)).map(|n| n as u8)
}

fn key_u32(key: &Map<String, Value>, table: Table, name: &str) -> Result<u32, DeltaError> {
    key_number(key, table, name, u64::from(u32::MAX)).map(|n| n as u32)
}

fn key_u64(key: &Map<String, Value>, table: Table, name: &str) -> Result<u64, DeltaError> {
    key_number(key, table, name, u64::MAX)
}

fn key_number(
    key: &Map<String, Value>,
    table: Table,
    name: &str,
    max: u64,
) -> Result<u64, DeltaError> {
    let value = &key[name];
    value
        .as_u64()
        .filter(|n| *n <= max)
        .ok_or_else(|| DeltaError::MalformedKey {
            table,
            detail: format!("`{name}` must be a whole number in 0..={max}, found `{value}`"),
        })
}

fn parse_field_value(value: &Value, path: &str, field: &str) -> Result<FieldValue, DeltaError> {
    let tagged = object(value, path)?;
    expect_members(tagged, path, &["type", "value"])?;

    let tag = string(member(tagged, path, "type")?, &format!("{path}.type"))?;
    let ty = FieldType::parse(tag).ok_or_else(|| DeltaError::UnknownFieldType {
        field: field.to_owned(),
        found: tag.to_owned(),
    })?;
    let raw = member(tagged, path, "value")?;

    match ty {
        FieldType::U8 => {
            unsigned(raw, field, ty, u64::from(u8::MAX)).map(|n| FieldValue::U8(n as u8))
        }
        FieldType::U16 => {
            unsigned(raw, field, ty, u64::from(u16::MAX)).map(|n| FieldValue::U16(n as u16))
        }
        FieldType::U32 => {
            unsigned(raw, field, ty, u64::from(u32::MAX)).map(|n| FieldValue::U32(n as u32))
        }
        FieldType::U64 => unsigned_string(raw, field).map(FieldValue::U64),
        FieldType::I32 => signed(raw, field).map(FieldValue::I32),
        FieldType::F32 => float(raw, field).map(FieldValue::F32),
        FieldType::Bool => raw
            .as_bool()
            .map(FieldValue::Bool)
            .ok_or_else(|| out_of_range(raw, field, ty)),
        FieldType::Str => raw
            .as_str()
            .map(|s| FieldValue::Str(s.to_owned()))
            .ok_or_else(|| out_of_range(raw, field, ty)),
    }
}

fn unsigned(raw: &Value, field: &str, ty: FieldType, max: u64) -> Result<u64, DeltaError> {
    raw.as_u64()
        .filter(|n| *n <= max)
        .ok_or_else(|| out_of_range(raw, field, ty))
}

// JSON numbers lose integer precision above 2^53; u64 values travel as decimal strings.
fn unsigned_string(raw: &Value, field: &str) -> Result<u64, DeltaError> {
    let text = raw
        .as_str()
        .ok_or_else(|| out_of_range(raw, field, FieldType::U64))?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(out_of_range(raw, field, FieldType::U64));
    }
    text.parse::<u64>()
        .map_err(|_| out_of_range(raw, field, FieldType::U64))
}

fn signed(raw: &Value, field: &str) -> Result<i32, DeltaError> {
    raw.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| out_of_range(raw, field, FieldType::I32))
}

fn float(raw: &Value, field: &str) -> Result<f32, DeltaError> {
    let wide = raw
        .as_f64()
        .ok_or_else(|| out_of_range(raw, field, FieldType::F32))?;
    let narrow = wide as f32;
    if narrow.is_finite() {
        Ok(narrow)
    } else {
        Err(out_of_range(raw, field, FieldType::F32))
    }
}

fn out_of_range(raw: &Value, field: &str, ty: FieldType) -> DeltaError {
    DeltaError::ValueOutOfRange {
        field: field.to_owned(),
        ty,
        literal: raw.to_string(),
    }
}

pub(crate) fn object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, DeltaError> {
    value.as_object().ok_or(DeltaError::WrongJsonType {
        path: path.to_owned(),
        expected: "an object",
    })
}

pub(crate) fn string<'a>(value: &'a Value, path: &str) -> Result<&'a str, DeltaError> {
    value.as_str().ok_or(DeltaError::WrongJsonType {
        path: path.to_owned(),
        expected: "a string",
    })
}

pub(crate) fn member<'a>(
    map: &'a Map<String, Value>,
    path: &str,
    name: &str,
) -> Result<&'a Value, DeltaError> {
    map.get(name).ok_or_else(|| DeltaError::MissingMember {
        path: join(path, name),
    })
}

pub(crate) fn expect_members(
    map: &Map<String, Value>,
    path: &str,
    allowed: &[&str],
) -> Result<(), DeltaError> {
    for name in map.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(DeltaError::UnexpectedMember {
                path: if path.is_empty() {
                    "the artifact root".to_owned()
                } else {
                    path.to_owned()
                },
                member: name.clone(),
            });
        }
    }
    Ok(())
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}
