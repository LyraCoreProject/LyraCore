//! Actor verbs. See `docs/actor-verbs.md` for the contract and rationale.

use crate::import_meta::game_import_meta;
use spacetimedb::ReducerContext;

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) struct ImportRevision {
    pub source_sha: String,
    pub file_hash: String,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn import_revision(ctx: &ReducerContext, family: &str) -> Option<ImportRevision> {
    ctx.db
        .game_import_meta()
        .family()
        .find(family.to_string())
        .map(|meta| ImportRevision {
            source_sha: meta.source_sha,
            file_hash: meta.file_hash,
        })
}

macro_rules! debug_only {
    ($(pub(crate) use $path:path as $verb:ident;)+) => {
        $(
            #[cfg_attr(not(feature = "debug_reducers"), allow(unused_imports))]
            pub(crate) use $path as $verb;
        )+
    };
}

// Package-only verbs need no caller when no Rust Package is installed. Restore the unused-import
// lint when a Package is present so unused verbs remain visible during Package development.
macro_rules! package_only {
    ($(pub(crate) use $path:path as $verb:ident;)+) => {
        $(
            #[cfg_attr(not(has_packages), allow(unused_imports))]
            pub(crate) use $path as $verb;
        )+
    };
}

package_only! { pub(crate) use crate::combat::apply_start_attack as attack; }
package_only! { pub(crate) use crate::combat::request_attack as request_attack; }
package_only! { pub(crate) use crate::spell::request_cast as request_cast; }
package_only! { pub(crate) use crate::spell::cast_readiness as cast_readiness; }
debug_only! { pub(crate) use crate::combat::apply_start_ranged_attack as ranged_attack; }

/// Disarm the actor's outgoing auto-attack (melee or ranged). Shape adapter ONLY: the core returns
/// `()` (it never fails); this lifts it into the uniform `Result` verb shape.
#[cfg_attr(not(has_packages), allow(dead_code))] // package-only consumer — see `package_only!`
pub(crate) fn stop_attack(ctx: &ReducerContext, actor_guid: u64) -> Result<(), String> {
    crate::combat::stop_attack_for(ctx, actor_guid);
    Ok(())
}

/// Compatibility adapter for callers that only need acceptance. Use `request_cast` to retain
/// the scheduled identity and observe completion through `on_cast_finished`.
#[allow(
    dead_code,
    reason = "Package API v1 retains the result-only cast_at adapter"
)]
pub(crate) fn cast_at(
    ctx: &ReducerContext,
    actor_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), String> {
    crate::spell::request_cast(ctx, actor_guid, spell_id, target_guid)
        .map(|_| ())
        .map_err(Into::into)
}

// ---- quests ----

package_only! {
    pub(crate) use crate::quest::apply_accept_quest as accept_quest;
    pub(crate) use crate::quest::request_accept_quest as request_accept_quest;
    pub(crate) use crate::quest::apply_turn_in_quest as turn_in_quest;
    pub(crate) use crate::quest::request_turn_in_quest as request_turn_in_quest;
}

#[cfg(all(has_packages, feature = "debug_reducers"))]
pub(crate) use crate::bridge::party_command_fixture_drive as fixture_command_drive;
debug_only! { pub(crate) use crate::quest::grant_quest_unchecked as stage_quest; }

// ---- loot / inventory / vendor ----

pub(crate) use crate::loot::open_creature_corpse as open_creature_loot;
package_only! {
    pub(crate) use crate::items::apply_item_use as use_item;
    pub(crate) use crate::loot::request_open_creature_corpse as request_open_creature_loot;
    pub(crate) use crate::items::apply_take_loot as take_loot;
    pub(crate) use crate::items::request_profile_item as reconcile_profile_item;
    pub(crate) use crate::items::apply_equip_profile_upgrade as equip_profile_upgrade;
    pub(crate) use crate::items::request_take_loot as request_take_loot;
    // A Package bot also loots money through this verb.
    pub(crate) use crate::loot::apply_loot_money as loot_money;
}
pub(crate) use crate::creatures::apply_item_target_spell as cast_item_target;
debug_only! {
    pub(crate) use crate::items::apply_buy_item as buy_item;
    pub(crate) use crate::items::apply_equip_item as equip_item;
    pub(crate) use crate::items::apply_item_sell as sell_item;
}
package_only! {
    pub(crate) use crate::trainer::reconcile_profile_spell as reconcile_profile_spell;
    pub(crate) use crate::skill::reconcile_profile_skill as reconcile_profile_skill;
    pub(crate) use crate::talent::reconcile_profile_talent as learn_profile_talent;
    pub(crate) use crate::talent::select_profile_talent as select_profile_talent;
    pub(crate) use crate::seed::reconcile_curated_starter_role_levels as reconcile_starter_role_spell_levels;
}

// ---- NPC services / world ----

package_only! {
    pub(crate) use crate::gameobject::apply_use_gameobject as use_gameobject;
    pub(crate) use crate::gameobject::request_use_gameobject as request_use_gameobject;
}
debug_only! { pub(crate) use crate::trainer::apply_trainer_buy as trainer_buy; }
package_only! {
    pub(crate) use crate::spell::do_resurrect_response as respond_resurrect;
    pub(crate) use crate::world::do_repop as repop;
    pub(crate) use crate::world::do_spirit_healer_res as spirit_res;
    pub(crate) use crate::spell::do_self_resurrect as self_resurrect;
}

// ---- social ----

package_only! {
    pub(crate) use crate::chat::emit_system_message as system_message;
    pub(crate) use crate::group::accept_invite_for as accept_group_invite;
    pub(crate) use crate::sessionless::set_sessionless_action_consent as set_sessionless_action_consent;
    pub(crate) use crate::sessionless::action_gate as sessionless_action_gate;
    pub(crate) use crate::sessionless::movement_gate as sessionless_movement_gate;
    pub(crate) use crate::group::companion_target_facts as companion_target_facts;
    pub(crate) use crate::bridge::AdmittedClientCommand as AdmittedClientCommand;
    pub(crate) use crate::bridge::CommandOutcome as CommandOutcome;
    pub(crate) use crate::bridge::ParsedClientCommand as ParsedClientCommand;
    pub(crate) use crate::bridge::RECEIPT_CAPACITY as COMMAND_RECEIPT_CAPACITY;
    pub(crate) use crate::quest::area_trigger_route as area_trigger_route;
    pub(crate) use crate::quest::enter_sessionless_areatrigger as enter_sessionless_areatrigger;
}

/// A Refusal classified at the operation's Gate. Detail preserves existing client messages.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct ActionRefusal {
    pub kind: ActionRefusalKind,
    pub detail: String,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionRefusalKind {
    MissingActor,
    MissingTarget,
    DeadActor,
    DeadTarget,
    CannotAct,
    OtherPartition,
    OutOfRange,
    InventoryFull,
    Other,
    MissingResource,
    Class,
    Level,
    Prerequisite,
    ProfileLimit,
    TransferPending,
}

impl ActionRefusal {
    pub(crate) fn new(kind: ActionRefusalKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}
impl From<String> for ActionRefusal {
    fn from(detail: String) -> Self {
        Self::new(ActionRefusalKind::Other, detail)
    }
}
impl From<ActionRefusal> for String {
    fn from(reason: ActionRefusal) -> Self {
        reason.detail
    }
}
impl std::fmt::Display for ActionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.detail.fmt(f)
    }
}
