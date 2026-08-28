//! The Package Delta artifact: its types, its one refusing constructor per level, and the parser
//! that builds it from untrusted JSON.
//!
//! Nothing in this module writes anywhere. A `PackageDelta` that exists has already passed every
//! check the artifact promises, so an applier reads it without re-deciding anything.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::canonical;
use crate::error::DeltaError;
use crate::ids::{
    is_fixture_reserved_item_id, is_fixture_reserved_loot_id, is_fixture_reserved_quest_id,
    is_fixture_reserved_spell_id, is_package_item_id, is_package_loot_id, is_package_quest_id,
    is_package_spell_id, packed_quest_objective_id, packed_quest_reward_choice_id,
    packed_quest_reward_item_id, packed_spell_effect_id, MAX_QUEST_OBJECTIVE_INDEX,
    MAX_QUEST_REWARD_CHOICE_INDEX, MAX_SPELL_EFFECT_INDEX,
};
use crate::schema::{FieldType, FieldValue, Table};

/// The artifact version this build reads and writes.
pub const DELTA_VERSION: u64 = 1;

/// The identity of the Package a claim came from.
///
/// Lowercase on purpose: two Packages differing only in case would canonicalize to two rows that
/// look like the same Package in a conflict report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(String);

impl PackageId {
    /// Accepts 1 to 64 characters of `a-z`, `0-9`, `-`, `_` or `.`.
    ///
    /// # Errors
    /// [`DeltaError::InvalidPackageId`] when the identity is empty, too long, or uses any other
    /// character.
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

    /// The identity as written.
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

/// The digest of the Datascript source a Package Delta was generated from.
///
/// The artifact only carries it; nothing here recomputes it. It exists so a replay can tell that a
/// Package's source changed without diffing every claim, and so a conflict report can name the
/// source that produced a claim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceHash(String);

impl SourceHash {
    /// Accepts exactly 64 lowercase hexadecimal characters, the shape of the BLAKE3 digest the
    /// importer already uses.
    ///
    /// # Errors
    /// [`DeltaError::InvalidSourceHash`] for any other length or character.
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

    /// The digest as written.
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

/// What a claim does to its row.
///
/// There is no `Delete`. Uninstalling a Package replays the affected families without its claims, so
/// there is no pre-image a delete could be undone from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    /// The row does not exist yet; the claim carries every column.
    Insert,
    /// The row exists; the claim carries only the columns it changes.
    Update,
}

impl Operation {
    /// The value the artifact's `operation` member carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
        }
    }

    /// Resolves an `operation` member.
    ///
    /// # Errors
    /// [`DeltaError::DeleteNotSupported`] for `delete`, which is a refusal rather than a typo, and
    /// [`DeltaError::UnknownOperation`] for anything else.
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

/// The row a claim names, typed by its table.
///
/// One variant per [`Table`] variant, in the same family-grouped order, so the derived ordering is
/// the canonical one: `game_spell` before `game_spell_effect`, then by spell, then by effect index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimaryKey {
    // ---- spell ----
    /// A `game_spell` header row.
    Spell {
        /// The spell.
        spell_id: u32,
    },
    /// A `game_spell_effect` child row. The durable key is derived, never authored.
    SpellEffect {
        /// The spell the effect belongs to.
        spell_id: u32,
        /// Which of the spell's effects, 0..=[`MAX_SPELL_EFFECT_INDEX`].
        effect_index: u8,
    },
    // ---- items ----
    /// A `game_item_template` row.
    Item {
        /// The item.
        entry: u32,
    },
    // ---- quests ----
    /// A `game_quest_template` header row.
    Quest {
        /// The quest.
        entry: u32,
    },
    /// A `game_quest_text` row. 1:1 with its quest by `quest_entry`.
    QuestText {
        /// The quest this text belongs to.
        quest_entry: u32,
    },
    /// A `game_quest_objective` row. The durable key is derived, never authored.
    QuestObjective {
        /// The quest this objective belongs to.
        quest_entry: u32,
        /// Which objective slot, 0..=[`MAX_QUEST_OBJECTIVE_INDEX`].
        obj_index: u8,
    },
    /// A `game_quest_cast_objective` row. The durable key is derived, never authored.
    QuestCastObjective {
        /// The quest this cast requirement belongs to.
        quest_entry: u32,
        /// Which objective slot, 0..=[`MAX_QUEST_OBJECTIVE_INDEX`].
        obj_index: u8,
    },
    /// A `game_quest_reward_item` row. The durable key is derived, never authored.
    QuestRewardItem {
        /// The quest this reward belongs to.
        quest_entry: u32,
        /// The rewarded item. A quest rewards one item at most once, so this is the natural key.
        item_entry: u32,
    },
    /// A `game_quest_reward_choice` row. The durable key is derived, never authored.
    QuestRewardChoice {
        /// The quest this reward belongs to.
        quest_entry: u32,
        /// Which choice slot, 0..=[`MAX_QUEST_REWARD_CHOICE_INDEX`].
        choice_index: u8,
    },
    // ---- loot ----
    /// A `game_pickpocket_loot` row.
    PickpocketLoot {
        /// The row.
        id: u64,
    },
    /// A `game_gameobject_loot` row.
    GameobjectLoot {
        /// The row.
        id: u64,
    },
    /// A `game_skinning_loot` row.
    SkinningLoot {
        /// The row.
        id: u64,
    },
    /// A `game_fishing_loot` row.
    FishingLoot {
        /// The row.
        id: u64,
    },
}

