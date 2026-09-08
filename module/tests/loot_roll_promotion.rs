mod support;

use std::collections::BTreeMap;
use std::sync::Barrier;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use support::Standalone;

const CORPSE: u64 = 5_090_001;
const ITEM: u32 = 5_090_002;
const VOTERS: &str = "[5090003,5090004,5090005]";
const SOURCE: &str = r#"{"__identity__":"0x1"}"#;
const ZERO: &str = r#"{"__identity__":"0x0"}"#;

struct Promotion {
    corpse: u64,
    slot: u8,
    item: u32,
    deadline: i64,
    voters: String,
    random_property_id: u32,
    source: String,
    source_roll_id: u64,
}

impl Default for Promotion {
    fn default() -> Self {
        Self {
            corpse: CORPSE,
            slot: 0,
            item: ITEM,
            deadline: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64
                + 600_000_000,
            voters: VOTERS.into(),
            random_property_id: 117,
            source: SOURCE.into(),
            source_roll_id: 10,
        }
    }
}

impl Promotion {
    fn args(&self) -> Vec<String> {
        vec![
            "0".into(),
            self.corpse.to_string(),
            self.slot.to_string(),
            self.item.to_string(),
            support::actor("0"),
            "0".into(),
            self.deadline.to_string(),
            self.voters.clone(),
            self.random_property_id.to_string(),
            self.source.clone(),
            self.source_roll_id.to_string(),
        ]
    }

