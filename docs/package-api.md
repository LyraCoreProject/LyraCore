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
| `crate::game_client_command!(PARSE, APPLY, REPLY)` | the single Package parser, admitted apply operation and reply command name for authenticated addon commands |
| `crate::character_owned!(delete \| restamp \| transfer \| not_transported, ..)` | a Package table's character-keyed sweeps and its cross-shard transport arm |
| `crate::encounter_package!(BINDING, fn NAME(ctx, instance_id, signal) { .. })` | encounter authority for one Encounter Binding |
| `crate::game_package_characters!(fn NAME(ctx) { .. })` | the guids of the Characters the Package controls on this Shard, read by Package Teardown |

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
| `on_cast_finished` | `crate::hooks::CastFinishedPayload` |
| `on_loot` | `crate::hooks::LootPayload` |
| `on_quest_accept` | `crate::hooks::QuestAcceptPayload` |
| `on_quest_turnin` | `crate::hooks::QuestTurninPayload` |
| `on_login` | `crate::hooks::LoginPayload` |
| `on_character_relocated` | `crate::hooks::CharacterRelocatedPayload` |
| `on_logout` | `crate::hooks::LogoutPayload` |
| `on_gossip_select` | `crate::hooks::GossipSelectPayload` |
| `on_creature_death` | `crate::hooks::CreatureDeathPayload` |
| `on_hp_threshold` | `crate::hooks::HpThresholdPayload` |
| `on_go_used` | `crate::hooks::GoUsedPayload` |

### Session-less action consent

`actor::set_sessionless_action_consent(ctx, character_guid, allowed)` records a core-owned consent
projection during bot setup or controller selection. Call it for repeated selection too: every call
removes that Character's unclaimed Group Intents. Do not call it on decision ticks. Absent consent
preserves Legacy behavior. Core Gates check consent when accepting an invite or claiming an Intent.

The Gateway obtains acknowledged admission from the owning World Shard before it accepts at
Realm-core. Admission committed before a concurrent controller change can finish afterwards; this
ordering does not promise an atomic operation across Shards.

### Package-owned Accounts

`package_account::create_package_character(ctx, package_name, name, race, class) -> Result<u64, String>`
creates a Character with no Session and returns its guid. It applies the Refusals a client-created
Character meets: `NAME_IN_USE` for a taken name, `INVALID_RACE_CLASS` for a pair the imported
CharBaseInfo does not list, and the guid range Refusal. These Refusals write nothing.

The Character goes on the first Account that `package_name` owns with room for another Character.
When every owned Account is full, Core creates a new Account without credentials, so no login can
reach it. `game_package_account` records `package_name` as its owner. That record is a Core table,
so it stays when the Package is disabled. Pass the Package's own name.

The Character starts at level 1 at its race and class start position, with zero gender and
appearance bytes. The Package places it, levels it and builds its live entity.

### Package Teardown

`teardown_package(package_name)` is an Operator reducer. Run it on every Shard of the Realm before a
Package leaves the build, then run it once more on each Shard. The second pass catches a Character
that crossed into a Shard that was already torn down. `lyracore packages disable` runs both passes.
In one transaction, teardown does these steps:

- It makes each Character of the Package a Dormant Character. The Character goes offline and loses
  its live entity, its Sessionless Action Consent and its pending Transfer and Group Intents. Its
  Account and Character rows stay.
- It empties every table the Package declares.
- It deletes the Package's Package Config rows.
- It stops every hook, tick pass, encounter handler and client command the Package registered.

The Characters of a Package are the Characters on Accounts it owns on this Shard, plus the guids its
`game_package_characters!` read returns. Register the read when Core cannot find every Character
through ownership. Two cases need it: a Character that crossed from another Shard, and a Character
on an Account made before Package-owned Accounts existed. A Character with a World Session is never
made Dormant.

Teardown refuses while one of these Characters has a Transfer escrow row or a claimed Transfer
Intent on the Shard, because the escrow names the Package's tables. A Refusal writes nothing.
Retry when the crossing settles.

A Package stays stopped until the first tick of a build that does not compile it. Enabling it again
then starts it fresh: empty tables, default Package Config, and no Characters from before. Its old
Characters stay Dormant.

### Encounter kernel

`crate::encounter` holds the encounter state machine and the choreography verbs a Package drives it
with: `get_encounter_state`, `set_encounter_state`, `get_encounter_data`, `set_encounter_data`,
`watch_hp_threshold`, `reset_hp_fired`, `open_door`, `spawn_wave`, `equip_swap`, `move_to_point`,
`encounter_reset`, `encounter_reset_full`, and the four `ENCOUNTER_*` state constants. Core ships no
encounter content; the kernel exists for Packages.

### Actor verbs and helpers

