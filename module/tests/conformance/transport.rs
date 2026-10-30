use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use lyracore_test_support::{ReducerConnection, Standalone};
use serde::Deserialize;
use serde_json::{json, value::RawValue};
use wire_client::runner::{MultiTransport, Transport};
use wire_client::scenario::{FixtureVerb, Observation, Scenario};

use super::properties::{self, Batch, CreateContext, Packet, Projection};

pub struct OperatorTransport<'a> {
    shard: &'a Standalone,
    connection: ReducerConnection,
    guid: u64,
    fixtures: BTreeMap<String, u64>,
    opening: Vec<Observation>,
    delivery: Delivery,
    pending: VecDeque<(u16, Vec<u8>)>,
    batches: Vec<Batch>,
    virtual_micros: u64,
}

impl<'a> OperatorTransport<'a> {
    pub fn prepare(shard: &'a Standalone, scenario: &Scenario) -> Result<Self> {
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
        let guid = prepare_character(shard, "CONFORMANCE", name, race, class, *level, at)?;
        let fixtures = prepare_fixtures(shard, guid, &scenario.fixtures[1..], at)?;
        let transport = Self::connect(shard, guid, fixtures)?;
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

    fn connect(shard: &'a Standalone, guid: u64, fixtures: BTreeMap<String, u64>) -> Result<Self> {
        let mut connection = shard.connect_reducers(&[
            "SELECT * FROM game_native_packet",
            "SELECT * FROM game_native_conformance_projection",
            "SELECT * FROM game_native_conformance_create",
        ]);
        connection
            .call("native_world_open", json!([42]))
            .map_err(|e| anyhow!(e))?;
        connection.drain_packets();
        connection
            .call("debug_native_enter_world", json!([guid]))
            .map_err(|e| anyhow!(e))?;
        let mut transport = Self {
            shard,
            delivery: Delivery::new(connection.connection_id()),
            connection,
            guid,
            fixtures,
            opening: Vec::new(),
            pending: VecDeque::new(),
            batches: Vec::new(),
            virtual_micros: 0,
        };
        transport.capture()?;
        transport.opening = transport
            .pending
            .drain(..)
            .map(|(opcode, body)| Observation::opening(opcode, &body))
            .collect();
        Ok(transport)
    }

    pub fn batches(&self) -> &[Batch] {
        &self.batches
    }

    pub(super) fn capture(&mut self) -> Result<()> {
        // The call is a barrier for earlier packet transactions on this recipient's socket.
        let projected = self
            .connection
            .call("debug_native_project", json!([]))
            .map_err(|error| anyhow!(error))?;
        self.connection
            .drain_inserts("game_native_conformance_projection");
        let rows = self.connection.drain_packets();
        let packets = self.delivery.route_rows(rows)?;
        let mut projections = BTreeMap::new();
        for row in projected.inserts("game_native_conformance_projection") {
            let (guid, object_type, owner_guid, same_raid, body, extra): (
                u64,
                u8,
                u64,
                bool,
                String,
                String,
            ) = serde_json::from_str(row.get())?;
            let mut fields = BTreeMap::new();
            for body in [&body, &extra] {
                if !body.is_empty() {
                    for update in properties::updates(0x00a9, &decode_hex(body)?)? {
                        fields.extend(update.fields);
                    }
                }
            }
            projections.insert(
                guid,
                Projection {
                    fields,
                    object_type,
                    owner_guid,
                    same_raid,
                    special_info: false,
                },
            );
        }
        let creates = self
            .connection
            .drain_inserts("game_native_conformance_create")
            .into_iter()
            .map(|row| {
                let (id, newly_added, layer, gameobject_type): (u64, bool, u8, u8) =
                    serde_json::from_str(row.get())?;
                Ok((
                    id,
                    CreateContext {
                        newly_added,
                        layer,
                        gameobject_type,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        self.batches.push(Batch {
            recipient: self.guid,
            reset: self.batches.is_empty(),
            packets: packets.clone(),
            projections,
            creates,
        });
        self.pending.extend(
            packets
                .into_iter()
                .map(|packet| (packet.opcode, packet.body)),
        );
        Ok(())
    }
}

fn decode_hex(encoded: &str) -> Result<Vec<u8>> {
    ensure!(
        encoded.is_ascii() && encoded.len().is_multiple_of(2),
        "invalid packet hex"
    );
    (0..encoded.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&encoded[offset..offset + 2], 16).map_err(Into::into))
        .collect()
}

fn prepare_character(
    shard: &Standalone,
    account_name: &str,
    name: &str,
    race: &str,
    class: &str,
    level: u8,
    at: &wire_client::scenario::Position,
) -> Result<u64> {
    let credentials = serde_json::to_string(&vec![0u8; 32])?;
    shard.assert_call(
        "provision_account",
        &[
            &serde_json::to_string(account_name)?,
            &credentials,
            &credentials,
        ],
    );
    let account = &shard.query_rows(&format!(
        "SELECT id FROM game_account WHERE username = '{account_name}'"
    ))[0]["id"];
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

impl Transport for OperatorTransport<'_> {
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
            Ok(_) => (),
            Err(reason) => {
                let Some(reply) = reason.strip_prefix("native_reply:") else {
                    bail!(reason)
                };
                let packets: Vec<(u16, Vec<u8>)> = serde_json::from_str(reply)?;
                self.connection
                    .call("native_world_reply", json!([packets]))
                    .map_err(|e| anyhow!(e))?;
            }
        }
        self.capture()
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<(u16, Vec<u8>)>> {
        let deadline = self.now() + timeout;
        loop {
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
            self.capture()?;
        }
    }
}

struct MultiCharacter<'a> {
    guid: u64,
    fixtures: BTreeMap<String, u64>,
    transport: Option<OperatorTransport<'a>>,
    completed: Vec<Batch>,
}

pub struct MultiOperatorTransport<'a> {
    shard: &'a Standalone,
    characters: BTreeMap<String, MultiCharacter<'a>>,
    virtual_micros: u64,
}

impl<'a> MultiOperatorTransport<'a> {
    pub fn prepare(shard: &'a Standalone, scenario: &Scenario) -> Result<Self> {
        scenario.validate()?;
        ensure!(
            scenario.schema_version == 4,
            "scenario needs named Characters"
        );
        let mut characters = BTreeMap::new();
        for character in &scenario.characters {
            let FixtureVerb::Character {
                name,
                race,
                class,
                level,
                at,
            } = &character.fixtures[0]
            else {
                bail!("first fixture must be a Character");
            };
            let account = format!("CONFORMANCE_{}", character.id.to_ascii_uppercase());
            let guid = prepare_character(shard, &account, name, race, class, *level, at)?;
            let fixtures = prepare_fixtures(shard, guid, &character.fixtures[1..], at)?;
            // Resolve rest before logout so peer CREATE uses the persisted flag.
            shard.assert_call(
                "debug_check_rest_at",
                &[
                    &guid.to_string(),
                    &at.map.to_string(),
                    &at.x.to_string(),
                    &at.y.to_string(),
                ],
            );
            shard.assert_call("debug_logout_character", &[&guid.to_string()]);
            characters.insert(
                character.id.clone(),
                MultiCharacter {
                    guid,
                    fixtures,
                    transport: None,
                    completed: Vec::new(),
                },
            );
        }
        let mut transport = Self {
            shard,
            characters,
            virtual_micros: 0,
        };
        for character in &scenario.characters {
            if character.initially_connected {
                transport.connect(&character.id)?;
            }
        }
        Ok(transport)
    }

    pub fn character_guids(&self) -> BTreeMap<String, u64> {
        self.characters
            .iter()
            .map(|(id, character)| (id.clone(), character.guid))
            .collect()
    }

    pub fn batches(&self) -> Vec<Batch> {
        self.characters
            .values()
            .flat_map(|character| {
                character.completed.iter().cloned().chain(
                    character
                        .transport
                        .as_ref()
                        .into_iter()
                        .flat_map(|transport| transport.batches().iter().cloned()),
                )
            })
            .collect()
    }

    fn capture_all(&mut self) -> Result<()> {
        for character in self.characters.values_mut() {
            if let Some(transport) = &mut character.transport {
                transport.capture()?;
            }
        }
        Ok(())
    }

    fn character(&self, id: &str) -> Result<&MultiCharacter<'a>> {
        self.characters
            .get(id)
            .context("unknown scenario Character")
    }

    fn transport(&mut self, id: &str) -> Result<&mut OperatorTransport<'a>> {
        self.characters
            .get_mut(id)
            .context("unknown scenario Character")?
            .transport
            .as_mut()
            .context("scenario Character is disconnected")
    }
}