    fn send(&self, standalone: &Standalone) {
        let args = self.args();
        standalone.assert_call(
            "realm_loot_op",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }

    fn assert_refused(&self, standalone: &Standalone) {
        let args = self.args();
        let output = standalone.call(
            "realm_loot_op",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert!(!output.status.success());
        let message = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(message.contains("loot:roll_unavailable"), "{message}");
    }
}

fn fixture(name: &str) -> Standalone {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone
}

fn vote(standalone: &Standalone, voter: &str, kind: &str) {
    standalone.assert_call(
        "realm_loot_op",
        &[
            "1",
            &CORPSE.to_string(),
            "0",
            "0",
            &support::actor(voter),
            kind,
            "0",
            "[]",
            "0",
            ZERO,
            "0",
        ],
    );
}

fn resolve(standalone: &Standalone) {
    for voter in ["5090003", "5090004", "5090005"] {
        vote(standalone, voter, "0");
    }
    assert_no_active_roll(standalone);
}

fn assert_no_active_roll(standalone: &Standalone) {
    assert!(
        standalone
            .query_rows("SELECT * FROM game_loot_roll")
            .is_empty(),
        "replaying a resolved START must not recreate an active Loot Roll"
    );
    assert!(
        votes(standalone).is_empty(),
        "resolved Loot Roll votes must stay deleted"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn resolved_promotion_does_not_recreate_roll_or_votes() {
    let mut standalone = fixture("loot-roll-replay");
    let mut promotion = Promotion::default();
    promotion.send(&standalone);
    resolve(&standalone);
    let receipt = standalone.query_rows("SELECT * FROM game_loot_roll_promotion_receipt");
    assert_eq!(receipt.len(), 1);
    promotion.send(&standalone);
    assert_no_active_roll(&standalone);
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_loot_roll_promotion_receipt"),
        receipt
    );

    // A normal publish retains the receipt after the active roll and votes have been cleaned up.
    standalone.publish_module();
    promotion.send(&standalone);
    assert_no_active_roll(&standalone);

    promotion.source_roll_id = 11;
    promotion.send(&standalone);
    let active = standalone.query_rows("SELECT * FROM game_loot_roll");
    let active_votes = votes(&standalone);
    assert_eq!(active.len(), 1);
    assert_eq!(active_votes.len(), 3);
    promotion.source_roll_id = 10;
    promotion.send(&standalone);
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_loot_roll"),
        active
    );
    assert_eq!(votes(&standalone), active_votes);
    resolve(&standalone);
    for older in [11, 10, 1] {
        promotion.source_roll_id = older;
        promotion.send(&standalone);
        assert_no_active_roll(&standalone);
    }
    let retained = standalone.query_rows("SELECT * FROM game_loot_roll_promotion_receipt");
    assert_eq!(retained.len(), 1, "a later generation replaces its receipt");
    assert_eq!(retained[0]["id"], receipt[0]["id"]);
    assert_eq!(retained[0]["source_roll_id"], "11");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn concurrent_promotions_preserve_one_roll_and_its_votes() {
    let standalone = fixture("loot-roll-promotion");
    let mut promotion = Promotion::default();
    let ready = Barrier::new(2);
    thread::scope(|threads| {
        for _ in 0..2 {
            threads.spawn(|| {
                ready.wait();
                promotion.send(&standalone);
            });
        }
    });
    let rolls = standalone.query_rows("SELECT * FROM game_loot_roll");
    assert_eq!(
        rolls.len(),
        1,
        "concurrent START calls must share one live Loot Roll"
    );
    assert_eq!(rolls[0]["random_property_id"], "117");
    let initial_votes = votes(&standalone);
    assert_eq!(initial_votes.len(), 3);
    let mut voters: Vec<_> = initial_votes
        .iter()
        .map(|v| v["voter_guid"].as_str())
        .collect();
    voters.sort_unstable();
    assert_eq!(voters, ["5090003", "5090004", "5090005"]);
    for vote in initial_votes {
        assert_eq!(vote["roll_id"], rolls[0]["id"]);
        assert_eq!(vote["voted"], "false");
    }
    vote(&standalone, "5090003", "1");
    let voted = votes(&standalone);
    assert_eq!(voted.iter().filter(|v| v["voted"] == "true").count(), 1);
    promotion.item += 1;
    promotion.random_property_id += 1;
    promotion.deadline += 60_000_000;
    promotion.voters = "[5090006]".into();
    promotion.send(&standalone);
    assert_eq!(standalone.query_rows("SELECT * FROM game_loot_roll"), rolls);
    assert_eq!(votes(&standalone), voted);

    // Sequence order belongs to each source/corpse/slot key, not to all promotions from a source.
    promotion.voters = VOTERS.into();
    promotion.source_roll_id = 1;
    promotion.slot = 1;
    promotion.send(&standalone);
    promotion.corpse += 1;
    promotion.slot = 0;
    promotion.send(&standalone);
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_loot_roll").len(),
        3
    );
    assert_eq!(votes(&standalone).len(), 9);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn newer_generation_waits_without_consuming_its_receipt() {
    let standalone = fixture("loot-roll-generation");
    let mut promotion = Promotion::default();
    promotion.send(&standalone);
    let rolls = standalone.query_rows("SELECT * FROM game_loot_roll");
    let initial_votes = votes(&standalone);
    let receipt = standalone.query_rows("SELECT * FROM game_loot_roll_promotion_receipt");
    promotion.source_roll_id += 1;
    promotion.assert_refused(&standalone);
    assert_eq!(standalone.query_rows("SELECT * FROM game_loot_roll"), rolls);
    assert_eq!(votes(&standalone), initial_votes);
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_loot_roll_promotion_receipt"),
        receipt
    );
    resolve(&standalone);
    promotion.send(&standalone);
    assert_eq!(votes(&standalone).len(), 3);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn source_namespaces_remain_independent_after_resolution() {
    let standalone = fixture("loot-roll-source");
    let mut promotion = Promotion::default();
    promotion.send(&standalone);
    resolve(&standalone);
    promotion.source = r#"{"__identity__":"0x2"}"#.into();
    promotion.source_roll_id = 1;
    promotion.send(&standalone);
    resolve(&standalone);
    promotion.source = SOURCE.into();
    promotion.source_roll_id = 10;
    promotion.send(&standalone);
    assert_no_active_roll(&standalone);
    assert_eq!(
        standalone
            .query_rows("SELECT * FROM game_loot_roll_promotion_receipt")
            .len(),
        2
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn missing_promotion_identity_is_refused_without_rows() {
    let standalone = fixture("loot-roll-missing-identity");
    for (source, source_roll_id) in [(ZERO, 10), (SOURCE, 0)] {
        Promotion {
            source: source.into(),
            source_roll_id,
            ..Promotion::default()
        }
        .assert_refused(&standalone);
    }
    assert_no_active_roll(&standalone);
    assert!(standalone
        .query_rows("SELECT * FROM game_loot_roll_promotion_receipt")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn staging_identity_survives_publish_and_old_cleanup_cannot_clear_a_later_roll() {
    let mut source = fixture("loot-roll-staging");
    let realm = fixture("loot-roll-staging-realm");
    source.assert_call("debug_stage_loot_roll_fixture", &[]);
    let first = source.query_rows("SELECT * FROM game_loot_roll").remove(0);
    assert_eq!(first["random_property_id"], "117");
    let owner = source.query_rows("SELECT * FROM game_operator").remove(0);
    assert_ne!(
        first["promotion_source"], owner["identity"],
        "source identity must not be the Owner Token identity"
    );
    assert!(!first["promotion_source"]
        .trim_start_matches("0x")
        .chars()
        .all(|c| c == '0'));
    let mut promotion = Promotion {
        source: format!(r#"{{"__identity__":"{}"}}"#, first["promotion_source"]),
        source_roll_id: first["id"].parse().unwrap(),
        deadline: first["deadline_micros"].parse().unwrap(),
        ..Promotion::default()
    };
    promotion.send(&realm);
    resolve(&realm);
    source.publish_module();
    assert_eq!(source.query_rows("SELECT * FROM game_loot_roll")[0], first);
    promotion.send(&realm);
    assert_no_active_roll(&realm);
    source.assert_call(
        "clear_promoted_loot_roll",
        &[&first["id"], &support::actor("0")],
    );
    assert_no_active_roll(&source);
    source.assert_call("debug_stage_loot_roll_fixture", &[]);
    let next = source.query_rows("SELECT * FROM game_loot_roll").remove(0);
    assert!(next["id"].parse::<u64>().unwrap() > first["id"].parse::<u64>().unwrap());
    assert_eq!(next["promotion_source"], first["promotion_source"]);
    promotion.source_roll_id = next["id"].parse().unwrap();
    promotion.deadline = next["deadline_micros"].parse().unwrap();
    promotion.send(&realm);
    assert_eq!(votes(&realm).len(), 3);
    source.assert_call(
        "clear_promoted_loot_roll",
        &[&first["id"], &support::actor("0")],
    );
    assert_eq!(source.query_rows("SELECT * FROM game_loot_roll")[0], next);
    assert_eq!(votes(&source).len(), 3);
}

fn votes(standalone: &Standalone) -> Vec<BTreeMap<String, String>> {
    let mut rows = standalone.query_rows("SELECT * FROM game_loot_roll_vote");
    rows.sort_by(|a, b| a["id"].cmp(&b["id"]));
    rows
}