impl PrimaryKey {
    /// Names a spell header row.
    ///
    /// # Errors
    /// [`DeltaError::MalformedKey`] for spell 0, and [`DeltaError::SpellIdFixtureReserved`] for a
    /// seeded fixture spell.
    pub fn spell(spell_id: u32) -> Result<Self, DeltaError> {
        check_claimable_spell_id(Table::Spell, spell_id)?;
        Ok(Self::Spell { spell_id })
    }

    /// Names one effect of a spell. The packed durable key follows from the two components.
    ///
    /// # Errors
    /// The same identifier refusals as [`PrimaryKey::spell`], plus
    /// [`DeltaError::EffectIndexOutOfRange`] for a slot no spell has.
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

    /// Names an item row.
    ///
    /// # Errors
    /// [`DeltaError::MalformedKey`] for entry 0, and [`DeltaError::ItemIdFixtureReserved`] for a
    /// seeded fixture item.
    pub fn item(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_item_id(entry)?;
        Ok(Self::Item { entry })
    }

    /// Names a quest header row.
    ///
    /// # Errors
    /// [`DeltaError::MalformedKey`] for quest 0, and [`DeltaError::QuestIdFixtureReserved`] for a
    /// seeded fixture quest.
    pub fn quest(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::Quest, entry)?;
        Ok(Self::Quest { entry })
    }

    /// Names a quest's text row. Shares the quest's own identifier policy: it is 1:1 with the
    /// header by `quest_entry`, not a separately banded child.
    ///
    /// # Errors
    /// Same as [`PrimaryKey::quest`].
    pub fn quest_text(quest_entry: u32) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::QuestText, quest_entry)?;
        Ok(Self::QuestText { quest_entry })
    }

    /// Names one objective slot of a quest. The packed durable key follows from the two
    /// components.
    ///
    /// # Errors
    /// The same identifier refusals as [`PrimaryKey::quest`], plus
    /// [`DeltaError::QuestObjectiveIndexOutOfRange`] for a slot no quest has.
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

    /// Names the spell-cast requirement on one objective slot of a quest.
    ///
    /// # Errors
    /// Same as [`PrimaryKey::quest_objective`].
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

    /// Names a guaranteed reward-item row. The packed durable key follows from the two
    /// components.
    ///
    /// # Errors
    /// The same identifier refusals as [`PrimaryKey::quest`], applied to `quest_entry`.
    /// `item_entry` names the OTHER side of the relation (a `game_item_template` row) and is not
    /// itself checked here. The items family's own policy governs whether that row exists.
    pub fn quest_reward_item(quest_entry: u32, item_entry: u32) -> Result<Self, DeltaError> {
        check_claimable_quest_id(Table::QuestRewardItem, quest_entry)?;
        Ok(Self::QuestRewardItem {
            quest_entry,
            item_entry,
        })
    }

    /// Names one choice-reward slot of a quest. The packed durable key follows from the two
    /// components.
    ///
    /// # Errors
    /// The same identifier refusals as [`PrimaryKey::quest`], plus
    /// [`DeltaError::QuestRewardChoiceIndexOutOfRange`] for a slot no quest has.
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

    /// Names a pickpocket-loot row.
    ///
    /// # Errors
    /// [`DeltaError::MalformedKey`] for id 0, and [`DeltaError::LootIdFixtureReserved`] for a
    /// seeded fixture row.
    pub fn pickpocket_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::PickpocketLoot, id)?;
        Ok(Self::PickpocketLoot { id })
    }

    /// Names a gameobject (chest) loot row.
    ///
    /// # Errors
    /// Same as [`PrimaryKey::pickpocket_loot`].
    pub fn gameobject_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::GameobjectLoot, id)?;
        Ok(Self::GameobjectLoot { id })
    }

    /// Names a skinning-loot row.
    ///
    /// # Errors
    /// Same as [`PrimaryKey::pickpocket_loot`].
    pub fn skinning_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::SkinningLoot, id)?;
        Ok(Self::SkinningLoot { id })
    }

    /// Names a fishing-loot row.
    ///
    /// # Errors
    /// Same as [`PrimaryKey::pickpocket_loot`].
    pub fn fishing_loot(id: u64) -> Result<Self, DeltaError> {
        check_claimable_loot_id(Table::FishingLoot, id)?;
        Ok(Self::FishingLoot { id })
    }

    /// The table this row lives in.
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
        }
    }

    /// The spell this row belongs to, for a spell-family key.
    ///
    /// Every caller sits inside the spell family's own dispatch (`module/src/package_import/
    /// spell.rs`), where `check_claims_belong_to` has already refused a foreign-family row before
    /// this could run — so a foreign-family key reaching here is an internal invariant break, not
    /// untrusted input.
    ///
    /// # Panics
    /// If called on a non-spell key.
    #[must_use]
    pub fn spell_id(self) -> u32 {
        match self {
            Self::Spell { spell_id } | Self::SpellEffect { spell_id, .. } => spell_id,
            _ => unreachable!(
                "`spell_id` is a spell-family accessor; a foreign-family key never reaches it"
            ),
        }
    }

    /// The quest this row belongs to, for a quest-family key. The header's own `entry` counts,
    /// every child table names the same quest by `quest_entry`, so this is the one identifier a
    /// quest-family row is always Package-owned or not through.
    ///
    /// Every caller sits inside the quest family's own dispatch
    /// (`module/src/package_import/quest.rs`), where `check_claims_belong_to` has already refused
    /// a foreign-family row before this could run, so a foreign-family key reaching here is an
    /// internal invariant break, not untrusted input.
    ///
    /// # Panics
    /// If called on a non-quest key.
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

    /// The durable primary-key value: `spell_id` for a header, the packed
    /// `(spell_id << 2) | effect_index` for an effect, `entry` for an item.
    #[must_use]
    pub const fn row_id(self) -> u64 {
        match self {
            Self::Spell { spell_id } => spell_id as u64,
            Self::SpellEffect {
                spell_id,
                effect_index,
            } => packed_spell_effect_id(spell_id, effect_index),
            Self::Item { entry } | Self::Quest { entry } => entry as u64,
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
            | Self::FishingLoot { id } => id,
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
            Self::Item { entry } | Self::Quest { entry } => write!(f, "{{entry={entry}}}"),
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
            | Self::FishingLoot { id } => write!(f, "{{id={id}}}"),
        }
    }
}

