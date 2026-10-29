use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Result};
use lyracore_test_support::{ReducerConnection, Standalone};
use serde::Deserialize;
use serde_json::{json, value::RawValue};
use wire_client::runner::Transport;
use wire_client::scenario::{FixtureVerb, Observation, Scenario};

pub struct OperatorTransport {
    connection: ReducerConnection,
    guid: u64,
    fixtures: BTreeMap<String, u64>,
    opening: Vec<Observation>,
    delivery: Delivery,
    pending: VecDeque<(u16, Vec<u8>)>,
    virtual_micros: u64,
}

impl OperatorTransport {
    pub fn prepare(shard: &Standalone, scenario: &Scenario) -> Result<Self> {
        scenario.validate()?;
        let FixtureVerb::Character {
            name,
            race,
            class,
            level,
            at,
        } = &scenario.fixtures[0]
        else {
            bail!("first fixture must be a Character");
        };
        let guid = prepare_character(shard, name, race, class, *level, at)?;
        let fixtures = prepare_fixtures(shard, guid, &scenario.fixtures[1..], at)?;
        let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
        connection
            .call("native_world_open", json!([42]))
            .map_err(|e| anyhow!(e))?;
        connection.drain_packets();
        connection
            .call("debug_native_enter_world", json!([guid]))
            .map_err(|e| anyhow!(e))?;
        let mut transport = Self {
            delivery: Delivery::new(connection.connection_id()),
            connection,
            guid,
            fixtures,
            opening: Vec::new(),
            pending: VecDeque::new(),
            virtual_micros: 0,
        };
        let setup = transport.connection.drain_packets();
        transport.opening = transport
            .delivery
            .route_rows(setup)?
            .into_iter()
            .map(|(opcode, body)| Observation::opening(opcode, &body))
            .collect();
        let character = &shard.query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {guid}"
        ))[0];
        ensure!(
            character["level"].parse::<u8>()? == *level,
            "fixture level did not apply"
        );
        ensure!(
            character["map_id"].parse::<u32>()? == at.map,
            "fixture map did not apply"
        );
        for (field, expected) in [
            ("x", at.x),
            ("y", at.y),
            ("z", at.z),
            ("orientation", at.orientation),
        ] {
            ensure!(
                (character[field].parse::<f32>()? - expected).abs() < 0.001,
                "fixture {field} did not apply"
            );
        }
        Ok(transport)
    }
}

