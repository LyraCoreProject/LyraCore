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

/// The Import Family that owns the gossip tables: the per-creature menu, the menu profiles a
/// Package or a relay selects at runtime, their options, and the NPC greeting text they resolve to.
/// The same name the `--dump` importer's `gossip` `--family` block stamps.
pub const GOSSIP_FAMILY: &str = "gossip";

/// The Import Family that owns the small world-wide reference tables: the two stat curves, the
/// start position, the graveyard zone links, the areatrigger destinations, and the two createinfo
/// tables. The same name the `--dump` importer's `globals` `--family` block stamps.
///
/// `game_start_item` is loaded by the same block and is deliberately NOT in this catalogue. That
/// table has two owners: the `--dump` globals pass owns identifiers from 1,000,000 up, and the
/// `--dbc` `CharStartOutfit` pass owns everything below. Only the first has a Package Delta stage,
/// so a claim on a `--dbc`-owned row would be reverted by a pass this family cannot follow, with
/// nothing to reapply it.
///
/// `game_graveyard` and `game_area_trigger` are NOT in it either, for a plainer reason: both are
/// `--dbc` tables, not part of this family's block at all. They are still what this family's
/// references are checked against.
pub const GLOBALS_FAMILY: &str = "globals";

/// The Import Family that owns the spell metadata tables: the rank chain, the auto-learn
/// dependents, and the proc overlay. The same name the `--dump` importer's `spellmeta` `--family`
/// block stamps.
///
/// Distinct from [`SPELL_FAMILY`], which owns the spell catalogue itself. Two of this family's
/// three tables key on a spell identifier, so they take the spell family's identifier policy while
/// still belonging to this family's apply.
pub const SPELLMETA_FAMILY: &str = "spellmeta";

/// The Import Family that owns the creature catalogue and its spawns
/// (`game_creature_template`, `game_creature_spawn`). The same name the `--dump` importer's
/// `creatures` `--family` block stamps.
///
/// `game_creature_waypoint` is loaded by the same block and is deliberately NOT in this catalogue.
/// A waypoint names its creature by spawn guid and carries no map of its own, so a claim on one
/// states no map for the routing rule every spatial claim is filtered by. Reaching a patrol from a
/// Package is a named gap, not a table this build can route.
pub const CREATURE_FAMILY: &str = "creatures";

/// The Import Family that owns the gameobject catalogue and its spawns
/// (`game_gameobject_template`, `game_gameobject_trap`, `game_gameobject`). The same name the
/// `--dump` importer's `gameobjects` `--family` block stamps.
///
/// `game_gameobject_pool` and `game_gameobject_pool_member` are NOT in it. No base import writes
/// either: they are seeded (`module/src/seed.rs`) or built by a debug reducer, so there is no
/// family reload for a pool claim to replay after, and a pool member only becomes a live node when
/// something arms the pool. Worth a maintainer's second look if authored pools are wanted.
pub const GAMEOBJECT_FAMILY: &str = "gameobjects";

