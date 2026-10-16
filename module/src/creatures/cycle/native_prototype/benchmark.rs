use super::{scenario, Action, Scenario};

pub const SCENARIOS: &[&str] = &["idle_drift", "combat_advance", "stopped_target"];

pub fn workload(name: &str, creatures: u32) -> Result<Scenario, String> {
    if !SCENARIOS.contains(&name) || !(1..=5_000).contains(&creatures) {
        return Err("benchmark requires a listed scenario and 1..=5000 creatures".into());
    }
    let original = scenario(name)?;
    if original
        .requests
        .iter()
        .any(|request| !matches!(request.action, Action::Tick))
    {
        return Err("scaled benchmark scenarios must contain only ticks".into());
    }
    let mut scaled = scenario(name)?;
    scaled.entities.clear();
    scaled.splines.clear();
    scaled.attacks.clear();
    for copy in 0..creatures {
        let offset = u64::from(copy) * 2;
        for row in &original.entities {
            let mut row = row.clone();
            row.guid += offset;
            scaled.entities.push(row);
        }
        for row in &original.splines {
            let mut row = row.clone();
            row.guid += offset;
            scaled.splines.push(row);
        }
        for row in &original.attacks {
            let mut row = row.clone();
            row.attacker_guid += offset;
            row.target_guid += offset;
            scaled.attacks.push(row);
        }
    }
    Ok(scaled)
}

pub fn digest(snapshot: &serde_json::Value) -> String {
    blake3::hash(&serde_json::to_vec(snapshot).unwrap())
        .to_hex()
        .to_string()
}
