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
        .query_database_rows(database, &format!("SELECT * FROM {table}"))
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

// A Package Character creation result that the target refuses is retried every second. Once the
// Operator removes the target from `game_peer_shard`, the retry stops and the request settles as
// created.
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

fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

// Publishes two Shards where only the first lists the second, so the second refuses what the
// first sends.
fn shard_listing_a_refusing_peer(name: &str) -> (Standalone, String, String) {
    let mut shard = Standalone::start_persistent(name);
    shard.publish_module();
    let first = shard.shard_name().to_owned();
    let second = format!("{first}-second");
    shard.publish_named_module_bytes(&second, module_bytes());
    for database in [&first, &second] {
        shard.assert_call_database(database, "claim_operator", &[]);
    }
    shard.assert_call_database(
        &first,
        "operator_add_peer_shard",
        &[&identity(&shard, &second), r#""second""#],
    );
    (shard, first, second)
}

fn assert_removal_refused(shard: &Standalone, first: &str, second: &str, pending: &str) {
    let output = shard.call_database(
        first,
        "operator_remove_peer_shard",
        &[&identity(shard, second)],
    );
    let output_text = text(&output);
    assert!(
        !output.status.success() && output_text.contains(pending),
        "removal should refuse with {pending}: {output_text}"
    );
    assert!(
        confirmed(shard, first).is_some(),
        "the refused removal still removed the Peer Shard"
    );
}

// A send in flight when a retry table is cleared queues a new retry, so this retries until a
// removal lands while nothing is pending.
fn remove_once_settled(shard: &Standalone, first: &str, second: &str, retry_tables: &[&str]) {
    assert!(
        poll_until(Duration::from_secs(20), || {
            for table in retry_tables {
                shard.assert_call_database(
                    first,
                    "debug_clear_schedule",
                    &[&format!(r#""{table}""#)],
                );
            }
            shard
                .call_database(
                    first,
                    "operator_remove_peer_shard",
                    &[&identity(shard, second)],
                )
                .status
                .success()
        }),
        "the Peer Shard stayed listed after its work finished"
    );
    assert!(
        confirmed(shard, first).is_none(),
        "the Peer Shard is still listed"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pending_transfer_release_blocks_removing_its_peer_shard() {
    let (shard, first, second) = shard_listing_a_refusing_peer("transfer-removal-peer");
    shard.assert_call_database(
        &first,
        "debug_queue_native_transfer_release",
        &[&identity(&shard, &second), "9"],
    );
    assert!(
        poll_until(Duration::from_secs(20), || count(
            &shard,
            &first,
            "game_native_transfer_release_retry"
        ) == 1),
        "the refused release was not scheduled for retry"
    );

    assert_removal_refused(&shard, &first, &second, "1 Transfer message(s)");

    remove_once_settled(
        &shard,
        &first,
        &second,
        &["game_native_transfer_release_retry"],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pending_package_relocation_blocks_removing_its_peer_shard() {
    let (shard, first, second) = shard_listing_a_refusing_peer("relocation-removal-peer");
    shard.assert_call_database(
        &first,
        "debug_queue_package_relocation_release",
        &[&identity(&shard, &second), &identity(&shard, &second)],
    );
    assert!(
        poll_until(Duration::from_secs(20), || count(
            &shard,
            &first,
            "game_package_relocation_retry"
        ) == 1),
        "the refused relocation release was not scheduled for retry"
    );

    assert_removal_refused(&shard, &first, &second, "1 Package relocation message(s)");

    remove_once_settled(&shard, &first, &second, &["game_package_relocation_retry"]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn work_toward_a_destination_blocks_removing_it_though_another_shard_receives_it() {
    let (mut shard, first, second) = shard_listing_a_refusing_peer("destination-removal-peer");
    let third = format!("{first}-third");
    shard.publish_named_module_bytes(&third, module_bytes());
    shard.assert_call_database(&third, "claim_operator", &[]);
    shard.assert_call_database(
        &first,
        "operator_add_peer_shard",
        &[&identity(&shard, &third), r#""third""#],
    );
    shard.assert_call_database(
        &first,
        "debug_queue_native_transfer_moved",
        &[&identity(&shard, &second), &identity(&shard, &third)],
    );
    shard.assert_call_database(
        &first,
        "debug_queue_package_relocation_release",
        &[&identity(&shard, &second), &identity(&shard, &third)],
    );
    assert!(
        poll_until(Duration::from_secs(20), || {
            count(&shard, &first, "game_native_transfer_moved_retry") == 1
                && count(&shard, &first, "game_package_relocation_retry") == 1
        }),
        "the refused messages were not scheduled for retry"
    );

    assert_removal_refused(
        &shard,
        &first,
        &third,
        "1 Transfer message(s), 1 Package relocation message(s)",
    );

    remove_once_settled(
        &shard,
        &first,
        &third,
        &[
            "game_native_transfer_moved_retry",
            "game_package_relocation_retry",
        ],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_transfer_arriving_from_a_peer_shard_blocks_removing_it() {
    let (shard, first, second) = shard_listing_a_refusing_peer("transfer-arrival-removal-peer");
    shard.assert_call_database(
        &first,
        "debug_insert_native_transfer_arrival",
        &[&identity(&shard, &second), "7"],
    );

    assert_removal_refused(&shard, &first, &second, "1 Transfer arrival(s)");

    shard.assert_sql_database(
        &first,
        "DELETE FROM game_native_handoff WHERE transfer_id = 7",
    );
    shard.assert_sql_database(&first, "DELETE FROM game_transfer_in WHERE transfer_id = 7");
    remove_once_settled(&shard, &first, &second, &[]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_relocation_arriving_from_a_peer_shard_blocks_removing_it() {
    let (shard, first, second) = shard_listing_a_refusing_peer("relocation-arrival-removal-peer");
    shard.assert_call_database(
        &first,
        "debug_insert_package_arrival",
        &[&identity(&shard, &second), "7"],
    );

    assert_removal_refused(&shard, &first, &second, "1 Package relocation arrival(s)");

    shard.assert_sql_database(&first, "DELETE FROM game_package_arrival WHERE id = 7");
    remove_once_settled(&shard, &first, &second, &[]);
}
