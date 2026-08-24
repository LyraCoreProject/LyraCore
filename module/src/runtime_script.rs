//! The Runtime Script Host: the contained Lua interpreter a Runtime Script runs inside.
//!
//! One [`RuntimeScriptHost`] owns one embedded piccolo VM and a compiler cache. The cache holds
//! compiled chunks only, keyed by a hash of the source, so it can never hand back stale code for
//! an edited script and it holds no Lua state between invocations.
//!
//! Every invocation gets:
//!
//! * a FRESH environment table — writes land there and die with the invocation, reads fall through
//!   a `__index` metatable to the shared stdlib, so nothing a script assigns survives it;
//! * a [`FUEL_BUDGET_PER_INVOCATION`] of metered VM work, plus [`MAX_STEPS_PER_INVOCATION`] as the
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
//! What this host deliberately does NOT do: durable script storage or event bindings, a broad
//! gameplay host API, or any Lua state that outlives an invocation.
//!
//! CAVEAT for the host API to come: the fresh environment chains to piccolo's `Lua::core()`
//! globals, so a script can still reach `math`, `string` and friends, and can mutate those SHARED
//! stdlib tables. Replacing the fallthrough with an explicit allowlist is the curated-host-API
//! work, not this layer. For the same reason `math.random`'s stream is shared across invocations
//! and reproducible only for a shard replaying the same sequence of calls — a Runtime Script must
//! not decide a durable outcome with it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use piccolo::closure::UpValueState;
use piccolo::{
    Callback, CallbackReturn, Closure, Executor, Fuel, Lua, StashedClosure, Table, Value,
};

/// Fuel handed to one `Executor::step`. The VM only checks its budget between operations, so a
/// step can overshoot slightly; the slice is small enough that the overshoot stays negligible
/// against the budget below.
const FUEL_PER_STEP: i32 = 64;

/// Metered VM work one invocation may perform before it is cut off as a fuel failure.
///
/// Sized by measurement, not by guess. `REPRESENTATIVE_SCRIPT` — the transpiler-shaped workload of
/// list building, a higher-order function, string work and a host call that this host exists to
/// run — costs 2,054 fuel over 30 steps, so the budget is roughly a hundred of those. Fifty times
/// its list still costs only 96,442. At the other end, a bare `while true do end` reaches this
/// number in under a millisecond of VM time, which is what keeps a runaway script off the 0.5s
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

const TRUNCATION_MARK: &str = "…[truncated]";

/// piccolo's stdlib subset omits `table.concat`, which transpiler output uses constantly. Loaded
/// once into the shared globals at host construction, in the form the runtime spike proved.
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
/// and what kind of failure it was, so an operator can act on it without a stack trace.
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
            script: script.to_string(),
            event: event.to_string(),
            kind,
            message: bounded(message),
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

/// Cuts `message` down to [`DIAGNOSTIC_MESSAGE_CAP`] bytes INCLUDING the truncation mark, on a
/// char boundary so the result is still valid UTF-8 to log.
fn bounded(message: String) -> String {
    if message.len() <= DIAGNOSTIC_MESSAGE_CAP {
        return message;
    }
    let mut keep = DIAGNOSTIC_MESSAGE_CAP - TRUNCATION_MARK.len();
    while keep > 0 && !message.is_char_boundary(keep) {
        keep -= 1;
    }
    let mut out = message[..keep].to_string();
    out.push_str(TRUNCATION_MARK);
    out
}

/// A gameplay operation a Runtime Script asked for. Held, not performed, until the invocation
/// that staged it succeeds.
///
/// Deliberately one variant: this proves commit-on-success and discard-on-failure against a real
/// core operation. Growing this into a gameplay host API is separate work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum StagedEffect {
    GrantXp { character_guid: u64, amount: u32 },
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
            }
        }
    }
}

