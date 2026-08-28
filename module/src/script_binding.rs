//! **Event Binding**: which Runtime Scripts run for which event, and the dispatch that runs them.
//!
//! `game_script` is the durable half. Every row is a whole Runtime Script a Package ships, put
//! there by the `script` Import Family's apply (`package_import/script.rs`) and by nothing else.
//! This module owns the table and the question the engine asks of it once per event.
//!
//! # The dispatch
//!
//! [`fire`] is called from every generated `fire_*` in `hooks.rs`, after that event's compiled
//! `game_hook!` handlers have run. It is a lookup, an ordering, and one call into the Runtime
//! Script Host:
//!
//!  * **Bound and enabled only.** The `by_event` index answers the lookup, and a disabled row is
//!    skipped here rather than left out of the table — a Package that ships a script switched off
//!    has still shipped it, and an Operator reading `game_script` should see that.
//!  * **Priority ascending, then `script_id` ascending.** Total, so two Shards running one plan run
//!    it in one order. `priority` is the author's lever and the identifier is the tiebreak, never
//!    the Package name or the order the artifacts arrived in.
//!  * **The Host is the failure boundary.** `run_event` invokes each script, commits what the ones
//!    that succeeded staged, and returns a diagnostic for each one that did not. A failing script
//!    stops neither the next script nor the core work after this call, so nothing here has error
//!    handling of its own to get wrong.
//!
//! # What an event costs when nothing is bound
//!
//! The common case is a Shard with no Package scripts at all, and `on_damage_taken` fires on every
//! swing. So the ordering, the entity reads and the Host borrow all happen AFTER the lookup finds
//! something: an unbound event is one indexed range scan that returns nothing.
//!
//! # Re-entry
//!
//! A Runtime Script's Staged Effect commits through the same core operations the engine uses, and
//! those fire hooks. So a script bound to `on_levelup` that grants XP can reach this function again
//! from inside its own invocation. `with_host` refuses the second borrow and that invocation simply
//! does not happen — refusing rather than recursing, because recursion here would be unbounded and
//! a panic would take the whole reducer down with it. Event Bindings are what made that reachable
//! for the first time.

use spacetimedb::{log, table, ReducerContext};

use crate::runtime_script::{
    run_event, with_host, CoreEffects, EntityView, RuntimeScript, ScriptEvent,
};

/// One Runtime Script a Package ships, reconciled onto this Shard.
///
/// NOT public, for the same reason `game_import_meta` and `game_package_import` are not: nothing
/// subscribes it, so it needs no gateway binding. The apply reducer writes it and an Operator reads
/// it with `spacetime sql`.
///
/// There is no upload path. A row exists here because an enabled Package's Script Artifact put it
/// there, which is what makes reconciliation total: the apply clears the Package script range and
/// rewrites it, so a Package that left the enabled set takes its scripts with it. The debug lever
/// (`debug_run_runtime_script`) still takes raw source as a reducer argument, but it writes nothing
/// here — it runs a script once against the live world and forgets it.
#[table(accessor = game_script, index(accessor = by_event, btree(columns = [event])))]
pub struct Script {
    /// The identifier this script keeps on every Shard, inside the Package script range.
    #[primary_key]
    pub script_id: u32,
    /// The unique human-readable name, and the label a Script Diagnostic carries.
    #[unique]
    pub name: String,
    /// The Package that ships it.
    pub package: String,
    /// The digest of the Datascript source the artifact was generated from, carried verbatim from
    /// the artifact — the same value `game_package_import.source_hash` records for the Package.
    /// Identifies the REVISION the script came from.
    pub source_hash: String,
    /// BLAKE3 digest of `source`. DERIVED at apply, never authored, so it cannot disagree with the
    /// Lua beside it. It is also exactly the Runtime Script Host's compiler-cache key, so two rows
    /// with one `content_hash` compile once.
    pub content_hash: String,
    /// The event this script runs for: one name from `GAME_HOOK_EVENT_NAMES`.
    pub event: String,
    /// Lower runs first among the scripts bound to one event; `script_id` breaks a tie.
    pub priority: i32,
    /// Whether any event invokes it. A disabled row is applied and skipped, not withheld.
    pub enabled: bool,
    /// The Lua the Runtime Script Host runs.
    pub source: String,
}

/// Run every enabled Runtime Script bound to `event`, in dispatch order.
///
/// `actor_guid` caused the event and `target_guid` is what it acted on; either may be `0`, or may
/// name something that is not a live entity (a corpse, a gameobject, a character in transit), which
/// reaches the script as an absent `event.actor`/`event.target` rather than as a failure.
///
/// Called only from the generated `fire_*` dispatchers, which is what keeps the event label here
/// identical to the one a Package binds to.
pub(crate) fn fire(ctx: &ReducerContext, event: &str, actor_guid: u64, target_guid: u64) {
    let bound = dispatch_order(ctx.db.game_script().by_event().filter(event).collect());
    if bound.is_empty() {
        return;
    }

    // Read after the lookup: an unbound event must not pay for two entity reads. Read BEFORE the
    // invocation, because the Host takes the participants as values — a script never reaches a row.
    let script_event = ScriptEvent {
        name: event.to_string(),
        actor: EntityView::read(ctx, actor_guid),
        target: EntityView::read(ctx, target_guid),
    };
    let scripts: Vec<RuntimeScript<'_>> = bound
        .iter()
        .map(|script| RuntimeScript {
            name: &script.name,
            source: &script.source,
        })
        .collect();

    let Some(diagnostics) =
        with_host(|host| run_event(host, &mut CoreEffects { ctx }, &script_event, &scripts))
    else {
        log::warn!(
            "`{event}`: the Runtime Script Host is already running a script, so this event's \
             {} bound script(s) did not run. A script reached the Host again through an effect \
             it staged.",
            scripts.len()
        );
        return;
    };
    for diagnostic in diagnostics {
        log::warn!("{diagnostic}");
    }
}

