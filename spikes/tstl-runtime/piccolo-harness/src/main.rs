use piccolo::{Closure, Executor, Fuel, Lua, Value};
use std::{env, fs};
const EXPECTED: &str = "HOOK:18:6-12|HOOK:9:9|27";
const FUEL_PER_TICK: i32 = 64;
const MAX_TICKS: usize = 10_000;
const PICCOLO_SHIM: &str = r#"
table.concat = function(list, separator)
    local out = ""
    for i = 1, #list do
        if i > 1 then out = out .. separator end
        out = out .. tostring(list[i])
    end
    return out
end
"#;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or("usage: tstl-piccolo-spike <script.lua>")?;
    let generated = fs::read(&path)?;
    let source: Vec<u8> = PICCOLO_SHIM.bytes().chain(generated).collect();
    let mut lua = Lua::core();
    let executor = lua.try_enter(|ctx| {
        let closure = Closure::load(ctx, Some(&path), source.as_slice())?;
        Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
    })?;
    let mut ticks = 0;
    loop {
        ticks += 1;
        let done = lua.enter(|ctx| {
            let mut fuel = Fuel::with(FUEL_PER_TICK);
            ctx.fetch(&executor).step(ctx, &mut fuel)
        });
        if done {
            break;
        }
        if ticks >= MAX_TICKS {
            return Err(format!("script exceeded {MAX_TICKS} metered ticks").into());
        }
    }
    lua.execute::<()>(&executor)?;
    let actual_bytes = lua.enter(|ctx| match ctx.get_global("SPIKE_RESULT") {
        Value::String(value) => Ok(value.as_bytes().to_vec()),
        other => Err(format!(
            "SPIKE_RESULT was {}, not a string",
            other.type_name()
        )),
    })?;
    let actual = String::from_utf8(actual_bytes)?;
    if actual != EXPECTED {
        return Err(format!("unexpected result: {actual:?}; expected {EXPECTED:?}").into());
    }
    println!("PASS piccolo=0.3.3 fuel_per_tick={FUEL_PER_TICK} ticks={ticks} result={actual}");
    Ok(())
}
