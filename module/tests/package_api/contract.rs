//! Package API version 2: the crate-root paths a Package may name.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use super::paths::{tokens, tree_leaves, RootedPath};
use super::source::{strip_source, DEBUG_REDUCERS_FILE_CFG, TEST_FILE_CFG};

/// The Package API roots. Everything beneath one is on the surface, except a gated `package`
/// child outside its gate.
pub const ROOTS: &[&str] = &["actor", "hooks", "package", "tables", "script_binding"];

/// `package` children that exist only in a build with their whole-file gate. Core compiles each
/// only under that cfg, so naming one from any other file breaks a build without it.
const GATED_PACKAGE_CHILDREN: &[(&str, &str)] = &[
    ("fixture", DEBUG_REDUCERS_FILE_CFG),
    ("test", TEST_FILE_CFG),
];

/// Crate-root names that are neither a root nor a table: the marker macros and the generated
/// character-owned table manifest.
const ROOT_ITEMS: &[&str] = &[
    "CHARACTER_OWNED_TABLES",
    "character_owned",
    "encounter_package",
    "game_client_command",
    "game_hook",
    "game_package_characters",
    "game_tick_pass",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    OnSurface,
    Outside {
        replacement: Option<&'static str>,
    },
    GatedChild {
        child: &'static str,
        gate: &'static str,
    },
    /// The `package` module itself, whose import would carry its gated children past their gate.
    PackageModule,
}

/// The surface: the fixed roots and items above plus every name in the table catalog.
pub struct Contract {
    catalog: BTreeSet<String>,
}

impl Contract {
    /// The contract for the catalog in `tables_rs`, Core's `module/src/tables.rs`.
    pub fn read(tables_rs: &Path) -> Self {
        let source = fs::read_to_string(tables_rs)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", tables_rs.display()));
        Self::from_catalog(&source)
    }

    /// Every name the catalog's `use crate::<family>::{..};` lines bind. The crate root
    /// re-exports the catalog, so each is also a crate-root name.
    pub fn from_catalog(source: &str) -> Self {
        let code = strip_source(source).code;
        let tokens = tokens(&code);
        let mut leaves = Vec::new();
        for (index, window) in tokens.windows(3).enumerate() {
            if window[0].text == "use" && window[1].text == "crate" && window[2].text == "::" {
                tree_leaves(&tokens, index + 3, &RootedPath::default(), &mut leaves);
            }
        }
        let catalog: BTreeSet<String> = leaves
            .into_iter()
            .map(|leaf| match leaf.alias.or(leaf.segments.last().cloned()) {
                Some(name) if name != "*" => name,
                _ => panic!(
                    "the table catalog names each table; `{}` does not",
                    leaf.written
                ),
            })
            .collect();
        assert!(!catalog.is_empty(), "the table catalog names no table");
        Self { catalog }
    }

    pub fn catalogs(&self, name: &str) -> bool {
        self.catalog.contains(name)
    }

    /// Whether a Package file whose whole-file gate is `file_gate` may name the crate-root path
    /// `segments`.
    pub fn verdict(&self, segments: &[String], file_gate: Option<&str>) -> Verdict {
        let root = segments.first().map_or("", String::as_str);
        if root == "package" {
            let child = segments.get(1).map(String::as_str);
            return match child {
                None | Some("*") => Verdict::PackageModule,
                Some(child) => match GATED_PACKAGE_CHILDREN.iter().find(|(c, _)| *c == child) {
                    Some((child, gate)) if file_gate != Some(*gate) => {
                        Verdict::GatedChild { child, gate }
                    }
                    _ => Verdict::OnSurface,
                },
            };
        }
        let generated_package = root
            .strip_prefix("pkg_")
            .is_some_and(|name| !name.is_empty());
        if ROOTS.contains(&root)
            || ROOT_ITEMS.contains(&root)
            || generated_package
            || self.catalog.contains(root)
        {
            return Verdict::OnSurface;
        }
        Verdict::Outside {
            replacement: replacement(segments),
        }
    }
}

