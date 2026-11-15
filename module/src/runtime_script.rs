//! Before changing Runtime Script execution, read docs/runtime-script-host.md.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use spacetimedb::log;

use piccolo::closure::UpValueState;
use piccolo::{
    Callback, CallbackReturn, Closure, Context, Error, Execution, Executor, Fuel, IntoValue, Lua,
    MetaMethod, StashedClosure, Table, UserData, Value,
};

// Piccolo may overshoot one step; enforce the total Fuel Budget after each step.
const FUEL_PER_STEP: i32 = 64;

const FUEL_BUDGET_PER_INVOCATION: i32 = 200_000;

// Bound zero-fuel steps too, or an Invocation could loop without spending its Fuel Budget.
const MAX_STEPS_PER_INVOCATION: usize = 20_000;

const DIAGNOSTIC_MESSAGE_CAP: usize = 512;

const DIAGNOSTIC_LABEL_CAP: usize = 128;

const TRUNCATION_MARK: &str = "…[truncated]";

const MAX_EFFECT_AMOUNT: i64 = 1_000_000;

// Fuel meters interpreter work; this separate cap bounds staged durable effects.
const MAX_STAGED_EFFECTS_PER_INVOCATION: usize = 256;

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
    ("table", &["concat", "pack", "unpack"]),
];

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FailureKind {
    Syntax,
    Runtime,
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct StagedEffects(Vec<StagedEffect>);

impl StagedEffects {
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

#[derive(Debug)]
pub(crate) struct Invocation {
    pub effects: StagedEffects,
    pub answer: Option<f64>,
}

pub(crate) trait EffectSink {
    fn grant_xp(&mut self, character_guid: u64, amount: u32);
    fn heal(&mut self, healer_guid: u64, target_guid: u64, amount: u32);
    fn send_chat(&mut self, recipient_guid: u64, message: &str);
}

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

#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct ScriptEvent {
    pub name: String,
    pub actor: Option<EntityView>,
    pub target: Option<EntityView>,
    pub payload: ScriptPayload,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct ScriptPayload {
    pub character: bool,
    pub fields: Vec<(&'static str, ScriptScalar)>,
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum ScriptScalar {
    Number(f64),
    Boolean(bool),
    Text(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct EntityHandle {
    guid: u64,
    is_player: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct RuntimeScript<'a> {
    pub name: &'a str,
    pub source: &'a str,
}

pub(crate) struct RuntimeScriptHost {
    lua: Lua,
    chunks: HashMap<[u8; 32], StashedClosure>,
    compilations: usize,
}

impl RuntimeScriptHost {
    pub(crate) fn new() -> Self {
        let mut lua = Lua::core();
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

    #[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
    pub(crate) fn compilations(&self) -> usize {
        self.compilations
    }

    pub(crate) fn clear_chunks(&mut self) {
        self.chunks.clear();
    }

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
            // Rebind _ENV for every Invocation so cached code cannot retain script-authored state.
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

        let outcome = self
            .lua
            .enter(|ctx| match ctx.fetch(&executor).take_result::<Value>(ctx) {
                Ok(Ok(returned)) => Ok(script_answer(returned)),
                Ok(Err(error)) => Err(error.to_string()),
                Err(mode) => Err(mode.to_string()),
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

fn script_answer(returned: Value<'_>) -> Option<f64> {
    match returned {
        Value::Integer(number) => Some(number as f64),
        Value::Number(number) => Some(number),
        _ => None,
    }
}

fn fresh_environment<'gc>(
    ctx: Context<'gc>,
    event: &ScriptEvent,
    staged: Rc<RefCell<Vec<StagedEffect>>>,
) -> Table<'gc> {
    let env = allowlisted_standard_library(ctx);

    let actor_guid = event.actor.as_ref().map(|actor| actor.guid);
    set(ctx, env, "event", event_table(ctx, event));
    set(ctx, env, "assert", located_assert_operation(ctx));
    set(ctx, env, "error", located_error_operation(ctx));
    set(ctx, env, "heal", heal_operation(ctx, actor_guid, &staged));
    set(ctx, env, "send_chat", send_chat_operation(ctx, &staged));
    set(ctx, env, "grant_xp", grant_xp_operation(ctx, &staged));
    env
}

fn located_assert_operation<'gc>(ctx: Context<'gc>) -> Callback<'gc> {
    Callback::from_fn(&ctx, |ctx, execution, stack| {
        if stack.get(0).to_bool() {
            return Ok(CallbackReturn::Return);
        }
        let error = if stack.get(1).is_nil() {
            text(ctx, "assertion failed!").into_value(ctx).into()
        } else {
            Error::from(stack.get(1))
        };
        Err(error_with_location(ctx, &execution, error))
    })
}

fn located_error_operation<'gc>(ctx: Context<'gc>) -> Callback<'gc> {
    Callback::from_fn(&ctx, |ctx, execution, stack| {
        let error = Error::from(stack.get(0));
        Err(error_with_location(ctx, &execution, error))
    })
}

fn error_with_location<'gc>(
    ctx: Context<'gc>,
    execution: &Execution<'gc, '_>,
    error: Error<'gc>,
) -> Error<'gc> {
    let Some(frame) = execution.upper_lua_frame() else {
        return error;
    };
    let message = match error {
        Error::Lua(error) => error.to_string(),
        Error::Runtime(error) => error.to_string(),
    };
    text(ctx, &format!("line {}: {message}", frame.current_line))
        .into_value(ctx)
        .into()
}

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

fn event_table<'gc>(ctx: Context<'gc>, event: &ScriptEvent) -> Table<'gc> {
    let table = Table::new(&ctx);
    set(ctx, table, "name", text(ctx, &event.name));
    for (field, view) in [("actor", &event.actor), ("target", &event.target)] {
        let value = match view {
            Some(view) => Value::UserData(entity_handle(ctx, view)),
            None => Value::Nil,
        };
        set(ctx, table, field, value);
        if field == "actor"
            && event.payload.character
            && view.as_ref().is_some_and(|view| view.is_player)
        {
            set(ctx, table, "player", value);
        }
    }
    for (field, scalar) in &event.payload.fields {
        let value = match scalar {
            ScriptScalar::Number(number) => Value::Number(*number),
            ScriptScalar::Boolean(boolean) => Value::Boolean(*boolean),
            ScriptScalar::Text(value) => Value::String(text(ctx, value)),
        };
        set(ctx, table, field, value);
    }
    table
}

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