`crate::actor` holds explicit-guid operations with the Gates of the core operation each names.
Existing verbs keep `fn verb(ctx, actor_guid, ..) -> Result<(), String>`. The table in
`module/src/actor.rs` lists their contracts.

The same root exposes the typed client-command parser and admitted apply values, the target Receipt
capacity that bounds retained per-issuer command fences, and the exact `companion_target_facts`
read. Core authenticates the issuer and the Gateway certifies Realm-core party authority before a
Package receives an admitted command.

`actor::cast_readiness(ctx, actor_guid, spell_id, target_guid)` applies the same read-only spellbook,
supported-lifecycle, range, line-of-sight, and ordinary cast Gates used at cast start. A Package can
turn an `OutOfRange` or `NoLineOfSight` Refusal into a movement prerequisite without spending power,
starting a cooldown, or creating a cast.

Provisioning uses six Actor operations. `select_profile_talent` performs a bounded preferred-tree
read through the owning talent admission calculation. `reconcile_profile_spell`,
`reconcile_profile_skill`, and `learn_profile_talent` return `Result<bool, ActionRefusal>`; `true`
means they changed normal Character state and `false` means the selected profile entry was already
satisfied.
`reconcile_profile_item` returns the granted count and caps its target at 200.
`equip_profile_upgrade` returns whether it equipped the carried item and preserves equal or stronger
gear. These verbs do not authorize an Actor. A Package must first pass the same controller and
Sessionless Action Gate as any other gameplay request. Their grants have no money cost because the
selected profile is the source of the entitlement.

`actor::reconcile_starter_role_spell_levels(ctx)` repairs source-derived training levels only on
complete old curated header shapes. Imported or Operator-tuned rows remain authoritative.

`actor::request_cast(ctx, actor_guid, spell_id, target_guid)` returns
`Result<spell::CastStart, spell::CastRefusal>`. `Started` carries a Cast Handle. `Waiting` carries
an existing cast's original identity, spell, target, and current due time, even when the new request
names a different spell or target. It never restarts that cast. `Resolved` means effects dispatched
synchronously, which does not imply a projectile hit. Channeled spells return `UnsupportedChannel`
until bot requests can retain their lifecycle. Client casts keep their existing channel behavior.
`actor::cast_at` is the compatibility adapter that discards this distinction. Character Actor
requests require the spell in `game_player_spell`. Creature and triggered spell-engine entries keep
their existing spellbook exemption.

A scheduled cast ends through `on_cast_finished`. Its payload carries the caster, target, scheduled
identity, and `spell::CastFinish`. Packages must match the scheduled identity before updating retained
work. The hook runs after the pending row is removed. An instant cast returns its result directly and
does not fire this hook. `on_cast_resolved` keeps its existing effect-dispatch contract.

`spell::pending_cast` reads the current Cast Handle by caster. Refusal kinds are supplied by the
owning Gate. `Other` preserves a Gate's message when no current caller needs a separate policy.
`UnlearnedSpell` and `NoLineOfSight` identify the spellbook and targeted visibility Gates.

`spell::cancel_cast_attempt(ctx, caster_guid, scheduled_id)` cancels only that scheduled cast.
`spell::expire_cast_attempt(ctx, caster_guid, scheduled_id, deadline_micros)` also requires the
caller's action deadline to have passed. They return whether they removed the cast and report
`Cancelled` or `Expired` through `on_cast_finished`. A stale identity cannot cancel a replacement.

`crate::helpers` holds the reads a Package needs before it acts: `live_entity`, `require_character`,
`character_by_guid`, `character_by_name`, `entity_by_owner`, `acting_entity_by_guid`, `entities_near`,
`nearest_entity`, `in_same_partition`, `require_operator`.

`group::party_facts(ctx, character_guid)` reads the Character's local durable party mirror. It names
the leader and every member, with nullable live position, health, and death facts. An absent local
Unit may instead carry a Realm-core-certified map, instance, and locator revision. Pending Transfer,
unknown location, and departed-member fences expose no partition. Enemy facts cover hostile
creatures with current party melee, cast, threat, or control evidence. Hostile Characters are
excluded because PvP party assistance is outside this contract. An absent membership returns
`Ok(None)`.
A Realm-owned Roster Revision orders the complete member list, leader, loot rules, Group kind, and
every Raid Slot. World Shards retain its disband state, so delayed Gateway fanout cannot remove a
newer member, restore older party rules, or recreate a disbanded party.
A membership whose Group row is missing returns `MissingGroup`. Party facts cover every
Raid member, whatever the Subgroup. `FightLimit` reports more than 40 members, which only a damaged
mirror holds. It also reports 24 incoming melee or threat rows for one member, one pending cast for
one member, or 24 aggregate enemy GUIDs. For each retained enemy, the read permits 80 threat
sources, 64 control auras, and three effects on a pending spell.
Either failure returns `PartyFactsUnavailable`, so a Package can hold party control instead of
acting from an arbitrary prefix.

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

