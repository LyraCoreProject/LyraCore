//! The Runtime Script Host: the contained Lua interpreter a Runtime Script runs inside.
//!
//! One [`RuntimeScriptHost`] owns one embedded piccolo interpreter and a compiler cache. The cache holds
//! compiled chunks only, keyed by a hash of the source, so it can never hand back stale code for
//! an edited script and it holds no Lua state between invocations.
//!
//! Every invocation gets:
//!
//! * a fresh environment table with its own standard-library tables, so nothing a script assigns
//!   survives it;
//! * a [`FUEL_BUDGET_PER_INVOCATION`] of metered interpreter work, plus
//!   [`MAX_STEPS_PER_INVOCATION`] as the
//!   stall guard for the case where a step burns no fuel at all;
//! * a staging buffer. A host operation a script calls does not touch the world; it appends a
//!   [`StagedEffect`]. Only a fully successful invocation returns [`StagedEffects`], which the
//!   caller commits through core gameplay operations. A syntax, runtime, or fuel failure returns a
//!   [`ScriptDiagnostic`] instead and every effect staged by that invocation is dropped with it.
//!
//! [`run_event`] is the failure boundary: it invokes each Runtime Script in turn, commits the ones
//! that succeeded, and collects a bounded diagnostic for the ones that did not. One bad script
//! never stops the next one, and never stops the core work that follows.
//!
//! # The curated gameplay surface
//!
//! An Invocation sees one global per host operation and one `event` table, and nothing else of the
//! Module:
//!
//! ```lua
//! event.name              -- the event label, a string
//! event.actor             -- the Entity Handle that caused the event, or nil
//! event.target            -- the Entity Handle the event acted on, or nil
//!
//! -- An Entity Handle's readable fields, snapshotted when the Invocation started:
//! e.name, e.is_player, e.level, e.health, e.max_health, e.map_id, e.x, e.y, e.z
//!
//! heal(entity, amount)    -- stage a heal, crediting heal-threat to event.actor
//! send_chat(player, text) -- stage a System Message to one online player
//! grant_xp(player, amount)-- stage an experience grant
//!
//! return 42               -- the Script Answer: a number the asking caller reads back
//! ```
//!
//! A chunk that returns a number answers the caller that asked; returning nothing, or anything that
//! is not a number, answers nothing. Only [`ask_event`] reads an answer — [`run_event`] discards it,
//! because a core hook event has no caller waiting on one.
//!
//! An Entity Handle is opaque: a script cannot read a guid out of it and cannot mint one, so the
//! only entities a Runtime Script can act on are the ones the Host resolved for that Invocation.
//! Every readable field is a snapshot taken before the script ran; writing to one changes nothing.
//!
//! A host operation called with a missing entity, the wrong type, an out-of-range amount, or past
//! the staging cap raises a Lua error naming the call and the fault. That fails the Invocation —
//! a [`ScriptDiagnostic`], no staged effect committed — and never panics out of the reducer.
//!
//! # What an Invocation is allowed
//!
//! The environment is built from [`ALLOWED_GLOBALS`] and [`ALLOWED_LIBRARY_MEMBERS`], name by name,
//! into tables made for that Invocation. A name outside those lists is nil inside a Runtime Script,
//! and a write to an allowed library table is invisible to the next Invocation.
//!
//! What this host deliberately does NOT do: durable script storage, event bindings, damage, items,
//! spawning, scheduling, any query surface over the database, or any Lua state that outlives an
//! Invocation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use spacetimedb::log;

use piccolo::closure::UpValueState;
use piccolo::{
    Callback, CallbackReturn, Closure, Context, Error, Executor, Fuel, IntoValue, Lua, MetaMethod,
    StashedClosure, Table, UserData, Value,
};

/// Fuel handed to one `Executor::step`. The interpreter checks its budget between operations, so a
/// step can overshoot slightly. The Host refuses the Invocation if that overshoot crosses the total
/// budget.
const FUEL_PER_STEP: i32 = 64;

/// Metered interpreter work one Invocation may perform before it is cut off as a fuel failure.
///
/// Sized by measurement, not by guess. `REPRESENTATIVE_SCRIPT` — the transpiler-shaped workload of
/// list building, a higher-order function, string work and a host call that this host exists to
/// run — costs 2,054 fuel over 30 steps, so the budget is roughly a hundred of those. Fifty times
/// its list still costs only 96,442. At the other end, a bare `while true do end` reaches this
/// number in under a millisecond of interpreter time, which keeps a runaway script off the 0.5s
/// tick.
const FUEL_BUDGET_PER_INVOCATION: i32 = 200_000;

/// Hard cap on `Executor::step` calls per invocation.
///
/// The fuel budget is the real limit: a working script spends about 68 fuel per step, so the
/// budget above runs out around 3,000 steps and this cap never fires. It exists for the step that
/// returns having burned NO fuel, which would otherwise loop here forever with the budget
/// untouched.
const MAX_STEPS_PER_INVOCATION: usize = 20_000;

/// Hard cap, in bytes, on the script-authored text recorded in a [`ScriptDiagnostic`]. A failing
/// script controls its own error message, so this is the only unbounded part of a diagnostic; the
/// script name and event are host-supplied labels.
const DIAGNOSTIC_MESSAGE_CAP: usize = 512;

/// Maximum bytes retained for each host-supplied diagnostic label.
const DIAGNOSTIC_LABEL_CAP: usize = 128;

const TRUNCATION_MARK: &str = "…[truncated]";

/// Largest amount any host operation accepts. Far above any authored heal or experience award, so a
/// bigger number is a script defect rather than a design; refusing it reads as a Script Diagnostic
/// instead of disappearing into a clamp against max health.
const MAX_EFFECT_AMOUNT: i64 = 1_000_000;

/// Most Staged Effects one Invocation may hold. The Fuel Budget meters interpreter work, not
/// durable writes, and a tight loop around a host call buys thousands of them inside the budget.
/// This is the bound on what one script can ask a single reducer transaction to perform. Crossing
/// it fails the Invocation, so a script cannot half-commit its way past the cap either.
const MAX_STAGED_EFFECTS_PER_INVOCATION: usize = 256;

/// The exact global names an Invocation receives from piccolo's core library.
///
/// Deliberately absent: `collectgarbage` (interpreter state, not gameplay), `coroutine` (an
/// Invocation is one straight run under one Fuel Budget, so a suspended thread has nothing to
/// resume into), and `_G` (globals-as-state does not survive an Invocation, so handing a script a
/// mirror of its environment only invites the idiom that cannot work). `print` never arrives
/// because the Host builds on `Lua::core()`, which loads no I/O library.
const ALLOWED_GLOBALS: &[&str] = &[
    "assert",
    "error",
    "getmetatable",
    "ipairs",
    "next",
    "pairs",
    "pcall",
    "rawget",
    "rawset",
    "select",
    "setmetatable",
    "tostring",
    "type",
];

/// The allowlisted members of each standard-library table, by library name.
///
/// Each library is rebuilt from these names into a table made for the Invocation, so a member this
/// list omits is nil and a write to one of these tables is invisible to the next Invocation.
///
/// `math.random` and `math.randomseed` are omitted: the Module's `getrandom` backend is a fixed
/// stream on purpose (see [`fixed_entropy`]), so `math.random` returns the same sequence on every
/// replica and every replay. Durable randomness comes from `ReducerContext::rng` on the Module
/// side of a host operation, never from the interpreter.
const ALLOWED_LIBRARY_MEMBERS: &[(&str, &[&str])] = &[
    (
        "math",
        &[
            "abs",
            "acos",
            "asin",
            "atan",
            "ceil",
            "cos",
            "deg",
            "exp",
            "floor",
            "fmod",
            "huge",
            "log",
            "max",
            "maxinteger",
            "min",
            "mininteger",
            "modf",
            "pi",
            "rad",
            "sin",
            "sqrt",
            "tan",
            "tointeger",
            "type",
            "ult",
        ],
    ),
    ("string", &["len", "lower", "reverse", "sub", "upper"]),
    // `concat` is the shim's, loaded into the shared globals at Host construction.
    ("table", &["concat", "pack", "unpack"]),
];

