//! Operator lever for the Runtime Script Host: run one Runtime Script against the live world.
//!
//! Package storage and event bindings are separate work, so the source arrives as a reducer
//! argument. This is the only path that exercises the host against real gameplay operations
//! instead of a Fake, which makes it the way to eyeball the host on a realm.

use spacetimedb::{log, reducer, ReducerContext};

use crate::runtime_script::{run_event, CoreEffects, RuntimeScript};

/// Run `source` as a Runtime Script for `event`. Commits what the script stages if it succeeds;
/// refuses with the bounded diagnostic if it does not.
#[reducer]
pub fn debug_run_runtime_script(
    ctx: &ReducerContext,
    script_name: String,
    event: String,
    source: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let Some((diagnostics, compilations)) = crate::runtime_script::with_host(|host| {
        let diagnostics = run_event(
            host,
            &mut CoreEffects { ctx },
            &event,
            &[RuntimeScript {
                name: &script_name,
                source: &source,
            }],
        );
        (diagnostics, host.compilations())
    }) else {
        return Err("the runtime script host is already running a script".to_string());
    };
    match diagnostics.first() {
        Some(diagnostic) => Err(diagnostic.to_string()),
        None => {
            // The compilation count is how an operator sees the compiler cache working: call this
            // twice with the same source and it must not move.
            log::info!(
                "debug_run_runtime_script: `{script_name}` on `{event}` committed \
                 ({compilations} chunks compiled since this module instance started)"
            );
            Ok(())
        }
    }
}