/// The Import Family that owns the `EventAI` catalogue: the lines a speak action says, the placements
/// a summon action spawns at, and the quests that need their source event before ordinary
/// objectives can complete them. The same name the `--dump` importer's `creature-ai` `--family`
/// block stamps.
///
/// The family's scripted definitions are deliberately NOT in this catalogue.
/// `game_creature_ai_definition` carries a creature's whole rule set as a nested
/// `Vec<EventAiRule>`, which no scalar column shape can state; the only claim left would be one
/// opaque blob, and a Package Delta claims typed rows, never a script blob. Its base import is a
/// whole-family reducer replace rather than row SQL, so there is no row for a claim to merge into
/// either. `game_creature_ai_relay_definition` is out for both of those reasons.
///
/// `game_creature_ai_event` is out for a plainer one. It is the retained flat migration schema: no
/// import writes it and no runtime reads it (`module/src/creatures/eventai/tables.rs`), so a claim
/// there would apply cleanly and change nothing a player could see. `game_creature_ai_state` and
/// `game_creature_ai_rule_state` are live state a running Shard writes, not import data.
/// `game_creature_ai_spell_metadata` is loaded by the `--dbc` spell pass, not by this family's
/// block, so a claim on it would be reverted by an import this family cannot follow.
///
/// Reaching a creature's rules from a Package is a named gap, not a table this build can claim.
pub const CREATURE_AI_FAMILY: &str = "creature-ai";

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
    /// `game_gossip_menu` — which greeting text a creature template shows. Update-only: the
    /// primary key names a creature template, which no Package may invent. See
    /// [`crate::DeltaError::InsertNotSupported`].
    GossipMenu,
    /// `game_gossip_menu_profile` — a source gossip menu kept by menu id, for runtime selection.
    /// Insert and partial update: `menu_id` is an identifier space of its own, not a creature.
    GossipMenuProfile,
    /// `game_gossip_menu_profile_option` — one clickable line of a menu profile. Insert and
    /// partial update on its own surrogate `row_id`.
    GossipMenuProfileOption,
    /// `game_gossip_option` — one clickable line of a creature's own menu. Insert and partial
    /// update on its own surrogate `row_id`; the creature it belongs to is a claimed column, not
    /// the key.
    GossipOption,
    /// `game_npc_text` — one greeting body, by text id. Insert and partial update.
    NpcText,
    /// `game_npc_text_slot` — one weighted greeting variant of a text id. Insert and partial
    /// update on its own surrogate `id`.
    NpcTextSlot,
    // ---- globals ----
    /// `game_class_level_stats` — base health and mana for one (class, level). Update-only: the
    /// key names a class and a level the client fixes, neither of which a Package may invent.
    ClassLevelStats,
    /// `game_level_stats` — the five base attributes for one (race, class, level). Update-only,
    /// for the same reason as [`Table::ClassLevelStats`].
    LevelStats,
    /// `game_start_position` — where a fresh character of one (race, class) starts. Update-only:
    /// the key names a race and a class the client fixes.
    StartPosition,
    /// `game_graveyard_zone` — which zone an imported graveyard serves. Insert and partial update
    /// on its own surrogate `row_id`.
    GraveyardZone,
    /// `game_areatrigger_teleport` — where a portal sends the player. Update-only: the key is an
    /// `AreaTrigger.dbc` trigger id, client data no Package may invent.
    AreatriggerTeleport,
    /// `game_createinfo_spell` — one spell a fresh character of a matching (race, class) starts
    /// with. Insert and partial update on its own surrogate `id`.
    CreateinfoSpell,
    /// `game_createinfo_action` — one default action-bar button for a matching (race, class).
    /// Insert and partial update on its own surrogate `row_id`.
    CreateinfoAction,
    // ---- spellmeta ----
    /// `game_spell_chain` — one spell's rank-chain link. Insert and partial update, but the key IS
    /// a spell identifier, so an insert must name a Package spell
    /// ([`crate::PACKAGE_SPELL_ID_FLOOR`]) rather than an identifier of this family's own.
    SpellChain,
    /// `game_spell_learn` — one "learning this also teaches that" dependent. Insert and partial
    /// update on its own surrogate `id`.
    SpellLearn,
    /// `game_spell_proc_event` — the proc overlay for one spell. Insert and partial update, with
    /// the same spell-identifier policy as [`Table::SpellChain`].
    SpellProcEvent,
    // ---- creatures ----
    /// `game_creature_template` — the creature catalogue header. Insert and partial update on its
    /// own `entry`, inside the Package creature range. A global catalogue: every Shard loads the
    /// whole of it, so a claim here reaches every Shard.
    CreatureTemplate,
    /// `game_creature_spawn` — one placed creature. Insert and partial update; the key names the
    /// map, the creature template and the spawn identifier, and the durable guid is derived from
    /// the last two. SPATIAL: the map in the key is what routes the claim, so only the Shards
    /// whose World Import Scope owns that map ever see it.
    CreatureSpawn,
    // ---- gameobjects ----
    /// `game_gameobject_template` — the gameobject catalogue header. Insert and partial update on
    /// its own `entry`, inside the Package gameobject range. A global catalogue, like
    /// [`Table::CreatureTemplate`].
    GameobjectTemplate,
    /// `game_gameobject_trap` — the spell and cooldown one TRAP template fires. Insert and partial
    /// update; its key IS a gameobject template entry, so it takes the same band rather than one of
    /// its own. Global, like the template it describes.
    GameobjectTrap,
    /// `game_gameobject` — one placed gameobject. Insert and partial update; the key names the map
    /// and the spawn identifier, and the durable guid is derived from the second. SPATIAL, like
    /// [`Table::CreatureSpawn`].
    GameobjectSpawn,
    // ---- creature-ai ----
    /// `game_creature_ai_broadcast_text` — one line an `EventAI` speak action says, with the emotes
    /// it plays alongside. Insert and partial update on its own `id`, inside the Package `EventAI`
    /// band. A global catalogue: the table names no map, so a claim here reaches every Shard.
    CreatureAiBroadcastText,
    /// `game_creature_ai_summon` — where an `EventAI` summon action places its creature. Insert and
    /// partial update on its own `id`. Global, like the text above: the row is a named placement
    /// the summoning creature resolves, not a row that belongs to a map.
    CreatureAiSummon,
    /// `game_quest_event_requirement` — a quest that needs its `EventAI` source event before
    /// ordinary objectives can complete it. Insert and partial update on its own `id`. It names a
    /// quest but the `creature-ai` block loads it, so it belongs to this family: an apply that ran
    /// under `quests` would be reverted by the block that owns it.
    QuestEventRequirement,
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

