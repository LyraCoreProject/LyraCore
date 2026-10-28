use core::fmt;

pub const SPELL_FAMILY: &str = "spell";

pub const ITEM_FAMILY: &str = "items";

pub const QUEST_FAMILY: &str = "quests";

pub const LOOT_FAMILY: &str = "loot";

pub const CAST_FAMILY: &str = "casts";

pub const TRAINER_FAMILY: &str = "trainers";

pub const GOSSIP_FAMILY: &str = "gossip";

pub const GLOBALS_FAMILY: &str = "globals";

pub const SPELLMETA_FAMILY: &str = "spellmeta";

pub const CREATURE_FAMILY: &str = "creatures";

pub const GAMEOBJECT_FAMILY: &str = "gameobjects";

pub const CREATURE_AI_FAMILY: &str = "creature-ai";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Table {
    Spell,
    SpellEffect,
    Item,
    Quest,
    QuestText,
    QuestObjective,
    QuestCastObjective,
    QuestRewardItem,
    QuestRewardChoice,
    PickpocketLoot,
    GameobjectLoot,
    SkinningLoot,
    FishingLoot,
    CreatureCast,
    CreatureSpell,
    TrainerSpell,
    GossipMenu,
    GossipMenuProfile,
    GossipMenuProfileOption,
    GossipOption,
    NpcText,
    NpcTextSlot,
    ClassLevelStats,
    LevelStats,
    StartPosition,
    GraveyardZone,
    AreatriggerTeleport,
    CreateinfoSpell,
    CreateinfoAction,
    SpellChain,
    SpellLearn,
    SpellProcEvent,
    CreatureTemplate,
    CreatureSpawn,
    GameobjectTemplate,
    GameobjectTrap,
    GameobjectSpawn,
    CreatureAiBroadcastText,
    CreatureAiSummon,
    QuestEventRequirement,
}