fn heal_operation<'gc>(
    ctx: Context<'gc>,
    actor_guid: Option<u64>,
    staged: &Rc<RefCell<Vec<StagedEffect>>>,
) -> Callback<'gc> {
    let staged = Rc::clone(staged);
    Callback::from_fn(&ctx, move |ctx, execution, mut stack| {
        (|| {
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
        })()
        .map_err(|error| error_with_location(ctx, &execution, error))
    })
}

fn send_chat_operation<'gc>(
    ctx: Context<'gc>,
    staged: &Rc<RefCell<Vec<StagedEffect>>>,
) -> Callback<'gc> {
    let staged = Rc::clone(staged);
    Callback::from_fn(&ctx, move |ctx, execution, mut stack| {
        (|| {
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
        })()
        .map_err(|error| error_with_location(ctx, &execution, error))
    })
}

fn grant_xp_operation<'gc>(
    ctx: Context<'gc>,
    staged: &Rc<RefCell<Vec<StagedEffect>>>,
) -> Callback<'gc> {
    let staged = Rc::clone(staged);
    Callback::from_fn(&ctx, move |ctx, execution, mut stack| {
        (|| {
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
        })()
        .map_err(|error| error_with_location(ctx, &execution, error))
    })
}

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

fn host_error<'gc>(ctx: Context<'gc>, call: &str, fault: &str) -> Error<'gc> {
    text(ctx, &format!("{call}: {fault}"))
        .into_value(ctx)
        .into()
}