/// `game_gossip_menu` minus its `entry` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `GossipMenu` struct. Update-only
/// ([`crate::DeltaError::InsertNotSupported`]), but a claim still sets a real value: retargeting
/// which greeting a creature shows.
const GOSSIP_MENU_COLUMNS: &[Column] = &[column("text_id", FieldType::U32)];

/// `game_gossip_menu_profile` minus its `menu_id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `GossipMenuProfile` struct.
const GOSSIP_MENU_PROFILE_COLUMNS: &[Column] = &[column("text_id", FieldType::U32)];

/// `game_gossip_menu_profile_option` minus its `row_id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `GossipMenuProfileOption` struct.
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

/// `game_gossip_option` minus its `row_id` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `GossipOption` struct. `entry` names
/// the creature template whose menu this line belongs to; it is a claimed column here, not part of
/// the key, so a Package may add an option to a real creature.
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

// ---- globals ----

/// `game_class_level_stats` minus its packed `class_level` key and the `class` / `level` that key
/// names.
///
/// Hand-maintained against `module/src/stats.rs`'s `ClassLevelStats` struct.
const CLASS_LEVEL_STATS_COLUMNS: &[Column] = &[
    column("base_health", FieldType::U32),
    column("base_mana", FieldType::U32),
];

/// `game_level_stats` minus its packed `race_class_level` key and the `race` / `class` / `level`
/// that key names.
///
/// Hand-maintained against `module/src/stats.rs`'s `LevelStats` struct.
const LEVEL_STATS_COLUMNS: &[Column] = &[
    column("strength", FieldType::U32),
    column("agility", FieldType::U32),
    column("stamina", FieldType::U32),
    column("intellect", FieldType::U32),
    column("spirit", FieldType::U32),
];

/// `game_start_position` minus its packed `race_class` key and the `race` / `class` that key names.
///
/// Hand-maintained against `module/src/config.rs`'s `StartPosition` struct.
const START_POSITION_COLUMNS: &[Column] = &[
    column("map_id", FieldType::U32),
    column("zone_id", FieldType::U32),
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("orientation", FieldType::F32),
    column("display_id", FieldType::U32),
];

/// `game_graveyard_zone` minus its `row_id` primary key.
///
/// Hand-maintained against `module/src/graveyard.rs`'s `GraveyardZone` struct.
const GRAVEYARD_ZONE_COLUMNS: &[Column] = &[
    column("safe_loc_id", FieldType::U32),
    column("zone_id", FieldType::U32),
    column("faction", FieldType::U32),
];

/// `game_areatrigger_teleport` minus its `trigger_id` primary key.
///
/// Hand-maintained against `module/src/quest.rs`'s `AreatriggerTeleport` struct. Update-only
/// ([`crate::DeltaError::InsertNotSupported`]), but every column here is a real value: a Package
/// may send an existing portal somewhere else.
const AREATRIGGER_TELEPORT_COLUMNS: &[Column] = &[
    column("target_map", FieldType::U32),
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("o", FieldType::F32),
    column("name", FieldType::Str),
];

/// `game_createinfo_spell` minus its `id` primary key.
///
/// Hand-maintained against `module/src/spell/spellbook.rs`'s `CreateinfoSpell` struct. `race` 0 and
/// `class` 0 are wildcards, which is why neither is bounded here beyond its own width.
const CREATEINFO_SPELL_COLUMNS: &[Column] = &[
    column("race", FieldType::U8),
    column("class", FieldType::U8),
    column("spell_id", FieldType::U32),
];

/// `game_createinfo_action` minus its `row_id` primary key.
///
/// Hand-maintained against `module/src/action_bar.rs`'s `CreateinfoAction` struct. `action` is
/// polymorphic on `action_type` (0 names a spell, other values an item or a macro), so no single
/// catalogue answers it and no reference check is made on it.
const CREATEINFO_ACTION_COLUMNS: &[Column] = &[
    column("race", FieldType::U8),
    column("class", FieldType::U8),
    column("button", FieldType::U8),
    column("action", FieldType::U32),
    column("action_type", FieldType::U8),
];

