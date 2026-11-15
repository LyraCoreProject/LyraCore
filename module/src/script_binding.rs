use spacetimedb::{log, table, ReducerContext};

use crate::runtime_script::{
    ask_event, with_host, CoreEffects, EntityView, RuntimeScript, ScriptEvent, ScriptPayload,
};

#[table(accessor = game_script, index(accessor = by_event, btree(columns = [event])))]
pub struct Script {
    #[primary_key]
    pub script_id: u32,
    #[unique]
    pub name: String,
    pub package: String,
    pub source_hash: String,
    pub content_hash: String,
    pub event: String,
    pub priority: i32,
    pub enabled: bool,
    pub source: String,
}

pub(crate) fn fire_hook(
    ctx: &ReducerContext,
    event: &str,
    actor_guid: u64,
    target_guid: u64,
    payload: impl FnOnce() -> ScriptPayload,
) {
    dispatch(ctx, event, actor_guid, target_guid, payload);
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn ask(
    ctx: &ReducerContext,
    event: &str,
    actor_guid: u64,
    target_guid: u64,
) -> Option<f64> {
    dispatch(ctx, event, actor_guid, target_guid, ScriptPayload::default)
}

fn dispatch(
    ctx: &ReducerContext,
    event: &str,
    actor_guid: u64,
    target_guid: u64,
    payload: impl FnOnce() -> ScriptPayload,
) -> Option<f64> {
    let bound = dispatch_order(
        ctx.db.game_script().by_event().filter(event).collect(),
        |script| script.enabled,
        |script| (script.priority, script.script_id),
    );
    if bound.is_empty() {
        return None;
    }

    let script_event = ScriptEvent {
        name: event.to_string(),
        actor: EntityView::read(ctx, actor_guid),
        target: EntityView::read(ctx, target_guid),
        payload: payload(),
    };
    let scripts: Vec<RuntimeScript<'_>> = bound
        .iter()
        .map(|script| RuntimeScript {
            name: &script.name,
            source: &script.source,
        })
        .collect();

    let Some((diagnostics, answer)) =
        with_host(|host| ask_event(host, &mut CoreEffects { ctx }, &script_event, &scripts))
    else {
        log::warn!(
            "`{event}`: the Runtime Script Host is already running a script, so this event's \
             {} bound script(s) did not run. A script reached the Host again through an effect \
             it staged.",
            scripts.len()
        );
        return None;
    };
    for diagnostic in diagnostics {
        log::warn!("{diagnostic}");
    }
    answer
}

/// Bound scripts run enabled only, by ascending priority, then by script id.
pub(crate) fn dispatch_order<T>(
    mut bound: Vec<T>,
    enabled: impl Fn(&T) -> bool,
    priority_and_id: impl Fn(&T) -> (i32, u32),
) -> Vec<T> {
    bound.retain(|script| enabled(script));
    bound.sort_by_key(|script| priority_and_id(script));
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
        dispatch_order(
            scripts,
            |script| script.enabled,
            |script| (script.priority, script.script_id),
        )
        .iter()
        .map(|script| script.script_id)
        .collect()
    }

    #[test]
    fn a_binding_reached_from_inside_an_invocation_is_refused_rather_than_recursing() {
        let outer = crate::runtime_script::with_host(|_host| {
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

    #[test]
    fn the_hook_catalogue_and_the_artifact_parsers_event_list_are_identical() {
        assert_eq!(
            crate::GAME_HOOK_EVENT_NAMES,
            lyracore_package_delta::HOOK_EVENT_NAMES,
            "run datascripts/runtime-scripts/generate-events.ts after changing events.json"
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
