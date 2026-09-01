//! The tables a Package Delta may claim, their claimable columns, and the typed values those
//! columns take.
//!
//! The catalogue is hand-written against the Module's own table definitions. It is deliberately a
//! closed list: an unknown table or column fails the parse instead of reaching a writer that would
//! discover the problem against a live shard.
//!
//! # Import Families
//!
//! Every table belongs to exactly one Import Family, the unit a base import clears and reloads and
//! the unit a Package Delta apply runs for. [`Table`] is one closed enum whose variants are grouped
//! by family rather than a family registry, so the catalogue stays one list and every `match` on it
//! stays exhaustive.

use core::fmt;

/// The Import Family that owns the spell tables.
pub const SPELL_FAMILY: &str = "spell";

/// The Import Family that owns the item catalogue. The same name the `--dump` importer's `items`
/// `--family` block stamps, so an apply for this family lines up with the base import it follows.
pub const ITEM_FAMILY: &str = "items";

/// The Import Family that owns the quest tables. The same name the `--dump` importer's `quests`
/// `--family` block stamps.
pub const QUEST_FAMILY: &str = "quests";

/// The Import Family that owns the non-creature loot tables (pickpocket, gameobject/chest,
/// skinning, fishing). The same name the `--dump` importer's `loot` `--family` block stamps.
/// `game_creature_loot` itself reloads under the `items` family (`importer/src/main.rs`) and is
/// out of this issue's scope. See this crate's `lib.rs` for why.
pub const LOOT_FAMILY: &str = "loot";

/// The Import Family that owns the caster-creature spell tables (`game_creature_cast`,
/// `game_creature_spell`). The same name the `--dump` importer's `casts` `--family` block stamps.
pub const CAST_FAMILY: &str = "casts";

/// The Import Family that owns `game_trainer_spell`. The same name the `--dump` importer's
/// `trainers` `--family` block stamps.
pub const TRAINER_FAMILY: &str = "trainers";

/// The Import Family that owns the six gossip tables (an NPC's menu, greeting text and clickable
/// options). The same name the `--dump` importer's `gossip` `--family` block stamps.
pub const GOSSIP_FAMILY: &str = "gossip";

/// A table a Package Delta may claim rows in. The names are the durable table names, so an applier
/// needs no translation step.
///
/// The variants are grouped by Import Family, in that family's canonical order. A family arrives as
/// a block of variants; every `match` on this enum that carries no wildcard then fails to compile
/// until the new tables are handled, which is how the catalogue, the identifier rules, the key
/// shapes and the Module's setters stay in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Table {
    // ---- spell ----
    /// `game_spell` — the spell header.
    Spell,
    /// `game_spell_effect` — one effect of a spell.
    SpellEffect,
    // ---- items ----
    /// `game_item_template` — the item catalogue header.
    Item,
    // ---- quests ----
    /// `game_quest_template`, the quest header.
    Quest,
    /// `game_quest_text`, a quest's free-text body, 1:1 by `quest_entry`.
    QuestText,
    /// `game_quest_objective`, one kill/collect/use objective slot of a quest.
    QuestObjective,
    /// `game_quest_cast_objective`, the spell-cast requirement on one objective slot.
    QuestCastObjective,
    /// `game_quest_reward_item`, one guaranteed turn-in reward item.
    QuestRewardItem,
    /// `game_quest_reward_choice`, one pick-1-of-N turn-in reward item.
    QuestRewardChoice,
    // ---- loot ----
    /// `game_pickpocket_loot`, a creature's pickpocket table.
    PickpocketLoot,
    /// `game_gameobject_loot`, a lootable gameobject's chest table.
    GameobjectLoot,
    /// `game_skinning_loot`, a level-banded skinning table.
    SkinningLoot,
    /// `game_fishing_loot`, a zone's fishing table.
    FishingLoot,
    // ---- casts ----
    /// `game_creature_cast` — a caster creature's single nuke/debuff spell. Update-only: the
    /// primary key names a creature template, which no Package may invent. See
    /// [`crate::DeltaError::InsertNotSupported`].
    CreatureCast,
    /// `game_creature_spell` — one rotation-spell row of a caster creature.
    CreatureSpell,
    // ---- trainers ----
    /// `game_trainer_spell` — one spell a trainer teaches.
    TrainerSpell,
    // ---- gossip ----
    /// `game_gossip_menu` — maps a creature template entry to the `game_npc_text` row it shows.
    /// Update-only: the key is the creature template entry, which no Package may invent. See
    /// [`crate::DeltaError::InsertNotSupported`].
    GossipMenu,
    /// `game_npc_text` — an NPC greeting's first (male, back-compat) text slot.
    NpcText,
    /// `game_npc_text_slot` — one of an NPC greeting's up to 8 weighted text slots.
    NpcTextSlot,
    /// `game_gossip_option` — one clickable line in a creature's gossip window.
    GossipOption,
    /// `game_gossip_menu_profile` — a runtime-selectable gossip menu, retained by menu id.
    GossipMenuProfile,
    /// `game_gossip_menu_profile_option` — one option belonging to a runtime-selectable menu.
    GossipMenuProfileOption,
}