### Package fixtures (debug only)

`crate::package_fixture` exists only in a Module built with `debug_reducers`. It holds the setup and
observation steps a Package's own debug fixtures need. A release build has no such root.

| operation | does |
|---|---|
| `apply_damage(ctx, target_guid, amount, attacker_guid)` | deals main-hand damage through the real Core damage pipeline, capped one below the target's health |
| `remove_live_character(ctx, character_guid)` | removes a live Character from the world as a logout does; `on_logout` fires and the Character row stays |
| `require_no_imported_content(ctx)` | refuses when the Shard holds imported content; the temporary weather seed a fresh Module stamps does not count |
| `top_threat_target(ctx, creature_guid)` | reads the highest-threat living source on the creature's map and instance |
| `client_cast(ctx, caster_guid, spell_id, target_guid)` | casts through the same Gates a client cast passes |
| `admit_to_instance(ctx, character_guid, map_id, instance_id, party_id, request_actor)` | stages the party's instance as a dungeon entry leaves it and binds the Character to it; refuses when entry resolves to another instance |
| `record_completed_transfer(ctx, character_guid, map_id, instance_id)` | records a finished Transfer in Realm-core's character-to-shard index |
| `declare_next_movement_tick(ctx, delay)` | makes the next creature movement tick fire once, `delay` from now; refuses unless the catch-all tick is the only movement schedule |

A fixture reads the navigation revision through `nav::inputs(ctx, map_id).imported_revision`.

Name this root only from a Package file whose first non-blank line is
`#![cfg(feature = "debug_reducers")]`. The lint refuses it anywhere else, and no exemption clears
it, because a release build would compile that file without the root.

### Package tests (test only)

`crate::package_test` exists only in a Module test build. It holds what a Package's own unit tests
need.

| operation | does |
|---|---|
| `ask_offline(event, actor, target, scripts)` | runs `scripts` in order on a fresh Runtime Script Host and returns the Script Answer as `script_binding::ask` reads it; discards Staged Effects and returns any Script Diagnostic as an error |
| `EntityView`, `RuntimeScript` | the event entity and script values `ask_offline` takes |
| `read_scanned(rel)` | reads a repository-relative source file; `None` when its optional directory is not installed |
| `code_of(src, signature)` | the body after `signature`, comments removed |
| `shape_of(src, signature)` | `code_of` with whitespace collapsed, for an exact comparison |

A source scan pins a chokepoint that no unit test can reach. Prefer a pure function and assert on
it wherever one exists.

Name this root only from a Package file whose first non-blank line is `#![cfg(test)]`. The lint
refuses it anywhere else, and no exemption clears it, because an ordinary build would compile that
file without the root.

### Tables

A Package declares its own tables with `#[table(accessor = pkg_<package>_<name>, ..)]`, the naming
rule `docs/schema.md` states. The Package name in the accessor is what keeps two Packages from
colliding. The accessor is also the table name Package Teardown empties, so the build refuses a
Package table that sets its own `name`.

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
group      helpers  hooks     items      loot       nav       package_account
package_config      quest     script_binding        spell     stats
terrain    transfer world     xp
```

Debug only: `package_fixture`, in a file gated on `debug_reducers` (see Package fixtures above).
Test only: `package_test`, in a file gated on `#![cfg(test)]` (see Package tests above).

Plus, at the crate root: any `game_*` name (a table accessor or registration marker), any
`pkg_*` name (a Package's own generated root module), any type name in UpperCamelCase (a row or
payload type), `character_owned!`, `encounter_package!`, and `CHARACTER_OWNED_TABLES`.

A root that is absent is core's own business. `auth`, `debug`, `runtime_script`, `test_scan`,
`realm_core`, `gw` and the rest are not promised, and naming one fails the build.

## The lint

`module/build.rs` reads every file under `packages/*/src/` and fails the build on the first path that
reaches a crate root outside the list above. The failure names the Package, file, line and path. Core
`src/` is never linted because the Package API is a promise core makes to Packages, not to itself.

The scanner recognizes `crate::`, `$crate::`, and a `super::` chain that leaves the Package module.
Raw identifiers have their ordinary meaning, so `crate::r#helpers` names the documented `helpers`
root.

The scanner uses the file path and inline modules to distinguish a crate-root escape from a
Package's own sibling or submodule. A crate-root glob such as `use crate::*` also fails because it
imports roots that the Package API does not name.

