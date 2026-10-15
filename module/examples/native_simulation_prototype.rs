use lyracore_module::native_prototype::{scenario, SCENARIOS};

fn main() -> Result<(), String> {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "moving_target_turn_stop".into());
    if name == "--list" {
        for name in SCENARIOS {
            println!("{name}");
        }
        return Ok(());
    }
    let scenario = scenario(&name)?;
    let mut simulation = scenario.simulation();
    println!(
        "{}",
        serde_json::json!({"scenario": name, "frame": simulation.snapshot()})
    );
    for request in &scenario.requests {
        let frame = simulation.step(request)?;
        println!(
            "{}",
            serde_json::json!({"scenario": name, "request": request, "frame": frame})
        );
    }
    Ok(())
}