// ---- spellmeta ----

/// `game_spell_chain` minus its `spell_id` primary key.
///
/// Hand-maintained against `module/src/spell/spellbook.rs`'s `SpellChain` struct.
const SPELL_CHAIN_COLUMNS: &[Column] = &[
    column("prev_spell", FieldType::U32),
    column("first_spell", FieldType::U32),
    column("rank", FieldType::U8),
    column("req_spell", FieldType::U32),
];

/// `game_spell_learn` minus its `id` primary key.
///
/// Hand-maintained against `module/src/spell/spellbook.rs`'s `SpellLearn` struct.
const SPELL_LEARN_COLUMNS: &[Column] = &[
    column("parent_spell", FieldType::U32),
    column("learn_spell", FieldType::U32),
];

/// `game_spell_proc_event` minus its `spell_id` primary key.
///
/// Hand-maintained against `module/src/spell/tables.rs`'s `SpellProcEvent` struct.
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

// ---- creatures ----

/// `game_creature_template` minus its `entry` primary key.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `CreatureTemplate` struct, the same
/// convention `ITEM_COLUMNS` follows.
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

/// `game_creature_spawn` minus its derived `guid` key, the `map_id` and `entry` that key names,
/// and the three columns the shard owns rather than the artifact.
///
/// Hand-maintained against `module/src/creatures/spawn.rs`'s `CreatureSpawn` struct. `map_id` and
/// `entry` sit in the claim's key: the map is what routes the claim to a Shard, and the entry is
/// half the derived guid, so neither may be re-stated here and disagree with itself. `respawn_at`,
/// `despawn_at` and `life_seq` are absent because they are live state a running Shard writes — an
/// import stamps them, an artifact cannot.
const CREATURE_SPAWN_COLUMNS: &[Column] = &[
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("orientation", FieldType::F32),
    column("movement_type", FieldType::U8),
    column("respawn_secs", FieldType::U32),
];

// ---- gameobjects ----

/// `game_gameobject_template` minus its `entry` primary key.
///
/// Hand-maintained against `module/src/gameobject.rs`'s `GameObjectTemplate` struct.
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

/// `game_gameobject_trap` minus its `entry` primary key.
///
/// Hand-maintained against `module/src/gameobject.rs`'s `GameObjectTrap` struct.
const GAMEOBJECT_TRAP_COLUMNS: &[Column] = &[
    column("spell_id", FieldType::U32),
    column("cooldown_secs", FieldType::U32),
];

/// `game_gameobject` minus its derived `guid` key, the `map_id` that key names, and the columns
/// the shard owns rather than the artifact.
///
/// Hand-maintained against `module/src/gameobject.rs`'s `GameObject` struct. `created_at`,
/// `respawn_at_micros` and `instance_id` are live state. `grid_x`, `grid_y` and `cell` are DERIVED
/// from `x` and `y` in the same write (`module/src/tripwires.rs`'s grid-cell tripwire enforces it),
/// so a claim on one could put the row in a cell its coordinates do not sit in.
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

// ---- creature-ai ----

/// `game_creature_ai_broadcast_text` minus its `id` primary key.
///
/// Hand-maintained against `module/src/creatures/eventai/tables.rs`'s `CreatureAiBroadcastText`
/// struct, the same convention `ITEM_COLUMNS` follows. The three emote pairs are client emote
/// identifiers and the delays that stagger them, so a claim tunes presentation and nothing else.
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

/// `game_creature_ai_summon` minus its `id` primary key.
///
/// Hand-maintained against `module/src/creatures/eventai/tables.rs`'s `CreatureAiSummon` struct.
/// The position is absolute world space with no map beside it: the summoning creature's own map is
/// where the summon lands, which is why this table is global rather than spatial.
const CREATURE_AI_SUMMON_COLUMNS: &[Column] = &[
    column("x", FieldType::F32),
    column("y", FieldType::F32),
    column("z", FieldType::F32),
    column("orientation", FieldType::F32),
    column("lifetime_ms", FieldType::U32),
];

/// `game_quest_event_requirement` minus its `id` primary key.
///
/// Hand-maintained against `module/src/quest.rs`'s `QuestEventRequirement` struct. One claimable
/// column, and it is a reference: `module/src/package_import/creature_ai.rs` refuses a claim whose
/// quest will not be there after the plan lands.
const QUEST_EVENT_REQUIREMENT_COLUMNS: &[Column] = &[column("quest_entry", FieldType::U32)];

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
