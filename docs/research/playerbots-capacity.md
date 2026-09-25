# Playerbots capacity

The target is 1,000 active level-one Cohort bots across six starting areas. On Argus, maps 0 and 1
belong to separate World Shards, so the final population is 500 bots per World Shard. The Instance
Pool and realm-core remain part of the deployment and health checks.

## Work and limits

Navigation and repeated nearby-cell reads dominated the retained 35-bot measurements. Movement
decisions accounted for 92.09% of elapsed decision time. Three slow imported routes each exhausted
16,384 search expansions.

The changes preserve collision checks and the expansion cap. Weighted search favors progress toward
the destination. It can choose a longer safe route. Waypoint simplification probes progressively
instead of repeatedly checking every distant point behind a wall. Creature activation queries each
active spatial cell once per World tick.

Decisions admit at most 256 bots from the oldest 1,024 due rows. Up to 16 Companion Orders and 64
combat decisions receive reserved places; remaining places go to the oldest other work. Existing
paths advance separately. New and continuing path requests alternate within a budget of 32 searches
and 65,536 expansions per pass. Queue time does not count as stalled movement.

Creature search uses the map, instance, cell and creature-entry index. Character rows do not consume
its 96-creature read budget. Solo Target Claims remain in force; party members may assist each other.

## Starting areas

| Area | Map | Supported population | Final count |
|---|---:|---|---:|
| Northshire | 0 | Human Warrior, Priest, Mage | 167 |
| Coldridge Valley | 0 | Dwarf Warrior/Priest, Gnome Mage | 167 |
| Deathknell | 0 | Undead Warrior, Priest, Mage | 166 |
| Shadowglen | 1 | Night Elf Warrior/Priest | 167 |
| Valley of Trials | 1 | Orc Warrior, Troll Priest/Mage | 167 |
| Red Cloud Mesa | 1 | Tauren Warrior | 166 |

`playerbots_spawn_starting_area` accepts an area name, a count from 1 through 50 and a Cohort or Frozen
Bot Controller. Each batch uses imported race/class start positions and walkable imported ground.
A missing input refuses the whole transaction. Existing coordinate-based spawn commands retain
their map-0 behavior. No unsupported class kit is implied by these profiles.

The supported quest catalog retains the 12 Human quests and adds 179, 233, 363, 364, 456, 457, 458,
747, 752 and 788. Definitions were checked against the catalog's existing source digest. Destinations
use each quest's map. New quest admission searches within 1,500 yards; held quests retain their
existing progress and recovery policy. This is bounded starting content, not complete zone coverage.

The `starting-eastern` World Import Profile adds Deathknell to `alliance-eastern`.
`starting-kalimdor` adds Valley of Trials and Red Cloud Mesa to `alliance-kalimdor`. Terrain,
navigation and collision use the same World Import Scope. Explicit CLI assignments are:

```text
--profile-shard starting-eastern=lyracore
--profile-shard starting-kalimdor=lyracore-world-1
--profile-shard instances=lyracore-instances
```

These arguments belong to the guarded import workflow. They are not a standalone deployment recipe.
Use the installed CLI's help and the Operator procedure for the declared Realm configuration.

## Evidence so far

Measurements run on Argus under `lyracore`, against isolated private SpacetimeDB nodes. No tests or
builds run on the workstation. The managed Realm remains unchanged during implementation.

| Synthetic travel | Bots | World tick p95 | Decision interval p95 |
|---|---:|---:|---:|
| Bounded planning, original simplification | 100 | 562 ms | 1.195 s |
| Progressive simplification | 100 | 189 ms | 1.004 s |
| Progressive simplification, complete capture | 500 | 330 ms | 1.005 s |
| Simultaneous World A | 500 | 346 ms | 1.005 s |
| Simultaneous World B | 500 | 352 ms | 1.004 s |

The complete 500-bot capture contains 23,250 decisions over 46.29 seconds. All 500 bots moved more
than 50 yards from their starting x position. No pass exceeded the search or expansion budget.
The simultaneous run moved all 1,000 bots, split between two private nodes. Its complete decision
windows span 46.33 and 44.73 seconds. These tests use synthetic routes around a wall and do not
establish imported-world combat capacity. Temporary Core timers were removed after these runs;
Package phase and path profiling remains opt-in.

SpacetimeDB log reads cap the returned bytes. The first 250- and 500-bot captures were truncated and
are excluded from full-window claims. The test now captures overlapping log windows and fails if
it loses a checkpoint. Raw captures, failed experiments and revision records are retained under
`/home/lyracore/deploy-logs/argus-capacity-20260925` on Argus.

## Release and acceptance gates

The Runner end-appends `path_pending` with default false. A populated upgrade test preserves every
existing Runner field and Character, then resumes movement. Existing rows remain in the continuing
movement queue until their next pass. The field and its index require human review under
`docs/danger-zones.md` before shipping. Gateway bindings must match the generated Core schema.

Before deployment, run the required importer, Module, Gateway and shared tests, `cargo build`, and
CLI preflight under the build account. Verify the owned-client terrain and navigation inputs for
all six areas. Record skipped checks explicitly. Preserve the full configured topology, perform
Package replay and post-publish repair, and reconcile the managed services through the CLI.

For imported-world acceptance, use fresh recorded GUIDs at totals 100, 250, 500 and 1,000, divided
evenly between the two World Shards. Record decision spacing, writer occupancy, World tick time,
memory, disk growth, refusal reasons, movement and quest progress, combat effects, deaths and
recovery. Stop increasing population on a failed timing or progress gate. At the passing target,
run a sustained session and record an unmodified client observing movement and combat.

Do not count spawned or Frozen bots as active load. Do not infer client observations from durable
state. PB012 attended acceptance and old-code retirement remain separate gates.
