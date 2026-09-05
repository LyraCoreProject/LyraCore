mod support;

use std::collections::BTreeMap;
use std::sync::Barrier;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use support::Standalone;

const CORPSE: u64 = 5_090_001;
const ITEM: u32 = 5_090_002;
const VOTERS: &str = "[5090003,5090004,5090005]";

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn concurrent_promotions_preserve_one_roll_and_its_votes() {
    let standalone = Standalone::start("loot-roll-promotion");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    let deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
        + 600_000_000;
    let ready = Barrier::new(2);
    thread::scope(|threads| {
        for _ in 0..2 {
            threads.spawn(|| {
                ready.wait();
                promote(&standalone, CORPSE, 0, ITEM, deadline, VOTERS);
            });
        }
    });

    let rolls = standalone.query_rows("SELECT * FROM game_loot_roll");
    assert_eq!(
        rolls.len(),
        1,
        "concurrent START calls must share one live Loot Roll: {rolls:?}"
    );
    let roll_id = &rolls[0]["id"];
    let initial_votes = standalone.query_rows("SELECT * FROM game_loot_roll_vote");
    assert_eq!(initial_votes.len(), 3);
    let mut voters: Vec<_> = initial_votes
        .iter()
        .map(|v| v["voter_guid"].as_str())
        .collect();
    voters.sort_unstable();
    assert_eq!(voters, ["5090003", "5090004", "5090005"]);
    for vote in initial_votes {
        assert_eq!(&vote["roll_id"], roll_id);
        assert_eq!(vote["voted"], "false");
    }

    standalone.assert_call(
        "realm_loot_op",
        &[
            "1",
            &CORPSE.to_string(),
            "0",
            "0",
            "5090003",
            "1",
            "0",
            "[]",
        ],
    );
    let voted = votes(&standalone);
    assert_eq!(voted.iter().filter(|v| v["voted"] == "true").count(), 1);
    promote(
        &standalone,
        CORPSE,
        0,
        ITEM + 1,
        deadline + 60_000_000,
        "[5090006]",
    );
    assert_eq!(standalone.query_rows("SELECT * FROM game_loot_roll"), rolls);
    assert_eq!(votes(&standalone), voted);

    promote(&standalone, CORPSE, 1, ITEM, deadline, VOTERS);
    promote(&standalone, CORPSE + 1, 0, ITEM, deadline, VOTERS);
    assert_eq!(
        standalone.query_rows("SELECT * FROM game_loot_roll").len(),
        3
    );
    assert_eq!(
        standalone
            .query_rows("SELECT * FROM game_loot_roll_vote")
            .len(),
        9
    );
}

fn promote(standalone: &Standalone, corpse: u64, slot: u8, item: u32, deadline: i64, voters: &str) {
    standalone.assert_call(
        "realm_loot_op",
        &[
            "0",
            &corpse.to_string(),
            &slot.to_string(),
            &item.to_string(),
            "0",
            "0",
            &deadline.to_string(),
            voters,
        ],
    );
}

fn votes(standalone: &Standalone) -> Vec<BTreeMap<String, String>> {
    let mut rows = standalone.query_rows("SELECT * FROM game_loot_roll_vote");
    rows.sort_by(|a, b| a["id"].cmp(&b["id"]));
    rows
}