fn prepare_character(
    shard: &Standalone,
    name: &str,
    race: &str,
    class: &str,
    level: u8,
    at: &wire_client::scenario::Position,
) -> Result<u64> {
    let credentials = serde_json::to_string(&vec![0u8; 32])?;
    shard.assert_call(
        "provision_account",
        &["\"CONFORMANCE\"", &credentials, &credentials],
    );
    let account =
        &shard.query_rows("SELECT id FROM game_account WHERE username = 'CONFORMANCE'")[0]["id"];
    let race = wire_client::cli::parse_race(race)?.as_int().to_string();
    let class = wire_client::cli::parse_class(class)?.as_int().to_string();
    shard.assert_call(
        "create_character",
        &[
            account,
            &serde_json::to_string(name)?,
            &race,
            &class,
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let character = &shard.query_rows(&format!(
        "SELECT * FROM game_character WHERE name = '{name}'"
    ))[0];
    ensure!(
        character["race"] == race && character["class"] == class,
        "fixture race or class did not apply"
    );
    let guid = character["guid"].parse::<u64>()?;
    shard.assert_call("debug_set_level", &[&guid.to_string(), &level.to_string()]);
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    shard.assert_call(
        "debug_teleport",
        &[
            &guid.to_string(),
            &at.map.to_string(),
            &at.x.to_string(),
            &at.y.to_string(),
            &at.z.to_string(),
            &at.orientation.to_string(),
        ],
    );
    Ok(guid)
}

fn prepare_fixtures(
    shard: &Standalone,
    guid: u64,
    verbs: &[FixtureVerb],
    at: &wire_client::scenario::Position,
) -> Result<BTreeMap<String, u64>> {
    let mut fixtures = BTreeMap::new();
    for fixture in verbs {
        match fixture {
            FixtureVerb::Character { .. } => bail!("duplicate Character fixture"),
            FixtureVerb::Money { copper } => {
                shard.assert_call("debug_set_money", &[&guid.to_string(), &copper.to_string()]);
                let rows = shard.query_rows(&format!(
                    "SELECT money FROM game_character WHERE guid = {guid}"
                ));
                ensure!(
                    rows[0]["money"].parse::<u32>()? == *copper,
                    "fixture money did not apply"
                );
            }
            FixtureVerb::KnownFlightNode { node_id } => {
                shard.assert_call(
                    "debug_learn_taxi_node",
                    &[&guid.to_string(), &node_id.to_string()],
                );
                let rows = shard.query_rows(&format!(
                    "SELECT node_id FROM game_character_taxi_node WHERE character_guid = {guid}"
                ));
                ensure!(
                    rows.iter().any(|row| row["node_id"] == node_id.to_string()),
                    "fixture node did not apply"
                );
            }
            FixtureVerb::Creature { id, entry, offset } => {
                let query = format!("SELECT * FROM game_world_entity WHERE entry = {entry}");
                let before: BTreeSet<_> = shard
                    .query_rows(&query)
                    .into_iter()
                    .map(|row| row["guid"].clone())
                    .collect();
                shard.assert_call(
                    "debug_spawn_at_feet",
                    &[&guid.to_string(), &entry.to_string(), &offset.to_string()],
                );
                let spawned: Vec<_> = shard
                    .query_rows(&query)
                    .into_iter()
                    .filter(|row| !before.contains(&row["guid"]))
                    .collect();
                ensure!(
                    spawned.len() == 1,
                    "fixture {id} did not spawn one creature"
                );
                let creature = &spawned[0];
                ensure!(
                    creature["map_id"].parse::<u32>()? == at.map && creature["instance_id"] == "0",
                    "fixture {id} partition differs"
                );
                for (field, expected) in [
                    ("x", at.x + offset * at.orientation.cos()),
                    ("y", at.y + offset * at.orientation.sin()),
                    ("z", at.z),
                ] {
                    ensure!(
                        (creature[field].parse::<f32>()? - expected).abs() < 0.01,
                        "fixture {id} {field} differs"
                    );
                }
                fixtures.insert(id.clone(), creature["guid"].parse()?);
            }
        }
    }
    Ok(fixtures)
}

impl Transport for OperatorTransport {
    fn now(&self) -> Duration {
        Duration::from_micros(self.virtual_micros)
    }

    fn fixture_guids(&self) -> BTreeMap<String, u64> {
        self.fixtures.clone()
    }

    fn take_opening(&mut self) -> Vec<Observation> {
        std::mem::take(&mut self.opening)
    }

    fn character_guid(&self) -> u64 {
        self.guid
    }

    fn send(&mut self, opcode: u32, body: &[u8]) -> Result<()> {
        match self
            .connection
            .call("native_world_receive", json!([opcode, body]))
        {
            Ok(_) => Ok(()),
            Err(reason) => {
                let Some(reply) = reason.strip_prefix("native_reply:") else {
                    bail!(reason)
                };
                let packets: Vec<(u16, Vec<u8>)> = serde_json::from_str(reply)?;
                self.connection
                    .call("native_world_reply", json!([packets]))
                    .map_err(|e| anyhow!(e))?;
                Ok(())
            }
        }
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<(u16, Vec<u8>)>> {
        let deadline = self.now() + timeout;
        loop {
            if let Some(packet) = self.pending.pop_front() {
                return Ok(Some(packet));
            }
            let rows = self.connection.drain_packets();
            self.pending.extend(self.delivery.route_rows(rows)?);
            if let Some(packet) = self.pending.pop_front() {
                return Ok(Some(packet));
            }
            let Some(remaining) = deadline.checked_sub(self.now()) else {
                return Ok(None);
            };
            if remaining.is_zero() {
                return Ok(None);
            }
            let to_boundary = 50_000 - self.virtual_micros % 50_000;
            let delta = remaining.as_micros().min(u128::from(to_boundary));
            let delta: u64 = delta.try_into()?;
            self.connection
                .call("debug_advance_virtual_clock", json!([delta]))
                .map_err(|e| anyhow!(e))?;
            self.virtual_micros += delta;
        }
    }
}

#[derive(Deserialize)]
struct Row(
    u64,
    (u8, Box<RawValue>),
    (u8, Box<RawValue>),
    String,
    bool,
    i64,
    bool,
);

struct Delivery {
    connection: u128,
    grants: BTreeSet<(u8, u64)>,
}

impl Delivery {
    fn new(connection: u128) -> Self {
        Self {
            connection,
            grants: BTreeSet::new(),
        }
    }

    fn route_rows(&mut self, rows: Vec<Box<RawValue>>) -> Result<Vec<(u16, Vec<u8>)>> {
        let mut rows = rows
            .iter()
            .map(|row| serde_json::from_str::<Row>(row.get()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.sort_by_key(|row| row.0);
        let mut packets = Vec::new();
        for row in rows {
            if let Some(packet) = self.route(row)? {
                packets.push(packet);
            }
        }
        Ok(packets)
    }

    fn route(&mut self, row: Row) -> Result<Option<(u16, Vec<u8>)>> {
        let Row(_, (audience, recipient), (option, opcode), body, close, not_before, authenticated) =
            row;
        let addressed = match audience {
            0 => serde_json::from_str::<(u128,)>(recipient.get())?.0 == self.connection,
            1 => {
                let (layer, guid, (tag, excluded)): (u8, u64, (u8, Box<RawValue>)) =
                    serde_json::from_str(recipient.get())?;
                let excluded = match tag {
                    0 => Some(serde_json::from_str::<(u128,)>(excluded.get())?.0),
                    1 => None,
                    _ => bail!("invalid excluded connection option {tag}"),
                };
                self.grants.contains(&(layer, guid)) && excluded != Some(self.connection)
            }
            2 | 3 => {
                let ((connection,), layer, guid): ((u128,), u8, u64) =
                    serde_json::from_str(recipient.get())?;
                if connection != self.connection {
                    return Ok(None);
                }
                if audience == 2 {
                    self.grants.insert((layer, guid));
                } else {
                    self.grants.remove(&(layer, guid));
                }
                true
            }
            4 => {
                if serde_json::from_str::<(u128,)>(recipient.get())?.0 == self.connection {
                    self.grants.clear();
                }
                false
            }
            _ => bail!("unknown native delivery audience {audience}"),
        };
        if !addressed {
            return Ok(None);
        }
        ensure!(!close, "native connection closed during the scenario");
        ensure!(
            !authenticated,
            "unexpected cipher transition on a fixture connection"
        );
        ensure!(
            not_before == 0,
            "tier 1 does not support delayed Host delivery"
        );
        match option {
            0 => {
                ensure!(body.is_ascii() && body.len() % 2 == 0, "invalid packet hex");
                let bytes = (0..body.len())
                    .step_by(2)
                    .map(|offset| u8::from_str_radix(&body[offset..offset + 2], 16))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(Some((serde_json::from_str(opcode.get())?, bytes)))
            }
            1 => Ok(None),
            _ => bail!("invalid opcode option {option}"),
        }
    }
}

#[test]
fn delivery_follows_grants_revoke_clear_and_excluded_connections() {
    let mut delivery = Delivery::new(u128::MAX);
    let row = |audience: String| format!("[1,{audience},[0,477],\"07000000\",false,0,false]");
    let broadcast = row("[1,[0,55,[1,[]]]]".into());
    assert!(delivery
        .route(serde_json::from_str(&broadcast).unwrap())
        .unwrap()
        .is_none());
    let grant = row(format!("[2,[[{}],0,55]]", u128::MAX));
    assert_eq!(
        delivery
            .route(serde_json::from_str(&grant).unwrap())
            .unwrap(),
        Some((477, vec![7, 0, 0, 0]))
    );
    assert!(delivery
        .route(serde_json::from_str(&broadcast).unwrap())
        .unwrap()
        .is_some());
    assert!(delivery
        .route(serde_json::from_str(&row("[1,[1,55,[1,[]]]]".into())).unwrap())
        .unwrap()
        .is_none());
    assert!(delivery
        .route(serde_json::from_str(&row(format!("[1,[0,55,[0,[{}]]]]", u128::MAX))).unwrap())
        .unwrap()
        .is_none());
    assert!(delivery
        .route(serde_json::from_str(&row("[0,[1]]".into())).unwrap())
        .unwrap()
        .is_none());
    delivery
        .route(serde_json::from_str(&row(format!("[3,[[{}],0,55]]", u128::MAX))).unwrap())
        .unwrap();
    assert!(delivery
        .route(serde_json::from_str(&broadcast).unwrap())
        .unwrap()
        .is_none());
    delivery
        .route(serde_json::from_str(&grant).unwrap())
        .unwrap();
    delivery
        .route(serde_json::from_str(&row(format!("[4,[{}]]", u128::MAX))).unwrap())
        .unwrap();
    assert!(delivery
        .route(serde_json::from_str(&broadcast).unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn unsupported_delivery_fails_instead_of_losing_evidence() {
    let mut delivery = Delivery::new(1);
    for source in [
        "[1,[9,[]],[1,[]],\"\",false,0,false]",
        "[1,[0,[1]],[0,477],\"00\",true,0,false]",
        "[1,[0,[1]],[0,477],\"00\",false,1,false]",
        "[1,[0,[1]],[0,477],\"xx\",false,0,false]",
    ] {
        assert!(delivery
            .route(serde_json::from_str(source).unwrap())
            .is_err());
    }
}

#[test]
fn delivery_uses_packet_ids_before_applying_grants() {
    let mut delivery = Delivery::new(1);
    let rows = [
        r#"[3,[1,[0,55,[1,[]]]],[0,477],"03",false,0,false]"#,
        r#"[1,[1,[0,55,[1,[]]]],[0,477],"01",false,0,false]"#,
        r#"[2,[2,[[1],0,55]],[0,169],"02",false,0,false]"#,
    ]
    .into_iter()
    .map(|row| RawValue::from_string(row.into()).unwrap())
    .collect();
    assert_eq!(
        delivery.route_rows(rows).unwrap(),
        [(169, vec![2]), (477, vec![3])]
    );
}