impl MultiTransport for MultiOperatorTransport<'_> {
    fn now(&self) -> Duration {
        Duration::from_micros(self.virtual_micros)
    }

    fn character_guid(&self, character: &str) -> Result<u64> {
        Ok(self.character(character)?.guid)
    }

    fn fixture_guids(&self, character: &str) -> Result<BTreeMap<String, u64>> {
        Ok(self.character(character)?.fixtures.clone())
    }

    fn take_opening(&mut self, character: &str) -> Result<Vec<Observation>> {
        Ok(self
            .characters
            .get_mut(character)
            .context("unknown scenario Character")?
            .transport
            .as_mut()
            .map_or_else(Vec::new, |transport| transport.take_opening()))
    }

    fn connected(&self, character: &str) -> bool {
        self.characters
            .get(character)
            .is_some_and(|entry| entry.transport.is_some())
    }

    fn connect(&mut self, character: &str) -> Result<()> {
        let entry = self
            .characters
            .get_mut(character)
            .context("unknown scenario Character")?;
        ensure!(entry.transport.is_none(), "scenario Character is connected");
        let mut transport =
            OperatorTransport::connect(self.shard, entry.guid, entry.fixtures.clone())?;
        transport.virtual_micros = self.virtual_micros;
        entry.transport = Some(transport);
        self.capture_all()
    }

    fn disconnect(&mut self, character: &str) -> Result<()> {
        let entry = self
            .characters
            .get_mut(character)
            .context("unknown scenario Character")?;
        let mut transport = entry
            .transport
            .take()
            .context("scenario Character is disconnected")?;
        transport.capture()?;
        entry.completed.extend_from_slice(transport.batches());
        self.shard
            .assert_call("debug_logout_character", &[&entry.guid.to_string()]);
        drop(transport);
        self.capture_all()
    }

    fn send(&mut self, character: &str, opcode: u32, body: &[u8]) -> Result<()> {
        self.transport(character)?.send(opcode, body)?;
        self.capture_all()
    }

    fn receive(&mut self, character: &str, timeout: Duration) -> Result<Option<(u16, Vec<u8>)>> {
        let deadline = self.now() + timeout;
        loop {
            if let Some(packet) = self.transport(character)?.pending.pop_front() {
                return Ok(Some(packet));
            }
            let Some(remaining) = deadline.checked_sub(self.now()) else {
                return Ok(None);
            };
            if remaining.is_zero() {
                return Ok(None);
            }
            let to_boundary = 50_000 - self.virtual_micros % 50_000;
            let delta: u64 = remaining
                .as_micros()
                .min(u128::from(to_boundary))
                .try_into()?;
            self.transport(character)?
                .connection
                .call("debug_advance_virtual_clock", json!([delta]))
                .map_err(|error| anyhow!(error))?;
            self.virtual_micros += delta;
            self.capture_all()?;
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

    fn route_rows(&mut self, rows: Vec<Box<RawValue>>) -> Result<Vec<Packet>> {
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

    fn route(&mut self, row: Row) -> Result<Option<Packet>> {
        let Row(
            id,
            (audience, recipient),
            (option, opcode),
            body,
            close,
            not_before,
            authenticated,
        ) = row;
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
                let bytes = decode_hex(&body)?;
                Ok(Some(Packet {
                    id,
                    opcode: serde_json::from_str(opcode.get())?,
                    body: bytes,
                }))
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
        Some(Packet {
            id: 1,
            opcode: 477,
            body: vec![7, 0, 0, 0]
        })
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
        [
            Packet {
                id: 2,
                opcode: 169,
                body: vec![2]
            },
            Packet {
                id: 3,
                opcode: 477,
                body: vec![3]
            }
        ]
    );
}
