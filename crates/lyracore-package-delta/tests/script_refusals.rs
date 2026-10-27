use lyracore_package_delta::ids::{
    CURATED_RESERVED_ID_FLOOR, FIXTURE_RESERVED_ID_CEIL, FIXTURE_RESERVED_ID_FLOOR,
};
use lyracore_package_delta::{
    is_package_script_id, DeltaError, ScriptArtifact, ScriptRefusal, PACKAGE_SCRIPT_ID_CEIL,
    PACKAGE_SCRIPT_ID_FLOOR,
};

const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn artifact_with(scripts: &str) -> String {
    format!(
        r#"{{"kind":"script","version":1,"package":"example.bolt","source_hash":"{HASH_A}","scripts":[{scripts}]}}"#
    )
}

fn one(script_id: u32, name: &str, event: &str, source: &str) -> String {
    format!(
        r#"{{"script_id":{script_id},"name":"{name}","event":"{event}","priority":0,"enabled":true,"source":"{source}"}}"#
    )
}

fn refusal(scripts: &str) -> DeltaError {
    ScriptArtifact::parse(&artifact_with(scripts)).expect_err("the artifact is refused")
}

fn script_refusal(scripts: &str) -> ScriptRefusal {
    match refusal(scripts) {
        DeltaError::Script(refusal) => refusal,
        other => panic!("expected a Script refusal, got {other:?}"),
    }
}

#[test]
fn bytes_that_are_not_json_are_refused() {
    let refusal = ScriptArtifact::parse("{ not even valid }").expect_err("the artifact is refused");

    assert!(matches!(refusal, DeltaError::Malformed(_)), "{refusal:?}");
}

#[test]
fn an_unknown_version_is_refused() {
    let json = format!(
        r#"{{"kind":"script","version":9,"package":"example.bolt","source_hash":"{HASH_A}","scripts":[]}}"#
    );

    assert_eq!(
        ScriptArtifact::parse(&json).expect_err("the artifact is refused"),
        DeltaError::UnsupportedVersion {
            found: "9".to_owned()
        }
    );
}

#[test]
fn an_unknown_member_at_the_root_is_refused_rather_than_ignored() {
    let json = format!(
        r#"{{"kind":"script","version":1,"package":"example.bolt","source_hash":"{HASH_A}","scripts":[],"notes":"hi"}}"#
    );

    assert_eq!(
        ScriptArtifact::parse(&json).expect_err("the artifact is refused"),
        DeltaError::UnexpectedMember {
            path: "the artifact root".to_owned(),
            member: "notes".to_owned()
        }
    );
}

#[test]
fn an_unknown_member_on_a_script_is_refused_rather_than_ignored() {
    let script = concat!(
        r#"{"script_id":100001,"name":"bolt.a","event":"on_login","priority":0,"#,
        r#""enabled":true,"source":"grant_xp(event.actor, 1)","once":true}"#,
    );

    assert_eq!(
        refusal(script),
        DeltaError::UnexpectedMember {
            path: "scripts[0]".to_owned(),
            member: "once".to_owned()
        }
    );
}

#[test]
fn a_script_missing_a_member_is_refused_by_that_members_name() {
    let script =
        r#"{"script_id":100001,"name":"bolt.a","event":"on_login","priority":0,"enabled":true}"#;

    assert_eq!(
        refusal(script),
        DeltaError::MissingMember {
            path: "scripts[0].source".to_owned()
        }
    );
}

#[test]
fn a_script_below_the_package_band_is_refused() {
    let refusal = refusal(&one(
        PACKAGE_SCRIPT_ID_FLOOR - 1,
        "bolt.a",
        "on_login",
        "grant_xp(event.actor, 1)",
    ));

    assert_eq!(
        refusal,
        DeltaError::Script(ScriptRefusal::IdNotInPackageBand {
            script_id: PACKAGE_SCRIPT_ID_FLOOR - 1
        })
    );
    assert!(
        refusal
            .to_string()
            .contains(&PACKAGE_SCRIPT_ID_FLOOR.to_string()),
        "{refusal}"
    );
}

#[test]
fn a_script_above_the_package_band_is_refused() {
    assert_eq!(
        script_refusal(&one(
            PACKAGE_SCRIPT_ID_CEIL + 1,
            "bolt.a",
            "on_login",
            "grant_xp(event.actor, 1)",
        )),
        ScriptRefusal::IdNotInPackageBand {
            script_id: PACKAGE_SCRIPT_ID_CEIL + 1
        }
    );
}

#[test]
fn script_zero_is_not_a_script() {
    assert_eq!(
        script_refusal(&one(0, "bolt.a", "on_login", "grant_xp(event.actor, 1)")),
        ScriptRefusal::IdNotInPackageBand { script_id: 0 }
    );
}