/// Refuses an `insert` at an identifier outside its family's Package band.
///
/// The band is a per-family policy, so this match carries one arm per family and no wildcard: a
/// family that arrives without stating its band cannot compile.
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
    }
}

/// A spell identifier is refused the same way under every operation when it is 0 or fixture-owned.
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

/// A quest identifier is refused the same way under every operation, on every table that names
/// one, when it is 0 or fixture-owned.
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

/// A loot row identifier is refused the same way under every operation when it is 0 or
/// fixture-owned.
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

/// An item identifier is refused the same way under every operation when it is 0 or fixture-owned.
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

/// One Package's claim on one row.
///
/// The table follows from the key, so a claim cannot name a table its key does not describe.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    key: PrimaryKey,
    operation: Operation,
    fields: BTreeMap<String, FieldValue>,
}

impl Claim {
    /// Builds a claim, refusing everything the artifact promises it will refuse.
    ///
    /// The identifier policy is operation-dependent, and this is the only place it is applied:
    ///
    ///  * An `insert` invents a row, so its identifier must sit in its family's Package band — for
    ///    a spell, the Package spell range ([`is_package_spell_id`]). Nothing else is safe: a real
    ///    client identifier would collide with the next base import.
    ///  * An `update` changes a row someone else owns, so any identifier is allowed except a
    ///    fixture-reserved one, which [`PrimaryKey`] already refused. Tuning real spells is the
    ///    common case and must stay possible.
    ///
    /// # Errors
    /// [`DeltaError::SpellIdNotClientSafe`] for an out-of-range insert;
    /// [`DeltaError::KeyColumnClaimed`], [`DeltaError::UnknownField`] or
    /// [`DeltaError::FieldTypeMismatch`] for a bad column; [`DeltaError::IncompleteInsert`] for a
    /// partial insert; [`DeltaError::EmptyUpdate`] for an update that states no change.
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