impl Table {
    pub const ALL: &'static [Self] = &[
        Self::Spell,
        Self::SpellEffect,
        Self::Item,
        Self::Quest,
        Self::QuestText,
        Self::QuestObjective,
        Self::QuestCastObjective,
        Self::QuestRewardItem,
        Self::QuestRewardChoice,
        Self::PickpocketLoot,
        Self::GameobjectLoot,
        Self::SkinningLoot,
        Self::FishingLoot,
        Self::CreatureCast,
        Self::CreatureSpell,
        Self::TrainerSpell,
        Self::GossipMenu,
        Self::GossipMenuProfile,
        Self::GossipMenuProfileOption,
        Self::GossipOption,
        Self::NpcText,
        Self::NpcTextSlot,
        Self::ClassLevelStats,
        Self::LevelStats,
        Self::StartPosition,
        Self::GraveyardZone,
        Self::AreatriggerTeleport,
        Self::CreateinfoSpell,
        Self::CreateinfoAction,
        Self::SpellChain,
        Self::SpellLearn,
        Self::SpellProcEvent,
        Self::CreatureTemplate,
        Self::CreatureSpawn,
        Self::GameobjectTemplate,
        Self::GameobjectTrap,
        Self::GameobjectSpawn,
        Self::CreatureAiBroadcastText,
        Self::CreatureAiSummon,
        Self::QuestEventRequirement,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spell => "game_spell",
            Self::SpellEffect => "game_spell_effect",
            Self::Item => "game_item_template",
            Self::Quest => "game_quest_template",
            Self::QuestText => "game_quest_text",
            Self::QuestObjective => "game_quest_objective",
            Self::QuestCastObjective => "game_quest_cast_objective",
            Self::QuestRewardItem => "game_quest_reward_item",
            Self::QuestRewardChoice => "game_quest_reward_choice",
            Self::PickpocketLoot => "game_pickpocket_loot",
            Self::GameobjectLoot => "game_gameobject_loot",
            Self::SkinningLoot => "game_skinning_loot",
            Self::FishingLoot => "game_fishing_loot",
            Self::CreatureCast => "game_creature_cast",
            Self::CreatureSpell => "game_creature_spell",
            Self::TrainerSpell => "game_trainer_spell",
            Self::GossipMenu => "game_gossip_menu",
            Self::GossipMenuProfile => "game_gossip_menu_profile",
            Self::GossipMenuProfileOption => "game_gossip_menu_profile_option",
            Self::GossipOption => "game_gossip_option",
            Self::NpcText => "game_npc_text",
            Self::NpcTextSlot => "game_npc_text_slot",
            Self::ClassLevelStats => "game_class_level_stats",
            Self::LevelStats => "game_level_stats",
            Self::StartPosition => "game_start_position",
            Self::GraveyardZone => "game_graveyard_zone",
            Self::AreatriggerTeleport => "game_areatrigger_teleport",
            Self::CreateinfoSpell => "game_createinfo_spell",
            Self::CreateinfoAction => "game_createinfo_action",
            Self::SpellChain => "game_spell_chain",
            Self::SpellLearn => "game_spell_learn",
            Self::SpellProcEvent => "game_spell_proc_event",
            Self::CreatureTemplate => "game_creature_template",
            Self::CreatureSpawn => "game_creature_spawn",
            Self::GameobjectTemplate => "game_gameobject_template",
            Self::GameobjectTrap => "game_gameobject_trap",
            Self::GameobjectSpawn => "game_gameobject",
            Self::CreatureAiBroadcastText => "game_creature_ai_broadcast_text",
            Self::CreatureAiSummon => "game_creature_ai_summon",
            Self::QuestEventRequirement => "game_quest_event_requirement",
        }
    }

    #[must_use]
    pub const fn family(self) -> &'static str {
        match self {
            Self::Spell | Self::SpellEffect => SPELL_FAMILY,
            Self::Item => ITEM_FAMILY,
            Self::Quest
            | Self::QuestText
            | Self::QuestObjective
            | Self::QuestCastObjective
            | Self::QuestRewardItem
            | Self::QuestRewardChoice => QUEST_FAMILY,
            Self::PickpocketLoot
            | Self::GameobjectLoot
            | Self::SkinningLoot
            | Self::FishingLoot => LOOT_FAMILY,
            Self::CreatureCast | Self::CreatureSpell => CAST_FAMILY,
            Self::TrainerSpell => TRAINER_FAMILY,
            Self::GossipMenu
            | Self::GossipMenuProfile
            | Self::GossipMenuProfileOption
            | Self::GossipOption
            | Self::NpcText
            | Self::NpcTextSlot => GOSSIP_FAMILY,
            Self::ClassLevelStats
            | Self::LevelStats
            | Self::StartPosition
            | Self::GraveyardZone
            | Self::AreatriggerTeleport
            | Self::CreateinfoSpell
            | Self::CreateinfoAction => GLOBALS_FAMILY,
            Self::SpellChain | Self::SpellLearn | Self::SpellProcEvent => SPELLMETA_FAMILY,
            Self::CreatureTemplate | Self::CreatureSpawn => CREATURE_FAMILY,
            Self::GameobjectTemplate | Self::GameobjectTrap | Self::GameobjectSpawn => {
                GAMEOBJECT_FAMILY
            }
            Self::CreatureAiBroadcastText
            | Self::CreatureAiSummon
            | Self::QuestEventRequirement => CREATURE_AI_FAMILY,
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "game_spell" => Some(Self::Spell),
            "game_spell_effect" => Some(Self::SpellEffect),
            "game_item_template" => Some(Self::Item),
            "game_quest_template" => Some(Self::Quest),
            "game_quest_text" => Some(Self::QuestText),
            "game_quest_objective" => Some(Self::QuestObjective),
            "game_quest_cast_objective" => Some(Self::QuestCastObjective),
            "game_quest_reward_item" => Some(Self::QuestRewardItem),
            "game_quest_reward_choice" => Some(Self::QuestRewardChoice),
            "game_pickpocket_loot" => Some(Self::PickpocketLoot),
            "game_gameobject_loot" => Some(Self::GameobjectLoot),
            "game_skinning_loot" => Some(Self::SkinningLoot),
            "game_fishing_loot" => Some(Self::FishingLoot),
            "game_creature_cast" => Some(Self::CreatureCast),
            "game_creature_spell" => Some(Self::CreatureSpell),
            "game_trainer_spell" => Some(Self::TrainerSpell),
            "game_gossip_menu" => Some(Self::GossipMenu),
            "game_gossip_menu_profile" => Some(Self::GossipMenuProfile),
            "game_gossip_menu_profile_option" => Some(Self::GossipMenuProfileOption),
            "game_gossip_option" => Some(Self::GossipOption),
            "game_npc_text" => Some(Self::NpcText),
            "game_npc_text_slot" => Some(Self::NpcTextSlot),
            "game_class_level_stats" => Some(Self::ClassLevelStats),
            "game_level_stats" => Some(Self::LevelStats),
            "game_start_position" => Some(Self::StartPosition),
            "game_graveyard_zone" => Some(Self::GraveyardZone),
            "game_areatrigger_teleport" => Some(Self::AreatriggerTeleport),
            "game_createinfo_spell" => Some(Self::CreateinfoSpell),
            "game_createinfo_action" => Some(Self::CreateinfoAction),
            "game_spell_chain" => Some(Self::SpellChain),
            "game_spell_learn" => Some(Self::SpellLearn),
            "game_spell_proc_event" => Some(Self::SpellProcEvent),
            "game_creature_template" => Some(Self::CreatureTemplate),
            "game_creature_spawn" => Some(Self::CreatureSpawn),
            "game_gameobject_template" => Some(Self::GameobjectTemplate),
            "game_gameobject_trap" => Some(Self::GameobjectTrap),
            "game_gameobject" => Some(Self::GameobjectSpawn),
            "game_creature_ai_broadcast_text" => Some(Self::CreatureAiBroadcastText),
            "game_creature_ai_summon" => Some(Self::CreatureAiSummon),
            "game_quest_event_requirement" => Some(Self::QuestEventRequirement),
            _ => None,
        }
    }

    #[must_use]
    pub const fn columns(self) -> &'static [Column] {
        match self {
            Self::Spell => SPELL_COLUMNS,
            Self::SpellEffect => SPELL_EFFECT_COLUMNS,
            Self::Item => ITEM_COLUMNS,
            Self::Quest => QUEST_COLUMNS,
            Self::QuestText => QUEST_TEXT_COLUMNS,
            Self::QuestObjective => QUEST_OBJECTIVE_COLUMNS,
            Self::QuestCastObjective => QUEST_CAST_OBJECTIVE_COLUMNS,
            Self::QuestRewardItem => QUEST_REWARD_ITEM_COLUMNS,
            Self::QuestRewardChoice => QUEST_REWARD_CHOICE_COLUMNS,
            Self::PickpocketLoot => PICKPOCKET_LOOT_COLUMNS,
            Self::GameobjectLoot => GAMEOBJECT_LOOT_COLUMNS,
            Self::SkinningLoot => SKINNING_LOOT_COLUMNS,
            Self::FishingLoot => FISHING_LOOT_COLUMNS,
            Self::CreatureCast => CREATURE_CAST_COLUMNS,
            Self::CreatureSpell => CREATURE_SPELL_COLUMNS,
            Self::TrainerSpell => TRAINER_SPELL_COLUMNS,
            Self::GossipMenu => GOSSIP_MENU_COLUMNS,
            Self::GossipMenuProfile => GOSSIP_MENU_PROFILE_COLUMNS,
            Self::GossipMenuProfileOption => GOSSIP_MENU_PROFILE_OPTION_COLUMNS,
            Self::GossipOption => GOSSIP_OPTION_COLUMNS,
            Self::NpcText => NPC_TEXT_COLUMNS,
            Self::NpcTextSlot => NPC_TEXT_SLOT_COLUMNS,
            Self::ClassLevelStats => CLASS_LEVEL_STATS_COLUMNS,
            Self::LevelStats => LEVEL_STATS_COLUMNS,
            Self::StartPosition => START_POSITION_COLUMNS,
            Self::GraveyardZone => GRAVEYARD_ZONE_COLUMNS,
            Self::AreatriggerTeleport => AREATRIGGER_TELEPORT_COLUMNS,
            Self::CreateinfoSpell => CREATEINFO_SPELL_COLUMNS,
            Self::CreateinfoAction => CREATEINFO_ACTION_COLUMNS,
            Self::SpellChain => SPELL_CHAIN_COLUMNS,
            Self::SpellLearn => SPELL_LEARN_COLUMNS,
            Self::SpellProcEvent => SPELL_PROC_EVENT_COLUMNS,
            Self::CreatureTemplate => CREATURE_TEMPLATE_COLUMNS,
            Self::CreatureSpawn => CREATURE_SPAWN_COLUMNS,
            Self::GameobjectTemplate => GAMEOBJECT_TEMPLATE_COLUMNS,
            Self::GameobjectTrap => GAMEOBJECT_TRAP_COLUMNS,
            Self::GameobjectSpawn => GAMEOBJECT_SPAWN_COLUMNS,
            Self::CreatureAiBroadcastText => CREATURE_AI_BROADCAST_TEXT_COLUMNS,
            Self::CreatureAiSummon => CREATURE_AI_SUMMON_COLUMNS,
            Self::QuestEventRequirement => QUEST_EVENT_REQUIREMENT_COLUMNS,
        }
    }

    /// Old Package Deltas may omit columns appended with these migration defaults.
    #[must_use]
    pub fn column_has_default(self, name: &str) -> bool {
        matches!(
            (self, name),
            (
                Self::Spell,
                "custom_flags" | "min_target_level" | "single_target_key"
            ) | (Self::SpellEffect, "bonus_coefficient")
        )
    }

    #[must_use]
    pub fn column_type(self, name: &str) -> Option<FieldType> {
        self.columns()
            .iter()
            .find(|column| column.name == name)
            .map(|column| column.ty)
    }
}

impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub name: &'static str,
    pub ty: FieldType,
}

const fn column(name: &'static str, ty: FieldType) -> Column {
    Column { name, ty }
}

const SPELL_COLUMNS: &[Column] = &[
    column("name", FieldType::Str),
    column("power_type", FieldType::U8),
    column("cost", FieldType::U32),
    column("cast_time_ms", FieldType::U32),
    column("gcd_ms", FieldType::U32),
    column("cooldown_ms", FieldType::U32),
    column("range_yd", FieldType::U32),
    column("duration_ms", FieldType::U32),
    column("school_mask", FieldType::U8),
    column("dispel_type", FieldType::U8),
    column("mechanic", FieldType::U8),
    column("max_stacks", FieldType::U8),
    column("aura_interrupt", FieldType::U16),
    column("attributes", FieldType::U32),
    column("spell_level", FieldType::U8),
    column("max_level", FieldType::U8),
    column("is_negative", FieldType::Bool),
    column("cast_flags", FieldType::U32),
    column("stances", FieldType::U8),
    column("family_name", FieldType::U8),
    column("family_flags", FieldType::U64),
    column("proc_flags", FieldType::U32),
    column("proc_chance", FieldType::U8),
    column("proc_charges", FieldType::U8),
    column("custom_flags", FieldType::U32),
    column("min_target_level", FieldType::U32),
    column("single_target_key", FieldType::U64),
];