fn text<'gc>(ctx: Context<'gc>, source: &str) -> piccolo::String<'gc> {
    piccolo::String::from_slice(&ctx, source)
}

fn set<'gc>(ctx: Context<'gc>, table: Table<'gc>, key: &'static str, value: impl IntoValue<'gc>) {
    table
        .set(ctx, key, value)
        .expect("a string is a valid Lua table key");
}

thread_local! {
    static HOST: RefCell<RuntimeScriptHost> = RefCell::new(RuntimeScriptHost::new());
}

// A borrowed Host means staged effects re-entered it; skip recursion rather than panic.
pub(crate) fn with_host<R>(f: impl FnOnce(&mut RuntimeScriptHost) -> R) -> Option<R> {
    HOST.with(|host| host.try_borrow_mut().ok().map(|mut host| f(&mut host)))
}

pub(crate) fn run_event<S: EffectSink>(
    host: &mut RuntimeScriptHost,
    sink: &mut S,
    event: &ScriptEvent,
    scripts: &[RuntimeScript<'_>],
) -> Vec<ScriptDiagnostic> {
    ask_event(host, sink, event, scripts).0
}

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

pub(crate) struct CoreEffects<'a> {
    pub ctx: &'a spacetimedb::ReducerContext,
}

impl EffectSink for CoreEffects<'_> {
    fn grant_xp(&mut self, character_guid: u64, amount: u32) {
        let Some(mut entity) = crate::helpers::acting_entity_by_guid(self.ctx, character_guid)
        else {
            return;
        };
        crate::xp::grant_xp(self.ctx, &mut entity, amount);
        entity.update(self.ctx);
    }

    fn heal(&mut self, healer_guid: u64, target_guid: u64, amount: u32) {
        crate::spell::apply_direct_heal(self.ctx, healer_guid, target_guid, amount as i32);
    }

    fn send_chat(&mut self, recipient_guid: u64, message: &str) {
        if let Err(reason) =
            crate::actor::system_message(self.ctx, recipient_guid, message.to_string())
        {
            log::info!("runtime script system message to {recipient_guid} did not land: {reason}");
        }
    }
}

impl EntityView {
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

    const GENERATED_SCRIPT: &str =
        include_str!("../../datascripts/runtime-scripts/fuel-workload.lua");

    const PLAYER_GUID: u64 = 7;
    const CREATURE_GUID: u64 = 9;

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

    fn unattended(name: &str) -> ScriptEvent {
        ScriptEvent {
            name: name.to_string(),
            ..ScriptEvent::default()
        }
    }

    fn engagement() -> ScriptEvent {
        ScriptEvent {
            name: "on_login".to_string(),
            actor: Some(player()),
            target: Some(creature()),
            ..ScriptEvent::default()
        }
    }