/// piccolo's stdlib subset omits `table.concat`, which transpiler output uses constantly. Loaded
/// once into the shared globals at host construction, in the form the Runtime Script Prototype
/// proved.
///
/// Two parameters and a type guard, both deliberate: pinned piccolo passes an inline table
/// constructor's ELEMENT COUNT as an extra argument, so `table.concat({"a", "b"})` arrives here as
/// `("a", "b" table, 2)`. A two-parameter function drops that extra value, and the guard catches
/// the one-argument case where the count lands in `separator`. See
/// `piccolo_leaks_a_table_constructors_element_count_as_an_extra_argument`. The `i`/`j` range
/// arguments are left out for the same reason — with the leak they cannot be told from real ones.
const PICCOLO_SHIM: &str = r#"
table.concat = function(list, separator)
    if type(separator) ~= "string" then separator = "" end
    local out = ""
    for i = 1, #list do
        if i > 1 then out = out .. separator end
        out = out .. tostring(list[i])
    end
    return out
end
"#;

/// Why an invocation was abandoned. Every one of these discards the invocation's staged effects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FailureKind {
    /// The source did not compile.
    Syntax,
    /// The script raised an error while running.
    Runtime,
    /// The script ran past its fuel budget or its step cap.
    Fuel,
}

impl FailureKind {
    fn as_str(self) -> &'static str {
        match self {
            FailureKind::Syntax => "syntax",
            FailureKind::Runtime => "runtime",
            FailureKind::Fuel => "fuel",
        }
    }
}

/// The bounded record of one failed invocation. Names the script, the event it was running for,
/// and what kind of failure it was, so an Operator can act on it without interpreter internals.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ScriptDiagnostic {
    pub script: String,
    pub event: String,
    pub kind: FailureKind,
    pub message: String,
}

impl ScriptDiagnostic {
    fn new(script: &str, event: &str, kind: FailureKind, message: String) -> Self {
        Self {
            script: bounded(script, DIAGNOSTIC_LABEL_CAP),
            event: bounded(event, DIAGNOSTIC_LABEL_CAP),
            kind,
            message: bounded(&message, DIAGNOSTIC_MESSAGE_CAP),
        }
    }
}

impl std::fmt::Display for ScriptDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "runtime script `{}` on `{}`: {} failure — {}",
            self.script,
            self.event,
            self.kind.as_str(),
            self.message
        )
    }
}

/// Cuts `text` down to `cap` bytes including the truncation mark, on a char boundary so the result
/// remains valid UTF-8.
fn bounded(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let mut keep = cap - TRUNCATION_MARK.len();
    while keep > 0 && !text.is_char_boundary(keep) {
        keep -= 1;
    }
    let mut out = text[..keep].to_string();
    out.push_str(TRUNCATION_MARK);
    out
}

/// A gameplay operation a Runtime Script asked for. Held, not performed, until the invocation
/// that staged it succeeds.
///
/// Every guid here was resolved by the Host from an Entity Handle. A Runtime Script never supplies
/// one, so a staged effect can only name an entity the Host already put in front of that
/// Invocation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum StagedEffect {
    GrantXp {
        character_guid: u64,
        amount: u32,
    },
    Heal {
        healer_guid: u64,
        target_guid: u64,
        amount: u32,
    },
    SendChat {
        recipient_guid: u64,
        message: String,
    },
}

/// Everything one SUCCESSFUL invocation staged, in the order the script staged it. A failed
/// invocation never produces one of these, so there is no "commit a failed run" state to get wrong.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct StagedEffects(Vec<StagedEffect>);

impl StagedEffects {
    /// Apply every staged effect through core gameplay operations, in staging order.
    pub(crate) fn commit<S: EffectSink>(self, sink: &mut S) {
        for effect in self.0 {
            match effect {
                StagedEffect::GrantXp {
                    character_guid,
                    amount,
                } => sink.grant_xp(character_guid, amount),
                StagedEffect::Heal {
                    healer_guid,
                    target_guid,
                    amount,
                } => sink.heal(healer_guid, target_guid, amount),
                StagedEffect::SendChat {
                    recipient_guid,
                    message,
                } => sink.send_chat(recipient_guid, &message),
            }
        }
    }
}

/// What one SUCCESSFUL Invocation produced: everything it staged, and the number it returned.
///
/// A failed Invocation produces a [`ScriptDiagnostic`] instead, so there is no "commit a failed
/// run" state and no answer from a script that did not finish.
#[derive(Debug)]
pub(crate) struct Invocation {
    /// The gameplay operations the script asked for, in staging order.
    pub effects: StagedEffects,
    /// The **Script Answer**: the number the chunk returned, when it returned one.
    // ponytail: a number is every answer a caller has needed so far. Widen to a small enum when one
    // needs a string or a table back.
    pub answer: Option<f64>,
}

/// The seam staged effects commit through: the real database in the Module, a Fake in tests.
pub(crate) trait EffectSink {
    fn grant_xp(&mut self, character_guid: u64, amount: u32);
    fn heal(&mut self, healer_guid: u64, target_guid: u64, amount: u32);
    fn send_chat(&mut self, recipient_guid: u64, message: &str);
}

/// The curated read of one creature or player, taken before the Invocation ran. A Runtime Script
/// reads these fields off an Entity Handle and can act on the entity through a host operation, but
/// never learns its guid and never reaches a row.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct EntityView {
    pub guid: u64,
    pub name: String,
    pub is_player: bool,
    pub level: u32,
    pub health: u32,
    pub max_health: u32,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// What one Invocation is running for: the event label and the entities it involves.
///
/// `actor` caused the event, `target` is what it acted on. Either can be absent — an event with no
/// target leaves `event.target` nil, which is the defined result a script tests for rather than a
/// failure it has to survive.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct ScriptEvent {
    pub name: String,
    pub actor: Option<EntityView>,
    pub target: Option<EntityView>,
}

/// What an Entity Handle carries: the identity the Host acts on, and nothing a script can read.
///
/// It lives inside a Lua userdata, which a Runtime Script can pass around but cannot construct,
/// inspect, or forge — so the Host can trust that every handle reaching a host operation is one it
/// minted. The handle lasts exactly as long as the Invocation, because the environment holding it
/// is built fresh for that Invocation and nothing carries to the next one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct EntityHandle {
    guid: u64,
    is_player: bool,
}

/// One Runtime Script as the host sees it: a name for diagnostics and the Lua to run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct RuntimeScript<'a> {
    pub name: &'a str,
    pub source: &'a str,
}

/// The embedded interpreter plus its compiler cache. Long-lived; holds no Lua state that a
/// Runtime Script can reach from one invocation to the next.
pub(crate) struct RuntimeScriptHost {
    lua: Lua,
    /// Compiled chunks keyed by the blake3 hash of their source. Hashing the source means an
    /// edited script can never hit a stale entry, so the cache needs no invalidation.
    chunks: HashMap<[u8; 32], StashedClosure>,
    compilations: usize,
}

impl RuntimeScriptHost {
    pub(crate) fn new() -> Self {
        let mut lua = Lua::core();
        // The shim is a compile-time constant against a pinned piccolo. A failure here is a build
        // defect, not a runtime condition; `the_shim_supplies_the_table_concat_piccolo_lacks`
        // catches it long before a publish.
        let shim = lua
            .try_enter(|ctx| {
                let closure =
                    Closure::load(ctx, Some("runtime_script_shim"), PICCOLO_SHIM.as_bytes())?;
                Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
            })
            .expect("the runtime script shim is a constant and must compile");
        lua.execute::<()>(&shim)
            .expect("the runtime script shim is a constant and must run");
        Self {
            lua,
            chunks: HashMap::new(),
            compilations: 0,
        }
    }

    /// How many chunks this host has compiled since it was built. A cache hit does not raise it,
    /// which is how "valid Lua compiles once and runs many times" is observable.
    ///
    /// An observation point, not a gameplay path: the tests read it, and so does
    /// `debug_run_runtime_script` so an Operator can watch the cache work on a live realm. Neither
    /// is compiled into a default release build, which is what the allow says.
    #[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
    pub(crate) fn compilations(&self) -> usize {
        self.compilations
    }

    /// Drop every compiled chunk.
    ///
    /// The cache is keyed by a hash of the source, so it can never serve stale code and needs no
    /// invalidation for CORRECTNESS. What it lacks is a reason to ever shrink: without this, every
    /// script source the Module has seen since the wasm instance started stays compiled in it
    /// forever, including the ones no Package ships any more.
    ///
    /// So there is one eviction point and it is the honest one: a `script` family apply, which is
    /// the only thing that changes which sources exist. Called from there, the cache holds the
    /// scripts the Shard is actually running, plus whatever `debug_run_runtime_script` has tried
    /// since. Nothing durable is lost — a chunk is derived from source and recompiles on demand.
    pub(crate) fn clear_chunks(&mut self) {
        self.chunks.clear();
    }

