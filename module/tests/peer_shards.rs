mod support;

use std::time::Duration;

use support::{module_bytes, poll_until, Standalone};

fn identity(shard: &Standalone, database: &str) -> String {
    let rows = shard.query_database_rows(database, "SELECT database_identity FROM st_module");
    let hex = rows[0]["database_identity"]
        .trim_start_matches("0x")
        .to_owned();
    format!(r#"{{"__identity__":"0x{hex}"}}"#)
}

fn confirmed(shard: &Standalone, database: &str) -> Option<String> {
    let rows = shard.query_database_rows(database, "SELECT confirmed FROM game_peer_shard");
    rows.first().map(|row| row["confirmed"].clone())
}

fn settled(shard: &Standalone, database: &str) -> bool {
    shard
        .query_database_rows(database, "SELECT id FROM game_peer_hello")
        .is_empty()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn peer_shards_confirm_each_other_once_both_list_the_other() {
    let mut shard = Standalone::start_persistent("peer-shards");
    shard.publish_module();
    let first = shard.shard_name().to_owned();
    let second = format!("{first}-second");
    shard.publish_named_module_bytes(&second, module_bytes());
    for database in [&first, &second] {
        shard.assert_call_database(database, "claim_operator", &[]);
    }
    let (first_identity, second_identity) = (identity(&shard, &first), identity(&shard, &second));

    shard.assert_call_database(
        &first,
        "operator_add_peer_shard",
        &[&second_identity, r#""second""#],
    );
    assert!(
        poll_until(Duration::from_secs(20), || settled(&shard, &first)),
        "the first Shard's hello was never answered"
    );
    assert_eq!(
        confirmed(&shard, &first).as_deref(),
        Some("false"),
        "a Shard that does not list the caller refuses its hello"
    );

    shard.assert_call_database(
        &second,
        "operator_add_peer_shard",
        &[&first_identity, r#""first""#],
    );
    assert!(
        poll_until(Duration::from_secs(20), || {
            settled(&shard, &second)
                && confirmed(&shard, &first).as_deref() == Some("true")
                && confirmed(&shard, &second).as_deref() == Some("true")
        }),
        "both Shards should confirm once each lists the other: first {:?}, second {:?}",
        confirmed(&shard, &first),
        confirmed(&shard, &second),
    );
}