/// The seam staged effects commit through: the real database in the Module, a Fake in tests.
pub(crate) trait EffectSink {
    fn grant_xp(&mut self, character_guid: u64, amount: u32);
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
    pub(crate) fn compilations(&self) -> usize {
        self.compilations
    }

    /// Run `script` for `event` in a fresh environment under a fuel budget.
    ///
    /// On success, returns everything the script staged — nothing has touched the world yet. On
    /// any failure, returns a bounded diagnostic and the staged effects are dropped unread.
    pub(crate) fn invoke(
        &mut self,
        script: RuntimeScript<'_>,
        event: &str,
    ) -> Result<StagedEffects, ScriptDiagnostic> {
        let chunk = self.compiled(script, event)?;

        let staged: Rc<RefCell<Vec<StagedEffect>>> = Rc::new(RefCell::new(Vec::new()));
        let staging_handle = Rc::clone(&staged);
        let executor = self.lua.enter(|ctx| {
            let env = fresh_environment(ctx, staging_handle);
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
            if finished {
                break;
            }
            if fuel_spent >= FUEL_BUDGET_PER_INVOCATION {
                return Err(ScriptDiagnostic::new(
                    script.name,
                    event,
                    FailureKind::Fuel,
                    format!("spent the {FUEL_BUDGET_PER_INVOCATION} fuel budget without finishing"),
                ));
            }
            if steps >= MAX_STEPS_PER_INVOCATION {
                return Err(ScriptDiagnostic::new(
                    script.name,
                    event,
                    FailureKind::Fuel,
                    format!(
                        "stalled: {MAX_STEPS_PER_INVOCATION} steps burned only {fuel_spent} fuel"
                    ),
                ));
            }
        }

        let outcome = self.lua.enter(|ctx| {
            match ctx.fetch(&executor).take_result::<()>(ctx) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(error.to_string()),
                // Unreachable: the loop only leaves through `finished`, which means Result mode.
                Err(mode) => Err(mode.to_string()),
            }
        });
        if let Err(message) = outcome {
            return Err(ScriptDiagnostic::new(
                script.name,
                event,
                FailureKind::Runtime,
                message,
            ));
        }

        // The script is gone; nothing else holds the staging buffer.
        let effects = staged.borrow().clone();
        Ok(StagedEffects(effects))
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

/// Build the environment ONE invocation sees: an empty table that reads through to the shared
/// stdlib and carries this invocation's host operations.
fn fresh_environment<'gc>(
    ctx: piccolo::Context<'gc>,
    staged: Rc<RefCell<Vec<StagedEffect>>>,
) -> Table<'gc> {
    let env = Table::new(&ctx);
    let fallthrough = Table::new(&ctx);
    let _ = fallthrough.set(ctx, "__index", ctx.globals());
    env.set_metatable(&ctx, Some(fallthrough));