impl Table {
    /// Every table this build knows, in canonical order, so a refusal can name the whole catalogue
    /// and a test can walk it. Kept in step with [`Table::parse`] by `tests/families.rs`.
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
        Self::NpcText,
        Self::NpcTextSlot,
        Self::GossipOption,
        Self::GossipMenuProfile,
        Self::GossipMenuProfileOption,
    ];

    /// The durable table name, and the value the artifact's `table` member carries.
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
            Self::NpcText => "game_npc_text",
            Self::NpcTextSlot => "game_npc_text_slot",
            Self::GossipOption => "game_gossip_option",
            Self::GossipMenuProfile => "game_gossip_menu_profile",
            Self::GossipMenuProfileOption => "game_gossip_menu_profile_option",
        }
    }

    /// The Import Family that owns this table.
    ///
    /// The same name `game_import_meta.family` records, the `apply_package_deltas` reducer takes,
    /// and `game_package_import.id` carries before the slash. An applier is called for one family
    /// at a time, so this is what tells it a claim belongs to the import it is running.
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
            | Self::NpcText
            | Self::NpcTextSlot
            | Self::GossipOption
            | Self::GossipMenuProfile
            | Self::GossipMenuProfileOption => GOSSIP_FAMILY,
        }
    }

    /// Resolves a `table` member. `None` for anything outside the closed list.
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
            "game_npc_text" => Some(Self::NpcText),
            "game_npc_text_slot" => Some(Self::NpcTextSlot),
            "game_gossip_option" => Some(Self::GossipOption),
            "game_gossip_menu_profile" => Some(Self::GossipMenuProfile),
            "game_gossip_menu_profile_option" => Some(Self::GossipMenuProfileOption),
            _ => None,
        }
    }

    /// The columns a claim may name, in the table's declaration order.
    ///
    /// Primary-key columns are absent on purpose. A claim carries them in its `key`, so naming one
    /// in `fields` would let a Package state the same identifier twice and disagree with itself.
    /// A derived `id` (`game_spell_effect`, and every packed quest child key below) is absent for
    /// the same reason plus one more: it is derived, never authored.
    ///
    /// `game_creature_quest` and `game_gameobject_quest` (which creature/gameobject starts or ends
    /// a quest) are deliberately NOT in this catalogue: every one of their non-`id` columns names
    /// the relation itself (creature/gameobject, quest, START-or-END role), so a claim on them
    /// would set zero columns. This is the shape `tests/families.rs`'s
    /// `every_table_has_claimable_columns` exists to catch. Reaching a Package quest from a giver
    /// is left as a named gap for a later change, not forced past that guard.
    ///
    /// `game_creature_cast` is the borderline case that DOES stay in the catalogue: both its
    /// columns describe the creature-to-spell relation too, but `spell_id` is not a key column, so
    /// a claim on it sets a real value — retargeting which spell a caster mob uses. It is
    /// update-only for a different reason: its key names a creature template, which no Package may
    /// invent. See [`crate::DeltaError::InsertNotSupported`].
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
            Self::NpcText => NPC_TEXT_COLUMNS,
            Self::NpcTextSlot => NPC_TEXT_SLOT_COLUMNS,
            Self::GossipOption => GOSSIP_OPTION_COLUMNS,
            Self::GossipMenuProfile => GOSSIP_MENU_PROFILE_COLUMNS,
            Self::GossipMenuProfileOption => GOSSIP_MENU_PROFILE_OPTION_COLUMNS,
        }
    }

    /// The type of one claimable column, or `None` when the table has no such column.
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

/// One claimable column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// The durable column name, and the `fields` member name in the artifact.
    pub name: &'static str,
    /// The type a claim on this column must declare.
    pub ty: FieldType,
}

const fn column(name: &'static str, ty: FieldType) -> Column {
    Column { name, ty }
}

// ---- spell ----

/// `game_spell` minus its `spell_id` primary key.
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
];

/// `game_spell_effect` minus its derived `id` key and the `spell_id` / `effect_index` the key names.
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
];

// ---- items ----

