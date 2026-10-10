# Runtime Script Host

The Runtime Script Host contains the Lua interpreter used by Runtime Scripts.

One `RuntimeScriptHost` owns one embedded piccolo interpreter and a compiler cache. The cache holds
compiled chunks only, keyed by a hash of the source, so it can never hand back stale code for
an edited script and it holds no Lua state between invocations.

Every invocation gets:

* a fresh environment table with its own standard-library tables, so nothing a script assigns
  survives it;
* a `FUEL_BUDGET_PER_INVOCATION` of metered interpreter work, plus
  `MAX_STEPS_PER_INVOCATION` as the
  stall guard for the case where a step burns no fuel at all;
* a staging buffer. A host operation a script calls does not touch the world; it appends a
  `StagedEffect`. Only a fully successful invocation returns `StagedEffects`, which the
  caller commits through core gameplay operations. A syntax, runtime, or fuel failure returns a
  `ScriptDiagnostic` instead and every effect staged by that invocation is dropped with it.

`run_event` is the failure boundary: it invokes each Runtime Script in turn, commits the ones
that succeeded, and collects a bounded diagnostic for the ones that did not. One bad script
never stops the next one, and never stops the core work that follows.

# Host operations

Authors bind a named function to an event. The toolchain emits a local wrapper that calls that
function with the event table and returns its Script Answer. Event Binding does not execute
source during the build or install persistent handlers in the interpreter.

The Host still supplies one global per Host Operation and one `event` table, so existing compiled
Script Artifacts remain valid:

```lua
event.name              -- the event label, a string
event.actor             -- the Entity Handle that caused the event, or nil
event.target            -- the Entity Handle the event acted on, or nil
event.player            -- a Character Entity Handle for login and level-up
event.newLevel          -- the attained level, only for level-up

-- An Entity Handle's readable fields, snapshotted when the Invocation started:
e.name, e.is_player, e.level, e.health, e.max_health, e.map_id, e.x, e.y, e.z

heal(entity, amount)    -- stage a heal, crediting heal-threat to event.actor
send_chat(player, text) -- stage a System Message to one online player
grant_xp(player, amount)-- stage an experience grant

return 42               -- the Script Answer: a number the asking caller reads back
```

A chunk that returns a number answers the caller that asked; returning nothing, or anything that
is not a number, answers nothing. Only `ask_event` reads an answer. `run_event` discards it,
because a core hook event has no caller waiting on one.

An Entity Handle is opaque: a script cannot read a guid out of it and cannot mint one, so the
only entities a Runtime Script can act on are the ones the Host resolved for that Invocation.
Every Entity Handle field is a snapshot taken before the script ran; writing to one changes nothing.
Hook payload fields come from `events.json`. The level-up hook carries `newLevel` even before its
Character row stores that level. The required `player` field aliases the actor's opaque handle;
the generated wrapper refuses to invoke a typed handler if that Character is absent. Scalar
64-bit identifiers use decimal strings to preserve their value in TypeScript.

A host operation called with a missing entity, the wrong type, an out-of-range amount, or past
the staging cap raises a Lua error naming the call and the fault. The Invocation returns a
`ScriptDiagnostic`, drops all staged effects, and leaves the reducer running.

# What an Invocation is allowed

The environment is built from `ALLOWED_GLOBALS` and `ALLOWED_LIBRARY_MEMBERS`, name by name,
into tables made for that Invocation. A name outside those lists is nil inside a Runtime Script,
and a write to an allowed library table is invisible to the next Invocation.

The Host provides no durable script storage, event bindings, damage, items, spawning, scheduling,
or database queries. Lua state ends with its Invocation.