const SPELL_EFFECT_COLUMNS: &[Column] = &[
    column("kind", FieldType::U8),
    column("base_points", FieldType::I32),
    column("die_sides", FieldType::I32),
    column("per_level", FieldType::F32),
    column("period_ms", FieldType::U32),
    column("target", FieldType::U8),
    column("radius_yd", FieldType::F32),
    column("chain_targets", FieldType::U8),
    column("trigger_spell", FieldType::U32),
    column("effect_mechanic", FieldType::U8),
    column("p0", FieldType::I32),
    column("p0_kind", FieldType::U8),
    column("p1", FieldType::I32),
    column("script_id", FieldType::U32),
    column("enters_combat", FieldType::Bool),
    column("bonus_coefficient", FieldType::F32),
];

const ITEM_COLUMNS: &[Column] = &[
    column("class", FieldType::U8),
    column("subclass", FieldType::U8),
    column("name", FieldType::Str),
    column("display_id", FieldType::U32),
    column("quality", FieldType::U8),
    column("inventory_type", FieldType::U8),
    column("item_level", FieldType::U8),
    column("required_level", FieldType::U8),
    column("max_durability", FieldType::U32),
    column("buy_price", FieldType::U32),
    column("sell_price", FieldType::U32),
    column("max_stack", FieldType::U32),
    column("damage_min", FieldType::F32),
    column("damage_max", FieldType::F32),
    column("delay_ms", FieldType::U32),
    column("stat_strength", FieldType::I32),
    column("stat_agility", FieldType::I32),
    column("stat_stamina", FieldType::I32),
    column("stat_intellect", FieldType::I32),
    column("stat_spirit", FieldType::I32),
    column("stat_crit", FieldType::I32),
    column("stat_hit", FieldType::I32),
    column("stat_armor", FieldType::I32),
    column("block_value", FieldType::I32),
    column("restores_power", FieldType::Bool),
    column("spellid_1", FieldType::U32),
    column("spelltrigger_1", FieldType::U8),
    column("spellid_2", FieldType::U32),
    column("spelltrigger_2", FieldType::U8),
    column("container_slots", FieldType::U8),
    column("sheath", FieldType::U8),
    column("bonding", FieldType::U8),
    column("holy_res", FieldType::I32),
    column("fire_res", FieldType::I32),
    column("nature_res", FieldType::I32),
    column("frost_res", FieldType::I32),
    column("shadow_res", FieldType::I32),
    column("arcane_res", FieldType::I32),
    column("spellid_3", FieldType::U32),
    column("spelltrigger_3", FieldType::U8),
    column("spellid_4", FieldType::U32),
    column("spelltrigger_4", FieldType::U8),
    column("spellid_5", FieldType::U32),
    column("spelltrigger_5", FieldType::U8),
    column("required_skill", FieldType::U32),
    column("required_skill_rank", FieldType::U32),
    column("required_reputation_faction", FieldType::U32),
    column("required_reputation_rank", FieldType::U32),
    column("max_count", FieldType::U32),
    column("item_flags", FieldType::U32),
    column("page_text", FieldType::U32),
    column("start_quest", FieldType::U32),
    column("bag_family", FieldType::U32),
    column("buy_count", FieldType::U32),
    column("food_type", FieldType::U8),
    column("allowed_class", FieldType::U32),
    column("allowed_race", FieldType::U32),
];