    fn script<'a>(name: &'a str, source: &'a str) -> RuntimeScript<'a> {
        RuntimeScript { name, source }
    }

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
    fn native_vm_and_fuel_diagnostics_keep_the_stable_invocation_context() {
        let mut host = RuntimeScriptHost::new();
        for (name, source, kind) in [
            (
                "native-fault",
                "local missing = nil\nreturn missing.value",
                FailureKind::Runtime,
            ),
            ("spin", "while true do end", FailureKind::Fuel),
        ] {
            let failure = host
                .invoke(script(name, source), &unattended("on_damage_taken"))
                .expect_err("the Runtime Script must fail");
            assert_eq!(failure.script, name);
            assert_eq!(failure.event, "on_damage_taken");
            assert_eq!(failure.kind, kind);
            assert!(!failure.message.is_empty());
            let report = failure.to_string();
            assert!(report.contains(name), "{report}");
            assert!(report.contains("on_damage_taken"), "{report}");
        }
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

    #[test]
    fn compiled_typescript_login_handler_sends_chat_through_its_event_parameter() {
        let mut host = RuntimeScriptHost::new();
        let event = ScriptEvent {
            payload: ScriptPayload {
                character: true,
                fields: vec![],
            },
            ..engagement()
        };
        let source = include_str!("../../datascripts/tests/fixtures/typed/welcome.generated.lua");
        assert_eq!(
            committed(&mut host, &event, source).expect("compiled login handler runs"),
            [Committed::Chat {
                recipient: PLAYER_GUID,
                message: "Welcome, Thrall".to_string(),
            }]
        );
    }

    #[test]
    fn compiled_typescript_login_handler_requires_a_character() {
        let mut host = RuntimeScriptHost::new();
        let source = include_str!("../../datascripts/tests/fixtures/typed/welcome.generated.lua");
        let missing = ScriptEvent {
            payload: ScriptPayload {
                character: true,
                fields: vec![],
            },
            ..unattended("on_login")
        };
        let failure = host
            .invoke(script("welcome", source), &missing)
            .expect_err("a typed handler cannot run without its required Character");
        assert_eq!(failure.kind, FailureKind::Runtime);
        assert!(failure.message.contains("requires a Character"));
    }

    #[test]
    fn compiled_lua_levelup_handler_reads_the_attained_level() {
        let mut host = RuntimeScriptHost::new();
        let event = ScriptEvent {
            name: "on_levelup".to_string(),
            actor: Some(player()),
            payload: ScriptPayload {
                character: true,
                fields: vec![("newLevel", ScriptScalar::Number(13.0))],
            },
            ..ScriptEvent::default()
        };
        assert_eq!(
            committed(
                &mut host,
                &event,
                include_str!("../../datascripts/tests/fixtures/typed/ding.generated.lua"),
            )
            .expect("compiled level-up handler runs"),
            [Committed::Chat {
                recipient: PLAYER_GUID,
                message: "Ding 13".to_string(),
            }]
        );
    }

    #[test]
    fn compiled_package_handler_keeps_zero_and_negative_answers() {
        let mut host = RuntimeScriptHost::new();
        let source = include_str!("../../datascripts/tests/fixtures/typed/answer.generated.lua");
        for (level, expected_answer) in [(12, 0.0), (11, -1.0)] {
            let mut character = player();
            character.level = level;
            let event = ScriptEvent {
                actor: Some(character),
                ..unattended("example.scripts.answer")
            };
            let mut sink = FakeEffects::default();
            let (diagnostics, answer) =
                ask_event(&mut host, &mut sink, &event, &[script("answer", source)]);
            assert!(diagnostics.is_empty());
            assert_eq!(answer, Some(expected_answer));
            assert_eq!(sink.committed, [xp(PLAYER_GUID, 3)]);
        }
    }

    #[test]
    fn compiled_package_handler_discards_staged_effects_when_it_fails() {
        let mut host = RuntimeScriptHost::new();
        let source = include_str!("../../datascripts/tests/fixtures/typed/answer.generated.lua");
        let mut character = player();
        character.level = 0;
        let event = ScriptEvent {
            actor: Some(character),
            ..unattended("example.scripts.answer")
        };
        let mut sink = FakeEffects::default();
        let (diagnostics, answer) =
            ask_event(&mut host, &mut sink, &event, &[script("answer", source)]);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(answer, None);
        assert!(sink.committed.is_empty());
    }

    #[test]
    fn a_levelup_handler_reads_the_hook_level_while_the_character_snapshot_is_stale() {
        let mut host = RuntimeScriptHost::new();
        let event = ScriptEvent {
            name: "on_levelup".to_string(),
            actor: Some(player()),
            payload: ScriptPayload {
                character: true,
                fields: vec![("newLevel", ScriptScalar::Number(13.0))],
            },
            ..ScriptEvent::default()
        };
        assert_eq!(
            committed(
                &mut host,
                &event,
                r#"
local function celebrate(event)
    if event.player ~= event.actor then error("the Character must keep one handle") end
    if event.player.level ~= 12 then error("the entity snapshot must keep its old level") end
    grant_xp(event.player, event.newLevel)
end
celebrate(event)
"#,
            )
            .expect("the hook's new level reaches the typed handler"),
            [xp(PLAYER_GUID, 13)]
        );
    }

    #[test]
    fn a_login_handler_receives_an_opaque_character_handle() {
        let mut host = RuntimeScriptHost::new();
        let event = ScriptEvent {
            payload: ScriptPayload {
                character: true,
                fields: vec![],
            },
            ..engagement()
        };
        assert_eq!(
            committed(
                &mut host,
                &event,
                r#"
local function welcome(event)
    if not event.player.is_player then error("login requires a Character") end
    if event.player.guid ~= nil or type(event.player) ~= "userdata" then error("the handle must stay opaque") end
    send_chat(event.player, "Welcome, " .. event.player.name)
end
welcome(event)
"#,
            )
            .expect("login carries a Character handle"),
            [Committed::Chat { recipient: PLAYER_GUID, message: "Welcome, Thrall".to_string() }]
        );
    }

    #[test]
    fn a_required_character_field_never_aliases_a_creature_or_an_absent_actor() {
        let mut host = RuntimeScriptHost::new();
        for actor in [None, Some(creature())] {
            let event = ScriptEvent {
                actor,
                payload: ScriptPayload {
                    character: true,
                    fields: vec![],
                },
                ..unattended("on_login")
            };
            committed(
                &mut host,
                &event,
                "if event.player ~= nil then error('no Character is present') end",
            )
            .expect("an absent Character field reads as nil");
        }
    }

    #[test]
    fn a_hook_payload_preserves_boolean_and_decimal_identifier_fields() {
        let mut host = RuntimeScriptHost::new();
        let event = ScriptEvent {
            payload: ScriptPayload {
                character: false,
                fields: vec![
                    ("assist", ScriptScalar::Boolean(true)),
                    (
                        "instanceId",
                        ScriptScalar::Text("18446744073709551615".to_string()),
                    ),
                ],
            },
            ..engagement()
        };
        assert_eq!(
            committed(
                &mut host,
                &event,
                "if event.assist then send_chat(event.actor, event.instanceId) end",
            )
            .expect("scalar payload fields retain their types"),
            [Committed::Chat {
                recipient: PLAYER_GUID,
                message: "18446744073709551615".to_string()
            }]
        );
    }

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
            ..ScriptEvent::default()
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
            ..ScriptEvent::default()
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

    #[test]
    fn a_misused_host_operation_names_the_call_and_the_fault() {
        let mut host = RuntimeScriptHost::new();
        let lonely = ScriptEvent {
            name: "on_tick".to_string(),
            actor: Some(player()),
            target: None,
            ..ScriptEvent::default()
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
            assert!(
                failure.message.contains("line 1"),
                "`{source}` must name generated Lua line 1, got `{}`",
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
            "finished scripts commit and the one after the failures still runs"
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

    #[test]
    fn an_event_with_no_scripts_answers_nothing() {
        let mut host = RuntimeScriptHost::new();
        let mut sink = FakeEffects::default();

        let (diagnostics, answer) = ask_event(&mut host, &mut sink, &engagement(), &[]);

        assert!(diagnostics.is_empty());
        assert_eq!(answer, None);
    }

    #[test]
    fn piccolo_leaks_a_table_constructors_element_count_as_an_extra_argument() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
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
    fn only_a_trailing_table_constructor_leaks_and_a_one_parameter_passthrough_stops_it() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();
        let preamble = "local function seen(a, b) if b == nil then return 1 end return 2 end\n";
        for (shape, source) in [
            (
                "a table constructor that is not the last argument",
                "grant_xp(event.actor, seen({7, 8, 9}, nil))",
            ),
            (
                "a table constructor passed through a one-parameter function",
                "local function pass(t) return t end\ngrant_xp(event.actor, seen(pass({7, 8, 9})))",
            ),
        ] {
            assert_eq!(
                granted_amounts(
                    &committed(&mut host, &event, &format!("{preamble}{source}")).unwrap()
                ),
                [1],
                "{shape} must not leak"
            );
        }
        assert_eq!(
            granted_amounts(
                &committed(
                    &mut host,
                    &event,
                    &format!("{preamble}grant_xp(event.actor, seen(({{7, 8, 9}})))")
                )
                .unwrap()
            ),
            [2],
            "parenthesising the constructor must still leak"
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
        let heavier = REPRESENTATIVE_SCRIPT.replace("1, 20 do", "1, 1000 do");
        host.invoke(script("heavier", &heavier), &event)
            .expect("fifty times the representative workload must still fit");
    }

    #[test]
    fn the_generated_representative_script_runs_answers_and_fits_the_fuel_budget() {
        let mut host = RuntimeScriptHost::new();
        let event = engagement();

        let invocation = host
            .invoke(script("fire_nova.generated", GENERATED_SCRIPT), &event)
            .expect("generated Lua must run inside the Fuel Budget");

        assert_eq!(invocation.answer, Some(130.0));
        let mut sink = FakeEffects::default();
        invocation.effects.commit(&mut sink);
        assert_eq!(granted_amounts(&sink.committed), [25]);
    }

    #[test]
    fn a_syntax_failure_names_a_line_of_the_generated_lua_not_of_its_typescript() {
        let mut host = RuntimeScriptHost::new();
        let generated_lines = GENERATED_SCRIPT.lines().count();
        let broken = format!("{GENERATED_SCRIPT}local a = 1 +\n");

        let diagnostic = host
            .invoke(
                script("fire_nova.generated", &broken),
                &unattended("on_login"),
            )
            .expect_err("the chunk does not parse");

        assert_eq!(diagnostic.kind, FailureKind::Syntax);
        assert!(
            diagnostic
                .message
                .contains(&format!("line {}", generated_lines + 1)),
            "the diagnostic must name line {} of the generated Lua: {}",
            generated_lines + 1,
            diagnostic.message
        );
        assert!(generated_lines > 25);
    }

    #[test]
    fn a_runtime_failure_names_its_line_in_the_generated_lua() {
        let mut host = RuntimeScriptHost::new();
        for fault in ["error('broken')", "assert(false, 'broken')"] {
            let generated = format!(
                "local function ____tbl(t) return t end\n\
                 local function script()\n\
                   local amount = 1\n\
                   {fault}\n\
                 end\n\
                 return script()\n"
            );

            let diagnostic = host
                .invoke(
                    script("fire_nova.generated", &generated),
                    &unattended("on_login"),
                )
                .expect_err("the generated Lua raises an error");

            assert_eq!(diagnostic.kind, FailureKind::Runtime);
            assert!(
                diagnostic.message.contains("line 4"),
                "the diagnostic must name generated Lua line 4: {}",
                diagnostic.message
            );
        }
    }

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

// Fixed hash entropy keeps replicated reducer replays deterministic.
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
                // SAFETY: the caller guarantees dest is writable for len bytes.
                unsafe { dest.add(written).write(byte) };
                written += 1;
            }
        }
    }

    #[unsafe(no_mangle)]
    unsafe fn __getrandom_custom(dest: *mut u8, len: usize) -> u32 {
        fill(dest, len);
        0
    }

    #[unsafe(no_mangle)]
    unsafe extern "Rust" fn __getrandom_v03_custom(
        dest: *mut u8,
        len: usize,
    ) -> Result<(), getrandom::Error> {
        fill(dest, len);
        Ok(())
    }
}
