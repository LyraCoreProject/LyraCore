//! Package Delta format. See `docs/package-delta-format.md` for the contract and rationale.

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::pedantic)]
// The parser narrows every integer with an explicit bound check immediately before the cast, so the
// `as` conversions in `delta.rs` are the checked ones, not the lossy ones this lint looks for.
#![allow(clippy::cast_possible_truncation)]

mod canonical;
mod delta;
mod error;
pub mod ids;
mod schema;
pub mod script;
mod trace;

pub use delta::{
    Claim, ClaimCounts, Operation, PackageDelta, PackageId, PrimaryKey, SourceHash, DELTA_VERSION,
};
pub use error::DeltaError;
pub use ids::{
    is_fixture_reserved_cast_id, is_fixture_reserved_creature_ai_id,
    is_fixture_reserved_creature_id, is_fixture_reserved_creature_spawn_id,
    is_fixture_reserved_gameobject_id, is_fixture_reserved_globals_id,
    is_fixture_reserved_gossip_id, is_fixture_reserved_item_id, is_fixture_reserved_loot_id,
    is_fixture_reserved_quest_id, is_fixture_reserved_spell_id, is_fixture_reserved_spellmeta_id,
    is_fixture_reserved_trainer_id, is_package_cast_id, is_package_creature_ai_id,
    is_package_creature_id, is_package_gameobject_id, is_package_globals_id, is_package_gossip_id,
    is_package_item_id, is_package_loot_id, is_package_quest_id, is_package_script_id,
    is_package_spell_id, is_package_spellmeta_id, is_package_trainer_id, packed_class_level_id,
    packed_creature_spawn_guid, packed_gameobject_spawn_guid, packed_quest_objective_id,
    packed_quest_reward_choice_id, packed_quest_reward_item_id, packed_race_class_id,
    packed_race_class_level_id, packed_spell_effect_id, FIXTURE_CREATURE_ID_CEIL,
    FIXTURE_CREATURE_ID_FLOOR, MAX_CREATURE_GUID_COMPONENT, MAX_QUEST_OBJECTIVE_INDEX,
    MAX_QUEST_REWARD_CHOICE_INDEX, MAX_SPELL_EFFECT_INDEX, MAX_STATS_LEVEL, PACKAGE_CAST_ID_CEIL,
    PACKAGE_CAST_ID_FLOOR, PACKAGE_CREATURE_AI_ID_CEIL, PACKAGE_CREATURE_AI_ID_FLOOR,
    PACKAGE_CREATURE_ID_CEIL, PACKAGE_CREATURE_ID_FLOOR, PACKAGE_GAMEOBJECT_ID_CEIL,
    PACKAGE_GAMEOBJECT_ID_FLOOR, PACKAGE_GLOBALS_ID_CEIL, PACKAGE_GLOBALS_ID_FLOOR,
    PACKAGE_GOSSIP_ID_CEIL, PACKAGE_GOSSIP_ID_FLOOR, PACKAGE_ITEM_ID_CEIL, PACKAGE_ITEM_ID_FLOOR,
    PACKAGE_LOOT_ID_CEIL, PACKAGE_LOOT_ID_FLOOR, PACKAGE_QUEST_ID_CEIL, PACKAGE_QUEST_ID_FLOOR,
    PACKAGE_SCRIPT_ID_CEIL, PACKAGE_SCRIPT_ID_FLOOR, PACKAGE_SPELLMETA_ID_CEIL,
    PACKAGE_SPELLMETA_ID_FLOOR, PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR,
    PACKAGE_TRAINER_ID_CEIL, PACKAGE_TRAINER_ID_FLOOR,
};
pub use schema::{
    Column, FieldType, FieldValue, Table, CAST_FAMILY, CREATURE_AI_FAMILY, CREATURE_FAMILY,
    GAMEOBJECT_FAMILY, GLOBALS_FAMILY, GOSSIP_FAMILY, ITEM_FAMILY, LOOT_FAMILY, QUEST_FAMILY,
    SPELLMETA_FAMILY, SPELL_FAMILY, TRAINER_FAMILY,
};
pub use script::{
    artifact_kind, trace_scripts, ArtifactKind, EventBinding, Script, ScriptArtifact,
    ScriptConflict, ScriptName, ScriptTrace, TracedScript, HOOK_EVENT_NAMES, SCRIPT_ARTIFACT_KIND,
    SCRIPT_FAMILY, SCRIPT_VERSION,
};
pub use trace::{trace, ClaimConflict, ClaimTrace, ClaimedField, TracedRow};

/// Reads an artifact and writes it back in canonical form.
///
/// Equivalent input produces identical bytes, whatever member order, whitespace or number spelling
/// it arrived with.
///
/// # Errors
/// Any [`DeltaError`] the parse would raise.
pub fn canonicalize(json: &str) -> Result<String, DeltaError> {
    Ok(PackageDelta::parse(json)?.to_canonical_json())
}
