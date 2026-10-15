use std::collections::{HashMap, HashSet};

use lyracore_shared::spatial;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use spacetimedb::sats::ser::serde::SerializeWrapper;

use super::{advance_legs, chase, Point, TickContext};
use crate::creatures::ai::TickScope;
use crate::creatures::tick::{advance_needs_persist, CreatureSpline};
use crate::{MeleeAttack, WorldEntity};

mod fixtures;
mod world;

pub use fixtures::{scenario, SCENARIOS};

pub const INSTANCE_ID: u64 = 900_000_001;
pub const CREATURE_GUID: u64 = 900_000_001;
pub const TARGET_GUID: u64 = 900_000_002;
pub const TRACE_PREFIX: &str = "NATIVE_SIMULATION_PARITY ";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub sequence: u64,
    pub at_micros: u64,
    pub action: Action,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Tick,
    MoveTarget {
        x: f32,
        y: f32,
        z: f32,
        orientation: f32,
        movement_flags: u32,
    },
    Immobilize {
        enabled: bool,
    },
    RemoveCreature,
}

pub struct Scenario {
    pub name: &'static str,
    pub requests: Vec<Request>,
    pub(super) entities: Vec<WorldEntity>,
    pub(super) splines: Vec<CreatureSpline>,
    pub(super) attacks: Vec<MeleeAttack>,
    pub(super) start_micros: u64,
}

impl Scenario {
    pub fn simulation(&self) -> Simulation {
        let mut slots: Vec<_> = self
            .entities
            .iter()
            .map(|entity| Slot {
                guid: entity.guid,
                entity: Some(entity.clone()),
                spline: None,
            })
            .collect();
        let by_guid: HashMap<_, _> = slots
            .iter()
            .enumerate()
            .map(|(i, slot)| (slot.guid, i))
            .collect();
        for spline in &self.splines {
            slots[by_guid[&spline.guid]].spline = Some(spline.clone());
        }
        Simulation {
            slots,
            by_guid,
            attacks: self.attacks.clone(),
            immobilized: HashSet::new(),
            advances: Vec::new(),
            at_micros: self.start_micros,
            sequence: 0,
        }
    }
}

struct Slot {
    guid: u64,
    entity: Option<WorldEntity>,
    spline: Option<CreatureSpline>,
}

pub struct Simulation {
    slots: Vec<Slot>,
    by_guid: HashMap<u64, usize>,
    attacks: Vec<MeleeAttack>,
    immobilized: HashSet<u64>,
    advances: Vec<(WorldEntity, WorldEntity)>,
    at_micros: u64,
    sequence: u64,
}

impl Simulation {
    // For the admitted inputs, see module/prototypes/native-simulation/README.md.
    pub fn step(&mut self, request: &Request) -> Result<Value, String> {
        self.admit(request)?;
        self.at_micros = request.at_micros;
        match request.action {
            Action::Tick => {
                let tick = tick_context(self.at_micros);
                advance_legs(self, &tick);
                chase(self, &tick);
                for (stored, advanced) in std::mem::take(&mut self.advances) {
                    let slot = self.slot_mut(stored.guid);
                    let Some(settled) = slot.entity.as_ref() else {
                        continue;
                    };
                    if !advance_needs_persist(
                        &stored,
                        &advanced,
                        settled,
                        slot.spline.as_ref(),
                        request.at_micros,
                    ) {
                        slot.entity = Some(stored);
                    }
                }
            }
            Action::MoveTarget {
                x,
                y,
                z,
                orientation,
                movement_flags,
            } => {
                let target = self.slot_mut(TARGET_GUID).entity.as_mut().unwrap();
                place(target, Point { x, y, z });
                target.orientation = orientation;
                target.movement_flags = movement_flags;
                target.last_move_ms = (request.at_micros / 1_000) as u32;
            }
            Action::Immobilize { enabled } => {
                if enabled {
                    self.immobilized.insert(CREATURE_GUID);
                } else {
                    self.immobilized.remove(&CREATURE_GUID);
                }
            }
            Action::RemoveCreature => {
                self.slot_mut(CREATURE_GUID).entity = None;
                self.immobilized.remove(&CREATURE_GUID);
            }
        }
        self.sequence = request.sequence;
        Ok(self.snapshot())
    }

    pub fn snapshot(&self) -> Value {
        snapshot(
            self.sequence,
            self.at_micros,
            self.slots.iter().filter_map(|s| s.entity.clone()).collect(),
            self.slots.iter().filter_map(|s| s.spline.clone()).collect(),
            self.attacks.clone(),
            self.immobilized.iter().copied().collect(),
        )
    }

