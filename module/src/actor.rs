//! The ACTOR VERB API: one documented surface over every explicit-guid action
//! core, so anything that acts ON BEHALF OF a unit — the `debug_*` harness reducers, the
//! playerbots brains, and the future Tier-2 Lua host API — consumes the SAME verbs the
//! player reducers do, with identical gates.
//!
//! Existing verbs retain their `Result<(), String>` contract. Typed requests expose accepted
//! work and pending cast identity without changing the client operations. All Gates remain
//! in the operation that owns them. Callers authorize the Actor before entering these functions.
//!
//! | verb | core | gate semantics (unchanged, documented here for consumers) |
//! |------|------|------------------------------------------------------------|
//! | `attack` | `combat::apply_start_attack` | CC-blocked rejected; no self/corpse/cross-map; friendly (green) target rejected when faction data exists; re-arm retargets |
//! | `ranged_attack` | `combat::apply_start_ranged_attack` | `attack` gates + ranged weapon equipped (slot 17) |
//! | `stop_attack` | `combat::stop_attack_for` | unconditional disarm of the actor's outgoing melee row |
//! | `cast_at` | `spell::request_cast` | normal cast lifecycle; an existing timed cast waits; level comes from the live entity |
//! | `request_cast` | `spell::request_cast` | typed start, waiting, and Refusal; completion uses `on_cast_finished` |
//! | `accept_quest` | `quest::apply_accept_quest` | alive + giver in range offering the quest + level/race/class/prereq/duplicate gates |
//! | `stage_quest` | `quest::grant_quest_unchecked` | HARNESS/BOT staging: same row shape, all accept gates SKIPPED (giver-less) |
//! | `turn_in_quest` | `quest::apply_turn_in_quest` | alive + giver in range ending the quest + objectives complete; rewards atomic |
//! | `open_creature_loot` | `loot::open_creature_corpse` | alive + dead creature corpse on the same map within 10yd + Loot Tag eligibility; authorizes the following read |
//! | `take_loot` | `items::apply_take_loot` | alive + corpse/GO on same map within 10yd + Loot Tag eligibility for creatures + slot occupied; inventory-full rolls back |
//! | `loot_money` | `loot::apply_loot_money` | alive + dead creature corpse, same map, 10yd, money > 0 + Loot Tag eligibility |
//! | `buy_item` | `items::apply_buy_item` | vendor in range + stocked + money; stacks/slots validated |
//! | `sell_item` | `items::apply_item_sell` | vendor in range + sellable item in slot; feeds the buyback ring |
//! | `use_item` | `items::apply_item_use` | alive + usable item in slot (consumable/on-use gates) |
//! | `cast_item_target` | `creatures::apply_item_target_spell` | spell kind + owned carried item + effect-specific gates |
//! | `equip_item` | `items::apply_equip_item` | slot type/level gates |
//! | `trainer_buy` | `trainer::apply_trainer_buy` | trainer resolved by guid + offering gates (class/level/money) |
//! | `use_gameobject` | `gameobject::apply_use_gameobject` | GO resolved by guid + range/use gates |
//! | `repop` | `world::do_repop` | dead actor releases to the graveyard ghost |
//! | `respond_resurrect` | `spell::do_resurrect_response` | consume the actor's pending rez offer; accept revives IN PLACE at the offer's % |
//! | `spirit_res` | `world::do_spirit_healer_res` | ghost actor res at the spirit healer (sickness applies) |
//! | `accept_group_invite` | `group::accept_invite_for` | pending invite exists + inviter still leads + group not full; roster events fire |
//! | `system_message` | `chat::emit_system_message` | recipient exists and is online on this Shard; text is trimmed, bounded, and non-empty |
//!
//! Adding a verb = add the row above + the `use ... as` below; if the underlying core is still
//! inlined in a `ctx.sender()`-gated player reducer, factor it out FIRST (code motion, reducer
//! delegates). A verb nobody consumes is DEAD — delete it, don't suppress the lint (buyback/repair
//! were culled exactly that way).
//!
//! Two of this file's consumers are OPTIONAL trees rather than absent ones, and each has a macro
//! that says so: `debug_only!` (the `debug_reducers`-gated harness) and `package_only!` (the
//! `packages/` drop-ins, discovered by `module/build.rs`). Both silence unused-import ONLY in the
//! build where the consumer isn't compiled, and neither is a licence to keep a verb no tree calls.

use spacetimedb::ReducerContext;

// ---- combat ----

// A `debug_only!` verb's sole consumer today is the feature-gated harness; a default build
// compiles debug.rs out, so unused-import is silenced ONLY there. A debug_reducers build still
// flags any verb the harness stopped consuming — that's the cue to delete it, not suppress it.
macro_rules! debug_only {
    ($(pub(crate) use $path:path as $verb:ident;)+) => {
        $(
            #[cfg_attr(not(feature = "debug_reducers"), allow(unused_imports))]
            pub(crate) use $path as $verb;
        )+
    };
}

// The same argument for the other optional consumer: `packages/` is a DROP-IN tree that build.rs
// discovers at build time, and a checkout with no REAL package installed (only the inert reference
// Package, `packages/example/`, which every checkout ships) is a designed, supported state — so a
// verb whose only consumer is a package is not dead, it is unbuilt. build.rs emits `has_packages`
// when it compiled at least one package OTHER than the reference one in; a build that DID gets the
// lint back, which is still the cue to delete a verb nobody consumes rather than suppress it.
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
#[allow(dead_code)] // Retained Package API v1 operation.
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
debug_only! { pub(crate) use crate::quest::grant_quest_unchecked as stage_quest; }

// ---- loot / inventory / vendor ----

package_only! {
    pub(crate) use crate::items::apply_item_use as use_item;
    pub(crate) use crate::loot::open_creature_corpse as open_creature_loot;
    pub(crate) use crate::items::apply_take_loot as take_loot;
    // `loot_money` also feeds playerbots' drink-at-rest behavior (work-item 154).
    pub(crate) use crate::loot::apply_loot_money as loot_money;
}
pub(crate) use crate::creatures::apply_item_target_spell as cast_item_target;
debug_only! {
    pub(crate) use crate::items::apply_buy_item as buy_item;
    pub(crate) use crate::items::apply_equip_item as equip_item;
    pub(crate) use crate::items::apply_item_sell as sell_item;
}

// ---- NPC services / world ----

debug_only! {
    pub(crate) use crate::gameobject::apply_use_gameobject as use_gameobject;
    pub(crate) use crate::trainer::apply_trainer_buy as trainer_buy;
}
package_only! {
    pub(crate) use crate::spell::do_resurrect_response as respond_resurrect;
    pub(crate) use crate::world::do_repop as repop;
    pub(crate) use crate::world::do_spirit_healer_res as spirit_res;
}

// ---- social ----

package_only! {
    pub(crate) use crate::chat::emit_system_message as system_message;
    pub(crate) use crate::group::accept_invite_for as accept_group_invite;
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