/// `game_item_template` minus its `entry` primary key.
///
/// Hand-maintained against `module/src/items/tables.rs`'s `ItemTemplate` struct, the same
/// convention `SPELL_COLUMNS` follows against `module/src/spell/tables.rs` — declaration order,
/// name and type must match exactly, or `every_claimable_item_column_has_a_setter`
/// (`module/src/package_import/items.rs`) fails against a live setter.
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

// ---- quests ----

/// `game_quest_template` minus its `entry` primary key.
///
/// Hand-maintained against `module/src/quest.rs`'s `QuestTemplate` struct, the same convention
/// `ITEM_COLUMNS` follows.
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
];

/// `game_quest_text` minus its `quest_entry` primary key.
///
/// Hand-maintained against `module/src/quest.rs`'s `QuestText` struct.
const QUEST_TEXT_COLUMNS: &[Column] = &[
    column("details", FieldType::Str),
    column("objectives", FieldType::Str),
    column("offer_reward_text", FieldType::Str),
    column("request_items_text", FieldType::Str),
];

/// `game_quest_objective` minus its derived `id` key and the `quest_entry` / `obj_index` the key
/// names.
///
/// Hand-maintained against `module/src/quest.rs`'s `QuestObjective` struct.
const QUEST_OBJECTIVE_COLUMNS: &[Column] = &[
    column("kind", FieldType::U8),
    column("target_entry", FieldType::U32),
    column("required_count", FieldType::U32),
];

/// `game_quest_cast_objective` minus its derived `id` key and the `quest_entry` / `obj_index` the
/// key names.
///
/// Hand-maintained against `module/src/quest.rs`'s `QuestCastObjective` struct.
const QUEST_CAST_OBJECTIVE_COLUMNS: &[Column] = &[column("spell_id", FieldType::U32)];

/// `game_quest_reward_item` minus its derived `id` key and the `quest_entry` / `item_entry` the
/// key names.
///
/// Hand-maintained against `module/src/quest.rs`'s `QuestRewardItem` struct.
const QUEST_REWARD_ITEM_COLUMNS: &[Column] = &[column("count", FieldType::U32)];

/// `game_quest_reward_choice` minus its derived `id` key and the `quest_entry` / `choice_index`
/// the key names.
///
/// Hand-maintained against `module/src/quest.rs`'s `QuestRewardChoice` struct.
const QUEST_REWARD_CHOICE_COLUMNS: &[Column] = &[
    column("item_entry", FieldType::U32),
    column("count", FieldType::U32),
];

// ---- loot ----

