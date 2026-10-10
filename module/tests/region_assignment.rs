mod support;

use support::Standalone;

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_unassigned_region_retains_its_epoch_and_refuses_stale_retries() {
    let mut shard = Standalone::start("region-assignment-tombstone");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("set_region_assignment", &["0", "2", "\"pool-b\"", "2"]);
    shard.assert_call("set_region_assignment", &["0", "2", "\"  \"", "3"]);

    let before = shard.query_rows("SELECT shard, epoch FROM game_region_assignment WHERE key = 2");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0]["shard"], "");
    assert_eq!(before[0]["epoch"], "3");
    for epoch in ["2", "3"] {
        let refused = shard.call("set_region_assignment", &["0", "2", "\"pool-b\"", epoch]);
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("stale epoch"));
        assert_eq!(
            shard.query_rows("SELECT shard, epoch FROM game_region_assignment WHERE key = 2"),
            before
        );
    }
    shard.assert_call("set_region_assignment", &["0", "2", "\" pool-c \"", "4"]);
    let after = shard.query_rows("SELECT shard, epoch FROM game_region_assignment WHERE key = 2");
    assert_eq!(after[0]["shard"], "pool-c");
    assert_eq!(after[0]["epoch"], "4");
}