const QUEST_COLUMNS: &[Column] = &[
    column("min_level", FieldType::U32),
    column("quest_level", FieldType::U32),
    column("title", FieldType::Str),
    column("reward_money", FieldType::U32),
    column("reward_xp", FieldType::U32),
    column("prev_quest_id", FieldType::U32),
    column("required_races", FieldType::U32),
    column("required_classes", FieldType::U32),
    column("zone_or_sort", FieldType::I32),
    column("rew_rep_faction_1", FieldType::U32),
    column("rew_rep_value_1", FieldType::I32),
    column("rew_rep_faction_2", FieldType::U32),
    column("rew_rep_value_2", FieldType::I32),
    column("src_item", FieldType::U32),
    column("src_item_count", FieldType::U32),
    column("repeatable", FieldType::Bool),
    column("next_quest_id", FieldType::U32),
    column("limit_time", FieldType::U32),
    column("reward_money_max_level", FieldType::U32),
    column("quest_type", FieldType::U32),
];

const QUEST_TEXT_COLUMNS: &[Column] = &[
    column("details", FieldType::Str),
    column("objectives", FieldType::Str),
    column("offer_reward_text", FieldType::Str),
    column("request_items_text", FieldType::Str),
];

const QUEST_OBJECTIVE_COLUMNS: &[Column] = &[
    column("kind", FieldType::U8),
    column("target_entry", FieldType::U32),
    column("required_count", FieldType::U32),
];

const QUEST_CAST_OBJECTIVE_COLUMNS: &[Column] = &[column("spell_id", FieldType::U32)];

const QUEST_REWARD_ITEM_COLUMNS: &[Column] = &[column("count", FieldType::U32)];

const QUEST_REWARD_CHOICE_COLUMNS: &[Column] = &[
    column("item_entry", FieldType::U32),
    column("count", FieldType::U32),
];