/// `game_pickpocket_loot` minus its `id` primary key.
///
/// Hand-maintained against `module/src/loot/mod.rs`'s `GamePickpocketLoot` struct.
const PICKPOCKET_LOOT_COLUMNS: &[Column] = &[
    column("creature_entry", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
    column("quest_only", FieldType::Bool),
];

/// `game_gameobject_loot` minus its `id` primary key.
///
/// Hand-maintained against `module/src/loot/mod.rs`'s `GameObjectLoot` struct.
const GAMEOBJECT_LOOT_COLUMNS: &[Column] = &[
    column("loot_id", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
    column("quest_only", FieldType::Bool),
];

/// `game_skinning_loot` minus its `id` primary key. No cmangos skinning row is
/// ever quest-gated (`module/src/loot/mod.rs`).
///
/// Hand-maintained against `module/src/loot/mod.rs`'s `GameSkinningLoot` struct.
const SKINNING_LOOT_COLUMNS: &[Column] = &[
    column("skin_loot_id", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
];

/// `game_fishing_loot` minus its `id` primary key. Fishing junk is not
/// quest-gated in the family this importer imports (`module/src/loot/mod.rs`).
///
/// Hand-maintained against `module/src/loot/mod.rs`'s `GameFishingLoot` struct.
const FISHING_LOOT_COLUMNS: &[Column] = &[
    column("zone_id", FieldType::U32),
    column("item_entry", FieldType::U32),
    column("chance_bp", FieldType::U32),
    column("count", FieldType::U32),
    column("group_id", FieldType::U32),
];

// ---- casts ----

/// `game_creature_cast` minus its `creature_entry` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `CreatureCast` struct. The table is
/// update-only ([`crate::DeltaError::InsertNotSupported`]), but a claim still names a real column:
/// `spell_id` is not part of the key.
const CREATURE_CAST_COLUMNS: &[Column] = &[column("spell_id", FieldType::U32)];

/// `game_creature_spell` minus its `id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `CreatureSpell` struct.
const CREATURE_SPELL_COLUMNS: &[Column] = &[
    column("creature_entry", FieldType::U32),
    column("spell_id", FieldType::U32),
    column("priority", FieldType::U8),
    column("condition", FieldType::U8),
    column("condition_value", FieldType::U8),
];

// ---- trainers ----

/// `game_trainer_spell` minus its `id` primary key.
///
/// Hand-maintained against `module/src/trainer.rs`'s `TrainerSpell` struct. `spell_id` on a
/// profession offering (`learn_skill_line > 0`) is a synthetic marker, never a `game_spell` row —
/// see `module/src/package_import/trainers.rs`'s `check_references`.
const TRAINER_SPELL_COLUMNS: &[Column] = &[
    column("trainer_entry", FieldType::U32),
    column("spell_id", FieldType::U32),
    column("cost", FieldType::U32),
    column("required_level", FieldType::U8),
    column("learn_skill_line", FieldType::U32),
    column("learn_skill_cap", FieldType::U32),
];

// ---- gossip ----

/// `game_gossip_menu` minus its `entry` primary key (the creature template entry — the importer
/// collapsed the cmangos menu-id indirection, see `module/src/creatures/spawn.rs`'s `GossipMenu`
/// doc comment).
///
/// Update-only ([`crate::DeltaError::InsertNotSupported`]), but the one column it does carry is a
/// real claim: pointing an existing NPC at a `game_npc_text` row a Package inserted.
const GOSSIP_MENU_COLUMNS: &[Column] = &[column("text_id", FieldType::U32)];

/// `game_npc_text` minus its `text_id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `NpcText` struct.
const NPC_TEXT_COLUMNS: &[Column] = &[column("text", FieldType::Str)];

/// `game_npc_text_slot` minus its `id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `NpcTextSlot` struct.
const NPC_TEXT_SLOT_COLUMNS: &[Column] = &[
    column("text_id", FieldType::U32),
    column("slot_index", FieldType::U8),
    column("text_male", FieldType::Str),
    column("text_female", FieldType::Str),
    column("probability", FieldType::F32),
];

/// `game_gossip_option` minus its `row_id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `GossipOption` struct.
/// `action_menu_id` is stored but never checked against `game_gossip_menu_profile` at claim time —
/// see `module/src/package_import/gossip.rs`'s `check_references` doc comment for why.
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

/// `game_gossip_menu_profile` minus its `menu_id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `GossipMenuProfile` struct. Unlike
/// `game_gossip_menu.entry`, `menu_id` belongs to the gossip family alone, so a Package may invent
/// one.
const GOSSIP_MENU_PROFILE_COLUMNS: &[Column] = &[column("text_id", FieldType::U32)];

/// `game_gossip_menu_profile_option` minus its `row_id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `GossipMenuProfileOption` struct: the
/// same nine columns as [`GOSSIP_OPTION_COLUMNS`], with `menu_id` in place of `entry`.
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

/// The type tag a claimed value carries.
///
/// Every value states its type explicitly. That makes a wrong-width claim a parse failure rather
/// than a silent truncation at apply, and it is what lets `U64` travel as a decimal string: JSON has
/// one number type and it loses precision above 2^53, so an unsigned 64-bit value that went through
/// a JSON number would come back wrong with nothing to notice it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldType {
    /// JSON number, 0..=255.
    U8,
    /// JSON number, 0..=65535.
    U16,
    /// JSON number, 0..=4294967295.
    U32,
    /// JSON **string** of decimal digits, 0..=18446744073709551615.
    U64,
    /// JSON number, -2147483648..=2147483647.
    I32,
    /// JSON number, finite, representable as `f32`.
    F32,
    /// JSON boolean.
    Bool,
    /// JSON string.
    Str,
}

impl FieldType {
    /// The value the artifact's `type` member carries.
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

    /// Resolves a `type` member. `None` for anything outside the closed list.
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

/// One claimed column value, already narrowed to the column's type.
///
/// `PartialEq` is total here even though it holds an `f32`: the parser refuses a non-finite float,
/// so no `NaN` can reach a `FieldValue`. Two Packages claiming the same column with the same value
/// therefore compare equal, which is what the tracer needs to describe a conflict exactly.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// An 8-bit unsigned value.
    U8(u8),
    /// A 16-bit unsigned value.
    U16(u16),
    /// A 32-bit unsigned value.
    U32(u32),
    /// A 64-bit unsigned value. Travels as a decimal string.
    U64(u64),
    /// A 32-bit signed value.
    I32(i32),
    /// A finite 32-bit float.
    F32(f32),
    /// A boolean.
    Bool(bool),
    /// A string.
    Str(String),
}

impl FieldValue {
    /// The type tag this value serializes with.
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

/// The value in its canonical JSON spelling, without the surrounding type tag, so a conflict report
/// quotes it exactly as the artifact writes it.
impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::canonical::scalar_literal(self))
    }
}
