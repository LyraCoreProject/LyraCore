//! Before changing artifact parsing or claims, read docs/package-delta-format.md.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
// Integer casts follow explicit bounds checks in the parser.
#![allow(clippy::cast_possible_truncation)]

mod canonical;
mod delta;
mod error;
pub mod fixture_ids;
pub mod ids;
mod schema;
pub mod script;
mod trace;

pub use delta::{Claim, ClaimCounts, Operation, PackageDelta, PackageId, PrimaryKey, SourceHash};
pub use error::{DeltaError, ScriptRefusal};
pub use ids::{
    is_fixture_reserved_creature_id, is_operator_spawn_id, is_package_cast_id,
    is_package_creature_ai_id, is_package_creature_id, is_package_gameobject_id,
    is_package_globals_id, is_package_gossip_id, is_package_item_id, is_package_loot_id,
    is_package_quest_id, is_package_script_id, is_package_spell_id, is_package_spellmeta_id,
    is_package_trainer_id, packed_class_level_id, packed_creature_spawn_guid,
    packed_gameobject_spawn_guid, packed_quest_objective_id, packed_quest_reward_choice_id,
    packed_quest_reward_item_id, packed_race_class_id, packed_race_class_level_id,
    packed_spell_effect_id, MAX_CREATURE_GUID_COMPONENT, OPERATOR_SPAWN_ID_CEIL,
    OPERATOR_SPAWN_ID_FLOOR,
};
pub use schema::{
    Column, FieldType, FieldValue, Table, CAST_FAMILY, CREATURE_AI_FAMILY, CREATURE_FAMILY,
    DBC_FAMILY, GAMEOBJECT_FAMILY, GLOBALS_FAMILY, GOSSIP_FAMILY, ITEM_FAMILY, LOOT_FAMILY,
    QUEST_FAMILY, SPELLMETA_FAMILY, SPELL_FAMILY, TRAINER_FAMILY,
};
pub use script::{
    artifact_kind, trace_scripts, ArtifactKind, ScriptArtifact, ScriptTrace, TracedScript,
    HOOK_EVENT_NAMES, SCRIPT_FAMILY,
};
pub use trace::{trace, ClaimConflict, ClaimTrace, ClaimedField, TracedRow};

pub fn canonicalize(json: &str) -> Result<String, DeltaError> {
    Ok(PackageDelta::parse(json)?.to_canonical_json())
}
