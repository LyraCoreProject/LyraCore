# AzerothCore playerbots as a LyraCore architecture reference

## Source pin and scope

This note examines `mod-playerbots/mod-playerbots` at commit
[`b949b50bfcdd4fab937781bac2d7765e39330e4b`](https://github.com/mod-playerbots/mod-playerbots/commit/b949b50bfcdd4fab937781bac2d7765e39330e4b),
committed 2026-09-04 and read on 2026-09-08. The fixed tree contains 1,374 C++ headers and source
files with 243,560 lines. Counts came from `find` and `wc` over `src/` in a shallow clone.

The question is narrow: what decision architecture should LyraCore take for autonomous leveling and
questing, and for companions in a human-led party? This is source research, not a claim that every
upstream feature works well. Upstream describes itself as under development, warns that its docs can
be stale, and requires a custom AzerothCore fork rather than stock AzerothCore
([README lines 33-49](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/README.md#L33-L49),
[lines 60-70](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/README.md#L60-L70)).
The repository identifies its license as GPL-2.0
([LICENSE](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/LICENSE)).

LyraCore's comparison point is its current architecture: the Module owns durable state, gameplay
rules, and periodic work; the Gateway owns protocol and cross-shard coordination
([architecture](../architecture.md#2-what-runs-where--the-authority-boundary-precisely)). A Package is
trusted Rust compiled into that Module, with a versioned Package API
([Package API](../package-api.md)).

## The decision kernel

The useful part of this repository is a compact utility scheduler with a small authoring grammar.
It has five main pieces.

1. A `Strategy` contributes default actions, trigger-to-action rules, multipliers, and action-node
   factories. It also carries role flags such as tank, damage, heal, ranged, and melee
   ([Strategy.h lines 25-86](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Strategy/Strategy.h#L25-L86)).
2. A `Trigger` checks one condition on its own interval and can carry an event. A `TriggerNode` maps
   it to one or more named actions with relevance scores
   ([Trigger.h lines 15-86](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Trigger/Trigger.h#L15-L86)).
3. An `Action` has separate useful and possible checks, then executes. An `ActionNode` adds
   prerequisites, alternatives, and continuers to those supplied by the action itself
   ([Action.h lines 48-142](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Action/Action.h#L48-L142)).
4. A `Multiplier` adjusts an action's relevance and can reduce it to zero
   ([Multiplier.h lines 15-22](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Multiplier.h#L15-L22)).
5. A `Value<T>` is a named fact. Calculated values cache by an interval; manual values retain state
   in memory
   ([Value.h lines 32-118](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Value/Value.h#L32-L118),
   [lines 321-342](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Value/Value.h#L321-L342)).

At initialization, the engine flattens all active Strategies into one trigger list, multiplier list,
and action-node factory map
([Engine.cpp lines 117-133](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L117-L133)).
Each decision tick checks due triggers, evaluates each shared trigger once, pushes its handlers, then
pushes Strategy defaults
([Engine.cpp lines 442-512](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L442-L512)).
The queue picks the highest relevance. It deduplicates by action name and raises the stored score
when a later proposal is higher
([Queue.cpp lines 11-27](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Script/WorldThr/Queue.cpp#L11-L27),
[lines 79-103](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Script/WorldThr/Queue.cpp#L79-L103)).

Execution has useful failure behavior. Multipliers run before expensive possibility checks.
Prerequisites are queued ahead of their parent. A failed or impossible action queues alternatives.
A successful action queues continuers and ends the tick
([Engine.cpp lines 180-240](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L180-L240)).
The loop bounds work as `queue size * iterationsPerTick`, and old candidates expire
([Engine.cpp lines 157-164](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L157-L164),
[lines 246-256](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L246-L256)).

This kernel is worth adapting almost directly. It is a better fit for the first release than a
general planner. Quest, follow, heal, flee, interrupt, eat, train, and repair decisions naturally
fit trigger, score, prerequisite, fallback, and follow-up rules.

Two details should change in the Rust translation. First, action execution returns only `bool`, even
though the outer API recognizes unknown, impossible, useless, failed, and successful outcomes
([Engine.h lines 24-31](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.h#L24-L31),
[Engine.cpp lines 310-350](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L310-L350)).
LyraCore should carry a typed action outcome with the core Refusal or movement failure. A fallback can
then depend on why an action failed, and diagnosis can state the real cause.

Second, queue identity is only the action name. When two triggers propose the same action for
different targets or events, the queue keeps the first basket and changes only its relevance. This
follows from `updateExistingBasket`, which never replaces the event
([Queue.cpp lines 64-77](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Script/WorldThr/Queue.cpp#L64-L77)).
The Rust candidate key should include the action kind and target or event identity. Equal scores need
a fixed tie-breaker, so replay produces the same decision.

## State, goals, and getting unstuck

Upstream has three top-level AI states: combat, non-combat, and dead. Each owns a separate engine.
Death clears transient targets before switching; resurrection switches back to non-combat
([PlayerbotAI.cpp lines 1483-1534](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotAI.cpp#L1483-L1534)).
This is a useful tactical partition. It should remain derived from durable Character state rather
than become a second authoritative state machine.

The stronger lesson is `TravelTarget`. It retains a destination and point across ticks, tracks
prepare, travel, work, cooldown, and expired states, records two retry counts, and owns a deadline
([TravelMgr.h lines 744-843](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Mgr/Travel/TravelMgr.h#L744-L843)).
Travel validity changes status when the destination becomes inactive, the bot arrives, or the
deadline passes
([TravelMgr.cpp lines 1548-1667](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Mgr/Travel/TravelMgr.cpp#L1548-L1667)).
Quest destinations stay active only while their underlying quest or objective needs work
([TravelMgr.cpp lines 3942-3967](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Mgr/Travel/TravelMgr.cpp#L3942-L3967)).

Movement uses a stable offset derived from the bot and destination, which prevents the destination
point from changing every tick. A failed move increments the target's movement retry count. More
than five failures move it to cooldown
([MoveToTravelTargetAction.cpp lines 76-123](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Ai/Base/Actions/MoveToTravelTargetAction.cpp#L76-L123)).
The lower movement layer accepts normal, incomplete, and shortcut paths for combat movement
([MovementActions.cpp lines 824-843](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Ai/Base/Actions/MovementActions.cpp#L824-L843)).
LyraCore should preserve path completeness in the action outcome. An incomplete path can justify a
short retry, while no path should cool down that goal and try another objective or destination.

Goal retention has two limits. The chooser says it is mainly priority plus randomness and marks
itself for a smarter rewrite. It uses several independent random branches, then has a 90 percent
chance to continue the current destination
([ChooseTravelTargetAction.cpp lines 37-42](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Ai/Base/Actions/ChooseTravelTargetAction.cpp#L37-L42),
[lines 88-177](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Ai/Base/Actions/ChooseTravelTargetAction.cpp#L88-L177)).
The result can look purposeful while still making poor choices.

It also retains the goal only in memory. `TravelTargetValue` is a `ManualSetValue` with no save
override
([TargetValue.h lines 70-78](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Ai/Base/Value/TargetValue.h#L70-L78)).
The base serializer emits `?`, which the context omits from saved data
([Value.h lines 32-41](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Value/Value.h#L32-L41),
[AiObjectContext.cpp lines 60-83](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/AiObjectContext.cpp#L60-L83)).
Logout or restart therefore loses travel purpose. LyraCore can improve on the reference with one
durable goal row per bot Character that holds goal kind, quest and objective identity, destination,
phase, deadline, retry count, last typed failure, and the catalogue revision used to choose it.

Broad stuck recovery is too slow for the stated pain. Autonomous bots trigger reset after five
minutes without position change, and stronger checks wait ten to fifteen minutes. Both checks turn
off when a real player controls the bot
([StuckTriggers.cpp lines 13-58](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Ai/Base/Trigger/StuckTriggers.cpp#L13-L58),
[lines 60-141](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Ai/Base/Trigger/StuckTriggers.cpp#L60-L141)).
LyraCore should detect lack of progress per goal and movement leg within seconds and a small number
of attempts. It should keep companion recovery active, with follow distance and leader movement as
its evidence.

## Autonomous bots and human-led companions

Upstream supports both modes through the same action grammar, with different context and active
Strategies. A real player master or a party containing one forces full activity instead of the
background activity rotation
([PlayerbotAI.cpp lines 4669-4694](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotAI.cpp#L4669-L4694)).
When group leadership changes, the bot chooses a new master, resets Strategies, and enables follow
outside battlegrounds
([PlayerbotAI.cpp lines 413-469](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotAI.cpp#L413-L469)).
It also watches both bot and master packet streams and maps packets to actions
([PlayerbotAI.cpp lines 167-228](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotAI.cpp#L167-L228)).

LyraCore does not need synthetic World Sessions or packet observation inside the Package. Human-led
party intent is already durable realm state. The bot decision pass should read party membership,
leader, selected target, threat, health, auras, and position from Module tables. Cross-shard party
operations remain Group Intents for the Gateway. Ordinary combat, movement, spell, item, quest, and
loot operations stay in the Module.

Role assignment is concrete and useful, though its exact rules are WotLK-specific. The factory maps
class and talent tab to tank, healer, or damage roles
([AiFactory.cpp lines 141-188](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Factory/AiFactory.cpp#L141-L188)).
Role-bearing Strategies then affect targeting and positioning. The main tank uses an explicit party
flag first and otherwise chooses the first live tank
([PlayerbotAI.cpp lines 2408-2427](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotAI.cpp#L2408-L2427)).
LyraCore should make the party role an explicit durable choice with a deterministic default from
vanilla class, talents, spells, and equipment. Tactics can then compose a role Strategy with class
and situation Strategies. Do not copy WotLK talent-tab mappings.

## Scheduling, population, and performance

Several mechanisms are proven in source and worth adapting.

- A per-bot delay gates the decision loop. `YieldThread` adds a deterministic zero to 200 ms offset
  from the guid to spread work
  ([PlayerbotAIBase.cpp lines 51-60](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/PlayerbotAIBase.cpp#L51-L60)).
- Trigger and Value intervals avoid repeating expensive reads every tick
  ([Trigger.cpp lines 35-50](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Trigger/Trigger.cpp#L35-L50)).
- The random-bot manager updates and logs in only a configured batch, with separate initialization
  pacing
  ([RandomPlayerbotMgr.cpp lines 298-327](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/RandomPlayerbotMgr.cpp#L298-L327),
  [lines 404-450](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/RandomPlayerbotMgr.cpp#L404-L450)).
- Background activity can scale down against measured world update time. Combat, instances, and
  human-led parties remain active
  ([PlayerbotAI.cpp lines 4595-4615](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotAI.cpp#L4595-L4615),
  [lines 4788-4812](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotAI.cpp#L4788-L4812)).
- A separate world-thread queue caps itself at 10,000 operations, processes 100 every 50 ms, reports
  queue pressure and slow work, and drops new work at capacity
  ([PlayerbotWorldThreadProcessor.h lines 107-154](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Script/WorldThr/PlayerbotWorldThreadProcessor.h#L107-L154),
  [PlayerbotWorldThreadProcessor.cpp lines 33-65](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Script/WorldThr/PlayerbotWorldThreadProcessor.cpp#L33-L65)).

For SpacetimeDB, deterministic scheduling and bounded scheduled batches should replace process-local
delays. A due row should carry the Character guid and next decision time. Each scheduled pass should
claim a fixed number in stable key order, run at most a fixed candidate and action budget, and write
the next due time in the same transaction as the action outcome. Combat and human-led companions need
shorter intervals. Idle autonomous Characters can use coarse intervals. An overloaded pass should
leave excess due rows for the next run and record lag; silently dropping durable work would be the
wrong translation of the upstream queue.

Upstream performance instrumentation measures triggers, Values, actions, random-bot work, and total
time, with optional call stacks
([PerfMonitor.h lines 25-70](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Debug/PerfMonitor.h#L25-L70)).
LyraCore should record counts and bounded timings by decision phase plus queue lag, candidates
considered, chosen action, typed outcome, goal age, and consecutive no-progress count. The upstream
README claims thousands of bots, but this tree supplies no benchmark result that proves that claim.

## Automatic progression support

The upstream `PlayerbotFactory` is useful as a checklist for keeping autonomous bots playable. Its
refresh path learns default skills and available spells, chooses talents, equips items, supplies
bags, ammo, food, potions, reagents, and consumables
([PlayerbotFactory.cpp lines 679-829](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Factory/PlayerbotFactory.cpp#L679-L829)).
Trainer spells still pass the trainer's own `CanTeachSpell` check before learning
([lines 3213-3257](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Factory/PlayerbotFactory.cpp#L3213-L3257)).
Equipment replacement uses a score and requires a 20 percent improvement during incremental refresh
([lines 2345-2389](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Factory/PlayerbotFactory.cpp#L2345-L2389)).

Adapt these as explicit upkeep actions through existing vanilla Gates. The Operator can choose
Package Config for free training, gear floor, and supply replenishment. Each action should preserve
normal proficiency, level, class, equipment, inventory, and uniqueness rules. Keep quest XP and
credit tied to real gameplay outcomes. The full upstream refresh routine directly sets level, clears
quests and inventory, and can complete prerequisite or attunement quests
([lines 615-677](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Factory/PlayerbotFactory.cpp#L615-L677),
[lines 3575-3603](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Factory/PlayerbotFactory.cpp#L3575-L3603)).
That routine is unsuitable as one LyraCore operation.

## Direct adaptation and coupling

There are two different port proposals.

Translating the selection kernel is feasible and likely the best starting point. A reproducible
upper-bound inventory is 2,967 lines at this pin, counted with `wc -l` across these files:

- `src/Bot/Engine/{Engine,AiObject,AiObjectContext}.{h,cpp}`
- `src/Bot/Engine/Action/Action.{h,cpp}`
- `src/Bot/Engine/Strategy/Strategy.{h,cpp}`
- `src/Bot/Engine/Trigger/Trigger.{h,cpp}`
- `src/Bot/Engine/Value/Value.{h,cpp}`
- `src/Bot/Engine/Multiplier.h`
- `src/Bot/Engine/WorldPacket/Event.{h,cpp}`
- `src/Script/WorldThr/Queue.{h,cpp}`

That count includes comments, host-facing base objects, context serialization, and generic Value
helpers. It is not an estimate of the Rust result. The selection algorithm itself is much smaller.
A Rust translation would still be a small rewrite because upstream objects retain `PlayerbotAI*`,
`Player*`, `Unit*`, and `AiObjectContext*` directly
([AiObject.h lines 13-39](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/AiObject.h#L13-L39)).
Its events embed `WorldPacket` and `Player*`
([Event.h lines 10-44](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/WorldPacket/Event.h#L10-L44)).
The translation should keep the grammar and execution order while replacing pointers, strings, and
dynamic casts with Rust enums and durable identifiers.

Porting the whole runtime would be a different project. It creates synthetic WotLK `WorldSession`
objects and routes packet queues through AzerothCore opcode handlers
([PlayerbotMgr.cpp lines 198-218](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotMgr.cpp#L198-L218),
[lines 259-274](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/PlayerbotMgr.cpp#L259-L274)).
Actions call AzerothCore `Player`, `Group`, `MotionMaster`, `PathGenerator`, object stores, spell
manager, threat manager, and packet handlers throughout the tree. The code also assumes WotLK
talents, death knights, LFG, vehicles, glyphs, Outland, Northrend, and WotLK raids. LyraCore targets
an unmodified 1.12.1 client, uses durable relational state, and has no in-process AzerothCore object
graph. A whole-runtime port would first recreate the host it expects, then replace it. That brings
little advantage over writing behavior against LyraCore's existing actor, combat, nav, quest, item,
group, and transfer operations.

## Verification and diagnosis

The source has useful operational diagnosis. The engine can log every trigger, queued candidate,
multiplier elimination, prerequisite, result, and current Value set
([Engine.cpp lines 143-149](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L143-L149),
[lines 613-682](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L613-L682)).
Travel path attempts have optional CSV output with path type and geometry
([TravelMgr.cpp lines 779-814](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Mgr/Travel/TravelMgr.cpp#L779-L814)).
These answer "what did the bot see and choose?", which LyraCore currently needs.

The verification base is weaker. The main CI workflow compiles release builds against the custom
fork on three Linux compiler images
([core_build.yml lines 13-78](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/.github/workflows/core_build.yml#L13-L78)).
A separate job runs formatting and `cppcheck`
([codestyle_cpp.yml lines 20-41](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/.github/workflows/codestyle_cpp.yml#L20-L41)).
I found no automated behavioral tests in the fixed tree. The engine's `testMode` writes action lines
to `test.log`; it does not assert outcomes
([Engine.cpp lines 135-140](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L135-L140),
[lines 635-645](https://github.com/mod-playerbots/mod-playerbots/blob/b949b50bfcdd4fab937781bac2d7765e39330e4b/src/Bot/Engine/Engine.cpp#L635-L645)).
Maturity and breadth therefore do not prove stable decisions.

LyraCore should test the translated kernel as a pure deterministic calculation, then test action
outcomes through Module operations. Scenario tests should cover quest objective retention, mixed
objective quests, no-path cooldown and reselection, no-progress detection, interruption by combat,
resumption after combat, leader movement, tank target choice, healer triage, supply replenishment,
and restart with a durable goal. A small Headless Client suite can verify the final party behavior
through the real 1.12.1 protocol.

## Adopt, adapt, avoid

| Decision | Upstream reference | LyraCore choice |
| --- | --- | --- |
| Strategy composition | Strategies register triggers, defaults, multipliers, and action nodes | Adopt the grammar in Rust for class, role, autonomous, companion, and situation Strategies. |
| Candidate selection | Highest relevance after multipliers | Adopt with integer scores, a stable tie-breaker, and candidate identity that includes target. |
| Action graph | Prerequisites, alternatives, continuers | Adopt. Store the typed outcome that chose each edge. Bound graph depth and work per pass. |
| Derived facts | Named Values with timed caches | Adapt to typed derived reads cached within a pass or by durable revision. Do not build a global string registry. |
| AI state | Combat, non-combat, dead engines | Adapt as a derived tactical mode. Durable Character state remains authoritative. |
| Quest and travel purpose | `TravelTarget` with phase, validity, deadlines, retries | Adopt the state shape and make it durable. Retain quest, objective, destination, progress, and failure. |
| Target choice | Layered random branches and 90 percent continuation | Avoid. Score eligible goals deterministically, then use saved random state only for equal valid choices. |
| Movement failure | More than five failed moves cool down a target | Adapt to typed path outcomes, short bounded retries, per-destination backoff, and prompt reselection. |
| General stuck reset | Five to fifteen minute heuristics, disabled for human masters | Avoid. Detect progress against the retained goal and keep recovery active for companions. |
| Party roles | Class and WotLK talent tabs imply role; Strategies carry role flags | Adapt to explicit durable role plus vanilla-derived default. |
| Human-led control | Master and packet observation select follow and party behavior | Adapt to durable party state and Module events. Use Group Intents only for realm-core operations. |
| Population pacing | Staggered updates, fixed batches, activity scaling | Adopt in scheduled Module batches with stable ordering, due-row lag, and no silent work loss. |
| Training, gear, supplies | Factory refresh directly grants them | Adapt as separate Operator-configured upkeep actions through vanilla Gates. |
| Full C++ runtime | 243,560 lines tied to the custom WotLK fork | Avoid. Its host objects, protocol path, expansion rules, and process-local state do not match LyraCore. |
| Verification | Compile/static CI plus rich logs, no behavioral suite found | Adopt the observability shape and add deterministic behavior, Module, and Headless Client tests. |

The practical recommendation is a targeted rewrite with the AzerothCore decision grammar. Translate
the queue, scoring, Strategies, triggers, Values, action graph, and retained travel state into a
small typed Rust core. Implement each action through LyraCore's existing gameplay operations. This
keeps the part that explains why a bot chose something and discards the WotLK host coupling that
would make a direct runtime port larger and harder to verify.
