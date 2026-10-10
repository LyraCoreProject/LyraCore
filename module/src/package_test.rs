#![cfg_attr(not(has_packages), allow(dead_code, unused_imports))]

//! Steps a Package's own unit tests need. The root exists only under `cfg(test)`, and the Package
//! API lint accepts it only in a Package file gated on `#![cfg(test)]`.

use crate::runtime_script::{ask_event, EffectSink, RuntimeScriptHost, ScriptEvent};

pub(crate) use crate::runtime_script::{EntityView, RuntimeScript};
pub(crate) use crate::test_scan::{code_of, read_scanned, shape_of};

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
    fn an_offline_ask_refuses_a_failing_script() {
        let failure = ask_offline("pkg.flee_at", None, None, &[script("error('broken')")])
            .expect_err("a raising script is a diagnostic");
        assert!(
            failure.contains("runtime script `probe` on `pkg.flee_at`"),
            "{failure}"
        );
    }
}