/// The Version 2 path for the longest Version 1 prefix of `segments`, if one is documented.
fn replacement(segments: &[String]) -> Option<&'static str> {
    (1..=segments.len()).rev().find_map(|len| {
        let written = segments[..len].join("::");
        VERSION_1_REPLACEMENTS
            .iter()
            .find(|(old, _)| *old == written)
            .map(|(_, new)| *new)
    })
}

/// Version 1 crate-root paths an official Package named, and their Version 2 path. Paths that
/// did not change are absent.
#[rustfmt::skip]
const VERSION_1_REPLACEMENTS: &[(&str, &str)] = &[
    ("SessionActor", "crate::package::fixture::SessionActor"),
    ("chat::CHAT_YELL", "lyracore_shared::chat::broadcast_chat::YELL"),
    ("chat::apply_send_chat", "crate::actor::send_chat"),
    ("combat::AttackStart", "crate::actor::AttackStart"),
    ("combat::Hit", "crate::package::fixture::Hit"),
    ("combat::HitSource", "crate::package::fixture::HitSource"),
    ("combat::MeleeAttack", "crate::tables::MeleeAttack"),
    ("combat::apply_hit", "crate::package::fixture::apply_hit"),
    ("combat::enter_combat", "crate::package::fixture::enter_combat"),
    ("combat::final_damage", "crate::package::fixture::final_damage"),
    ("combat::fold_incoming_damage", "crate::package::fixture::fold_incoming_damage"),
    ("combat::kill_creature", "crate::package::fixture::kill_creature"),
    ("combat::request_attack", "crate::actor::request_attack"),
    ("combat::validate_attack_target", "crate::actor::attack_target_gate"),
    ("creatures::build_creature_entity", "crate::package::fixture::build_creature_entity"),
    ("creatures::build_player_entity", "crate::package::character::build_player_entity"),
    ("creatures::creature_spawn_evidence", "crate::actor::creature_spawn_evidence"),
    ("creatures::despawn_creature_entity", "crate::package::fixture::despawn_creature_entity"),
    ("creatures::insert_creature_entity", "crate::package::fixture::insert_creature_entity"),
    ("creatures::tick::emit_creature_leg", "crate::actor::movement::emit_creature_leg"),
    ("creatures::tick::emit_creature_path", "crate::actor::movement::emit_creature_path"),
    ("creatures::tick::emit_move_spline", "crate::package::fixture::emit_move_spline"),
    ("creatures::tick::stop_where_rendered", "crate::actor::movement::stop_where_rendered"),
    ("creatures::timer_never", "crate::package::fixture::timer_never"),
    ("encounter", "crate::package::encounter"),
    ("encounter::DOOR_OPEN_STATE", "crate::package::encounter::DOOR_OPEN_STATE"),
    ("encounter::ENCOUNTER_DONE", "crate::package::encounter::ENCOUNTER_DONE"),
    ("encounter::equip_swap", "crate::package::encounter::equip_swap"),
    ("encounter::get_encounter_state", "crate::package::encounter::get_encounter_state"),
    ("encounter::move_to_point", "crate::package::encounter::move_to_point"),
    ("encounter::open_door", "crate::package::encounter::open_door"),
    ("encounter::set_encounter_state", "crate::package::encounter::set_encounter_state"),
    ("encounter::spawn_wave", "crate::package::encounter::spawn_wave"),
    ("encounter::watch_hp_threshold", "crate::package::encounter::watch_hp_threshold"),
    ("encounter::wave_guid", "crate::package::encounter::wave_guid"),
    ("faction::is_friendly", "crate::actor::is_friendly"),
    ("gameobject::GameObject", "crate::tables::GameObject"),
    ("gameobject::GameObjectTemplate", "crate::tables::GameObjectTemplate"),
    ("gameobject::apply_use_gameobject", "crate::actor::use_gameobject"),
    ("gameobject::gameobject_destination_evidence", "crate::actor::gameobject_destination_evidence"),
    ("gameobject::go_type::CHEST", "crate::actor::go_type::CHEST"),
    ("gameobject::go_type::GOOBER", "crate::actor::go_type::GOOBER"),
    ("gameobject::go_type::QUESTGIVER", "crate::actor::go_type::QUESTGIVER"),
    ("group::GROUP_MAX_MEMBERS", "lyracore_shared::group::GROUP_MAX_MEMBERS"),
    ("group::GroupMemberPartition", "crate::tables::GroupMemberPartition"),
    ("group::PartyEnemyFacts", "crate::actor::PartyEnemyFacts"),
    ("group::PartyFactsUnavailable", "crate::actor::PartyFactsUnavailable"),
    ("group::PartyFactsUnavailableReason", "crate::actor::PartyFactsUnavailableReason"),
    ("group::PartyMemberFacts", "crate::actor::PartyMemberFacts"),
    ("group::PartyPartitionFacts", "crate::actor::PartyPartitionFacts"),
    ("group::PartyPartitionState", "crate::tables::PartyPartitionState"),
    ("group::PartyUnitFacts", "crate::actor::PartyUnitFacts"),
    ("group::emit_bot_invite_intent", "crate::actor::emit_bot_invite_intent"),
    ("group::emit_bot_leave_intent", "crate::actor::emit_bot_leave_intent"),
    ("group::group_of", "crate::actor::group_of"),
    ("group::leave_group_for", "crate::actor::leave_group_for"),
    ("group::party_facts", "crate::actor::party_facts"),
    ("group::sync_group_mirror", "crate::package::fixture::sync_group_mirror"),
    ("helpers::character_by_guid", "crate::actor::character_by_guid"),
    ("helpers::character_by_name", "crate::actor::character_by_name"),
    ("helpers::entities_near", "crate::actor::entities_near"),
    ("helpers::in_same_partition", "crate::actor::in_same_partition"),
    ("helpers::live_entity", "crate::actor::live_entity"),
    ("helpers::require_operator", "crate::package::require_operator"),
    ("items::grant_item", "crate::package::fixture::grant_item"),
    ("items::has_free_slot", "crate::actor::has_free_slot"),
    ("items::item_count", "crate::actor::item_count"),
    ("items::remove_items", "crate::package::fixture::remove_items"),
    ("loot::corpse_access", "crate::actor::corpse_access"),
    ("loot::corpse_eligible_for_access", "crate::actor::corpse_eligible_for_access"),
    ("loot::corpse_eligible_recipients", "crate::actor::corpse_eligible_recipients"),
    ("loot::death_entitlement", "crate::actor::death_entitlement"),
    ("loot::tag::LiveLootTagEligibility", "crate::actor::LiveLootTagEligibility"),
    ("loot::tag::clear", "crate::package::fixture::clear_loot_tag"),
    ("loot::tag::live_loot_tag_eligibility", "crate::actor::live_loot_tag_eligibility"),
    ("nav::CoverageEvidence", "crate::actor::movement::CoverageEvidence"),
    ("nav::LEG_MAX_EXPANSIONS", "crate::actor::movement::LEG_MAX_EXPANSIONS"),
    ("nav::NavChunk", "crate::tables::NavChunk"),
    ("nav::NavigationInputs", "crate::actor::movement::NavigationInputs"),
    ("nav::RoutePoint", "crate::actor::movement::RoutePoint"),
    ("nav::RouteStatus", "crate::actor::movement::RouteStatus"),
    ("nav::RouteStep", "crate::actor::movement::RouteStep"),
    ("nav::coverage_generation", "crate::actor::movement::coverage_generation"),
    ("nav::game_nav_chunk", "crate::tables::game_nav_chunk"),
    ("nav::game_navigation_revision", "crate::tables::game_navigation_revision"),
    ("nav::has_los", "crate::actor::movement::has_los"),
    ("nav::inputs", "crate::actor::movement::inputs"),
    ("nav::record_change", "crate::package::fixture::record_navigation_change"),
    ("nav::route_path", "crate::actor::movement::route_path"),
    ("nav::route_path_with_budget", "crate::actor::movement::route_path_with_budget"),
    ("nav::route_segment_clear", "crate::actor::movement::route_segment_clear"),
    ("nav::route_step", "crate::actor::movement::route_step"),
    ("nav::walkable", "crate::actor::movement::walkable"),
    ("package_account::create_package_character", "crate::package::create_character"),
    ("package_config::ensure_package_config_default", "crate::package::ensure_config_default"),
    ("package_config::game_package_config", "crate::tables::game_package_config"),
    ("package_fixture::admit_to_instance", "crate::package::fixture::admit_to_instance"),
    ("package_fixture::apply_damage", "crate::package::fixture::apply_damage"),
    ("package_fixture::client_cast", "crate::package::fixture::client_cast"),
    ("package_fixture::declare_next_movement_tick", "crate::package::fixture::declare_next_movement_tick"),
    ("package_fixture::record_completed_transfer", "crate::package::fixture::record_completed_transfer"),
    ("package_fixture::remove_live_character", "crate::package::fixture::remove_live_character"),
    ("package_fixture::require_no_imported_content", "crate::package::fixture::require_no_imported_content"),
    ("package_fixture::top_threat_target", "crate::package::fixture::top_threat_target"),
    ("package_test::EntityView", "crate::package::test::EntityView"),
    ("package_test::RuntimeScript", "crate::package::test::RuntimeScript"),
    ("package_test::ask_offline", "crate::package::test::ask_offline"),
    ("package_test::code_of", "crate::package::test::code_of"),
    ("package_test::read_scanned", "crate::package::test::read_scanned"),
    ("package_test::shape_of", "crate::package::test::shape_of"),
    ("quest::AreaTriggerRoute", "crate::actor::AreaTriggerRoute"),
    ("quest::CharacterQuest", "crate::tables::CharacterQuest"),
    ("quest::MAX_QUEST_LOG_SIZE", "crate::actor::MAX_QUEST_LOG_SIZE"),
    ("quest::accept_gates", "crate::actor::quest_acceptance"),
    ("quest::character_quest_row", "crate::actor::character_quest"),
    ("quest::objective_kind::COLLECT_ITEM", "crate::actor::objective_kind::COLLECT_ITEM"),
    ("quest::objective_kind::EXPLORE_AREATRIGGER", "crate::actor::objective_kind::EXPLORE_AREATRIGGER"),
    ("quest::objective_kind::KILL_CREATURE", "crate::actor::objective_kind::KILL_CREATURE"),
    ("quest::objective_kind::USE_GAMEOBJECT", "crate::actor::objective_kind::USE_GAMEOBJECT"),
    ("quest::quest_is_complete", "crate::actor::quest_complete"),
    ("quest::quest_role::END", "crate::actor::quest_role::END"),
    ("quest::quest_role::START", "crate::actor::quest_role::START"),
    ("spell::A_CONTROL", "crate::actor::A_CONTROL"),
    ("spell::A_PERIODIC_HEAL", "crate::actor::A_PERIODIC_HEAL"),
    ("spell::A_PERIODIC_TRIGGER", "crate::actor::A_PERIODIC_TRIGGER"),
    ("spell::A_PROC_TRIGGER", "crate::actor::A_PROC_TRIGGER"),
    ("spell::BuffStatus", "crate::actor::BuffStatus"),
    ("spell::BuffUnavailableReason", "crate::actor::BuffUnavailableReason"),
    ("spell::CastFinish", "crate::actor::CastFinish"),
    ("spell::CastHandle", "crate::actor::CastHandle"),
    ("spell::CastRefusal", "crate::actor::CastRefusal"),
    ("spell::CastRefusalKind", "crate::actor::CastRefusalKind"),
    ("spell::CastStart", "crate::actor::CastStart"),
    ("spell::ControlReadError", "crate::actor::ControlReadError"),
    ("spell::CreatureSpellCasterAdmission", "crate::package::fixture::CreatureSpellCasterAdmission"),
    ("spell::CreatureSpellStart", "crate::package::fixture::CreatureSpellStart"),
    ("spell::CreatureSpellStartMode", "crate::package::fixture::CreatureSpellStartMode"),
    ("spell::CreatureSpellTarget", "crate::package::fixture::CreatureSpellTarget"),
    ("spell::E_HEAL", "crate::actor::E_HEAL"),
    ("spell::E_HEAL_MAX_HEALTH", "crate::actor::E_HEAL_MAX_HEALTH"),
    ("spell::E_NEXT_SWING", "crate::actor::E_NEXT_SWING"),
    ("spell::E_SCRIPTED", "crate::actor::E_SCRIPTED"),
    ("spell::PlayerSpell", "crate::tables::PlayerSpell"),
    ("spell::SPELL_ATTR_CHANNELED", "lyracore_shared::spell::SPELL_ATTR_CHANNELED"),
    ("spell::Spell", "crate::tables::Spell"),
    ("spell::SpellChain", "crate::tables::SpellChain"),
    ("spell::SpellEffect", "crate::tables::SpellEffect"),
    ("spell::T_TARGET_ALLY", "crate::actor::T_TARGET_ALLY"),
    ("spell::T_TARGET_ENEMY", "crate::actor::T_TARGET_ENEMY"),
    ("spell::buff_status", "crate::actor::buff_status"),
    ("spell::cancel_cast_attempt", "crate::actor::cancel_cast_attempt"),
    ("spell::cast_triggered", "crate::package::fixture::cast_triggered"),
    ("spell::control_status", "crate::actor::control_status"),
    ("spell::do_cancel_aura", "crate::package::fixture::do_cancel_aura"),
    ("spell::expire_cast_attempt", "crate::actor::expire_cast_attempt"),
    ("spell::has_aura", "crate::actor::has_aura"),
    ("spell::knows_spell", "crate::actor::knows_spell"),
    ("spell::learn_spell", "crate::package::fixture::learn_spell"),
    ("spell::pending_cast", "crate::actor::pending_cast"),
    ("spell::stacking::SpellGroup", "crate::tables::SpellGroup"),
    ("spell::stacking::game_spell_group", "crate::tables::game_spell_group"),
    ("spell::start_creature_spell", "crate::package::fixture::start_creature_spell"),
    ("stats::set_character_level", "crate::package::character::set_character_level"),
    ("terrain::TerrainChunk", "crate::tables::TerrainChunk"),
    ("terrain::game_terrain_chunk", "crate::tables::game_terrain_chunk"),
    ("terrain::ground_z", "crate::actor::movement::ground_z"),
    ("terrain::snap_z", "crate::actor::movement::snap_z"),
    ("terrain::zone_id_at", "crate::actor::movement::zone_id_at"),
    ("transfer::game_bot_transfer_intent", "crate::tables::game_bot_transfer_intent"),
    ("transfer::game_transfer_in", "crate::tables::game_transfer_in"),
    ("world::cascade_delete_character", "crate::package::character::cascade_delete_character"),
    ("world::ghost_restored_fields", "crate::package::character::ghost_restored_fields"),
    ("world::remove_live_character", "crate::package::fixture::remove_live_character"),
    ("xp::grant_xp", "crate::package::fixture::grant_xp"),
    ("xp::rank_xp_multiplier", "crate::actor::rank_xp_multiplier"),
    ("xp::xp_for_kill", "crate::actor::xp_for_kill"),
];