    let grant_xp = Callback::from_fn(&ctx, move |ctx, _execution, mut stack| {
        let (character_guid, amount): (u64, u32) = stack.consume(ctx)?;
        staged.borrow_mut().push(StagedEffect::GrantXp {
            character_guid,
            amount,
        });
        Ok(CallbackReturn::Return)
    });
    let _ = env.set(ctx, "grant_xp", grant_xp);
    env
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
pub(crate) fn run_event<S: EffectSink>(
    host: &mut RuntimeScriptHost,
    sink: &mut S,
    event: &str,
    scripts: &[RuntimeScript<'_>],
) -> Vec<ScriptDiagnostic> {
    let mut diagnostics = Vec::new();
    for script in scripts {
        match host.invoke(*script, event) {
            Ok(effects) => effects.commit(sink),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    diagnostics
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
if #roster > 0 then grant_xp(4242, 25) end
"#;

    #[derive(Default)]
    struct FakeEffects {
        granted: Vec<(u64, u32)>,
    }

    impl EffectSink for FakeEffects {
        fn grant_xp(&mut self, character_guid: u64, amount: u32) {
            self.granted.push((character_guid, amount));
        }
    }

    fn script<'a>(name: &'a str, source: &'a str) -> RuntimeScript<'a> {
        RuntimeScript { name, source }
    }

    /// What one invocation actually puts through the sink: run it, then commit whatever it staged.
    fn committed(
        host: &mut RuntimeScriptHost,
        source: &str,
    ) -> Result<Vec<(u64, u32)>, ScriptDiagnostic> {
        let staged = host.invoke(script("probe", source), "on_login")?;
        let mut sink = FakeEffects::default();
        staged.commit(&mut sink);
        Ok(sink.granted)
    }

    #[test]
    fn valid_lua_compiles_once_and_runs_in_every_fresh_invocation() {
        let mut host = RuntimeScriptHost::new();
        let award = script("award", "grant_xp(7, 40)");
        for _ in 0..3 {
            let staged = host.invoke(award, "on_login").expect("valid Lua runs");
            let mut sink = FakeEffects::default();
            staged.commit(&mut sink);
            assert_eq!(sink.granted, [(7, 40)]);
        }
        assert_eq!(
            host.compilations(),
            1,
            "three invocations of one source must reuse one compiled chunk"
        );
        host.invoke(script("other", "grant_xp(8, 1)"), "on_login")
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
        for _ in 0..3 {
            assert_eq!(
                committed(
                    &mut host,
                    "visits = (visits or 0) + 1\ngrant_xp(77, visits)"
                )
                .unwrap(),
                [(77, 1)],
                "each invocation must start from an empty environment, so `visits` is always nil"
            );
        }
    }

    #[test]
    fn an_endless_loop_spends_the_fuel_budget_instead_of_stalling_the_tick() {
        let mut host = RuntimeScriptHost::new();
        let failure = host
            .invoke(script("spin", "while true do end"), "on_damage_taken")
            .expect_err("an endless loop cannot succeed");
        assert_eq!(failure.kind, FailureKind::Fuel);
    }

    #[test]
    fn a_diagnostic_names_the_script_the_event_and_the_failure_kind() {
        let mut host = RuntimeScriptHost::new();
        let failure = host
            .invoke(script("broken", "this is not lua ==="), "on_levelup")
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
            .invoke(shouty, "on_login")
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
        let cut = bounded(long);
        assert!(cut.len() <= DIAGNOSTIC_MESSAGE_CAP);
        assert!(cut.ends_with(TRUNCATION_MARK));
    }

    #[test]
    fn a_failed_runtime_script_stops_neither_the_next_script_nor_the_core() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();
        let diagnostics = run_event(
            &mut host,
            &mut sink,
            "on_kill",
            &[
                script("first", "grant_xp(1, 10)"),
                script("malformed", "="),
                script("raiser", "grant_xp(2, 20)\nerror(\"nope\")"),
                script("spin", "while true do end"),
                script("last", "grant_xp(3, 30)"),
            ],
        );
        assert_eq!(
            sink.granted,
            [(1, 10), (3, 30)],
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

    #[test]
    fn a_failure_discards_every_effect_that_invocation_staged() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();
        let diagnostics = run_event(
            &mut host,
            &mut sink,
            "on_loot",
            &[script(
                "half_done",
                "grant_xp(1, 10)\ngrant_xp(2, 20)\nerror(\"too late\")",
            )],
        );
        assert!(
            sink.granted.is_empty(),
            "the two effects staged before the error must never land"
        );
        assert_eq!(diagnostics.len(), 1);
    }

    /// A DEFECT in the pinned engine, recorded so it cannot change unnoticed: piccolo 0.3.3 passes
    /// an inline table constructor's element count as an extra argument, so `f({7, 8, 9})` reaches
    /// a three-parameter `f` as `f(table, 3, nil)` instead of `f(table, nil, nil)`. Passing the
    /// same table through a local is correct, so it is the call site, not the callee.
    ///
    /// It reproduces in the runtime spike's own harness, untouched by this host. `x = x or default`
    /// is the commonest idiom in Lua and in transpiler output, so anything generating Lua for this
    /// host has to know. When this test starts failing, the engine has fixed it — drop the type
    /// guard in `PICCOLO_SHIM` at the same time.
    #[test]
    fn piccolo_leaks_a_table_constructors_element_count_as_an_extra_argument() {
        let mut host = RuntimeScriptHost::new();
        // `b` should be nil in every one of these. It is the element count instead.
        for (source, leaked_count) in [
            ("grant_xp(1, seen({}) + 1)", 1),
            ("grant_xp(1, seen({7}) + 1)", 2),
            ("grant_xp(1, seen({7, 8, 9}) + 1)", 4),
        ] {
            let preamble = "local function seen(a, b) return b end\n";
            assert_eq!(
                committed(&mut host, &format!("{preamble}{source}"))
                    .expect("the leaked value is a number, so the arithmetic succeeds"),
                [(1, leaked_count)]
            );
        }
        // The same table through a local is passed correctly, which is what makes it a call-site
        // defect rather than a calling-convention one.
        assert_eq!(
            committed(
                &mut host,
                "local function seen(a, b) return tostring(b) end\n\
                 local t = {7, 8, 9}\n\
                 grant_xp(1, #seen(t))",
            )
            .unwrap(),
            [(1, 3)],
            "tostring(nil) is the three characters `nil`"
        );
    }

    #[test]
    fn the_shim_supplies_the_table_concat_piccolo_lacks() {
        let mut host = RuntimeScriptHost::new();
        for (source, width, joined) in [
            (
                "grant_xp(1, #table.concat({\"ab\", \"cd\", \"ef\"}, \"-\"))",
                8,
                "ab-cd-ef",
            ),
            // No separator: the leaked element count must not be joined in as one.
            ("grant_xp(1, #table.concat({\"ab\", \"cd\"}))", 4, "abcd"),
            (
                "local t = {\"ab\", \"cd\"} grant_xp(1, #table.concat(t, \"--\"))",
                6,
                "ab--cd",
            ),
        ] {
            assert_eq!(
                committed(&mut host, source).expect("table.concat must exist"),
                [(1, width)],
                "`{joined}` is {width} characters"
            );
        }
    }

    #[test]
    fn the_representative_script_fits_the_fuel_budget_with_room_to_spare() {
        let mut host = RuntimeScriptHost::new();
        host.invoke(script("representative", REPRESENTATIVE_SCRIPT), "on_login")
            .expect("the workload the budget is sized against must fit");
        // Fifty times the list, still inside the budget: the headroom is real, not marginal.
        let heavier = REPRESENTATIVE_SCRIPT.replace("1, 20 do", "1, 1000 do");
        host.invoke(script("heavier", &heavier), "on_login")
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

    #[test]
    fn the_production_effect_sink_is_the_pass_through_the_fake_replaces() {
        let src = include_str!("runtime_script.rs");
        assert_eq!(
            crate::test_scan::shape_of(src, "impl EffectSink for CoreEffects<'_> {"),
            "{ fn grant_xp(&mut self, character_guid: u64, amount: u32) { use \
             crate::game_world_entity; let Some(mut entity) = \
             crate::helpers::acting_entity_by_guid(self.ctx, character_guid) else { return; }; \
             crate::xp::grant_xp(self.ctx, &mut entity, amount); \
             self.ctx.db.game_world_entity().guid().update(entity); } }"
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            "the production sink is the one line of this host the Fake replaces, so nothing else \
             covers an edit to it"
        );
    }
}

/// piccolo reaches `getrandom` twice on `wasm32-unknown-unknown` — 0.2 through `rand`, when
/// `Lua::core()` seeds `math.random`, and 0.3 through `ahash`, when the first Lua table is built.
/// Neither generation has a backend on that target, and the JS backend the runtime spike used to
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
