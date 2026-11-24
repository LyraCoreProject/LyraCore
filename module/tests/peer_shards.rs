mod support;

use std::time::Duration;

use support::{module_bytes, poll_until, Standalone};

// `st_module` stores the database identity as a u256, which SQL prints in decimal.
fn identity_hex(shard: &Standalone, database: &str) -> String {
    let rows = shard.query_database_rows(database, "SELECT database_identity FROM st_module");
    let mut limbs = [0u64; 4];
    for digit in rows[0]["database_identity"].bytes() {
        let mut carry = u128::from(digit - b'0');
        for limb in limbs.iter_mut().rev() {
            let wide = u128::from(*limb) * 10 + carry;
            *limb = wide as u64;
            carry = wide >> 64;
        }
    }
    let hex: String = limbs.iter().map(|limb| format!("{limb:016x}")).collect();
    format!("0x{hex}")
}

fn identity(shard: &Standalone, database: &str) -> String {
    format!(r#"{{"__identity__":"{}"}}"#, identity_hex(shard, database))
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

fn count(shard: &Standalone, database: &str, table: &str) -> usize {
    shard
        .query_database_rows(database, &format!("SELECT id FROM {table}"))
        .len()
}

// An invitation that the target refuses is retried every second. The retry stops once the
// Operator removes the target from `game_peer_shard`.
#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_guild_retry_stops_when_its_target_leaves_the_peer_shards() {
    let mut shard = Standalone::start_persistent("package-guild-retry-peer");
    shard.publish_module();
    let first = shard.shard_name().to_owned();
    let second = format!("{first}-second");
    shard.publish_named_module_bytes(&second, module_bytes());
    for database in [&first, &second] {
        shard.assert_call_database(database, "claim_operator", &[]);
    }
    // Only the first Shard lists the second, so the second refuses everything the first sends.
    shard.assert_call_database(
        &first,
        "operator_add_peer_shard",
        &[&identity(&shard, &second), r#""second""#],
    );

    shard.assert_call_database(
        &first,
        "debug_queue_package_guild_invitation",
        &[&identity(&shard, &second), "9", "1"],
    );
    assert!(
        poll_until(Duration::from_secs(20), || count(
            &shard,
            &first,
            "game_package_guild_retry"
        ) == 1),
        "the refused invitation was not scheduled for retry"
    );

    shard.assert_call_database(
        &first,
        "operator_remove_peer_shard",
        &[&identity(&shard, &second)],
    );
    let idle = |shard: &Standalone| {
        count(shard, &first, "game_package_guild_retry") == 0
            && count(shard, &first, "game_package_guild_delivery") == 0
    };
    assert!(
        poll_until(Duration::from_secs(20), || idle(&shard)),
        "the retry kept sending to a Shard that is no longer a Peer Shard"
    );
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(250));
        assert!(idle(&shard), "the dropped retry was queued again");
    }
}

// A Package Character creation result that the Realm refuses is retried every second. Once the
// Operator removes the Realm from `game_peer_shard`, the retry stops and the request settles as
// created, because the Character already exists on the World Shard.
#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_creation_retry_stops_and_settles_when_its_target_leaves_the_peer_shards() {
    let mut shard = Standalone::start_persistent("package-creation-retry-peer");
    shard.publish_module();
    let first = shard.shard_name().to_owned();
    let second = format!("{first}-second");
    shard.publish_named_module_bytes(&second, module_bytes());
    for database in [&first, &second] {
        shard.assert_call_database(database, "claim_operator", &[]);
    }
    // Only the first Shard lists the second, so the second refuses everything the first sends.
    shard.assert_call_database(
        &first,
        "operator_add_peer_shard",
        &[&identity(&shard, &second), r#""second""#],
    );

    shard.assert_call_database(
        &first,
        "debug_queue_package_creation_result",
        &[&identity(&shard, &second), "9"],
    );
    assert!(
        poll_until(Duration::from_secs(20), || count(
            &shard,
            &first,
            "game_package_creation_retry"
        ) == 1),
        "the refused creation result was not scheduled for retry"
    );
    let result = |shard: &Standalone| {
        shard.query_database_rows(&first, "SELECT result FROM game_package_character_request")[0]
            ["result"]
            .clone()
    };
    assert_eq!(result(&shard), "(pending = ())");

    shard.assert_call_database(
        &first,
        "operator_remove_peer_shard",
        &[&identity(&shard, &second)],
    );
    let idle = |shard: &Standalone| {
        count(shard, &first, "game_package_creation_retry") == 0
            && count(shard, &first, "game_package_creation_delivery") == 0
    };
    assert!(
        poll_until(Duration::from_secs(20), || idle(&shard)),
        "the retry kept sending to a Shard that is no longer a Peer Shard"
    );
    assert_eq!(result(&shard), "(created = 9)");
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(250));
        assert!(idle(&shard), "the dropped retry was queued again");
    }
}
