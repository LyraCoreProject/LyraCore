use serde_json::{json, Value};
use spacetimedb_sats::ser::serde::SerializeWrapper;

use super::TickContext;
use crate::creatures::ai::TickScope;
use crate::creatures::tick::CreatureSpline;
use crate::{MeleeAttack, WorldEntity};

pub mod benchmark;
pub mod commit;
mod fixtures;

pub use fixtures::{scenario, SCENARIOS};

pub const INSTANCE_ID: u64 = 900_000_001;
pub const CREATURE_GUID: u64 = 900_000_001;
pub const TARGET_GUID: u64 = 900_000_002;
pub const TRACE_PREFIX: &str = "NATIVE_SIMULATION_PARITY ";

pub struct Request {
    pub sequence: u64,
    pub at_micros: u64,
    pub action: Action,
}

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