    /// Run `script` for `event` in a fresh environment under a fuel budget.
    ///
    /// On success, returns everything the script staged and the Script Answer it returned — nothing
    /// has touched the world yet. On any failure, returns a bounded diagnostic and both the staged
    /// effects and the answer are dropped unread.
    pub(crate) fn invoke(
        &mut self,
        script: RuntimeScript<'_>,
        event: &ScriptEvent,
    ) -> Result<Invocation, ScriptDiagnostic> {
        let chunk = self.compiled(script, &event.name)?;

        let staged: Rc<RefCell<Vec<StagedEffect>>> = Rc::new(RefCell::new(Vec::new()));
        let staging_handle = Rc::clone(&staged);
        let executor = self.lua.enter(|ctx| {
            let env = fresh_environment(ctx, event, staging_handle);
            let closure = ctx.fetch(&chunk);
            // Rebinding the cached chunk's `_ENV` upvalue is what makes the compiler cache a
            // CODE cache: the compiled prototype is reused, the environment it closes over is not.
            // A chunk that never reads or writes a global has no upvalue and needs no rebinding.
            if let Some(env_upvalue) = closure.upvalues().first() {
                env_upvalue.set(&ctx, UpValueState::Closed(Value::Table(env)));
            }
            ctx.stash(Executor::start(ctx, closure.into(), ()))
        });

        let mut fuel_spent: i32 = 0;
        let mut steps: usize = 0;
        loop {
            let (finished, burned) = self.lua.enter(|ctx| {
                let mut fuel = Fuel::with(FUEL_PER_STEP);
                let finished = ctx.fetch(&executor).step(ctx, &mut fuel);
                (finished, FUEL_PER_STEP.saturating_sub(fuel.remaining()))
            });
            fuel_spent = fuel_spent.saturating_add(burned.max(0));
            steps += 1;
            if fuel_spent > FUEL_BUDGET_PER_INVOCATION
                || (fuel_spent == FUEL_BUDGET_PER_INVOCATION && !finished)
            {
                return Err(ScriptDiagnostic::new(
                    script.name,
                    &event.name,
                    FailureKind::Fuel,
                    format!(
                        "exhausted the {FUEL_BUDGET_PER_INVOCATION} fuel budget (spent {fuel_spent})"
                    ),
                ));
            }
            if steps >= MAX_STEPS_PER_INVOCATION && !finished {
                return Err(ScriptDiagnostic::new(
                    script.name,
                    &event.name,
                    FailureKind::Fuel,
                    format!(
                        "stalled: {MAX_STEPS_PER_INVOCATION} steps burned only {fuel_spent} fuel"
                    ),
                ));
            }
            if finished {
                break;
            }
        }

        let outcome = self.lua.enter(|ctx| {
            match ctx.fetch(&executor).take_result::<Value>(ctx) {
                Ok(Ok(returned)) => Ok(script_answer(returned)),
                Ok(Err(error)) => Err(error.to_string()),
                // Unreachable: the loop only leaves through `finished`, which means Result mode.
                Err(mode) => Err(mode.to_string()),
            }
        });
        let answer = match outcome {
            Ok(answer) => answer,
            Err(message) => {
                return Err(ScriptDiagnostic::new(
                    script.name,
                    &event.name,
                    FailureKind::Runtime,
                    message,
                ))
            }
        };

        // The script is gone; nothing else holds the staging buffer.
        let effects = staged.borrow().clone();
        Ok(Invocation {
            effects: StagedEffects(effects),
            answer,
        })
    }

    fn compiled(
        &mut self,
        script: RuntimeScript<'_>,
        event: &str,
    ) -> Result<StashedClosure, ScriptDiagnostic> {
        let key: [u8; 32] = blake3::hash(script.source.as_bytes()).into();
        if let Some(chunk) = self.chunks.get(&key) {
            return Ok(chunk.clone());
        }
        let compiled = self.lua.try_enter(|ctx| {
            let closure = Closure::load(ctx, Some(script.name), script.source.as_bytes())?;
            Ok(ctx.stash(closure))
        });
        match compiled {
            Ok(chunk) => {
                self.compilations += 1;
                self.chunks.insert(key, chunk.clone());
                Ok(chunk)
            }
            Err(error) => Err(ScriptDiagnostic::new(
                script.name,
                event,
                FailureKind::Syntax,
                error.to_string(),
            )),
        }
    }
}

/// The Script Answer in what a chunk returned: a Lua number, and nothing else.
///
/// A string that reads as a number is not an answer. Lua would coerce it happily, but a script that
/// meant to answer returns a number, so the near miss reads as "no answer" rather than as a silent
/// conversion the author never asked for.
fn script_answer(returned: Value<'_>) -> Option<f64> {
    match returned {
        Value::Integer(number) => Some(number as f64),
        Value::Number(number) => Some(number),
        _ => None,
    }
}

/// Build the environment one Invocation sees: the allowlisted standard library, the `event` table
/// with its Entity Handles, and one global per host operation.
fn fresh_environment<'gc>(
    ctx: Context<'gc>,
    event: &ScriptEvent,
    staged: Rc<RefCell<Vec<StagedEffect>>>,
) -> Table<'gc> {
    let env = allowlisted_standard_library(ctx);

    let actor_guid = event.actor.as_ref().map(|actor| actor.guid);
    set(ctx, env, "event", event_table(ctx, event));
    set(ctx, env, "heal", heal_operation(ctx, actor_guid, &staged));
    set(ctx, env, "send_chat", send_chat_operation(ctx, &staged));
    set(ctx, env, "grant_xp", grant_xp_operation(ctx, &staged));
    env
}

/// The environment's standard library, rebuilt name by name from [`ALLOWED_GLOBALS`] and
/// [`ALLOWED_LIBRARY_MEMBERS`].
///
/// Each library table is a new table holding the allowlisted members, so a script that writes to
/// `string` writes to its own copy. The shared globals this reads from are never handed out, which
/// is what keeps one Invocation's damage to itself.
fn allowlisted_standard_library<'gc>(ctx: Context<'gc>) -> Table<'gc> {
    let core = ctx.globals();
    let env = Table::new(&ctx);
    for name in ALLOWED_GLOBALS {
        let value: Value = core.get(ctx, *name);
        set(ctx, env, name, value);
    }
    for (library, members) in ALLOWED_LIBRARY_MEMBERS {
        let Value::Table(source) = core.get(ctx, *library) else {
            continue;
        };
        let copy = Table::new(&ctx);
        for member in *members {
            let value: Value = source.get(ctx, *member);
            set(ctx, copy, member, value);
        }
        set(ctx, env, library, copy);
    }
    env
}

/// `event` as a Runtime Script reads it. A fresh table each Invocation, so a script that writes to
/// it changes nothing the Host will ever read.
fn event_table<'gc>(ctx: Context<'gc>, event: &ScriptEvent) -> Table<'gc> {
    let table = Table::new(&ctx);
    set(ctx, table, "name", text(ctx, &event.name));
    for (field, view) in [("actor", &event.actor), ("target", &event.target)] {
        let value = match view {
            Some(view) => Value::UserData(entity_handle(ctx, view)),
            None => Value::Nil,
        };
        set(ctx, table, field, value);
    }
    table
}

/// Mint the Entity Handle for one entity: an opaque userdata carrying the identity, with a
/// metatable serving the curated fields snapshotted from `view`.
fn entity_handle<'gc>(ctx: Context<'gc>, view: &EntityView) -> UserData<'gc> {
    let fields = Table::new(&ctx);
    set(ctx, fields, "name", text(ctx, &view.name));
    set(ctx, fields, "is_player", view.is_player);
    set(ctx, fields, "level", view.level as i64);
    set(ctx, fields, "health", view.health as i64);
    set(ctx, fields, "max_health", view.max_health as i64);
    set(ctx, fields, "map_id", view.map_id as i64);
    set(ctx, fields, "x", view.x as f64);
    set(ctx, fields, "y", view.y as f64);
    set(ctx, fields, "z", view.z as f64);

    let metatable = Table::new(&ctx);
    metatable
        .set(ctx, MetaMethod::Index, fields)
        .expect("a metamethod name is a valid Lua table key");

    let handle = UserData::new_static(
        &ctx,
        EntityHandle {
            guid: view.guid,
            is_player: view.is_player,
        },
    );
    handle.set_metatable(&ctx, Some(metatable));
    handle
}