    fn admit(&self, request: &Request) -> Result<(), String> {
        if self.sequence.checked_add(1) != Some(request.sequence) {
            return Err("request sequence must follow the last accepted request".into());
        }
        if request.at_micros < self.at_micros || request.at_micros > i64::MAX as u64 {
            return Err("time must be monotonic and fit the Module clock".into());
        }
        match request.action {
            Action::MoveTarget {
                x,
                y,
                z,
                orientation,
                movement_flags,
            } => {
                if self.entity(TARGET_GUID).is_none() {
                    return Err("the scenario has no target".into());
                }
                if ![x, y, z, orientation].into_iter().all(f32::is_finite) {
                    return Err("target coordinates and orientation must be finite".into());
                }
                if x.abs() > spatial::MAP_COORD_MAX || y.abs() > spatial::MAP_COORD_MAX {
                    return Err("target coordinates must be within the map".into());
                }
                if z != 0.0 || movement_flags & !crate::combat::MOVE_FLAG_FORWARD != 0 {
                    return Err(
                        "the prototype admits flat terrain and forward movement only".into(),
                    );
                }
            }
            Action::Immobilize { .. } | Action::RemoveCreature => {
                if self.entity(CREATURE_GUID).is_none() {
                    return Err("the creature is absent".into());
                }
            }
            Action::Tick => {}
        }
        Ok(())
    }

    fn entity(&self, guid: u64) -> Option<&WorldEntity> {
        self.by_guid
            .get(&guid)
            .and_then(|&i| self.slots[i].entity.as_ref())
    }

    fn slot_mut(&mut self, guid: u64) -> &mut Slot {
        &mut self.slots[self.by_guid[&guid]]
    }
}

pub(super) fn tick_context(now_micros: u64) -> TickContext {
    TickContext {
        now_micros,
        now_ms: (now_micros / 1_000) as u32,
        tick_secs: 0.5,
        sense: false,
        sense_secs: 1.0,
        scope: TickScope::Only(INSTANCE_ID),
    }
}

pub(super) fn place(entity: &mut WorldEntity, at: Point) {
    entity.x = at.x;
    entity.y = at.y;
    entity.z = at.z;
    (entity.grid_x, entity.grid_y) = spatial::grid_cell(at.x, at.y);
    entity.cell = spatial::grid_cell_id(entity.grid_x, entity.grid_y);
}

pub(super) fn snapshot(
    sequence: u64,
    at_micros: u64,
    mut entities: Vec<WorldEntity>,
    mut splines: Vec<CreatureSpline>,
    mut attacks: Vec<MeleeAttack>,
    mut immobilized: Vec<u64>,
) -> Value {
    entities.sort_unstable_by_key(|r| r.guid);
    splines.sort_unstable_by_key(|r| r.guid);
    attacks.sort_unstable_by_key(|r| r.attacker_guid);
    immobilized.sort_unstable();
    json!({
        "sequence": sequence,
        "at_micros": at_micros,
        "entities": SerializeWrapper::from_ref(&entities),
        "splines": SerializeWrapper::from_ref(&splines),
        "attacks": SerializeWrapper::from_ref(&attacks),
        "immobilized": immobilized,
    })
}

pub fn compare(expected: &Value, actual: &Value) -> Result<(), String> {
    compare_at("frame", expected, actual)
}

fn compare_at(path: &str, expected: &Value, actual: &Value) -> Result<(), String> {
    match (expected, actual) {
        (Value::Object(left), Value::Object(right)) => {
            for (field, value) in left {
                let field_path = format!("{path}.{field}");
                let actual = right
                    .get(field)
                    .ok_or_else(|| format!("{field_path}: missing"))?;
                compare_at(&field_path, value, actual)?;
            }
            if let Some(field) = right.keys().find(|field| !left.contains_key(*field)) {
                return Err(format!("{path}.{field}: unexpected field"));
            }
            Ok(())
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                return Err(format!(
                    "{path}: expected {} entries, got {}",
                    left.len(),
                    right.len()
                ));
            }
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                let row = left.get("guid").or_else(|| left.get("attacker_guid"));
                let name = row.map_or_else(|| index.to_string(), |guid| format!("guid={guid}"));
                compare_at(&format!("{path}[{name}]"), left, right)?;
            }
            Ok(())
        }
        (Value::Number(left), Value::Number(right)) if left.is_f64() && right.is_f64() => {
            if (left.as_f64().unwrap() - right.as_f64().unwrap()).abs() <= 0.00001 {
                Ok(())
            } else {
                Err(format!("{path}: expected {left}, got {right}"))
            }
        }
        _ if expected == actual => Ok(()),
        _ => Err(format!("{path}: expected {expected}, got {actual}")),
    }
}