        match operation {
            Operation::Insert => {
                let missing: Vec<String> = table
                    .columns()
                    .iter()
                    .filter(|column| !fields.contains_key(column.name))
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

    /// The row this claim names.
    #[must_use]
    pub const fn key(&self) -> PrimaryKey {
        self.key
    }

    /// The table this claim's row lives in.
    #[must_use]
    pub const fn table(&self) -> Table {
        self.key.table()
    }

    /// What this claim does to the row.
    #[must_use]
    pub const fn operation(&self) -> Operation {
        self.operation
    }

    /// The claimed columns, in canonical (name-sorted) order.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<String, FieldValue> {
        &self.fields
    }
}

/// True for a column that is part of the row's primary key, and therefore never claimable.
fn is_key_column(table: Table, name: &str) -> bool {
    match table {
        Table::Spell => name == "spell_id",
        Table::SpellEffect => matches!(name, "id" | "spell_id" | "effect_index"),
        Table::Item | Table::Quest => name == "entry",
        Table::QuestText => name == "quest_entry",
        Table::QuestObjective | Table::QuestCastObjective => {
            matches!(name, "id" | "quest_entry" | "obj_index")
        }
        Table::QuestRewardItem => matches!(name, "id" | "quest_entry" | "item_entry"),
        Table::QuestRewardChoice => matches!(name, "id" | "quest_entry" | "choice_index"),
        Table::PickpocketLoot
        | Table::GameobjectLoot
        | Table::SkinningLoot
        | Table::FishingLoot => name == "id",
    }
}

/// One Package's whole set of claims: its identity, the source it was generated from, and the rows
/// it changes.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageDelta {
    package: PackageId,
    source_hash: SourceHash,
    claims: Vec<Claim>,
}

impl PackageDelta {
    /// Builds a delta and puts its claims in canonical order.
    ///
    /// # Errors
    /// [`DeltaError::DuplicateClaim`] when the Package names one row twice. A Package states each
    /// row once and lists every column it sets there, so ordering the claims can never lose one.
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

    /// Reads an artifact from untrusted JSON.
    ///
    /// Every refusal happens here, before any caller could act on a claim.
    ///
    /// # Errors
    /// [`DeltaError::WrongArtifactKind`] when the bytes are another artifact kind — a Package ships
    /// every kind it has into one directory, so a reader walking `*.json` meets them all and has to
    /// be told which it is holding rather than shown a complaint about a member. Otherwise any
    /// [`DeltaError`] the members raise; the parse stops at the first problem.
    pub fn parse(json: &str) -> Result<Self, DeltaError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| DeltaError::Malformed(e.to_string()))?;
        let root = object(&root, "")?;

        // A Package Delta carries no `kind`: version 1 shipped before there was a second kind to
        // tell it from, and adding one now would change every artifact's canonical bytes. So a
        // `kind` member is proof these bytes are NOT this artifact, and it is checked before the
        // closed member list below, which would otherwise report it as an unexpected member.
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

    /// Writes the artifact back as canonical JSON.
    ///
    /// Byte-stable: two artifacts that say the same thing produce the same bytes, whatever member
    /// order, whitespace or number spelling they were written with.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        canonical::write_delta(self)
    }

    /// The Package this delta belongs to.
    #[must_use]
    pub const fn package(&self) -> &PackageId {
        &self.package
    }

    /// The digest of the Datascript source this delta was generated from.
    #[must_use]
    pub const fn source_hash(&self) -> &SourceHash {
        &self.source_hash
    }

    /// The claims, in canonical order.
    #[must_use]
    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }

    /// How many rows of each kind this Package claims.
    ///
    /// Counted from the claims alone, so it describes this Package on its own — an applier records
    /// it as provenance and a dry-run check prints it, and both read the same numbers.
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
                    | Table::FishingLoot,
                ) => counts.inserted_rows += 1,
            }
        }
        counts
    }
}

/// How many rows of each kind one Package Delta claims.
///
/// These are the counts `game_package_import` records, so the field names are durable column names
/// and a family adds counters rather than renaming these. [`PackageDelta::claim_counts`] tallies
/// them with a match over every table, which is where a new family states what its inserts count
/// as.
///
/// `inserted_spells`/`inserted_effects` are the spell family's own counts, kept under their
/// original names rather than renamed for a later family. `inserted_rows` is the family-generic
/// successor: every family's inserts count here, spell's included, so a reader that knows only this
/// column still sees the whole picture. A non-spell family (items today) leaves the legacy pair at
/// zero and reports through `inserted_rows` alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaimCounts {
    /// Rows the Package changes but does not own, across every table.
    pub updated_rows: u64,
    /// `game_spell` rows the Package invents.
    pub inserted_spells: u64,
    /// `game_spell_effect` rows the Package invents.
    pub inserted_effects: u64,
    /// Rows the Package invents, across every table its family owns.
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