const PICKPOCKET_LOOT_COLUMNS: &[Column] = &[
    column("creature_entry", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
    column("quest_only", FieldType::Bool),
];

const GAMEOBJECT_LOOT_COLUMNS: &[Column] = &[
    column("loot_id", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
    column("quest_only", FieldType::Bool),
];

const SKINNING_LOOT_COLUMNS: &[Column] = &[
    column("skin_loot_id", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
];

const FISHING_LOOT_COLUMNS: &[Column] = &[
    column("zone_id", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
];

const CREATURE_CAST_COLUMNS: &[Column] = &[column("spell_id", FieldType::U32)];

const CREATURE_SPELL_COLUMNS: &[Column] = &[
    column("creature_entry", FieldType::U32),
    column("spell_id", FieldType::U32),
    column("priority", FieldType::U8),
    column("condition", FieldType::U8),
    column("condition_value", FieldType::U8),
];

// Profession spell_id is a synthetic marker, not a game_spell reference.
const TRAINER_SPELL_COLUMNS: &[Column] = &[
    column("trainer_entry", FieldType::U32),
    column("spell_id", FieldType::U32),
    column("cost", FieldType::U32),
    column("required_level", FieldType::U8),
    column("learn_skill_line", FieldType::U32),
    column("learn_skill_cap", FieldType::U32),
];

const GOSSIP_MENU_COLUMNS: &[Column] = &[column("text_id", FieldType::U32)];

const GOSSIP_MENU_PROFILE_COLUMNS: &[Column] = &[column("text_id", FieldType::U32)];

const GOSSIP_MENU_PROFILE_OPTION_COLUMNS: &[Column] = &[
    column("menu_id", FieldType::U32),
    column("option_index", FieldType::U32),
    column("icon", FieldType::U32),
    column("text", FieldType::Str),
    column("action", FieldType::U32),
    column("action_menu_id", FieldType::U32),
    column("cond_type", FieldType::U32),
    column("cond_value1", FieldType::U32),
    column("cond_value2", FieldType::U32),
];

const GOSSIP_OPTION_COLUMNS: &[Column] = &[
    column("entry", FieldType::U32),
    column("option_index", FieldType::U32),
    column("icon", FieldType::U32),
    column("text", FieldType::Str),
    column("action", FieldType::U32),
    column("action_menu_id", FieldType::U32),
    column("cond_type", FieldType::U32),
    column("cond_value1", FieldType::U32),
    column("cond_value2", FieldType::U32),
];

const NPC_TEXT_COLUMNS: &[Column] = &[column("text", FieldType::Str)];

const NPC_TEXT_SLOT_COLUMNS: &[Column] = &[
    column("text_id", FieldType::U32),
    column("slot_index", FieldType::U8),
    column("text_male", FieldType::Str),
    column("text_female", FieldType::Str),
    column("probability", FieldType::F32),
];

const CLASS_LEVEL_STATS_COLUMNS: &[Column] = &[
    column("base_health", FieldType::U32),
    column("base_mana", FieldType::U32),
];

const LEVEL_STATS_COLUMNS: &[Column] = &[
    column("strength", FieldType::U32),
    column("agility", FieldType::U32),
    column("stamina", FieldType::U32),
    column("intellect", FieldType::U32),
    column("spirit", FieldType::U32),
];

const START_POSITION_COLUMNS: &[Column] = &[
    column("map_id", FieldType::U32),
    column("zone_id", FieldType::U32),
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("orientation", FieldType::F32),
    column("display_id", FieldType::U32),
];

const GRAVEYARD_ZONE_COLUMNS: &[Column] = &[
    column("safe_loc_id", FieldType::U32),
    column("zone_id", FieldType::U32),
    column("faction", FieldType::U32),
];

const AREATRIGGER_TELEPORT_COLUMNS: &[Column] = &[
    column("target_map", FieldType::U32),
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("o", FieldType::F32),
    column("name", FieldType::Str),
];

// Race 0 and class 0 are wildcards.
const CREATEINFO_SPELL_COLUMNS: &[Column] = &[
    column("race", FieldType::U8),
    column("class", FieldType::U8),
    column("spell_id", FieldType::U32),
];

// action_type selects whether action names a spell, item or macro.
const CREATEINFO_ACTION_COLUMNS: &[Column] = &[
    column("race", FieldType::U8),
    column("class", FieldType::U8),
    column("button", FieldType::U8),
    column("action", FieldType::U32),
    column("action_type", FieldType::U8),
];

const SPELL_CHAIN_COLUMNS: &[Column] = &[
    column("prev_spell", FieldType::U32),
    column("first_spell", FieldType::U32),
    column("rank", FieldType::U8),
    column("req_spell", FieldType::U32),
];

const SPELL_LEARN_COLUMNS: &[Column] = &[
    column("parent_spell", FieldType::U32),
    column("learn_spell", FieldType::U32),
];

const SPELL_PROC_EVENT_COLUMNS: &[Column] = &[
    column("proc_flags", FieldType::U32),
    column("proc_ex", FieldType::U32),
    column("school_mask", FieldType::U8),
    column("family_name", FieldType::U8),
    column("family_flags", FieldType::U64),
    column("ppm_rate", FieldType::F32),
    column("custom_chance", FieldType::U8),
    column("icd_ms", FieldType::U32),
];

const CREATURE_TEMPLATE_COLUMNS: &[Column] = &[
    column("name", FieldType::Str),
    column("subname", FieldType::Str),
    column("display_id", FieldType::U32),
    column("level", FieldType::U32),
    column("health", FieldType::U32),
    column("faction_template", FieldType::U32),
    column("npc_flags", FieldType::U32),
    column("unit_flags", FieldType::U32),
    column("creature_type", FieldType::U8),
    column("creature_family", FieldType::U8),
    column("type_flags", FieldType::U32),
    column("rank", FieldType::U8),
    column("scale", FieldType::F32),
    column("base_attack_time_ms", FieldType::U32),
    column("money_min", FieldType::U32),
    column("money_max", FieldType::U32),
    column("max_level", FieldType::U32),
    column("max_level_health", FieldType::U32),
    column("aggro_range", FieldType::U32),
    column("damage_min", FieldType::U32),
    column("damage_max", FieldType::U32),
    column("armor", FieldType::U32),
    column("pickpocket_loot_id", FieldType::U32),
    column("skin_loot_id", FieldType::U32),
    column("trainer_type", FieldType::U8),
    column("trainer_class", FieldType::U8),
];

// Spawn lifetime columns belong to the running Shard and cannot be claimed.
const CREATURE_SPAWN_COLUMNS: &[Column] = &[
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("orientation", FieldType::F32),
    column("movement_type", FieldType::U8),
    column("respawn_secs", FieldType::U32),
];

const GAMEOBJECT_TEMPLATE_COLUMNS: &[Column] = &[
    column("type_id", FieldType::U8),
    column("display_id", FieldType::U32),
    column("name", FieldType::Str),
    column("data0", FieldType::U32),
    column("data1", FieldType::U32),
    column("gather_skill_line", FieldType::U32),
    column("respawn_secs", FieldType::U32),
    column("gather_gray", FieldType::U32),
    column("lock_id", FieldType::U32),
    column("size", FieldType::F32),
];

const GAMEOBJECT_TRAP_COLUMNS: &[Column] = &[
    column("spell_id", FieldType::U32),
    column("cooldown_secs", FieldType::U32),
];

// Grid columns derive from coordinates in the same write and cannot be claimed.
const GAMEOBJECT_SPAWN_COLUMNS: &[Column] = &[
    column("template_entry", FieldType::U32),
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("orientation", FieldType::F32),
    column("state", FieldType::U8),
    column("rotation_0", FieldType::F32),
    column("rotation_1", FieldType::F32),
    column("rotation_2", FieldType::F32),
    column("rotation_3", FieldType::F32),
];

const CREATURE_AI_BROADCAST_TEXT_COLUMNS: &[Column] = &[
    column("male_text", FieldType::Str),
    column("female_text", FieldType::Str),
    column("chat_type", FieldType::U8),
    column("language_id", FieldType::U8),
    column("emote_delay_1_ms", FieldType::U32),
    column("emote_id_1", FieldType::U32),
    column("emote_delay_2_ms", FieldType::U32),
    column("emote_id_2", FieldType::U32),
    column("emote_delay_3_ms", FieldType::U32),
    column("emote_id_3", FieldType::U32),
];

const CREATURE_AI_SUMMON_COLUMNS: &[Column] = &[
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("orientation", FieldType::F32),
    column("lifetime_ms", FieldType::U32),
];

const QUEST_EVENT_REQUIREMENT_COLUMNS: &[Column] = &[column("quest_entry", FieldType::U32)];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldType {
    U8,
    U16,
    U32,
    U64,
    I32,
    F32,
    Bool,
    Str,
}

impl FieldType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I32 => "i32",
            Self::F32 => "f32",
            Self::Bool => "bool",
            Self::Str => "string",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "u8" => Some(Self::U8),
            "u16" => Some(Self::U16),
            "u32" => Some(Self::U32),
            "u64" => Some(Self::U64),
            "i32" => Some(Self::I32),
            "f32" => Some(Self::F32),
            "bool" => Some(Self::Bool),
            "string" => Some(Self::Str),
            _ => None,
        }
    }
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I32(i32),
    F32(f32),
    Bool(bool),
    Str(String),
}

impl FieldValue {
    #[must_use]
    pub const fn field_type(&self) -> FieldType {
        match self {
            Self::U8(_) => FieldType::U8,
            Self::U16(_) => FieldType::U16,
            Self::U32(_) => FieldType::U32,
            Self::U64(_) => FieldType::U64,
            Self::I32(_) => FieldType::I32,
            Self::F32(_) => FieldType::F32,
            Self::Bool(_) => FieldType::Bool,
            Self::Str(_) => FieldType::Str,
        }
    }
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::canonical::scalar_literal(self))
    }
}
