mod support;

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use spacetimedb::Timestamp;
use support::Standalone;

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and waits for the five-minute session reaper"]
fn logon_renews_expired_sessions_and_the_scheduler_reaps_only_expired_rows() {
    let standalone = Standalone::start("session-expiry");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    assert!(standalone
        .query_rows("SELECT * FROM game_session_reaper_schedule")
        .is_empty());

    for table in [
        "game_account",
        "game_session",
        "game_session_reaper_schedule",
    ] {
        let output = standalone.sql_anonymous(&format!("SELECT * FROM {table}"));
        assert!(!output.status.success(), "anonymous read exposed {table}");
    }
    assert!(standalone
        .sql_anonymous("SELECT * FROM game_realm")
        .status
        .success());

    let renewed = provision(&standalone, "SESSIONRENEW");
    let expired = provision(&standalone, "SESSIONEXPIRE");
    establish(&standalone, &renewed, 3);
    let first = session(&standalone, &renewed);
    assert_lifetime(&first);
    let schedule = standalone.query_rows("SELECT * FROM game_session_reaper_schedule");
    assert_eq!(schedule.len(), 1);
    assert!(schedule[0]["scheduled_at"].contains("+300.000000"));

    standalone.assert_call("debug_expire_session", &[&renewed]);
    let stale = session(&standalone, &renewed);
    assert_eq!(stale["expires_at"], stale["created_at"]);
    establish(&standalone, &renewed, 4);
    let fresh = session(&standalone, &renewed);
    assert_lifetime(&fresh);
    assert_ne!(fresh["session_key"], first["session_key"]);
    assert!(timestamp(&fresh["created_at"]) > timestamp(&first["created_at"]));
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_session_reaper_schedule"),
        schedule,
        "another logon must preserve the existing reaper schedule"
    );

    establish(&standalone, &expired, 5);
    standalone.assert_call("debug_expire_session", &[&expired]);
    let before = session(&standalone, &expired);
    let output = standalone.call(
        "reap_sessions",
        &[r#"{"scheduled_id":0,"scheduled_at":{"Interval":{"__time_duration_micros__":300000000}}}"#],
    );
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "an external caller ran the reaper"
    );
    assert!(message.contains("scheduler only"), "{message}");
    assert_eq!(session(&standalone, &expired), before);

    let deadline = Instant::now() + Duration::from_secs(330);
    while !standalone
        .query_rows(&format!(
            "SELECT * FROM game_session WHERE account_id = {expired}"
        ))
        .is_empty()
    {
        assert!(
            Instant::now() < deadline,
            "the scheduled reaper did not run"
        );
        thread::sleep(Duration::from_secs(2));
    }
    assert_eq!(session(&standalone, &renewed), fresh);
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_session_reaper_schedule"),
        schedule,
        "the recurring schedule remains armed after its first invocation"
    );
    establish(&standalone, &expired, 6);
    assert_lifetime(&session(&standalone, &expired));
}

fn provision(standalone: &Standalone, name: &str) -> String {
    let credential = serde_json::to_string(&vec![1u8; 32]).unwrap();
    standalone.assert_call(
        "provision_account",
        &[
            &serde_json::to_string(name).unwrap(),
            &credential,
            &credential,
        ],
    );
    let rows = standalone.query_rows(&format!(
        "SELECT id FROM game_account WHERE username = '{name}'"
    ));
    assert_eq!(rows.len(), 1);
    rows[0]["id"].clone()
}

fn establish(standalone: &Standalone, account: &str, key: u8) {
    let key = serde_json::to_string(&vec![key; 40]).unwrap();
    standalone.assert_call(
        "establish_session",
        &[account, &key, r#"{"__identity__":"0x1"}"#],
    );
}

fn session(standalone: &Standalone, account: &str) -> BTreeMap<String, String> {
    let mut rows = standalone.query_rows(&format!(
        "SELECT * FROM game_session WHERE account_id = {account}"
    ));
    assert_eq!(rows.len(), 1);
    rows.pop().unwrap()
}

fn timestamp(value: &str) -> i64 {
    Timestamp::parse_from_rfc3339(value)
        .unwrap_or_else(|error| panic!("invalid durable timestamp {value:?}: {error}"))
        .to_micros_since_unix_epoch()
}

fn assert_lifetime(row: &BTreeMap<String, String>) {
    assert_eq!(
        timestamp(&row["expires_at"]) - timestamp(&row["created_at"]),
        3_600_000_000,
        "a logon starts one fresh hour"
    );
}