fn parse_key(value: &Value, table: Table) -> Result<PrimaryKey, DeltaError> {
    let key = value.as_object().ok_or(DeltaError::MalformedKey {
        table,
        detail: "expected an object".to_owned(),
    })?;
    let expected: &[&str] = match table {
        Table::Spell => &["spell_id"],
        Table::SpellEffect => &["effect_index", "spell_id"],
        Table::Item | Table::Quest => &["entry"],
        Table::QuestText => &["quest_entry"],
        Table::QuestObjective | Table::QuestCastObjective => &["obj_index", "quest_entry"],
        Table::QuestRewardItem => &["item_entry", "quest_entry"],
        Table::QuestRewardChoice => &["choice_index", "quest_entry"],
        Table::PickpocketLoot
        | Table::GameobjectLoot
        | Table::SkinningLoot
        | Table::FishingLoot => &["id"],
    };
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

    match table {
        Table::Spell => {
            let spell_id = key_number(key, table, "spell_id", u64::from(u32::MAX))? as u32;
            PrimaryKey::spell(spell_id)
        }
        Table::SpellEffect => {
            let spell_id = key_number(key, table, "spell_id", u64::from(u32::MAX))? as u32;
            let effect_index = key_number(key, table, "effect_index", u64::from(u8::MAX))? as u8;
            PrimaryKey::spell_effect(spell_id, effect_index)
        }
        Table::Item => {
            let entry = key_number(key, table, "entry", u64::from(u32::MAX))? as u32;
            PrimaryKey::item(entry)
        }
        Table::Quest => {
            let entry = key_number(key, table, "entry", u64::from(u32::MAX))? as u32;
            PrimaryKey::quest(entry)
        }
        Table::QuestText => {
            let quest_entry = key_number(key, table, "quest_entry", u64::from(u32::MAX))? as u32;
            PrimaryKey::quest_text(quest_entry)
        }
        Table::QuestObjective => {
            let quest_entry = key_number(key, table, "quest_entry", u64::from(u32::MAX))? as u32;
            let obj_index = key_number(key, table, "obj_index", u64::from(u8::MAX))? as u8;
            PrimaryKey::quest_objective(quest_entry, obj_index)
        }
        Table::QuestCastObjective => {
            let quest_entry = key_number(key, table, "quest_entry", u64::from(u32::MAX))? as u32;
            let obj_index = key_number(key, table, "obj_index", u64::from(u8::MAX))? as u8;
            PrimaryKey::quest_cast_objective(quest_entry, obj_index)
        }
        Table::QuestRewardItem => {
            let quest_entry = key_number(key, table, "quest_entry", u64::from(u32::MAX))? as u32;
            let item_entry = key_number(key, table, "item_entry", u64::from(u32::MAX))? as u32;
            PrimaryKey::quest_reward_item(quest_entry, item_entry)
        }
        Table::QuestRewardChoice => {
            let quest_entry = key_number(key, table, "quest_entry", u64::from(u32::MAX))? as u32;
            let choice_index = key_number(key, table, "choice_index", u64::from(u8::MAX))? as u8;
            PrimaryKey::quest_reward_choice(quest_entry, choice_index)
        }
        Table::PickpocketLoot => {
            let id = key_number(key, table, "id", u64::MAX)?;
            PrimaryKey::pickpocket_loot(id)
        }
        Table::GameobjectLoot => {
            let id = key_number(key, table, "id", u64::MAX)?;
            PrimaryKey::gameobject_loot(id)
        }
        Table::SkinningLoot => {
            let id = key_number(key, table, "id", u64::MAX)?;
            PrimaryKey::skinning_loot(id)
        }
        Table::FishingLoot => {
            let id = key_number(key, table, "id", u64::MAX)?;
            PrimaryKey::fishing_loot(id)
        }
    }
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

/// An unsigned 64-bit value travels as a decimal string, because a JSON number loses precision
/// above 2^53 and would come back silently wrong.
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

/// JSON carries one float width, so a claimed `f32` arrives as an `f64` and is narrowed here. A
/// value that narrows to an infinity, or that was never finite, does not survive a round trip
/// through the artifact and is refused rather than stored.
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

/// Refuses any member outside the closed list. `allowed` is sorted, matching the parsed map, so the
/// member named in the error is the same one for the same input.
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