/// `heal(entity, amount)`. Credits heal-threat to the Invocation's actor, the way a cast heal
/// credits its caster; an event with no actor heals without pulling aggro for anyone.
fn heal_operation<'gc>(
    ctx: Context<'gc>,
    actor_guid: Option<u64>,
    staged: &Rc<RefCell<Vec<StagedEffect>>>,
) -> Callback<'gc> {
    let staged = Rc::clone(staged);
    Callback::from_fn(&ctx, move |ctx, _execution, mut stack| {
        let (entity, amount): (Value, Value) = stack.consume(ctx)?;
        let target = entity_argument(ctx, "heal", "target", entity)?;
        let amount = amount_argument(ctx, "heal", amount)?;
        stage(
            ctx,
            &staged,
            "heal",
            StagedEffect::Heal {
                healer_guid: actor_guid.unwrap_or(0),
                target_guid: target.guid,
                amount,
            },
        )?;
        Ok(CallbackReturn::Return)
    })
}

/// `send_chat(player, text)`. The text is normalized here, by the same rule the chat core applies,
/// so what the Invocation staged is exactly what commits.
fn send_chat_operation<'gc>(
    ctx: Context<'gc>,
    staged: &Rc<RefCell<Vec<StagedEffect>>>,
) -> Callback<'gc> {
    let staged = Rc::clone(staged);
    Callback::from_fn(&ctx, move |ctx, _execution, mut stack| {
        let (entity, text): (Value, Value) = stack.consume(ctx)?;
        let recipient = player_argument(ctx, "send_chat", "recipient", entity)?;
        let text = text_argument(ctx, "send_chat", text)?;
        let message = crate::chat::normalized_message(&text)
            .ok_or_else(|| host_error(ctx, "send_chat", "the message is empty"))?;
        stage(
            ctx,
            &staged,
            "send_chat",
            StagedEffect::SendChat {
                recipient_guid: recipient.guid,
                message,
            },
        )?;
        Ok(CallbackReturn::Return)
    })
}

/// `grant_xp(player, amount)`.
fn grant_xp_operation<'gc>(
    ctx: Context<'gc>,
    staged: &Rc<RefCell<Vec<StagedEffect>>>,
) -> Callback<'gc> {
    let staged = Rc::clone(staged);
    Callback::from_fn(&ctx, move |ctx, _execution, mut stack| {
        let (entity, amount): (Value, Value) = stack.consume(ctx)?;
        let character = player_argument(ctx, "grant_xp", "recipient", entity)?;
        let amount = amount_argument(ctx, "grant_xp", amount)?;
        stage(
            ctx,
            &staged,
            "grant_xp",
            StagedEffect::GrantXp {
                character_guid: character.guid,
                amount,
            },
        )?;
        Ok(CallbackReturn::Return)
    })
}

/// Record one Staged Effect, refusing once the Invocation has reached
/// [`MAX_STAGED_EFFECTS_PER_INVOCATION`]. The refusal fails the Invocation, which discards
/// everything it staged — a script cannot get its first 256 effects committed by overrunning.
fn stage<'gc>(
    ctx: Context<'gc>,
    staged: &Rc<RefCell<Vec<StagedEffect>>>,
    call: &str,
    effect: StagedEffect,
) -> Result<(), Error<'gc>> {
    let mut staged = staged.borrow_mut();
    if staged.len() >= MAX_STAGED_EFFECTS_PER_INVOCATION {
        return Err(host_error(
            ctx,
            call,
            &format!(
                "one invocation may stage at most {MAX_STAGED_EFFECTS_PER_INVOCATION} effects"
            ),
        ));
    }
    staged.push(effect);
    Ok(())
}

/// Resolve one host-call argument to the Entity Handle the Host minted for it.
fn entity_argument<'gc>(
    ctx: Context<'gc>,
    call: &str,
    role: &str,
    value: Value<'gc>,
) -> Result<EntityHandle, Error<'gc>> {
    match value {
        Value::UserData(data) => data
            .downcast_static::<EntityHandle>()
            .copied()
            .map_err(|_| host_error(ctx, call, &format!("the {role} is not an entity"))),
        Value::Nil => Err(host_error(ctx, call, &format!("there is no {role}"))),
        other => Err(host_error(
            ctx,
            call,
            &format!("the {role} is a {}, not an entity", other.type_name()),
        )),
    }
}

/// The same, for the operations that only mean anything against a player.
fn player_argument<'gc>(
    ctx: Context<'gc>,
    call: &str,
    role: &str,
    value: Value<'gc>,
) -> Result<EntityHandle, Error<'gc>> {
    let handle = entity_argument(ctx, call, role, value)?;
    if !handle.is_player {
        return Err(host_error(
            ctx,
            call,
            &format!("the {role} is a creature, not a player"),
        ));
    }
    Ok(handle)
}

/// Resolve an amount argument. Whole numbers inside `1..=`[`MAX_EFFECT_AMOUNT`] only: zero and
/// negatives are a script defect rather than a no-op worth performing.
fn amount_argument<'gc>(
    ctx: Context<'gc>,
    call: &str,
    value: Value<'gc>,
) -> Result<u32, Error<'gc>> {
    let Some(amount) = value.to_integer() else {
        return Err(host_error(
            ctx,
            call,
            &format!("the amount is a {}, not a whole number", value.type_name()),
        ));
    };
    if !(1..=MAX_EFFECT_AMOUNT).contains(&amount) {
        return Err(host_error(
            ctx,
            call,
            &format!("the amount {amount} is outside 1..={MAX_EFFECT_AMOUNT}"),
        ));
    }
    Ok(amount as u32)
}

/// Resolve a text argument to UTF-8. Lua strings are bytes, so a script can hand over something no
/// durable column can hold.
fn text_argument<'gc>(
    ctx: Context<'gc>,
    call: &str,
    value: Value<'gc>,
) -> Result<String, Error<'gc>> {
    let Value::String(text) = value else {
        return Err(host_error(
            ctx,
            call,
            &format!("the message is a {}, not a string", value.type_name()),
        ));
    };
    text.to_str()
        .map(str::to_string)
        .map_err(|_| host_error(ctx, call, "the message is not valid UTF-8"))
}

/// The Lua error a misused host operation raises: the call that was misused and the fault, so the
/// Script Diagnostic it becomes can be acted on without reading the script.
fn host_error<'gc>(ctx: Context<'gc>, call: &str, fault: &str) -> Error<'gc> {
    text(ctx, &format!("{call}: {fault}"))
        .into_value(ctx)
        .into()
}

/// A Lua string holding `source`. piccolo converts only a `&'static str` implicitly, and most of
/// what this module hands to a script is owned domain text.
fn text<'gc>(ctx: Context<'gc>, source: &str) -> piccolo::String<'gc> {
    piccolo::String::from_slice(&ctx, source)
}

/// Set one key on a Lua table. Every key this module writes is a fixed name, so a failure would
/// mean a nil or NaN key the code cannot produce.
fn set<'gc>(ctx: Context<'gc>, table: Table<'gc>, key: &'static str, value: impl IntoValue<'gc>) {
    table
        .set(ctx, key, value)
        .expect("a string is a valid Lua table key");
}

thread_local! {
    /// The Module's one Runtime Script Host. The wasm instance is single-threaded, and the host
    /// has to outlive a reducer call for its compiler cache to be worth anything. Nothing durable
    /// lives here: the cache is derived from script source and rebuilds itself on demand.
    static HOST: RefCell<RuntimeScriptHost> = RefCell::new(RuntimeScriptHost::new());
}

/// Borrow the Module's Runtime Script Host for the length of one call.
///
/// `None` means the host was already borrowed — a Runtime Script reached it again through a core
/// operation one of its own Staged Effects committed. Refusing the re-entry keeps it from
/// panicking out of the surrounding reducer, which would take the whole tick down with it; the
/// invocation that tried simply does not happen.
pub(crate) fn with_host<R>(f: impl FnOnce(&mut RuntimeScriptHost) -> R) -> Option<R> {
    HOST.with(|host| host.try_borrow_mut().ok().map(|mut host| f(&mut host)))
}

