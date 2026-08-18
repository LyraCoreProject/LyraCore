# Issue #22 — mounts: riding skill, mount auras, mounted speed, dismount rules

Source: `gh issue view 22`. The issue is maintainer-authored and authoritative: 34 user stories,
an Implementation Decisions block, and a Testing Decisions block. Read the issue, then this file,
then your own ticket, before touching code.

One branch `feat/issue-22-mounts`, one PR at the end. No shippable intermediates.

## Decisions (maintainer-approved, do not relitigate)

- **A mount is a normal cancelable self aura.** `A_MOUNTED` is the state of record.
  `WorldEntity.mount_display_id` is a *projection* for CREATE/VALUES, never a second state machine.
- **Riding trainer wiring is IN scope.** The Riding skill must be learnable from the existing
  riding-trainer NPC concept (`crates/lyracore-shared/src/trainer.rs::trainer_type::MOUNTS`), not
  only seeded. Fixtures may still seed the skill directly for headless tests, but the trainer path
  gets its own coverage.
- **Damage does not dismount.** This is DBC interrupt-flag driven through the existing
  `breaks_on_damage` machinery. Verify the imported data; write no mount-specific damage code.
- **Indoor enforcement design** (settled from a CMaNGOS research pass):
  - The current indoor test in `crates/lyracore-shared/src/vmap.rs` (~line 205) is wrong on both
    counts. Fix it to mangos's rule: a found WMO group is **OUTDOOR only when MOGP flag `0x8000`
    is set**. Today's `mogp_flags & 0x2000 != 0` is the wrong bit *and* the wrong polarity.
  - Add a short `AREA_PROBE_DOWN_YD` (~10 yd) for area queries. The 200 yd floor probe is a
    ray-cast budget, not an area-query budget.
  - Hoist the duplicated `game_vmap_generation.by_map_state` scan (`vmap_enabled` and `fetcher`
    both scan it today).
  - Runtime shape: a heartbeat check for mounted players only, gated at 100 ms off the *client*
    movement clock (`move_time_ms / 100 != old_move_ms / 100`) — the same stateless gate shape as
    the existing 1 Hz rest/breath gates in `module/src/world.rs::apply_movement_update`
    (lines ~1365 and ~1374). Taxi passengers never reach it:
    `crate::taxi::movement_is_suppressed` already early-returns upstream.
  - Cheap pre-reject: a **module-private per-cell indoor-presence table** keyed
    `(generation_id, cell_key)`, computed for free inside `verify_vmap_generation`, which already
    decodes every staged chunk exactly once. The heartbeat does one indexed find; only cells that
    actually contain indoor geometry pay a raycast. **Missing row means outdoors (fail open).**
    No manifest or digest change.
  - The shared indoor check is also called from the teleport path, which writes position outside
    the heartbeat.
  - **Everything fails open** when `vmap_enabled` is off or there is no ACTIVE generation. The
    mount feature must be fully shippable and correct with vmap off.
- **`E_DISMOUNT` is a typed instant effect**, translated at import from a raw `DISPEL_MECHANIC`
  effect whose parameter is the mount mechanic. No generic mechanic-dispel module. No branch on
  spell 1604 or on any spell name at runtime.
- **The full Dazed proc is out of scope.** Only the synthetic Dazed spell (`A_MOD_SPEED` +
  `E_DISMOUNT`) exists here, for headless verification.

## Taxonomy slots (settled — claim these, do not renumber)

Read `module/src/spell/taxonomy.rs` before writing. Verified free as of this ticket folder:

| Const | Value | Note |
|---|---|---|
| `E_DISMOUNT` | `0x23` | `0x01`–`0x22` are taken (`0x20`–`0x22` = tame/feed-pet/duel). **`0x1E` is reserved for `E_SUMMON_PORTAL` — do not use it.** |
| `A_MOUNTED` | `0xB3` | Highest live `A_*` is `A_MOD_DETECT_RANGE` `0xB2`; `A_FLAG` `0xBE` is the inert marker. |
| `P_DISPLAY_ID` | `14` | `13` is `P_GAMEOBJECT_ENTRY`. `p0` = the resolved creature display id. |

`SPEED_MOUNTED = 3` already exists in the `SPEED_*` block and is currently unused. Do not add a
second mounted-speed concept.

## The removal-convergence pattern (this is the whole design)

The module has **no single aura-deletion boundary**. Auras are deleted in `do_cancel_aura`,
the expiry reap and channel-end passes in `scheduler.rs`, dispel, death cleanup and
`spellbook.rs`. The codebase already solved this: each site collects a
`aura_moves_vitals(eff_kind, eff_p0)` / `aura_moves_sheet(...)` predicate result while deleting,
then calls `recompute_vitals(ctx, guid)` / `recompute_sheet(ctx, guid)` after the deletes.

**Mounts follow that pattern exactly**: a `mount_aura_moves_mount(eff_kind)` predicate plus a
`recompute_mount(ctx, guid)` that reads the target's aura set and derives the projection. Because
it is a recompute and not a delta operation, story 34 (every dismount trigger is idempotent) holds
by construction, and story 29 (expiry cleanup matches manual cancellation) is free.

Do not invent a second convergence mechanism.

## Seams

- **Aura apply:** `module/src/spell/cast/targeting.rs::aura_apply` — the single `game_aura`
  insertion site, pinned by a test in `module/src/spell/stacking.rs`.
- **Aura removal:** the sites listed above. Add the recompute call beside the existing
  `revitalize` / `resheet` calls, not in a new place.
- **Cancel entry point:** `gw.rs::gw_cancel_aura` → `spell/scheduler.rs::do_cancel_aura`.
- **Item use:** `module/src/items/ops.rs::apply_item_use`; `spell_is_recall_home` (line ~391) is
  the non-consuming-cast precedent to mirror — by effect kind, with no item-entry allowlist.