/// Every enabled script, in dispatch order: priority ascending, then `script_id` ascending.
///
/// Split from [`fire`] so the ordering is testable without a `ReducerContext`, which a native test
/// has no way to build. Total on purpose — two Shards running one plan must run it in one order,
/// so neither the Package name nor the order the rows came back in may reach this.
fn dispatch_order(mut bound: Vec<Script>) -> Vec<Script> {
    bound.retain(|script| script.enabled);
    bound.sort_by_key(|script| (script.priority, script.script_id));
    bound
}

#[cfg(test)]
mod tests {
    use super::{dispatch_order, Script};

    fn script(script_id: u32, priority: i32, enabled: bool) -> Script {
        Script {
            script_id,
            name: format!("example.s{script_id}"),
            package: "example.bolt".to_string(),
            source_hash: String::new(),
            content_hash: String::new(),
            event: "on_login".to_string(),
            priority,
            enabled,
            source: "grant_xp(event.actor, 1)".to_string(),
        }
    }

    fn ids(scripts: Vec<Script>) -> Vec<u32> {
        dispatch_order(scripts)
            .iter()
            .map(|script| script.script_id)
            .collect()
    }

    /// Event Bindings are what make Host re-entry REACHABLE for the first time: a script's Staged
    /// Effect commits through a core operation, that operation fires a hook, and the hook lands
    /// back in [`fire`] — inside the invocation that staged it.
    ///
    /// The Host must refuse the second borrow rather than recurse or panic, because recursion here
    /// is unbounded and a panic would take the whole reducer down. This drives the exact nesting
    /// `fire` performs: an outer borrow that is live while an inner one is attempted.
    #[test]
    fn a_binding_reached_from_inside_an_invocation_is_refused_rather_than_recursing() {
        let outer = crate::runtime_script::with_host(|_host| {
            // Standing in for the committed effect that fires a hook, which calls `fire`, which
            // borrows the Host again.
            crate::runtime_script::with_host(|inner| inner.compilations())
        });

        assert_eq!(
            outer,
            Some(None),
            "the outer borrow must succeed and the re-entrant one must be refused"
        );
        assert!(
            crate::runtime_script::with_host(|host| host.compilations()).is_some(),
            "the refusal must leave the Host usable for the next event"
        );
    }

    /// The catalog has two homes that cannot see each other: `HOOK_EVENTS` in `module/build.rs`,
    /// which generates the dispatch, and `HOOK_EVENT_NAMES` in the Package Delta crate, which
    /// refuses a Package binding to an event that does not exist. An event added to one and not the
    /// other would make every Package binding to it unshippable, with nothing to say why.
    #[test]
    fn the_hook_catalogue_and_the_artifact_parsers_event_list_are_identical() {
        assert_eq!(
            crate::GAME_HOOK_EVENT_NAMES,
            lyracore_package_delta::HOOK_EVENT_NAMES,
            "add the event to `HOOK_EVENT_NAMES` in crates/lyracore-package-delta/src/script.rs \
             as well as to `HOOK_EVENTS` in module/build.rs"
        );
    }

    #[test]
    fn lower_priority_runs_first() {
        assert_eq!(
            ids(vec![
                script(100_001, 10, true),
                script(100_002, -5, true),
                script(100_003, 0, true),
            ]),
            [100_002, 100_003, 100_001]
        );
    }

    /// The tiebreak has to be total, or two Shards running one plan could run it in two orders.
    #[test]
    fn one_priority_is_broken_by_the_identifier() {
        assert_eq!(
            ids(vec![
                script(100_009, 0, true),
                script(100_002, 0, true),
                script(100_005, 0, true),
            ]),
            [100_002, 100_005, 100_009]
        );
    }

    /// The identifier never outranks the priority: a later script may still run first.
    #[test]
    fn a_higher_identifier_still_runs_first_when_its_priority_is_lower() {
        assert_eq!(
            ids(vec![script(100_001, 5, true), script(100_999, 1, true)]),
            [100_999, 100_001]
        );
    }

    #[test]
    fn a_disabled_script_is_bound_but_never_invoked() {
        assert_eq!(
            ids(vec![
                script(100_001, 0, true),
                script(100_002, 0, false),
                script(100_003, 0, true),
            ]),
            [100_001, 100_003]
        );
    }

    #[test]
    fn an_event_with_nothing_bound_dispatches_nothing() {
        assert!(ids(vec![]).is_empty());
        assert!(ids(vec![script(100_001, 0, false)]).is_empty());
    }
}