/// Run every Runtime Script bound to `event`, committing the ones that succeed.
///
/// This is the failure boundary. A script that fails to compile, raises, or runs out of fuel
/// contributes a diagnostic and nothing else: no effect of its own lands, and the scripts after it
/// still run. The returned diagnostics are the caller's to log.
///
/// A core hook event has no caller waiting on a Script Answer, so any answer is discarded here.
pub(crate) fn run_event<S: EffectSink>(
    host: &mut RuntimeScriptHost,
    sink: &mut S,
    event: &ScriptEvent,
    scripts: &[RuntimeScript<'_>],
) -> Vec<ScriptDiagnostic> {
    ask_event(host, sink, event, scripts).0
}

/// [`run_event`], plus the Script Answer the scripts gave.
///
/// The answer is the FIRST number returned, in the order the scripts were handed over — the caller
/// decided that order, so the answer is decided by it too. The scripts after the answering one
/// still run: they may stage effects of their own, and a Package that wanted them not to orders
/// them ahead of it. A script that fails contributes no answer and does not stop the next one, so a
/// broken script leaves its caller on whatever fallback "no answer" means there.
pub(crate) fn ask_event<S: EffectSink>(
    host: &mut RuntimeScriptHost,
    sink: &mut S,
    event: &ScriptEvent,
    scripts: &[RuntimeScript<'_>],
) -> (Vec<ScriptDiagnostic>, Option<f64>) {
    let mut diagnostics = Vec::new();
    let mut answer = None;
    for script in scripts {
        match host.invoke(*script, event) {
            Ok(invocation) => {
                answer = answer.or(invocation.answer);
                invocation.effects.commit(sink);
            }
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    (diagnostics, answer)
}

/// Run exactly one Runtime Script chosen by identity, for a caller that already knows which script
/// to invoke and has no Event Binding to look up — the `E_SCRIPTED` spell effect: its `script_id`
/// names the script outright, so there is no `by_event` dispatch to run. Mirrors what
/// `script_binding::fire` does around [`run_event`] (one Host borrow, a succeeding invocation's
/// effects committed to the real database, a failure logged) minus the `by_event` lookup and the
/// priority ordering `fire` needs only when several scripts answer one event.
///
/// Returns whether the invocation succeeded and committed. Every failure shape collapses to
/// `false` here — a compile/runtime/fuel failure, or the Host already being borrowed by an outer
/// invocation — because the caller has exactly one thing to decide either way: this effect did not
/// happen. The failure is still logged, the same `ScriptDiagnostic` line `fire` logs for its own
/// scripts.
pub(crate) fn invoke_by_identity(
    ctx: &spacetimedb::ReducerContext,
    script: RuntimeScript<'_>,
    event: &ScriptEvent,
) -> bool {
    let Some(diagnostics) = with_host(|host| {
        run_event(
            host,
            &mut CoreEffects { ctx },
            event,
            std::slice::from_ref(&script),
        )
    }) else {
        log::warn!(
            "`{}`: the Runtime Script Host is already running a script, so `{}` did not run.",
            event.name,
            script.name
        );
        return false;
    };
    match diagnostics.into_iter().next() {
        Some(diagnostic) => {
            log::warn!("{diagnostic}");
            false
        }
        None => true,
    }
}

/// The production [`EffectSink`]: staged effects become real gameplay operations here, and only
/// here.
pub(crate) struct CoreEffects<'a> {
    pub ctx: &'a spacetimedb::ReducerContext,
}

impl EffectSink for CoreEffects<'_> {
    fn grant_xp(&mut self, character_guid: u64, amount: u32) {
        use crate::game_world_entity;
        let Some(mut entity) = crate::helpers::acting_entity_by_guid(self.ctx, character_guid)
        else {
            return;
        };
        crate::xp::grant_xp(self.ctx, &mut entity, amount);
        self.ctx.db.game_world_entity().guid().update(entity);
    }

    fn heal(&mut self, healer_guid: u64, target_guid: u64, amount: u32) {
        // The amount is bounded by `MAX_EFFECT_AMOUNT`, well inside `i32`.
        crate::spell::apply_direct_heal(self.ctx, healer_guid, target_guid, amount as i32);
    }

    fn send_chat(&mut self, recipient_guid: u64, message: &str) {
        // The recipient was online when the Invocation read it and may not be by now. That is an
        // ordinary outcome, not a script defect, so it does not become a Script Diagnostic.
        if let Err(reason) =
            crate::actor::system_message(self.ctx, recipient_guid, message.to_string())
        {
            log::info!("runtime script system message to {recipient_guid} did not land: {reason}");
        }
    }
}

impl EntityView {
    /// Read the live row for `guid` as the curated view one Invocation sees.
    ///
    /// `None` when the guid names nothing an Invocation may act on — including a character in
    /// transit between Shards, which the acting-entity gate already treats as out of the world.
    /// A caller with no participant to name passes 0 and gets `None`.
    pub(crate) fn read(ctx: &spacetimedb::ReducerContext, guid: u64) -> Option<Self> {
        use crate::game_creature_template;
        let entity = crate::helpers::acting_entity_by_guid(ctx, guid)?;
        let is_player = entity.is_player();
        let name = if is_player {
            crate::helpers::character_by_guid(ctx, guid).map(|character| character.name)
        } else {
            ctx.db
                .game_creature_template()
                .entry()
                .find(entity.entry)
                .map(|template| template.name)
        };
        Some(Self {
            guid,
            name: name.unwrap_or_default(),
            is_player,
            level: entity.level,
            health: entity.health,
            max_health: entity.max_health,
            map_id: entity.map_id,
            x: entity.x,
            y: entity.y,
            z: entity.z,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workload this host exists to run: a transpiler-shaped chunk of list building, a higher
    /// order function, string work through the shim, and a host call. The fuel budget is sized
    /// against this.
    const REPRESENTATIVE_SCRIPT: &str = r#"
local function map(list, f)
    local out = {}
    for i = 1, #list do out[i] = f(list[i]) end
    return out
end
local names = {}
for i = 1, 20 do names[i] = "unit" .. i end
local shouted = map(names, function(name) return string.upper(name) end)
local roster = table.concat(shouted, ",")
if #roster > 0 then grant_xp(event.actor, 25) end
"#;

    const PLAYER_GUID: u64 = 7;
    const CREATURE_GUID: u64 = 9;

    /// One committed gameplay operation, in the order the sink received it.
    #[derive(Clone, PartialEq, Eq, Debug)]
    enum Committed {
        Xp {
            character: u64,
            amount: u32,
        },
        Heal {
            healer: u64,
            target: u64,
            amount: u32,
        },
        Chat {
            recipient: u64,
            message: String,
        },
    }

    #[derive(Default)]
    struct FakeEffects {
        committed: Vec<Committed>,
    }

    impl EffectSink for FakeEffects {
        fn grant_xp(&mut self, character_guid: u64, amount: u32) {
            self.committed.push(Committed::Xp {
                character: character_guid,
                amount,
            });
        }

        fn heal(&mut self, healer_guid: u64, target_guid: u64, amount: u32) {
            self.committed.push(Committed::Heal {
                healer: healer_guid,
                target: target_guid,
                amount,
            });
        }

        fn send_chat(&mut self, recipient_guid: u64, message: &str) {
            self.committed.push(Committed::Chat {
                recipient: recipient_guid,
                message: message.to_string(),
            });
        }
    }

    fn player() -> EntityView {
        EntityView {
            guid: PLAYER_GUID,
            name: "Thrall".to_string(),
            is_player: true,
            level: 12,
            health: 340,
            max_health: 420,
            map_id: 1,
            x: 1.5,
            y: -2.5,
            z: 3.0,
        }
    }

    fn creature() -> EntityView {
        EntityView {
            guid: CREATURE_GUID,
            name: "Kobold Miner".to_string(),
            is_player: false,
            level: 4,
            health: 30,
            max_health: 60,
            map_id: 1,
            x: 10.5,
            y: -20.25,
            z: 30.75,
        }
    }

    /// An event with a label and no participants, for the tests that only care about the label.
    fn unattended(name: &str) -> ScriptEvent {
        ScriptEvent {
            name: name.to_string(),
            ..ScriptEvent::default()
        }
    }

    /// The event most tests run against: a player acting on a creature.
    fn engagement() -> ScriptEvent {
        ScriptEvent {
            name: "on_login".to_string(),
            actor: Some(player()),
            target: Some(creature()),
        }
    }

    fn script<'a>(name: &'a str, source: &'a str) -> RuntimeScript<'a> {
        RuntimeScript { name, source }
    }

    /// What one invocation actually puts through the sink: run it, then commit whatever it staged.
    fn committed(
        host: &mut RuntimeScriptHost,
        event: &ScriptEvent,
        source: &str,
    ) -> Result<Vec<Committed>, ScriptDiagnostic> {
        let invocation = host.invoke(script("probe", source), event)?;
        let mut sink = FakeEffects::default();
        invocation.effects.commit(&mut sink);
        Ok(sink.committed)
    }

    /// The experience amounts one invocation granted. Several tests use `grant_xp` as the way a
    /// script reports a number back to Rust.
    fn granted_amounts(committed: &[Committed]) -> Vec<u32> {
        committed
            .iter()
            .filter_map(|effect| match effect {
                Committed::Xp { amount, .. } => Some(*amount),
                _ => None,
            })
            .collect()
    }

    fn xp(character: u64, amount: u32) -> Committed {
        Committed::Xp { character, amount }
    }

    #[test]
    fn valid_lua_compiles_once_and_runs_in_every_fresh_invocation() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        let award = script("award", "grant_xp(event.actor, 40)");
        for _ in 0..3 {
            let invocation = host.invoke(award, &event).expect("valid Lua runs");
            let mut sink = FakeEffects::default();
            invocation.effects.commit(&mut sink);
            assert_eq!(sink.committed, [xp(PLAYER_GUID, 40)]);
        }
        assert_eq!(
            host.compilations(),
            1,
            "three invocations of one source must reuse one compiled chunk"
        );
        host.invoke(script("other", "grant_xp(event.actor, 1)"), &event)
            .expect("valid Lua runs");
        assert_eq!(
            host.compilations(),
            2,
            "a different source is a different chunk"
        );
    }

    #[test]
    fn a_global_written_in_one_invocation_is_absent_from_the_next() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        for _ in 0..3 {
            assert_eq!(
                committed(
                    &mut host,
                    &event,
                    "visits = (visits or 0) + 1\ngrant_xp(event.actor, visits)"
                )
                .unwrap(),
                [xp(PLAYER_GUID, 1)],
                "each invocation must start from an empty environment, so `visits` is always nil"
            );
        }
    }

    #[test]
    fn an_endless_loop_spends_the_fuel_budget_instead_of_stalling_the_tick() {
        let mut host = RuntimeScriptHost::new();
        let failure = host
            .invoke(
                script("spin", "while true do end"),
                &unattended("on_damage_taken"),
            )
            .expect_err("an endless loop cannot succeed");
        assert_eq!(failure.kind, FailureKind::Fuel);
    }

    #[test]
    fn a_diagnostic_names_the_script_the_event_and_the_failure_kind() {
        let mut host = RuntimeScriptHost::new();
        let failure = host
            .invoke(
                script("broken", "this is not lua ==="),
                &unattended("on_levelup"),
            )
            .expect_err("malformed source cannot compile");
        assert_eq!(failure.script, "broken");
        assert_eq!(failure.event, "on_levelup");
        assert_eq!(failure.kind, FailureKind::Syntax);
        assert!(!failure.message.is_empty());
    }

    #[test]
    fn a_diagnostic_message_is_capped_however_much_the_script_raises() {
        let mut host = RuntimeScriptHost::new();
        let shouty = script(
            "shouty",
            "local s = \"x\"\nfor i = 1, 14 do s = s .. s end\nerror(s)",
        );
        let failure = host
            .invoke(shouty, &unattended("on_login"))
            .expect_err("a raised error is a failure");
        assert_eq!(failure.kind, FailureKind::Runtime);
        assert!(
            failure.message.len() <= DIAGNOSTIC_MESSAGE_CAP,
            "a 16KiB error message must not reach the log: got {} bytes",
            failure.message.len()
        );
        assert!(failure.message.ends_with(TRUNCATION_MARK));
    }

    #[test]
    fn a_truncated_diagnostic_message_stays_valid_utf8() {
        let long = "é".repeat(DIAGNOSTIC_MESSAGE_CAP);
        let cut = bounded(&long, DIAGNOSTIC_MESSAGE_CAP);
        assert!(cut.len() <= DIAGNOSTIC_MESSAGE_CAP);
        assert!(cut.ends_with(TRUNCATION_MARK));
    }

    #[test]
    fn diagnostic_labels_are_bounded_and_stay_valid_utf8() {
        let label = "é".repeat(DIAGNOSTIC_LABEL_CAP);
        let diagnostic =
            ScriptDiagnostic::new(&label, &label, FailureKind::Runtime, "failed".to_string());
        assert!(diagnostic.script.len() <= DIAGNOSTIC_LABEL_CAP);
        assert!(diagnostic.event.len() <= DIAGNOSTIC_LABEL_CAP);
        assert!(diagnostic.script.ends_with(TRUNCATION_MARK));
        assert!(diagnostic.event.ends_with(TRUNCATION_MARK));
    }

    // ---- the allowlisted environment ----

    /// The whole surface an Invocation gets, pinned. A name arriving here that this list does not
    /// carry is a widening of the host API, which is a decision rather than an accident.
    #[test]
    fn an_invocation_receives_exactly_the_allowlisted_surface() {
        let mut host = RuntimeScriptHost::new();
        let staged = Rc::new(RefCell::new(Vec::new()));
        let names = host.lua.enter(|ctx| {
            let env = fresh_environment(ctx, &engagement(), staged);
            let mut names: Vec<String> = env
                .iter()
                .filter_map(|(key, _)| match key {
                    Value::String(name) => Some(name.to_string()),
                    _ => None,
                })
                .collect();
            names.sort();
            names
        });
        assert_eq!(
            names,
            [
                "assert",
                "error",
                "event",
                "getmetatable",
                "grant_xp",
                "heal",
                "ipairs",
                "math",
                "next",
                "pairs",
                "pcall",
                "rawget",
                "rawset",
                "select",
                "send_chat",
                "setmetatable",
                "string",
                "table",
                "tostring",
                "type",
            ]
        );
    }

    #[test]
    fn a_name_outside_the_allowlist_is_nil_inside_a_script() {
        let mut host = RuntimeScriptHost::new();
        let absent = [
            "collectgarbage",
            "coroutine",
            "print",
            "_G",
            "require",
            "load",
            "math.random",
            "math.randomseed",
            "string.format",
            "table.sort",
        ];
        for name in absent {
            let source = format!("if {name} ~= nil then error(\"{name} is reachable\") end");
            committed(&mut host, &unattended("on_login"), &source)
                .unwrap_or_else(|failure| panic!("{failure}"));
        }
    }

    #[test]
    fn a_library_table_written_in_one_invocation_is_clean_in_the_next() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        committed(
            &mut host,
            &event,
            "string.upper = function() return \"\" end\nmath.floor = nil\ntable.saved = 1",
        )
        .expect("rewriting its own library tables is a script's business");

        assert_eq!(
            committed(
                &mut host,
                &event,
                "if table.saved ~= nil or math.floor == nil then error(\"leaked\") end\n\
                 grant_xp(event.actor, math.floor(#string.upper(\"ab\")))",
            )
            .expect("the next Invocation receives clean library tables"),
            [xp(PLAYER_GUID, 2)]
        );
    }

    #[test]
    fn a_failed_script_cannot_change_the_next_invocations_standard_library() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        host.invoke(
            script(
                "poison",
                "math.saved = 41\nstring.saved = 42\nerror(\"discard me\")",
            ),
            &event,
        )
        .expect_err("the first Invocation fails after changing its own library tables");

        assert_eq!(
            committed(
                &mut host,
                &event,
                "if math.saved ~= nil or string.saved ~= nil then error(\"leaked\") end\n\
                 grant_xp(event.actor, 1)",
            )
            .expect("the next Invocation receives clean library tables"),
            [xp(PLAYER_GUID, 1)]
        );
    }

    // ---- the event and its Entity Handles ----

    #[test]
    fn a_script_reads_the_curated_fields_of_the_event_actor_and_target() {
        let mut host = RuntimeScriptHost::new();
        let read = committed(
            &mut host,
            &engagement(),
            "local a, t = event.actor, event.target\n\
             send_chat(a, table.concat({\n\
                 event.name, a.name, tostring(a.is_player), tostring(a.level),\n\
                 t.name, tostring(t.is_player), tostring(t.health), tostring(t.max_health),\n\
                 tostring(t.map_id), tostring(t.x), tostring(t.y), tostring(t.z)\n\
             }, \"|\"))",
        )
        .expect("reading the curated fields is not a failure");
        assert_eq!(
            read,
            [Committed::Chat {
                recipient: PLAYER_GUID,
                message: "on_login|Thrall|true|12|Kobold Miner|false|30|60|1|10.5|-20.25|30.75"
                    .to_string(),
            }]
        );
    }

    /// The point of an Entity Handle: a script can act on the entity without ever learning which
    /// row it is.
    #[test]
    fn an_entity_handle_exposes_no_identifier_and_no_row() {
        let mut host = RuntimeScriptHost::new();
        assert_eq!(
            committed(
                &mut host,
                &engagement(),
                "local a = event.actor\n\
                 if a.guid ~= nil or a.id ~= nil or a.entry ~= nil or a.owner_identity ~= nil then\n\
                     error(\"an identifier is reachable\")\n\
                 end\n\
                 if type(a) ~= \"userdata\" then error(\"a handle must not be a table\") end\n\
                 grant_xp(a, 1)",
            )
            .expect("a handle carries no identifier"),
            [xp(PLAYER_GUID, 1)]
        );
    }

    #[test]
    fn an_absent_actor_or_target_reads_as_nil_rather_than_failing_the_invocation() {
        let mut host = RuntimeScriptHost::new();
        let lonely = ScriptEvent {
            name: "on_tick".to_string(),
            actor: Some(player()),
            target: None,
        };
        assert_eq!(
            committed(
                &mut host,
                &lonely,
                "if event.target ~= nil then error(\"there is no target\") end\n\
                 grant_xp(event.actor, 5)",
            )
            .expect("an absent target is a value a script tests, not a failure"),
            [xp(PLAYER_GUID, 5)]
        );
    }

    // ---- host operations ----

    #[test]
    fn a_staged_heal_commits_against_the_target_and_credits_the_actor() {
        let mut host = RuntimeScriptHost::new();
        assert_eq!(
            committed(
                &mut host,
                &engagement(),
                "heal(event.target, event.target.max_health - event.target.health)",
            )
            .expect("healing the event target is the operation this host exists for"),
            [Committed::Heal {
                healer: PLAYER_GUID,
                target: CREATURE_GUID,
                amount: 30,
            }]
        );
    }

    #[test]
    fn a_heal_with_no_actor_credits_no_healer() {
        let mut host = RuntimeScriptHost::new();
        let no_actor = ScriptEvent {
            name: "on_tick".to_string(),
            actor: None,
            target: Some(creature()),
        };
        assert_eq!(
            committed(&mut host, &no_actor, "heal(event.target, 5)").unwrap(),
            [Committed::Heal {
                healer: 0,
                target: CREATURE_GUID,
                amount: 5,
            }]
        );
    }

    #[test]
    fn a_staged_system_message_is_trimmed_and_bounded_before_it_is_staged() {
        let mut host = RuntimeScriptHost::new();
        let shouted = "x".repeat(400);
        assert_eq!(
            committed(
                &mut host,
                &engagement(),
                &format!("send_chat(event.actor, \"   {shouted}   \")"),
            )
            .unwrap(),
            [Committed::Chat {
                recipient: PLAYER_GUID,
                message: "x".repeat(255),
            }],
            "a Runtime Script cannot stage a message the chat core would refuse"
        );
    }

    #[test]
    fn the_effects_of_one_invocation_commit_in_staging_order() {
        let mut host = RuntimeScriptHost::new();
        assert_eq!(
            committed(
                &mut host,
                &engagement(),
                "send_chat(event.actor, \"first\")\n\
                 heal(event.target, 3)\n\
                 grant_xp(event.actor, 2)",
            )
            .unwrap(),
            [
                Committed::Chat {
                    recipient: PLAYER_GUID,
                    message: "first".to_string(),
                },
                Committed::Heal {
                    healer: PLAYER_GUID,
                    target: CREATURE_GUID,
                    amount: 3,
                },
                xp(PLAYER_GUID, 2),
            ]
        );
    }

    // ---- misuse is a Script Diagnostic, never a panic ----

    /// Every one of these is a script defect the Host has to name rather than absorb.
    #[test]
    fn a_misused_host_operation_names_the_call_and_the_fault() {
        let mut host = RuntimeScriptHost::new();
        let lonely = ScriptEvent {
            name: "on_tick".to_string(),
            actor: Some(player()),
            target: None,
        };
        for (event, source, fault) in [
            (
                engagement(),
                "heal(event.target, 0)",
                "heal: the amount 0 is outside 1..=1000000",
            ),
            (
                engagement(),
                "heal(event.target, 1000001)",
                "heal: the amount 1000001 is outside 1..=1000000",
            ),
            (
                engagement(),
                "heal(event.target, \"lots\")",
                "heal: the amount is a string, not a whole number",
            ),
            (
                lonely.clone(),
                "heal(event.target, 5)",
                "heal: there is no target",
            ),
            (
                engagement(),
                "local forged = {} heal(forged, 5)",
                "heal: the target is a table, not an entity",
            ),
            (
                engagement(),
                "send_chat(event.target, \"hello\")",
                "send_chat: the recipient is a creature, not a player",
            ),
            (
                engagement(),
                "send_chat(event.actor, \"   \")",
                "send_chat: the message is empty",
            ),
            (
                engagement(),
                "send_chat(event.actor, 12)",
                "send_chat: the message is a number, not a string",
            ),
            (
                engagement(),
                "grant_xp(event.target, 10)",
                "grant_xp: the recipient is a creature, not a player",
            ),
            (
                lonely,
                "grant_xp(nil, 10)",
                "grant_xp: there is no recipient",
            ),
        ] {
            let failure = host
                .invoke(script("misuse", source), &event)
                .expect_err("a misused host operation must fail its invocation");
            assert_eq!(failure.kind, FailureKind::Runtime);
            assert!(
                failure.message.contains(fault),
                "`{source}` must report `{fault}`, got `{}`",
                failure.message
            );
        }
    }

    #[test]
    fn a_misused_host_operation_discards_what_the_invocation_already_staged() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();
        let diagnostics = run_event(
            &mut host,
            &mut sink,
            &engagement(),
            &[script(
                "half_done",
                "heal(event.target, 5)\nsend_chat(event.target, \"you are not a player\")",
            )],
        );
        assert!(sink.committed.is_empty());
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn the_staging_cap_fails_the_invocation_instead_of_committing_a_flood() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();
        let flood = format!(
            "for i = 1, {} do heal(event.target, 1) end",
            MAX_STAGED_EFFECTS_PER_INVOCATION + 1
        );
        let diagnostics = run_event(
            &mut host,
            &mut sink,
            &engagement(),
            &[script("flood", &flood)],
        );
        assert!(
            sink.committed.is_empty(),
            "an invocation that overran the staging cap must commit nothing at all"
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, FailureKind::Runtime);
        assert!(diagnostics[0]
            .message
            .contains("heal: one invocation may stage at most 256 effects"));
    }

    #[test]
    fn a_fuel_failure_after_staging_commits_nothing() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();
        let diagnostics = run_event(
            &mut host,
            &mut sink,
            &engagement(),
            &[script(
                "spendthrift",
                "heal(event.target, 10)\ngrant_xp(event.actor, 10)\nwhile true do end",
            )],
        );
        assert!(sink.committed.is_empty());
        assert_eq!(diagnostics[0].kind, FailureKind::Fuel);
    }

    #[test]
    fn a_failed_runtime_script_stops_neither_the_next_script_nor_the_core() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();
        let diagnostics = run_event(
            &mut host,
            &mut sink,
            &engagement(),
            &[
                script("first", "grant_xp(event.actor, 10)"),
                script("malformed", "="),
                script("raiser", "grant_xp(event.actor, 20)\nerror(\"nope\")"),
                script("spin", "while true do end"),
                script("last", "heal(event.target, 30)"),
            ],
        );
        assert_eq!(
            sink.committed,
            [
                xp(PLAYER_GUID, 10),
                Committed::Heal {
                    healer: PLAYER_GUID,
                    target: CREATURE_GUID,
                    amount: 30,
                },
            ],
            "only the scripts that finished may reach the world, and the one after the failures \
             still runs"
        );
        assert_eq!(
            diagnostics
                .iter()
                .map(|d| (d.script.as_str(), d.kind))
                .collect::<Vec<_>>(),
            [
                ("malformed", FailureKind::Syntax),
                ("raiser", FailureKind::Runtime),
                ("spin", FailureKind::Fuel),
            ]
        );
    }

    // ---- the Script Answer ----

    /// The rule a Package Event rests on: the FIRST number wins, later scripts still run, and a
    /// failure contributes no answer and stops nothing.
    #[test]
    fn the_first_script_to_return_a_number_answers_and_the_rest_still_run() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();

        let (diagnostics, answer) = ask_event(
            &mut host,
            &mut sink,
            &engagement(),
            &[
                script("silent", "grant_xp(event.actor, 1)"),
                script("answering", "grant_xp(event.actor, 2)\nreturn 42"),
                script("later", "grant_xp(event.actor, 3)\nreturn 7"),
                script("broken", "this is not lua ==="),
            ],
        );

        assert_eq!(answer, Some(42.0));
        assert_eq!(
            granted_amounts(&sink.committed),
            [1, 2, 3],
            "a script after the answering one still runs and still stages what it staged"
        );
        assert_eq!(
            diagnostics
                .iter()
                .map(|d| (d.script.as_str(), d.kind))
                .collect::<Vec<_>>(),
            [("broken", FailureKind::Syntax)]
        );
    }

    /// A script that fails contributes no answer, whatever it returned before it failed, and the
    /// next script may still answer.
    #[test]
    fn a_failing_script_answers_nothing_and_the_next_one_still_can() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();

        let (diagnostics, answer) = ask_event(
            &mut host,
            &mut sink,
            &engagement(),
            &[
                script("raiser", "error(\"nope\")\nreturn 1"),
                script("answering", "return 42"),
            ],
        );

        assert_eq!(answer, Some(42.0));
        assert_eq!(diagnostics.len(), 1);
    }

    /// Nothing but a number is an answer. A string, a boolean, a table and a bare `return` all mean
    /// the same thing to a caller: this script did not decide, so keep the fallback.
    #[test]
    fn a_return_that_is_not_a_number_answers_nothing() {
        let mut host = RuntimeScriptHost::new();
        for source in [
            "return \"42\"",
            "return true",
            "return {}",
            "return nil",
            "return",
            "local unused = 1",
        ] {
            let mut sink = FakeEffects::default();

            let (diagnostics, answer) = ask_event(
                &mut host,
                &mut sink,
                &engagement(),
                &[script("probe", source)],
            );

            assert!(diagnostics.is_empty(), "`{source}` must run cleanly");
            assert_eq!(answer, None, "`{source}` must answer nothing");
        }
    }

    /// A float answers as itself, and a Lua integer answers as the same number — a caller reads one
    /// kind of answer, never two.
    #[test]
    fn an_integer_and_a_float_both_answer_as_one_number() {
        let mut host = RuntimeScriptHost::new();
        for (source, expected) in [
            ("return 3", 3.0),
            ("return 0.25", 0.25),
            ("return -7", -7.0),
        ] {
            let mut sink = FakeEffects::default();

            let (_, answer) = ask_event(
                &mut host,
                &mut sink,
                &engagement(),
                &[script("probe", source)],
            );

            assert_eq!(answer, Some(expected), "`{source}`");
        }
    }

    /// Nothing bound is the common case for a Package Event nobody scripted: no answer, and the
    /// caller keeps its fallback.
    #[test]
    fn an_event_with_no_scripts_answers_nothing() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();

        let (diagnostics, answer) = ask_event(&mut host, &mut sink, &engagement(), &[]);

        assert!(diagnostics.is_empty());
        assert_eq!(answer, None);
    }

    /// A defect in the pinned interpreter, recorded so it cannot change unnoticed: piccolo 0.3.3 passes
    /// an inline table constructor's element count as an extra argument, so `f({7, 8, 9})` reaches
    /// a three-parameter `f` as `f(table, 3, nil)` instead of `f(table, nil, nil)`. Passing the
    /// same table through a local is correct, so it is the call site, not the callee.
    ///
    /// It reproduces in the Runtime Script Prototype, untouched by this Host. `x = x or default`
    /// is the commonest idiom in Lua and in transpiler output, so anything generating Lua for this
    /// Host has to know. When this test starts failing, the interpreter has fixed it. Drop the type
    /// guard in `PICCOLO_SHIM` at the same time.
    #[test]
    fn piccolo_leaks_a_table_constructors_element_count_as_an_extra_argument() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        // `b` should be nil in every one of these. It is the element count instead.
        for (source, leaked_count) in [
            ("grant_xp(event.actor, seen({}) + 1)", 1),
            ("grant_xp(event.actor, seen({7}) + 1)", 2),
            ("grant_xp(event.actor, seen({7, 8, 9}) + 1)", 4),
        ] {
            let preamble = "local function seen(a, b) return b end\n";
            assert_eq!(
                granted_amounts(
                    &committed(&mut host, &event, &format!("{preamble}{source}"))
                        .expect("the leaked value is a number, so the arithmetic succeeds")
                ),
                [leaked_count]
            );
        }
        // The same table through a local is passed correctly, which is what makes it a call-site
        // defect rather than a calling-convention one.
        assert_eq!(
            granted_amounts(
                &committed(
                    &mut host,
                    &event,
                    "local function seen(a, b) return tostring(b) end\n\
                     local t = {7, 8, 9}\n\
                     grant_xp(event.actor, #seen(t))",
                )
                .unwrap()
            ),
            [3],
            "tostring(nil) is the three characters `nil`"
        );
    }

    #[test]
    fn the_shim_supplies_the_table_concat_piccolo_lacks() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        for (source, width, joined) in [
            (
                "grant_xp(event.actor, #table.concat({\"ab\", \"cd\", \"ef\"}, \"-\"))",
                8,
                "ab-cd-ef",
            ),
            // No separator: the leaked element count must not be joined in as one.
            (
                "grant_xp(event.actor, #table.concat({\"ab\", \"cd\"}))",
                4,
                "abcd",
            ),
            (
                "local t = {\"ab\", \"cd\"} grant_xp(event.actor, #table.concat(t, \"--\"))",
                6,
                "ab--cd",
            ),
        ] {
            assert_eq!(
                granted_amounts(
                    &committed(&mut host, &event, source).expect("table.concat must exist")
                ),
                [width],
                "`{joined}` is {width} characters"
            );
        }
    }

    #[test]
    fn the_representative_script_fits_the_fuel_budget_with_room_to_spare() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        host.invoke(script("representative", REPRESENTATIVE_SCRIPT), &event)
            .expect("the workload the budget is sized against must fit");
        // Fifty times the list, still inside the budget: the headroom is real, not marginal.
        let heavier = REPRESENTATIVE_SCRIPT.replace("1, 20 do", "1, 1000 do");
        host.invoke(script("heavier", &heavier), &event)
            .expect("fifty times the representative workload must still fit");
    }

    /// A Runtime Script that reaches the host again through an effect it staged must be refused,
    /// not panic the reducer it is running inside. Event bindings make that reachable; the guard
    /// is here so it is a refusal from the first day it can happen.
    #[test]
    fn re_entering_the_module_host_is_refused_rather_than_a_panic() {
        let outer = with_host(|_| with_host(|host| host.compilations()));
        assert_eq!(
            outer,
            Some(None),
            "the outer borrow must succeed and the inner one must be refused"
        );
        assert!(
            with_host(|host| host.compilations()).is_some(),
            "the refusal must not poison the host for the next caller"
        );
    }
}