- **Riding data:** `module/src/skilldata.rs::SkillAbility` (`spell_id`, `skill_line`, `min_skill`,
  `race_mask`, `class_mask`) joined against `module/src/skill.rs`'s `game_player_skill`
  (`by_character`). `min_skill` carries the 75 / 150 tiers.
- **Trainer:** `module/src/trainer.rs` (`trainer_buy_check`, `validate_trainer_interaction`,
  `apply_trainer_buy`), `gateway/src/world/handlers/trainer.rs`, and the fixture NPC precedent
  `module/src/seed/fixtures.rs::profession_trainer_template`.
- **Liquid gate:** `crates/lyracore-shared/src/env.rs::is_submerged`, the same predicate player
  breath uses. Do not create a second water-position interpretation.
- **Speed relay:** `run_speed_packet` in `gateway/src/stdb/subscriptions.rs` (call sites ~1365,
  ~1412, ~1444) plus the GM `.speed` `run_speed_mult_bp` diff at ~2267. `BASE_RUN_SPEED = 7.0`.
- **VALUES:** `gateway/src/codec/values.rs`. `build_taxi_presentation_values` (~line 187) couples
  mount display and the taxi unit flag *by design*; the sibling single-field builders
  (`build_health_values`, `build_unit_flags_values`, …) are the shape to copy for a standalone
  land-mount display builder.
- **Taxi boundary:** `module/src/taxi.rs` owns `mount_display_id` during a flight.
  `TaxiGateDenied::PlayerAlreadyMounted` already blocks taxi-while-mounted. Land-mount cleanup
  must never clear an active flight.

## Execution DAG

```
T1 (tracer, opus) ── blocks everything
 ├── T2 (sonnet)  ─┐
 ├── T4 (sonnet)  ─┤ parallel
 ├── T5 (sonnet)  ─┤
 └── T3 (opus, LATE — must land after / rebase onto the #195 PR)
                   └── T6 (opus, last, integrates + files the PR)
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|--------|-------|-------------|----------------|
| T1 | Taxonomy, aura wiring, shared dismount recompute (tracer) | Opus | ~200k | `module/src/spell/taxonomy.rs`, `module/src/spell/cast/targeting.rs`, `module/src/spell/cast/resolve.rs`, `module/src/spell/scheduler.rs`, `module/src/spell/spellbook.rs`, `module/src/mount.rs` (new), `module/src/lib.rs` |
| T2 | Riding gate, trainer wiring, item path, fixtures | Sonnet | ~170k | `module/src/mount.rs` (gate region), `module/src/items/ops.rs`, `module/src/skill.rs`, `module/src/trainer.rs`, `module/src/seed/`, `module/src/seed.rs`, `gateway/src/world/handlers/trainer.rs` |
| T3 | Dismount convergence, vmap indoor correctness, indoor-presence table | Opus | ~200k | `module/src/world.rs`, `module/src/vmap.rs`, `crates/lyracore-shared/src/vmap.rs`, `module/src/spell/cast/` (cast-start hook), `module/src/combat/` (attack-start hook), `module/src/mount.rs` (indoor region) |
| T4 | Mounted-speed fold, force-run-speed relay, standalone mount VALUES | Sonnet | ~150k | `gateway/src/codec/values.rs`, `gateway/src/codec/tests.rs`, `gateway/src/stdb/subscriptions.rs`, `module/src/stats.rs` (speed fold) |
| T5 | Importer: Mounted aura, `E_DISMOUNT`, mounted-speed normalization, riding data | Sonnet | ~130k | `importer/src/spell.rs`, `importer/src/dbc.rs` |
| T6 | Integration, verification doc, schema/architecture docs, files the PR | Opus | ~150k | `docs/schema.md`, `docs/architecture.md`, `docs/mount-verification.md` (new), reconciliation across all of the above |

## Shared rules for every ticket

- Pull the branch, work, push. Do not open a PR; T6 files the single PR.
- Runtime never branches on a spell id or a spell name. Import may name spells — `importer/` is
  the one place allowed to, and it already does so extensively.
- Every gate runs **before** item consumption, aura creation, field changes, speed changes,
  cooldowns and combat changes (story 33). A refused mount is atomic.
- Land-mount work never touches taxi authority. If you find yourself reading a taxi flight row
  from mount code, stop and re-read the taxi boundary above.
- Comments: one or two lines of rationale, matching surrounding naming and idiom. No issue
  numbers in comments. Do not copy the legacy essay-comment density.
- Green before push: `cargo fmt`, `cargo clippy`, `cargo test` for every crate you touched.

## Testing shape

The issue names the pinned build-5875 wire harness as the authoritative automated seam. **That
harness is a separate maintainer-owned repository** (`LyraCoreProject/wire-harness`, pinned by
`.wire-harness-rev`, currently `v0.1.0-alpha.3`). It is not a workspace member and a LyraCore pull
request cannot add a scenario to it. Do not patch the ignored cache under
`.lyracore/wire-harness/<sha>/`.

The established substitute is `docs/taxi-flight-verification.md`: a repo doc that specifies the
adapter scenario for the *next* harness release, plus a `needs-live-eyeball` marker on the PR.
T6 writes the mount equivalent. Every other ticket carries its behavior in module and gateway
tests in the local idiom (existing spell-gate, aura-cancellation, combat-fold and movement-edge
prior art; the gateway codec tests extend the taxi mount-display CREATE and VALUES prior art).

Tests assert observable outcomes: item counts, reducer results, aura rows, projected world-entity
fields, engagement state, decoded packets. They do not assert internal call order and do not
re-implement the taxonomy.
