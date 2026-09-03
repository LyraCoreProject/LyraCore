# Package API, version 1

The Package API is the part of the Module a Package may name. A Package compiles into the Module
wasm and can reach any `crate::` path the compiler resolves; this document says which of those paths
core will keep working, and `module/build.rs` fails the build on the rest.

This is a contract, not a tutorial. Read `packages/README.md` for how a Package is installed and
`packages/example/src/mod.rs` for the shape of one.

## Compatibility promise

**May be relied on.** Everything listed below keeps working across core changes at this version. A
hook event keeps its name and its payload fields. A listed operation keeps its name, its parameter
order and its Refusal behaviour. A table accessor keeps its name.

**May move with notice.** The surface grows by addition: a new hook event, a new payload field, a new
operation. A change that breaks a Package bumps this version and is called out in `CHANGELOG.md`. An
`_Avoid_` rename or a module split may move a path, and the lint below is what makes that visible at
build time rather than at run time.

**Never promised.** Every `crate::` root not listed here. Row layouts of tables a Package does not
own. The order core operations run in, beyond the ordering stated below. Anything reached through
`ctx.db` that is not a listed table accessor: it compiles, and core may change it in any release.

The Package API is a compatibility contract, not a sandbox. Compiled Package code runs inside the
Module with full access to every table, exactly as `lyracore packages add`'s Trust Review states. The
lint tells a Package author when core moved under them; it stops nothing at run time.

## The surface

### Marker macros

The sanctioned extension points. Each is a text-scanned marker `module/build.rs` turns into a
registry entry; the macro documentation in `module/src/lib.rs` is authoritative for the exact shape,
which must be written literally.

| marker | registers |
|---|---|
| `crate::game_hook!(EVENT, fn NAME(ctx, payload) { .. })` | a notify handler for one hook event |
| `crate::game_tick_pass!(fn NAME(ctx) { .. })` | a periodic pass, run at the end of every `tick_creatures` tick (0.5s), after every core pass |
| `crate::character_owned!(delete \| restamp \| transfer \| not_transported, ..)` | a Package table's character-keyed sweeps and its cross-shard transport arm |
| `crate::encounter_package!(BINDING, fn NAME(ctx, instance_id, signal) { .. })` | encounter authority for one Encounter Binding |

A Package table that is keyed by `character_guid` needs a `delete` marker and a transport arm. Without
them a despawned character leaves rows behind, and a character that crosses a shard loses them.

### Hook catalogue

Handlers are notify-only: they observe the payload and may act through the operations below. There is
no veto and no fold. Payload fields are documented at the struct in `module/src/hooks.rs`, which is
authoritative; this list is the set of event names and their payload types.

| event | payload |
|---|---|
| `on_damage_taken` | `crate::hooks::DamageTakenPayload` |
| `on_death_prevented` | `crate::hooks::DeathPreventedPayload` |
| `on_creature_spawn` | `crate::hooks::CreatureSpawnPayload` |
| `on_levelup` | `crate::hooks::LevelupPayload` |
| `on_group_invite` | `crate::hooks::GroupInvitePayload` |
| `on_death` | `crate::hooks::DeathPayload` |
| `on_kill` | `crate::hooks::KillPayload` |
| `on_aggro` | `crate::hooks::AggroPayload` |
| `on_cast_resolved` | `crate::hooks::CastResolvedPayload` |
| `on_loot` | `crate::hooks::LootPayload` |
| `on_quest_accept` | `crate::hooks::QuestAcceptPayload` |
| `on_quest_turnin` | `crate::hooks::QuestTurninPayload` |
| `on_login` | `crate::hooks::LoginPayload` |
| `on_logout` | `crate::hooks::LogoutPayload` |
| `on_gossip_select` | `crate::hooks::GossipSelectPayload` |
| `on_creature_death` | `crate::hooks::CreatureDeathPayload` |
| `on_hp_threshold` | `crate::hooks::HpThresholdPayload` |
| `on_go_used` | `crate::hooks::GoUsedPayload` |

### Encounter kernel

`crate::encounter` holds the encounter state machine and the choreography verbs a Package drives it
with: `get_encounter_state`, `set_encounter_state`, `get_encounter_data`, `set_encounter_data`,
`watch_hp_threshold`, `reset_hp_fired`, `open_door`, `spawn_wave`, `equip_swap`, `move_to_point`,
`encounter_reset`, `encounter_reset_full`, and the four `ENCOUNTER_*` state constants. Core ships no
encounter content; the kernel exists for Packages.