/// piccolo reaches `getrandom` twice on `wasm32-unknown-unknown` — 0.2 through `rand`, when
/// `Lua::core()` seeds `math.random`, and 0.3 through `ahash`, when the first Lua table is built.
/// Neither generation has a backend on that target, and the JS backend the Runtime Script
/// Prototype used to
/// get a compile emits `wasm-bindgen` imports a SpacetimeDB host cannot resolve. So the Module
/// supplies its own, selected by `getrandom_backend="custom"` in `.cargo/config.toml`.
///
/// It is a FIXED stream on purpose. The Module is replicated and replayed; a random hash seed
/// would let table iteration order differ between two runs of the same reducer sequence. Nothing
/// in the Module uses `getrandom` for security — gameplay randomness comes from `ctx.rng()`, which
/// SpacetimeDB seeds from the reducer timestamp.
#[cfg(target_arch = "wasm32")]
mod fixed_entropy {
    const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

    fn fill(dest: *mut u8, len: usize) {
        let mut state = SEED;
        let mut written = 0usize;
        while written < len {
            state = state.wrapping_add(SEED);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            for byte in z.to_le_bytes() {
                if written >= len {
                    break;
                }
                // SAFETY: the caller guarantees `dest` is writable for `len` bytes.
                unsafe { dest.add(written).write(byte) };
                written += 1;
            }
        }
    }

    /// getrandom 0.2's custom backend, enabled by the `spacetimedb` crate's `custom` feature.
    #[unsafe(no_mangle)]
    unsafe fn __getrandom_custom(dest: *mut u8, len: usize) -> u32 {
        fill(dest, len);
        0
    }

    /// getrandom 0.3's custom backend.
    #[unsafe(no_mangle)]
    unsafe extern "Rust" fn __getrandom_v03_custom(
        dest: *mut u8,
        len: usize,
    ) -> Result<(), getrandom::Error> {
        fill(dest, len);
        Ok(())
    }
}
