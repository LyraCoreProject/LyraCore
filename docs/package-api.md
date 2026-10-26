# Package API, version 4

The Package API is the part of the Module a Package may name. A Package compiles into the Module
wasm and can reach any `crate::` path the compiler resolves. This document says which of those paths
core will keep working, and `cargo test -p lyracore-module --test package_api_lint` reports paths
outside that contract.

This is a contract, not a tutorial. Read `packages/README.md` for how a Package is installed and
[the Reference Packages](https://github.com/LyraCoreProject/packages#packages) for working examples.

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

## Spell effect kinds

`crate::actor::EffectKind` defines the known effect codes shared by the Module, protocol crate and
Importer. Convert a row's `kind` byte with `EffectKind::try_from`; handle unknown authored codes
explicitly. The Actor byte constants keep their values.

## The surface

### Marker macros

The sanctioned extension points. Each is a text-scanned marker `module/build.rs` turns into a
registry entry; the macro patterns in `module/src/lib.rs` are authoritative for the exact shape,
which must be written literally.

| marker | registers |
|---|---|
| `crate::game_hook!(EVENT, fn NAME(ctx, payload) { .. })` | a notify handler for one hook event |
| `crate::game_tick_pass!(fn NAME(ctx) { .. })` | a periodic pass, run at the end of every `tick_creatures` tick (0.5s), after every core pass |
| `crate::game_client_command!(PARSE, APPLY, REPLY)` | the single Package parser, admitted apply operation and reply command name for authenticated addon commands |
| `crate::character_owned!(delete \| restamp \| transfer \| not_transported, ..)` | a Package table's character-keyed sweeps and its transport arm |
| `crate::encounter_package!(BINDING, fn NAME(ctx, instance_id, signal) { .. })` | encounter authority for one Encounter Binding |
| `crate::game_package_characters!(fn NAME(ctx) { .. })` | the guids of the Characters the Package controls, read by Package Teardown |

A Package table that is keyed by `character_guid` needs a `delete` marker and a transport arm. Without
them a despawned Character leaves rows behind.

### Hook catalogue

Handlers are notify-only: they observe the payload and may act through the operations below. There is
no veto and no fold. The structs in `module/src/hooks.rs` define the payload fields and their types.
This list pairs each event name with its payload type.

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
projection during bot setup or controller selection. Call it for repeated selection too. Each call
clears that Character's unclaimed Group Intents and last completed Character Relocation. Do not call
it on decision ticks. Absent consent permits sessionless actions.

Core checks current consent before local invite, leave and automatic acceptance operations. The
consent Gate and group action run in one transaction. Suppressed consent leaves an incoming
invitation unanswered. `emit_bot_invite_intent` and `emit_bot_leave_intent` apply these operations
directly and create no Group Intent. A group invariant failure aborts the transaction because those
Package operations return no outcome.

### Package-owned Accounts

`package::create_character(ctx, package_name, name, race, class) -> Result<u64, String>`
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

`teardown_package(package_name)` is an Operator reducer. Run it before a Package leaves the build.
`lyracore packages disable` runs it. In one transaction, teardown does these steps:

- It makes each Character of the Package a Dormant Character. The Character goes offline and loses
  its live entity, its Sessionless Action Consent and its pending Transfer and Group Intents. Its
  Account and Character rows stay.
- It empties every table the Package declares.
- It deletes the Package's Package Config rows.
- It stops every hook, tick pass, encounter handler and client command the Package registered.

The Characters of a Package are the Characters on Accounts it owns, plus the guids its
`game_package_characters!` read returns. Register the read when Core cannot find every Character
through ownership, as with a Character on an Account made before Package-owned Accounts existed. A
Character with a World Session is never made Dormant.

Teardown refuses, and writes nothing, while one of these Characters has a Transfer escrow row or a
claimed Transfer Intent, because the escrow names the Package's tables.

A Package stays stopped until the first tick of a build that does not compile it. Enabling it again
then starts it fresh: empty tables, default Package Config, and no Characters from before. Its old
Characters stay Dormant.

### Encounter kernel

`crate::package::encounter` holds the encounter state machine and the choreography verbs a Package drives it
with: `get_encounter_state`, `set_encounter_state`, `get_encounter_data`, `set_encounter_data`,
`watch_hp_threshold`, `reset_hp_fired`, `open_door`, `spawn_wave`, `equip_swap`, `move_to_point`,
`encounter_reset`, `encounter_reset_full`, and the four `ENCOUNTER_*` state constants. Core ships no
encounter content; the kernel exists for Packages.

`move_to_point` sends the creature on one leg from the point the client draws it at. The creature's
stored position follows the leg and reaches the destination when the leg lands, so a Package that
needs the arrival schedules it from the leg, as a relay arrival does, and does not read the
destination from the row at once.

### Actor verbs and helpers

`crate::actor` holds explicit-guid operations that apply the Gates of the core operation each names.
Player reducers, debug reducers and Package bots share them. Every Gate stays in the operation that
owns it, and the caller authorizes the Actor before entering. The named re-exports in
`module/src/actor.rs` are the supported operations. Existing verbs keep
`fn verb(ctx, actor_guid, ..) -> Result<(), String>`. A `request_*` verb returns a typed outcome
instead: accepted work or a Refusal, and a Cast Handle for a pending cast.

| verb | core | Gates and outcomes |
|------|------|------------------------------------------------------------|
| `attack` | `combat::apply_start_attack` | CC-blocked rejected; no self/corpse/cross-map; friendly (green) target rejected when faction data exists; re-arm retargets |
| `request_attack` | `combat::request_attack` | typed `AttackStart` or Refusal; see Action observations |
| `ranged_attack` | `combat::apply_start_ranged_attack` | `attack` gates + ranged weapon equipped (slot 17) |
| `stop_attack` | `combat::stop_attack_for` | unconditional disarm of the actor's outgoing melee row |
| `cast_at` | `spell::request_cast` | result-only adapter over `request_cast`; level comes from the live entity |
| `cast_readiness` | `spell::cast_readiness` | read-only spellbook, supported-lifecycle, range, line-of-sight and ordinary cast Gate check |
| `request_cast` | `spell::request_cast` | typed start, waiting, and Refusal; completion uses `on_cast_finished` |
| `accept_quest` | `quest::apply_accept_quest` | alive + giver in range offering the quest + level/race/class/prereq/duplicate gates |
| `request_accept_quest` | `quest::request_accept_quest` | the same Gates, typed Refusal; see Action observations |
| `stage_quest` | `quest::grant_quest_unchecked` | Debug and bot staging: same row shape, all accept gates SKIPPED (giver-less) |
| `turn_in_quest` | `quest::apply_turn_in_quest` | alive + giver in range ending the quest + objectives complete; rewards atomic |
| `request_turn_in_quest` | `quest::request_turn_in_quest` | the same Gates, typed Refusal; see Action observations |
| `open_creature_loot` | `loot::open_creature_corpse` | result-only adapter over `request_open_creature_loot` |
| `request_open_creature_loot` | `loot::request_open_creature_corpse` | typed open acceptance or Refusal; applies the same corpse and Loot Tag Gates |
| `take_loot` | `items::apply_take_loot` | result-only adapter over `request_take_loot` |
| `request_take_loot` | `items::request_take_loot` | typed take completion or Refusal; inventory-full leaves the item and Loot Source unchanged |
| `loot_money` | `loot::apply_loot_money` | alive + dead creature corpse, same map, 10yd, money > 0 + Loot Tag eligibility |
| `buy_item` | `items::apply_buy_item` | vendor in range + stocked + money; stacks/slots validated |
| `sell_item` | `items::apply_item_sell` | vendor in range + sellable item in slot; sells the whole stack and feeds the buyback ring. Native client requests can also sell a given count |
| `use_item` | `items::apply_item_use` | alive + usable item in slot (consumable/on-use gates) |
| `cast_item_target` | `creatures::apply_item_target_spell` | spell kind + owned carried item + effect-specific gates |
| `equip_item` | `items::apply_equip_item` | slot type/level gates |
| `trainer_buy` | `trainer::apply_trainer_buy` | trainer resolved by guid + offering gates (class/level/money) |
| `reconcile_profile_spell` | `trainer::reconcile_profile_spell` | free profile grant + trainer/class/level/rank gates; explicit no-import demo fallback |
| `reconcile_profile_skill` | `skill::reconcile_profile_skill` | free profile grant + race/class/level availability gates |
| `select_profile_talent` | `talent::select_profile_talent` | bounded preferred-tree selection through the owning talent Gates |
| `learn_profile_talent` | `talent::reconcile_profile_talent` | class/race/level/point/tier/prerequisite gates |
| `reconcile_profile_item` | `items::request_profile_item` | bounded top-up (target capped at 200) + capacity/uniqueness gates; returns the granted count |
| `equip_profile_upgrade` | `items::apply_equip_profile_upgrade` | normal equip gates + preserves equal or stronger gear; returns whether it equipped |
| `reconcile_starter_role_spell_levels` | `seed::reconcile_curated_starter_role_levels` | repairs only exact old curated level-zero headers; preserves imported or tuned rows |
| `use_gameobject` | `gameobject::apply_use_gameobject` | GO resolved by guid + range/use gates |
| `request_use_gameobject` | `gameobject::request_use_gameobject` | typed target, partition, range, and use Refusal |
| `area_trigger_route` | `quest::area_trigger_route` | exact imported source volume and target map; landing remains private |
| `enter_sessionless_areatrigger` | `quest::enter_sessionless_areatrigger` | imported source volume + expected certified party partition + session-less action Gates |
| `import_revision` | `game_import_meta` read | current importer source and file identities for one family |
| `repop` | `world::do_repop` | dead actor releases to the graveyard ghost |
| `respond_resurrect` | `spell::do_resurrect_response` | consume the actor's pending rez offer; accept revives IN PLACE at the offer's % |
| `spirit_res` | `world::do_spirit_healer_res` | ghost actor res without a healer Gate; 25% carried-item durability loss and level-scaled sickness apply |
| `self_resurrect` | `spell::do_self_resurrect` | dead actor with a Self-Resurrection Option revives in place; no sickness |
| `accept_group_invite` | `group::accept_invite_for` | pending invite exists + its Group still exists (or a solo inviter is still ungrouped) + group not full; roster events fire |
| `set_sessionless_action_consent` | `sessionless::set_sessionless_action_consent` | update Package consent and clear unclaimed Group Intents atomically |
| `companion_target_facts` | `group::companion_target_facts` | exact hostile creature + partition/death/control gates; never selects a substitute |
| `system_message` | `chat::emit_system_message` | recipient exists and is online on this Shard; text is trimmed, bounded, and non-empty |

Add a consumed verb to this table and the re-exports in `module/src/actor.rs`. If its operation is
inlined in a sender-bound reducer, extract the operation and let both callers use it. Delete unused
verbs. `package_only!` allows unused imports because Packages select what they need; `debug_only!`
allows them only in a build without `debug_reducers`.

The same root exposes the typed client-command parser and admitted apply values, the target Receipt
capacity that bounds retained per-issuer command fences, and the exact `companion_target_facts`
read. Core authenticates the issuer and checks local party authority and Sessionless Action Consent
before a Package receives an admitted native command. It also exposes Character and live-entity
reads, nearby entities, partition checks, quest eligibility, loot entitlement, spell readiness and
cast observations. Package authorization, Character setup and encounter operations are named in
`module/src/package.rs`.

`actor::cast_readiness(ctx, actor_guid, spell_id, target_guid)` lets a Package turn an `OutOfRange` or
`NoLineOfSight` Refusal into a movement prerequisite without spending power, starting a cooldown, or
creating a cast.

The six provisioning verbs are `select_profile_talent`, `reconcile_profile_spell`,
`reconcile_profile_skill`, `learn_profile_talent`, `reconcile_profile_item` and
`equip_profile_upgrade`. `reconcile_profile_spell`, `reconcile_profile_skill` and
`learn_profile_talent` return `Result<bool, ActionRefusal>`; `true` means they changed normal
Character state and `false` means the selected profile entry was already satisfied. None of them
authorizes an Actor. A Package must first pass the same controller and Sessionless Action Gate as
any other gameplay request. Their grants have no money cost because the selected profile is the
source of the entitlement.

`actor::request_cast(ctx, actor_guid, spell_id, target_guid)` returns
`Result<actor::CastStart, actor::CastRefusal>`. `Started` carries a Cast Handle. `Waiting` carries
an existing cast's original identity, spell, target, and current due time, even when the new request
names a different spell or target. It never restarts that cast. `Resolved` means effects dispatched
synchronously, which does not imply a projectile hit. Channeled spells return `UnsupportedChannel`
until bot requests can retain their lifecycle. Client casts keep their existing channel behavior.
`actor::cast_at` is the adapter that discards this distinction. Character Actor requests require the
spell in `game_player_spell`. Creature and triggered spell-engine entries keep their existing
spellbook exemption.

A scheduled cast ends through `on_cast_finished`. Its payload carries the caster, target, scheduled
identity, and `actor::CastFinish`. Packages must match the scheduled identity before updating retained
work. The hook runs after the pending row is removed. An instant cast returns its result directly and
does not fire this hook. `on_cast_resolved` keeps its effect-dispatch contract.

`actor::pending_cast` reads the current Cast Handle by caster. Refusal kinds are supplied by the
owning Gate. `Other` preserves a Gate's message when no current caller needs a separate policy.
`UnlearnedSpell` and `NoLineOfSight` identify the spellbook and targeted visibility Gates.

`actor::cancel_cast_attempt(ctx, caster_guid, scheduled_id)` cancels only that scheduled cast.
`actor::expire_cast_attempt(ctx, caster_guid, scheduled_id, deadline_micros)` also requires the
caller's action deadline to have passed. They return whether they removed the cast and report
`Cancelled` or `Expired` through `on_cast_finished`. A stale identity cannot cancel a replacement.

`actor::party_facts(ctx, character_guid)` reads the Character's durable party roster. It names the
leader and every member, with nullable live position, health, and death facts. Live Units supply
their current map and Instance. An absent Unit can retain a certified partition from retained
coordination records. Pending Transfer, unknown location and departed-member fences expose no
partition. Enemy facts cover hostile creatures with current party melee, cast, threat, or control
evidence. Hostile Characters are excluded because PvP party assistance is outside this contract. An
absent membership returns `Ok(None)`. Party facts cover every Raid member, whatever the Subgroup.

A membership whose Group row is missing returns `MissingGroup`. `FightLimit` reports more than 40
members, 24 incoming melee or threat rows for one member, one pending cast for one member, or 24
aggregate enemy GUIDs. For each retained enemy, the read permits 80 threat sources, 64 control
auras, and three effects on a pending spell. Either failure returns `PartyFactsUnavailable`, so a
Package can hold party control instead of acting from an arbitrary prefix.

### Action observations

`actor::request_attack` returns `AttackStart::Armed` or `AlreadyArmed`. The latter keeps the current
melee swing timer. After either result, an in-range Character turns toward the exact admitted target when Core's facing Gate would block its swing,
unless an active movement leg still owns its position and facing. Both results mean an engagement
was accepted; neither proves a hit. The existing `actor::attack` and client operations retain their
re-arm behavior.

`actor::request_accept_quest` and `request_turn_in_quest` complete synchronously. They return
`Result<(), ActionRefusal>`, with the same core Gates and effects as the client operations. They
have no waiting, cancellation, or expiry phase. Attack requests also return `ActionRefusal`.
Its `kind` identifies actionable Gates and `detail` preserves the client message. `Other` is an
opaque refusal; Package policy must not classify its text.

`actor::movement::route_step` accepts the same arguments as `nav_step` and returns a `RouteStep`. Its `status`
is `Complete`, `Partial`, `Blocked`, or `Direct` when navigation is disabled. Complete and Partial
describe the planned route. `first_waypoint`, `expansions`, and `clipping` retain search and commit
Gate evidence. `endpoint == from` means no movement was approved, even if a complete path was
planned. A blocked search holds position. Creature callers retain `nav_step` and its existing
collision-gated fallback. `actor::movement::LEG_MAX_EXPANSIONS` is the expansion cap `route_step`
uses, so a Package can reserve that work before selecting movement.

`actor::movement::coverage_generation(ctx, map_id)` returns the active complete coverage generation when its
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

`actor::stop_movement(ctx, actor_guid)` stops the current leg where the client renders the Actor
and writes the resulting position. If the body is absent, it removes the stale leg. The caller
must check its movement ownership before stopping a leg.

`actor::restore_sessionless_body(ctx, character_guid)` returns an existing body or rebuilds a
missing one from an available Character row. It preserves released ghost state and removes the
old movement leg when it rebuilds. With no body, an absent or Transfer-fenced Character returns
`None`. The Package checks controller consent and pending Transfer Intent policy before calling.

`actor::movement::emit_creature_path(ctx, mover, points, run)` sends the mover along a checked Route
Path and writes the row. A mover on a leg starts the path from the point that
`actor::stop_movement` would stop it at, so a renewal between two advance firings does not move the
client back.

`actor::enter_sessionless_areatrigger` rechecks the current body, the `area_trigger_route` volume,
the exact existing dungeon instance lease, and a live party member in the expected partition. It
applies the imported landing and records a completed Character Relocation in the same transaction.
The returned id names that completed relocation. A repeat for the same controller generation,
AreaTrigger and destination returns the same id while the Character remains in that destination
partition. Only the latest relocation remains. A Refusal leaves the Character, Instance binding and
relocation unchanged. No Transfer Intent or Escrow is created. `area_trigger_route` exposes the
source center and containment rule for Candidate movement.

### Package Config

An Operator-tunable value, seeded by the Package and edited without a republish.
`crate::package::ensure_config_default(ctx, package, key, default)` inserts only when
the row is absent, so a Package calls it from its own ensure path on every run. The Operator edits
with the `set_package_config` reducer.

### Package fixtures (debug only)

`crate::package::fixture` exists only in a Module built with `debug_reducers`. It holds the setup and
observation steps a Package's own debug fixtures need. A release build has no such root.

| operation | does |
|---|---|
| `apply_damage(ctx, target_guid, amount, attacker_guid)` | deals main-hand damage through the real Core damage pipeline, capped one below the target's health |
| `remove_live_character(ctx, character_guid)` | removes a live Character from the world as a logout does; `on_logout` fires and the Character row stays |
| `require_no_imported_content(ctx)` | refuses when the Shard holds imported content; the temporary weather seed a fresh Module stamps does not count |
| `top_threat_target(ctx, creature_guid)` | reads the highest-threat living source on the creature's map and instance |
| `client_cast(ctx, caster_guid, spell_id, target_guid)` | casts through the same Gates a client cast passes |
| `admit_to_instance(ctx, character_guid, map_id, instance_id, party_id, request_actor)` | stages the party's instance as a dungeon entry leaves it and binds the Character to it; refuses when entry resolves to another instance |
| `record_completed_transfer(ctx, character_guid, map_id, instance_id)` | records a finished Transfer in the `game_character_shard` index |
| `declare_next_movement_tick(ctx, delay)` | makes the next creature movement tick fire once, `delay` from now; refuses unless the catch-all tick is the only movement schedule |
| `emit_move_spline(ctx, guid, start, destination, duration_ms, run, spline_id, map_id, instance_id, grid)` | emits a straight creature movement segment with the existing ten-argument fixture contract |

The root also re-exports Core setup steps such as `grant_item`, `remove_items`, `grant_xp`,
`learn_spell`, `apply_hit`, `kill_creature`, `sync_group_mirror` and `SessionActor`;
`module/src/package_fixture.rs` lists them. A fixture reads the navigation revision through
`actor::movement::inputs(ctx, map_id).imported_revision`.

Name this root only from a Package file whose first non-blank line is
`#![cfg(feature = "debug_reducers")]`. The lint refuses it anywhere else, and no exemption clears
it, because a release build would compile that file without the root.

### Package tests (test only)

`crate::package::test` exists only in a Module test build. It holds what a Package's own unit tests
need.

| operation | does |
|---|---|
| `ask_offline(event, actor, target, scripts)` | runs `scripts` in order on a fresh Runtime Script Host and returns the Script Answer as `script_binding::ask` reads it; discards Staged Effects and returns any Script Diagnostic as an error |
| `ask_artifact_offline(event, actor, target, artifact_json)` | parses a Script Artifact, selects its enabled Event Bindings in dispatch order, and runs them through `ask_offline` |
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

Core table accessors and row types are named under `crate::tables`, for example
`use crate::tables::{game_world_entity, WorldEntity};`. The catalog in
`module/src/tables.rs` lists them explicitly. Crate-root row aliases remain for Core and schema
checks; a Package uses the catalog path. `crate::CHARACTER_OWNED_TABLES` is the generated manifest
of character-keyed tables.

### Module roots

A Package may name these five roots. Each operation and type is a named declaration or re-export.

```
actor hooks package tables script_binding
```

`actor` holds Actor operations and observations. `actor::movement` holds navigation and movement
operations. `hooks` holds event payloads. `package` holds Package-owned Character setup, Package
Config, authorization and encounter operations. `tables` holds table accessors and row types.
`script_binding` holds Runtime Script dispatch.

`package::fixture` requires a file gated on `debug_reducers`. `package::test` requires a file gated
on `#![cfg(test)]`. These child paths retain the gates described above.

The marker macros listed above, `CHARACTER_OWNED_TABLES`, and generated `pkg_*` modules also remain
available at the crate root. Arbitrary `game_*` or UpperCamelCase names do not qualify.

The roots `chat`, `combat`, `creatures`, `encounter`, `faction`, `gameobject`, `group`, `helpers`,
`items`, `loot`, `nav`, `package_account`, `package_config`, `quest`, `spell`, `stats`, `terrain`,
`transfer`, `world`, and `xp` are internal and have no compatibility aliases. Call the named Actor or
Package operation and import tables from `crate::tables`. The lint names the replacement when it
knows one.

## The lint

`cargo test -p lyracore-module --test package_api_lint` reads every Rust file under
`packages/*/src/` and fails when a path reaches a crate root outside the list above. The failure
names the Package, file, line and path. Core `src/` is never linted because the Package API is a
promise core makes to Packages, not to itself. CI runs the lint with the Official Packages installed.

The scanner recognizes `crate::`, `$crate::`, and a `super::` chain that leaves the Package module.
Raw identifiers have their ordinary meaning, so `crate::r#actor` names the documented `actor`
root. The scanner uses the file path and inline modules to distinguish a crate-root escape from a
Package's own sibling or submodule. A crate-root glob such as `use crate::*` also fails because it
imports roots that the Package API does not name.

Whole-crate aliases such as `use crate as core`, `use crate::{self as core}`, and
`extern crate self as core` fail at their declaration. Spell every core dependency as
`crate::<Package API root>` so the lint can check it where it appears. `#[path]` and a `cfg_attr`
that supplies `path` also fail. `include!` fails because it can add unscanned Rust source;
`include_str!` and `include_bytes!` remain available for data. Package modules use Rust's normal
`mod.rs`, `<name>.rs`, or `<name>/mod.rs` layout, which keeps filesystem depth and `super` depth
equal.

This is a lexical compatibility check, not Rust name resolution. It does not expand macros or
follow a re-export declared in another file. Comments and string literals are stripped before the
scan, so a path quoted in a doc example is inert.

A Package that genuinely needs a path off the surface writes the reason on the line that names it:

```rust
crate::weather::normalize_intensity(kind, 0.5) // package-api: exempt the Package clamps storm intensity as weather does
```

The marker clears that line and no other. There is no global toggle, and the reason is required. A
bare marker does not clear. Every exemption is a gap in this document; raise it with the maintainers
so the surface can grow or the Package can move off it.

An exemption cannot enable a whole-crate alias or `#[path]`, because either spelling can hide
dependencies on other lines or in files the lint cannot locate. It cannot clear a gated
`package::fixture` or `package::test` path, or the bare `crate::package` module, either.

## Runtime Scripts

A Runtime Script is Lua supplied from outside the core rather than compiled into it. It reaches a
Shard only through a Package's Script Artifact; there is no upload path. Its name lets a diagnostic
identify it. `docs/runtime-script-host.md` describes the Host that runs it.

**Package Events.** A Package Event runs the same dispatch a core hook event runs, so a Package
exposes one of its own decisions to a Runtime Script without a new core seam. A Package fires its own
event and reads back a Script Answer with
`crate::script_binding::ask(ctx, event, actor_guid, target_guid) -> Option<f64>`. Core hook events
reach bound scripts on their own. A Package may only bind and fire events it ships; the artifact
parser enforces this against the artifact's own Package identity.

**Event Binding and order.** A script binds to a name from the Module's hook catalogue or to a
Package Event of the shipping Package. The author-time build refuses anything else. Several scripts
may bind to one event. Lower priority runs first and the script identifier breaks a tie, so every
Shard runs one plan in one order.

**Script Answer.** The first number returned in dispatch order is the answer. Later scripts still
run and still stage what they stage. With no answer (nothing bound, nothing returning a number, or
every script failing) the caller keeps its own fallback. That is what makes a Runtime Script an
override rather than a dependency.

**The Host.** The Runtime Script Host gives each Invocation a fresh environment and a Fuel Budget,
and contains every failure. An Invocation sees only the allowlisted standard library, the event with
its Entity Handles, and the Host Operations `heal`, `send_chat` and `grant_xp`. An Entity Handle
carries no guid and no row, so a script can neither forge one nor name an entity the Host did not
resolve. A successful Invocation commits its Staged Effects through core operations. Running out of
fuel, a misused Host Operation or any other failure produces a Script Diagnostic that records the
Runtime Script, the event, the failure kind (syntax, runtime or fuel) and a truncated message, and
discards every Staged Effect.

**Script Artifacts.** A Script Artifact holds the Package identity, the source revision, and one
whole row per script: identifier, name, Event Binding, priority, enabled state and Lua. A Runtime
Script has no base import, so the Package owns the whole row, and two Packages meeting on one row is
a collision rather than a merge. Script Artifacts and Package Deltas both live in
`packages/<name>/data/.generated/`, told apart by a top-level kind.

**Event Bindings.** Each source file binds one named function to an event, for example
`events.player.onLogin(welcome)`. The function takes the event payload and may return a numeric
Script Answer. TypeScript and Lua use the same Event Binding names. TypeScript declarations and
Lua editor definitions come from the catalogue in `datascripts/runtime-scripts/events.json`.
Login and level-up guarantee `event.player`; level-up carries its attained level in `newLevel`.
Other fields follow each event's declaration.

**Script Identities.** The toolchain records numeric IDs in the Package-root `script-ids.json`,
keyed by source-file stem. Commit it with the sources and Script Artifact. Renaming a function
keeps its identity. A source-file rename creates a new identity. Deleted entries remain reserved.
Existing Script Artifacts and legacy `@event`/`@id` directives supply migration IDs, and the
toolchain reserves recorded and legacy IDs across the enabled Package Inventory before it allocates
new ones. A conflict between recorded IDs names both owners and requires reconciling their published
identities; the compiler never silently renumbers them.

**Runtime Script Toolchain.** Bun, `typescript-to-lua`, the Lua parser, event catalogue, generated
declarations and emitter live in `datascripts/runtime-scripts/`. They run only at author time.
The emitted Lua captures the declared function and calls it with the event in one Invocation.
An Operator can install the prebuilt Script Artifact. See
[Runtime Script authoring](../packages/README.md#building-source) for examples and migration.

## Package identifier ranges

Each Import Family that lets a Package insert rows owns one Package Identifier Range.
`crates/lyracore-package-delta/src/ids.rs` holds the numbers. A band's floor sits two decimal orders
above the highest identifier a real client holds for its tables, clear of every reserved band. An
apply clears the whole band before it writes.

| Range | Family | Band | Checked against |
| --- | --- | --- | --- |
| Package Script Range | script | 100,000 to 999,999 | `game_script` identifier |
| Package Spell Range | spell | 6,000,000 to 6,999,999 | spell id, for `game_spell` and `game_spell_effect` |
| Package Item Range | items | 7,000,000 to 7,999,999 | `game_item_template.entry` |
| Package Quest Range | quest | 8,000,000 to 8,999,999 | `quest_entry` |
| Package Loot Range | loot | 9,000,000 to 9,999,999 | the loot row's own identifier |
| Package Cast Range | casts | 10,000,000 to 10,999,999 | `game_creature_spell.id` |
| Package Trainer Range | trainers | 11,000,000 to 11,999,999 | `game_trainer_spell.id` |
| Package Gossip Range | gossip | 12,000,000 to 12,999,999 | each insertable gossip table's own key |
| Package Globals Range | globals | 13,000,000 to 13,999,999 | the surrogate key of three tables |
| Package Spell Metadata Range | spellmeta | 14,000,000 to 14,999,999 | `game_spell_learn.id` |
| Package Creature Range | creatures | 15,000,000 to 15,999,999 | template `entry` and spawn `spawn_id` |
| Package Gameobject Range | gameobjects | 16,000,000 to 16,999,999 | template `entry`, trap `entry` and spawn `spawn_id` |
| Package EventAI Range | Creature-AI | 17,000,000 to 17,999,999 | three catalogue tables' `id` |

Each band after the spell band sits one whole decade above the one before it, so the millions
column says which family invented a row.

The Operator Spawn Range, 14,000,000 to 14,999,999, is not a Package band. It holds the creature and
gameobject spawn identifiers that the [Operator Content API](./operator-content-api.md) allocates.
It shares its numbers with the Package Spell Metadata Range, but a spawn identifier and a
`game_spell_learn.id` never meet. It sits below the Package Creature Range because a creature spawn
identifier must fit 24 bits.

- **Script.** No client and no import holds a Runtime Script identifier, so the band has no real data
  to clear and sits below every reserved band rather than above one. It is the whole of
  `game_script` by construction, which makes a script apply a total reconciliation.
- **Spell.** Two decimal orders above the highest real client spell and above every reserved band,
  so an inserted spell never collides with imported or fixture data. It is the worked example the
  other bands follow.
- **Quest.** `game_quest_template` and every child table (`game_quest_text` and the rest) are
  Package-owned exactly when their quest is, so one band covers the whole family.
- **Loot.** No loot table's owning entity (a creature, a gameobject or a zone) is ever
  Package-invented, so the band checks a loot row's own identifier. The four loot tables
  (pickpocket, gameobject or chest, skinning, fishing) share the band; each has its own primary-key
  space, so they cannot collide.
- **Casts.** The loot shape: the owning creature is never Package-invented. `game_creature_cast` has
  no band. Its key names a creature template, which no Package may invent, so every insert on it is
  refused.
- **Trainers.** The loot shape. The curated trainer overrides at 5,200,000
  (`CURATED_TRAINER_ID_BASE`) are a reserved band this range clears, not a Package range.
- **Gossip.** One band covers `game_npc_text`, `game_npc_text_slot`, `game_gossip_option`,
  `game_gossip_menu_profile` and `game_gossip_menu_profile_option`. `game_gossip_menu` has no band:
  its key names a creature template, so every insert on it is refused.
- **Globals.** Covers `game_graveyard_zone`, `game_createinfo_spell` and `game_createinfo_action`.
  The other four tables have no band because no Package may invent their keys:
  `game_class_level_stats`, `game_level_stats` and `game_start_position` key on a race, class and
  level the client fixes, and `game_areatrigger_teleport` keys on an `AreaTrigger.dbc` trigger id.
- **Spell metadata.** `game_spell_chain` and `game_spell_proc_event` key on a spell identifier, so an
  insert there takes the Package Spell Range. A metadata row cannot outlive the `game_spell` row it
  describes.
- **Creatures.** A creature spawn's durable guid packs the template entry and the spawn identifier
  into 24-bit fields, so the whole band must fit inside one. The seeded creature fixtures at 51,000
  to 51,999 are Fixture-Reserved Identifiers no Package may tune. `game_creature_waypoint` is not
  claimable: it names its creature by spawn guid and carries no map, so a Spatial Claim on it could
  not be routed.
- **Gameobjects.** Template `entry` and trap `entry` share one identifier space on purpose: a trap
  row describes the template of the same entry. The two gameobject pool tables are not claimable,
  because no base import writes either, so a claim on one would have no family reload to replay
  after.
- **EventAI.** Covers `game_creature_ai_broadcast_text.id`, `game_creature_ai_summon.id` and
  `game_quest_event_requirement.id`. The family's scripted definitions are not claimable: a
  definition carries a creature's whole rule set as a nested payload, which no claimed column can
  state. Reaching a creature's rules from a Package remains a named gap.
