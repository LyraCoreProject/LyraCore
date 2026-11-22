mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use support::{event_rows, number, poll_until, Row, Standalone, TableSubscription, POLL_TIMEOUT};

const PLAYER: u64 = 1;
const NOT_IN_RANGE: u8 = 1;

struct Swing {
    shard: Standalone,
    wolf: u64,
    x: f32,
    y: f32,
}

impl Swing {
    fn start(name: &str) -> Self {
        let mut shard = Standalone::start(name);
        shard.publish_module();
        shard.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
        shard.assert_call(
            "debug_spawn_at_feet",
            &[&PLAYER.to_string(), &TEST_WOLF_ENTRY.to_string(), "20"],
        );
        let wolf = shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
            ))
            .iter()
            .map(|row| number::<u64>(row, "guid"))
            .max()
            .expect("the spawned wolf must exist");
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER} OR guid = {wolf}"
        ));
        // Face +x, toward the wolf `debug_spawn_at_feet` placed ahead on x.
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET orientation = 0.0 WHERE guid = {PLAYER}"
        ));
        let player = entity(&shard, PLAYER);
        let (x, y) = (number(&player, "x"), number(&player, "y"));
        Self { shard, wolf, x, y }
    }

    fn place_wolf(&self, distance: f32) {
        let (x, y, wolf) = (self.x + distance, self.y, self.wolf);
        self.shard.assert_sql(&format!(
            "UPDATE game_world_entity SET x = {x}, y = {y} WHERE guid = {wolf}"
        ));
    }

    fn give_wolf_combat_reach(&self, combat_reach: f32) {
        let wolf = entity(&self.shard, self.wolf);
        let display_id: u32 = number(&wolf, "display_id");
        let scale: f32 = number(&wolf, "scale_x");
        self.shard.assert_sql(&format!(
            "INSERT INTO game_display_combat_reach (display_id, combat_reach, display_scale) \
             VALUES ({display_id}, {combat_reach}, {scale})"
        ));
    }

    fn subscribe(&self) -> TableSubscription {
        self.shard.subscribe(&[
            "SELECT * FROM game_combat_event_live WHERE attacker_guid = 1",
            "SELECT * FROM game_melee_attack WHERE attacker_guid = 1",
        ])
    }

    fn engage(&self) {
        self.shard.assert_call(
            "debug_engage",
            &[&PLAYER.to_string(), &self.wolf.to_string()],
        );
    }

    /// Stamping the swing clock at 1 makes the next swing tick due at once.
    fn make_swing_due(&self) {
        self.shard.assert_sql(&format!(
            "UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {PLAYER}"
        ));
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_swing_reaches_a_large_creature_from_beyond_five_yards() {
    let swing = Swing::start("melee-reach-large");
    swing.place_wolf(7.0);
    let events = swing.subscribe();
    swing.engage();
    wait_for_swing_error(&events, NOT_IN_RANGE);
    assert!(
        own_swings(&events).is_empty(),
        "1.5 + 1.5 + 4/3 yd leaves a normal wolf 7 yd away out of reach"
    );

    swing.give_wolf_combat_reach(5.0);
    swing.make_swing_due();
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(&events).is_empty()),
        "1.5 + 5 + 4/3 yd reaches a wolf 7 yd away"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_normal_pair_reaches_five_yards() {
    let swing = Swing::start("melee-reach-normal");
    swing.place_wolf(5.3);
    let events = swing.subscribe();
    swing.engage();
    wait_for_swing_error(&events, NOT_IN_RANGE);
    assert!(
        own_swings(&events).is_empty(),
        "5.3 yd is past the 5 yd floor"
    );

    swing.place_wolf(4.8);
    swing.make_swing_due();
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(&events).is_empty()),
        "4.8 yd is inside the 5 yd floor"
    );
}

fn entity(shard: &Standalone, guid: u64) -> Row {
    shard
        .query_rows(&format!(
            "SELECT x, y, display_id, scale_x FROM game_world_entity WHERE guid = {guid}"
        ))
        .pop()
        .expect("the entity must be live")
}

fn swing_error(events: &TableSubscription) -> Option<u8> {
    event_rows(events, "game_melee_attack")
        .pop()
        .map(|row| number(&row, "swing_error"))
}

fn wait_for_swing_error(events: &TableSubscription, expected: u8) {
    assert!(
        poll_until(POLL_TIMEOUT, || swing_error(events) == Some(expected)),
        "the Swing Error must become {expected}, got {:?}",
        swing_error(events)
    );
}

fn own_swings(events: &TableSubscription) -> Vec<Row> {
    event_rows(events, "game_combat_event_live")
}