Whole-crate aliases such as `use crate as core`, `use crate::{self as core}`, and
`extern crate self as core` fail at their declaration. Spell every core dependency as
`crate::<Package API root>` so the lint can check it where it appears. `#[path]` and a `cfg_attr`
that supplies `path` also fail. `include!` fails because it can add unscanned Rust source;
`include_str!` and `include_bytes!` remain available for data. Package modules use Rust's normal
`mod.rs`, `<name>.rs`, or `<name>/mod.rs` layout, which keeps filesystem depth and `super` depth
equal.

This remains a lexical compatibility check, not Rust name resolution. It does not expand macros or
follow a re-export declared in another file. Rust still compiles every Package after this check;
these limits do not turn the Package API into a sandbox.

Comments and string literals are stripped before the scan, so a path quoted in a doc example is
inert.

A Package that genuinely needs a path off the surface writes the reason on the line that names it:

```rust
crate::realm_core::record_shard(ctx, ..) // package-api: exempt fixture models a completed Realm locator crossing
```

The marker clears that line and no other. There is no global toggle, and the reason is required — a
bare marker does not clear. Every exemption is a gap in this document; raise it with the maintainers
so the surface can grow or the Package can move off it.

An exemption cannot enable a whole-crate alias or `#[path]`. Either spelling can hide dependencies
on other lines or in files the lint cannot locate, so the build always refuses it.

## Action observations

`actor::request_attack` returns `AttackStart::Armed` or `AlreadyArmed`. The latter keeps the current
melee swing timer. After either result, an in-range Character turns toward the exact admitted target
when Core's facing Gate would block its swing, unless an active movement leg still owns its position
and facing. Both results mean an engagement was accepted; neither proves a hit. The existing
`actor::attack` and client operations retain their re-arm behavior.

`actor::request_accept_quest` and `request_turn_in_quest` complete synchronously. They return
`Result<(), ActionRefusal>`, with the same core Gates and effects as the client operations. They
have no waiting, cancellation, or expiry phase. Attack requests also return `ActionRefusal`.
Its `kind` identifies actionable Gates and `detail` preserves the client message. `Other` is an
opaque refusal; Package policy must not classify its text.

`nav::route_step` accepts the same arguments as `nav_step` and returns a `RouteStep`. Its `status`
is `Complete`, `Partial`, `Blocked`, or `Direct` when navigation is disabled. Complete and Partial
describe the planned route. `first_waypoint`, `expansions`, and `clipping` retain search and commit
Gate evidence. `endpoint == from` means no movement was approved, even if a complete path was
planned. A blocked search holds position. Creature callers retain `nav_step` and its existing
collision-gated fallback.

`nav::coverage_generation(ctx, map_id)` returns the active complete coverage generation when its
Gate is enabled, or `None`. Include it with map and instance in deferred destination keys. A changed
generation invalidates a retained route decision; `None` preserves unknown coverage.

`coverage` is `Unknown` unless every consulted navigation cell matches the active complete
manifest. `VerifiedCells` names that generation and the number of checked cells. It proves only
those cells' derivation, not world coverage or the presence of imported terrain and client
geometry. A Package must measure actual position on later observations to establish advancement
or arrival; the proposed endpoint cannot establish either.

`actor::sessionless_action_gate(ctx, character_guid)` checks current Account Claim and Fence ownership, Character availability, and World Session status before Package gameplay. It permits a missing live entity so Legacy can restore a body. Group admission also requires a live entity and current controller consent.

`actor::sessionless_movement_gate(ctx, character_guid)` adds current controller consent and a live,
living body to that authority check. A pending cast or movement-suppressing crowd control refuses
movement. Call it before issuing a movement leg, including continuation between decision turns. The Gate reads
current state and does not cancel casts or change position.

`creatures::tick::stop_where_rendered(ctx, &mut mover)` stops the mover's current leg where the
client renders it now, facing along its Route Path. It moves the row to that point and sends a stop
there. The caller writes the row. A mover's stored position can lag its leg, so a stop at the stored
position moves the client back. A blocked Route Path segment or changed navigation inputs halt the
stop where a leg advance would halt, so it never lands past an obstruction.

`creatures::tick::emit_creature_path(ctx, mover, points, run)` sends the mover along a checked Route
Path and writes the row. A mover on a leg starts the path from the point that
`stop_where_rendered` would stop it at, so a renewal between two advance firings does not move the
client back.

`actor::area_trigger_route(ctx, trigger_id)` reads one exact imported AreaTrigger source volume and
target map. It exposes the source center and containment rule for Candidate movement while keeping
the landing coordinates private. `actor::enter_sessionless_areatrigger` rechecks the current body,
volume, exact existing dungeon instance lease, and a Realm-certified party member in the expected
partition before it applies the imported landing and writes one Transfer Intent. A Refusal leaves
the Character, instance binding, and intent unchanged.

`nav::LEG_MAX_EXPANSIONS` is the expansion cap used by `nav::route_step`. A Package can reserve that work before selecting movement.
