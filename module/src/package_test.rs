#![cfg_attr(not(has_packages), allow(dead_code, unused_imports))]

//! Steps a Package's own unit tests need. The root exists only under `cfg(test)`, and the Package
//! API lint accepts it only in a Package file gated on `#![cfg(test)]`.

use crate::runtime_script::{ask_event, EffectSink, RuntimeScriptHost, ScriptEvent};

pub(crate) use crate::runtime_script::{EntityView, RuntimeScript};
pub(crate) use crate::test_scan::read_scanned;
pub(crate) use lyracore_test_support::source_scan::{code_of, shape_of};

/// Run `scripts` in order for one event on a fresh Runtime Script Host and return the Script
/// Answer, read as `script_binding::ask` reads it. Staged Effects are discarded. Any Script
/// Diagnostic is an error, one diagnostic per line.
pub(crate) fn ask_offline(
    event: &str,
    actor: Option<EntityView>,
    target: Option<EntityView>,
    scripts: &[RuntimeScript<'_>],
) -> Result<Option<f64>, String> {
    let event = ScriptEvent {
        name: event.to_string(),
        actor,
        target,
        ..ScriptEvent::default()
    };
    let (diagnostics, answer) = ask_event(
        &mut RuntimeScriptHost::new(),
        &mut NoEffects,
        &event,
        scripts,
    );
    if diagnostics.is_empty() {
        Ok(answer)
    } else {
        Err(diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

/// Parse a Script Artifact and ask its enabled Event Bindings in priority and identifier order.
/// Staged Effects are discarded. Parse failures and Script Diagnostics return their text.
pub(crate) fn ask_artifact_offline(
    event: &str,
    actor: Option<EntityView>,
    target: Option<EntityView>,
    artifact_json: &str,
) -> Result<Option<f64>, String> {
    let artifact = lyracore_package_delta::ScriptArtifact::parse(artifact_json)
        .map_err(|failure| failure.to_string())?;
    let mut bound: Vec<_> = artifact
        .scripts()
        .iter()
        .filter(|script| script.enabled() && script.event().as_str() == event)
        .collect();
    bound.sort_by_key(|script| (script.priority(), script.script_id()));
    let scripts: Vec<_> = bound
        .iter()
        .map(|script| RuntimeScript {
            name: script.name().as_str(),
            source: script.source(),
        })
        .collect();
    ask_offline(event, actor, target, &scripts)
}

struct NoEffects;

impl EffectSink for NoEffects {
    fn grant_xp(&mut self, _character_guid: u64, _amount: u32) {}
    fn heal(&mut self, _healer_guid: u64, _target_guid: u64, _amount: u32) {}
    fn send_chat(&mut self, _recipient_guid: u64, _message: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(source: &str) -> RuntimeScript<'_> {
        RuntimeScript {
            name: "probe",
            source,
        }
    }

    #[test]
    fn an_offline_ask_returns_the_first_answer() {
        let scripts = [
            script("return nil"),
            script("return 15"),
            script("return 40"),
        ];
        assert_eq!(
            ask_offline("pkg.flee_at", None, None, &scripts),
            Ok(Some(15.0))
        );
    }

    #[test]
    fn an_artifact_ask_selects_enabled_bindings_in_dispatch_order() {
        let scripts = [
            (100_001, "pkg.slow", "pkg.flee_at", 5, true, "return 40"),
            (
                100_002,
                "pkg.disabled",
                "pkg.flee_at",
                -10,
                false,
                "error('disabled')",
            ),
            (100_003, "pkg.first", "pkg.flee_at", -1, true, "return 10"),
            (100_004, "pkg.later", "pkg.flee_at", -1, true, "return 15"),
            (
                100_005,
                "pkg.other",
                "pkg.other",
                -20,
                true,
                "error('another event')",
            ),
        ];
        let artifact = serde_json::json!({
            "kind": "script",
            "version": 1,
            "package": "pkg",
            "source_hash": "0".repeat(64),
            "scripts": scripts.map(|(script_id, name, event, priority, enabled, source)| {
                serde_json::json!({ "script_id": script_id, "name": name, "event": event,
                    "priority": priority, "enabled": enabled, "source": source })
            }),
        });
        assert_eq!(
            ask_artifact_offline("pkg.flee_at", None, None, &artifact.to_string()),
            Ok(Some(10.0))
        );
    }

    #[test]
    fn an_offline_ask_refuses_a_failing_script() {
        let failure = ask_offline("pkg.flee_at", None, None, &[script("error('broken')")])
            .expect_err("a raising script is a diagnostic");
        assert!(
            failure.contains("runtime script `probe` on `pkg.flee_at`"),
            "{failure}"
        );
    }
}