#[test]
fn both_ends_of_the_package_band_are_shippable() {
    for script_id in [PACKAGE_SCRIPT_ID_FLOOR, PACKAGE_SCRIPT_ID_CEIL] {
        assert!(is_package_script_id(script_id));
        ScriptArtifact::parse(&artifact_with(&one(
            script_id,
            "bolt.a",
            "on_login",
            "grant_xp(event.actor, 1)",
        )))
        .unwrap_or_else(|e| panic!("script {script_id} must be shippable: {e}"));
    }
}

#[test]
fn no_reserved_identifier_is_inside_the_package_script_band() {
    for reserved in [
        FIXTURE_RESERVED_ID_FLOOR,
        FIXTURE_RESERVED_ID_CEIL,
        CURATED_RESERVED_ID_FLOOR,
    ] {
        assert!(
            !is_package_script_id(reserved),
            "reserved identifier {reserved} must not be shippable as a script"
        );
    }
}

#[test]
fn a_name_that_is_not_a_name_is_refused() {
    for bad in ["", "Bolt.Greet", "bolt greet", "bolt/greet"] {
        assert_eq!(
            script_refusal(&one(100_001, bad, "on_login", "grant_xp(event.actor, 1)")),
            ScriptRefusal::InvalidName {
                found: bad.to_owned()
            }
        );
    }
}

#[test]
fn an_event_outside_the_hook_catalogue_is_refused_and_names_the_ones_that_exist() {
    let refusal = script_refusal(&one(
        100_001,
        "bolt.a",
        "on_tuesday",
        "grant_xp(event.actor, 1)",
    ));

    assert_eq!(
        refusal,
        ScriptRefusal::UnknownEvent {
            found: "on_tuesday".to_owned(),
            package: "example.bolt".to_owned()
        }
    );
    let message = refusal.to_string();
    assert!(message.contains("`on_login`"), "{message}");
    assert!(message.contains("`on_go_used`"), "{message}");
}

#[test]
fn a_package_event_belonging_to_another_package_is_refused() {
    let refusal = script_refusal(&one(
        100_001,
        "bolt.a",
        "example.other.pick_target",
        "grant_xp(event.actor, 1)",
    ));

    assert_eq!(
        refusal,
        ScriptRefusal::UnknownEvent {
            found: "example.other.pick_target".to_owned(),
            package: "example.bolt".to_owned()
        }
    );
    let message = refusal.to_string();
    assert!(message.contains("`example.bolt.<name>`"), "{message}");
}

#[test]
fn a_package_event_with_a_misshapen_name_is_refused() {
    for bad in [
        "example.bolt.",
        "example.bolt.Pick",
        "example.bolt.2nd",
        "example.bolt.pick-target",
        "example.bolt.pick.target",
        "example.bolt",
    ] {
        assert_eq!(
            script_refusal(&one(100_001, "bolt.a", bad, "grant_xp(event.actor, 1)")),
            ScriptRefusal::UnknownEvent {
                found: bad.to_owned(),
                package: "example.bolt".to_owned()
            }
        );
    }
}

#[test]
fn a_script_with_no_source_is_refused() {
    assert_eq!(
        script_refusal(&one(100_001, "bolt.a", "on_login", "")),
        ScriptRefusal::EmptySource {
            name: "bolt.a".to_owned()
        }
    );
}

#[test]
fn a_script_whose_source_is_only_whitespace_is_refused() {
    assert_eq!(
        script_refusal(&one(100_001, "bolt.a", "on_login", r"  \n\t  ")),
        ScriptRefusal::EmptySource {
            name: "bolt.a".to_owned()
        }
    );
}

#[test]
fn one_package_shipping_two_scripts_at_one_identifier_is_refused() {
    let scripts = [
        one(100_001, "bolt.a", "on_login", "grant_xp(event.actor, 1)"),
        one(100_001, "bolt.b", "on_kill", "grant_xp(event.actor, 2)"),
    ]
    .join(",");

    assert_eq!(
        script_refusal(&scripts),
        ScriptRefusal::DuplicateId { script_id: 100_001 }
    );
}

#[test]
fn one_package_shipping_two_scripts_under_one_name_is_refused() {
    let scripts = [
        one(100_001, "bolt.a", "on_login", "grant_xp(event.actor, 1)"),
        one(100_002, "bolt.a", "on_kill", "grant_xp(event.actor, 2)"),
    ]
    .join(",");

    assert_eq!(
        script_refusal(&scripts),
        ScriptRefusal::DuplicateName {
            name: "bolt.a".to_owned()
        }
    );
}