### Actor verbs and helpers

`crate::actor` is the documented verb set over every explicit-guid action: one uniform shape,
`fn verb(ctx, actor_guid, ..) -> Result<(), String>`, with the gates of the core operation it names.
The table at the top of `module/src/actor.rs` lists every verb and its gate semantics.

`crate::helpers` holds the reads a Package needs before it acts: `live_entity`, `require_character`,
`character_by_guid`, `character_by_name`, `entity_by_owner`, `acting_entity_by_guid`, `entities_near`,
`nearest_entity`, `in_same_partition`, `require_operator`.

### Package Config

An Operator-tunable value, seeded by the Package and edited without a republish.
`crate::package_config::ensure_package_config_default(ctx, package, key, default)` inserts only when
the row is absent, so a Package calls it from its own ensure path on every run. The Operator edits
with the `set_package_config` reducer.

### Package Events and Runtime Scripts

A Package fires its own event and reads back a Script Answer with
`crate::script_binding::ask(ctx, event, actor_guid, target_guid) -> Option<f64>`. The answer is the
first number a bound script returned, in dispatch order; later scripts still run. No answer means the
caller keeps its own fallback, which is what makes a Runtime Script an override rather than a
dependency. Core hook events reach bound scripts on their own; a Package fires only its own Package
Events.

### Tables

A Package declares its own tables with `#[table(accessor = pkg_<package>_<name>, ..)]`, the naming
rule `docs/schema.md` states. The Package name in the accessor is what keeps two Packages from
colliding.

Core table accessors are named `game_*` and are reached at the crate root:
`use crate::{game_world_entity, game_character};`. Row types are re-exported at the crate root under
their type names: `crate::WorldEntity`, `crate::CharacterQuest`. Both forms are on the surface.
`crate::CHARACTER_OWNED_TABLES` is the generated manifest of character-keyed tables.

### Module roots

The `crate::` roots a Package may name. Everything under a listed root is on the surface at this
version; root granularity is deliberate, so a core refactor inside a root does not churn the
contract.

```
actor      chat     combat    creatures  encounter  faction   gameobject
group      helpers  hooks     items      loot       nav       package_config
quest      script_binding     spell      stats      terrain   transfer
world      xp
```

Plus, at the crate root: any `game_*` name (a table accessor, `game_hook!`, `game_tick_pass!`), any
`pkg_*` name (a Package's own generated root module), any type name in UpperCamelCase (a row or
payload type), `character_owned!`, `encounter_package!`, and `CHARACTER_OWNED_TABLES`.

A root that is absent is core's own business. `auth`, `debug`, `runtime_script`, `test_scan`,
`realm_core`, `gw` and the rest are not promised, and naming one fails the build.

## The lint

`module/build.rs` reads every file under `packages/*/src/` and fails the build on the first path that
reaches a crate root outside the list above. The failure names the Package, file, line and path. Core
`src/` is never linted because the Package API is a promise core makes to Packages, not to itself.

The scanner recognizes `crate::`, `$crate::`, and a `super::` chain that leaves the Package module.
It also follows an unqualified crate-root alias declared in the same file. It recognizes
`use crate as name`. Grouped `self as name` uses and `extern crate self as name` also work.

The scanner uses the file path and inline modules to distinguish a crate-root escape from a
Package's own sibling or submodule. A crate-root glob such as `use crate::*` also fails because it
imports roots that the Package API does not name.

This remains a lexical compatibility check, not Rust name resolution. It does not expand macros or
follow an alias or re-export declared in another file. A Package should use the normal file module
layout rather than `#[path]` when it uses relative imports, because the scanner derives module depth
from that layout. Rust still compiles every Package after this check; these limits do not turn the
Package API into a sandbox.

Comments and string literals are stripped before the scan, so a path quoted in a doc example is
inert.

A Package that genuinely needs a path off the surface writes the reason on the line that names it:

```rust
crate::auth::create_character(ctx, ..) // package-api: exempt a bot Character is created without a Session
```

The marker clears that line and no other. There is no global toggle, and the reason is required — a
bare marker does not clear. Every exemption is a gap in this document; raise it with the maintainers
so the surface can grow or the Package can move off it.
