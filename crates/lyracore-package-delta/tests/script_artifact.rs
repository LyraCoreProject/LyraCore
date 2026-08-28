//! The Script Artifact's shape: what it reads, and the bytes it writes back.
//!
//! The JSON here is written by hand, the way a Datascript generator would emit it, so these
//! exercise the parser rather than a builder that could drift with it.

use lyracore_package_delta::{ScriptArtifact, HOOK_EVENT_NAMES, PACKAGE_SCRIPT_ID_FLOOR};

const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn script(script_id: u32, name: &str, event: &str, priority: i32, enabled: bool) -> String {
    format!(
        r#"{{"script_id":{script_id},"name":"{name}","event":"{event}","priority":{priority},"enabled":{enabled},"source":"grant_xp(event.actor, 10)"}}"#
    )
}

fn artifact(package: &str, scripts: &[String]) -> String {
    format!(
        r#"{{"kind":"script","version":1,"package":"{package}","source_hash":"{HASH_A}","scripts":[{}]}}"#,
        scripts.join(",")
    )
}

#[test]
fn an_artifact_reads_every_member_of_every_script() {
    let parsed = ScriptArtifact::parse(&artifact(
        "example.bolt",
        &[script(100_001, "bolt.greet", "on_login", -5, true)],
    ))
    .expect("artifact parses");

    assert_eq!(parsed.package().as_str(), "example.bolt");
    assert_eq!(parsed.source_hash().as_str(), HASH_A);
    assert_eq!(parsed.scripts().len(), 1);
    let script = &parsed.scripts()[0];
    assert_eq!(script.script_id(), 100_001);
    assert_eq!(script.name().as_str(), "bolt.greet");
    assert_eq!(script.event().as_str(), "on_login");
    assert_eq!(script.priority(), -5);
    assert!(script.enabled());
    assert_eq!(script.source(), "grant_xp(event.actor, 10)");
}

/// A Package that ships nothing is a legal statement, not an empty file to guess about. It is how
/// a Package that used to ship scripts says it no longer does.
#[test]
fn an_artifact_may_ship_no_scripts_at_all() {
    let parsed = ScriptArtifact::parse(&artifact("example.quiet", &[])).expect("artifact parses");

    assert!(parsed.scripts().is_empty());
}

/// A disabled script is applied to the Shard and never invoked, so it has to survive the parse.
#[test]
fn a_disabled_script_is_read_rather_than_dropped() {
    let parsed = ScriptArtifact::parse(&artifact(
        "example.bolt",
        &[script(100_001, "bolt.off", "on_login", 0, false)],
    ))
    .expect("artifact parses");

    assert_eq!(parsed.scripts().len(), 1);
    assert!(!parsed.scripts()[0].enabled());
}

#[test]
fn scripts_come_back_in_identifier_order_however_they_were_written() {
    let parsed = ScriptArtifact::parse(&artifact(
        "example.bolt",
        &[
            script(100_009, "bolt.last", "on_login", 0, true),
            script(100_002, "bolt.first", "on_kill", 0, true),
        ],
    ))
    .expect("artifact parses");

    let ids: Vec<u32> = parsed.scripts().iter().map(|s| s.script_id()).collect();
    assert_eq!(ids, [100_002, 100_009]);
}

/// Every event the Module fires must be bindable, or a package author meets a refusal for an event
/// that plainly exists.
#[test]
fn every_event_in_the_catalogue_is_bindable() {
    for event in HOOK_EVENT_NAMES {
        let json = artifact(
            "example.bolt",
            &[script(PACKAGE_SCRIPT_ID_FLOOR, "bolt.one", event, 0, true)],
        );

        let parsed = ScriptArtifact::parse(&json)
            .unwrap_or_else(|e| panic!("event `{event}` must be bindable: {e}"));

        assert_eq!(parsed.scripts()[0].event().as_str(), *event);
    }
}

// ===========================================================================================
//  Canonical bytes
// ===========================================================================================

#[test]
fn the_canonical_form_has_a_fixed_member_order_and_no_whitespace() {
    let parsed = ScriptArtifact::parse(&artifact(
        "example.bolt",
        &[script(100_001, "bolt.greet", "on_login", 3, true)],
    ))
    .expect("artifact parses");

    assert_eq!(
        parsed.to_canonical_json(),
        concat!(
            r#"{"kind":"script","version":1,"package":"example.bolt","#,
            r#""source_hash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","#,
            r#""scripts":[{"script_id":100001,"name":"bolt.greet","event":"on_login","#,
            r#""priority":3,"enabled":true,"source":"grant_xp(event.actor, 10)"}]}"#,
        )
    );
}

/// The canonical bytes are what a digest, a diff and a provenance row all describe, so how the
/// artifact was spelled must not reach them.
/// Member order, whitespace and the sign on a number are all spellings, not content.
#[test]
fn how_the_artifact_was_written_cannot_change_its_canonical_bytes() {
    let compact = artifact(
        "example.bolt",
        &[script(100_001, "bolt.greet", "on_login", 0, true)],
    );
    let spelled_differently = format!(
        r#"{{
            "version" : 1,
            "scripts" : [ {{
                "source"    : "grant_xp(event.actor, 10)",
                "enabled"   : true,
                "priority"  : 0,
                "event"     : "on_login",
                "name"      : "bolt.greet",
                "script_id" : 100001
            }} ],
            "source_hash" : "{HASH_A}",
            "package" : "example.bolt",
            "kind" : "script"
        }}"#
    );

    let from_compact = ScriptArtifact::parse(&compact).expect("artifact parses");
    let from_spelled = ScriptArtifact::parse(&spelled_differently).expect("artifact parses");

    assert_eq!(
        from_compact.to_canonical_json(),
        from_spelled.to_canonical_json()
    );
}

/// The applier packs several artifacts one per line, which only works because a canonical artifact
/// escapes every control character. Lua source is the member most likely to hold a newline.
#[test]
fn a_canonical_artifact_never_contains_a_raw_newline_however_the_lua_is_written() {
    let json = format!(
        r#"{{"kind":"script","version":1,"package":"example.bolt","source_hash":"{HASH_A}","scripts":[{{"script_id":100001,"name":"bolt.multi","event":"on_login","priority":0,"enabled":true,"source":"local n = 1\nif n > 0 then\n\tgrant_xp(event.actor, n)\nend"}}]}}"#
    );

    let canonical = ScriptArtifact::parse(&json)
        .expect("artifact parses")
        .to_canonical_json();

    assert!(!canonical.contains('\n'), "{canonical}");
    assert!(!canonical.contains('\t'), "{canonical}");
    assert!(
        canonical.contains(r"\n"),
        "the newline is escaped: {canonical}"
    );
}

#[test]
fn canonical_bytes_read_back_as_the_same_artifact() {
    let parsed = ScriptArtifact::parse(&artifact(
        "example.bolt",
        &[
            script(100_002, "bolt.b", "on_kill", -1, false),
            script(100_001, "bolt.a", "on_login", 7, true),
        ],
    ))
    .expect("artifact parses");

    let round_tripped =
        ScriptArtifact::parse(&parsed.to_canonical_json()).expect("canonical bytes parse");

    assert_eq!(round_tripped, parsed);
    assert_eq!(
        round_tripped.to_canonical_json(),
        parsed.to_canonical_json()
    );
}
