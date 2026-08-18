# T4 — mounts: mounted-speed fold, force-run-speed relay, standalone mount VALUES

Parent: issue #22. **After T1. Parallel with T2, T3, T5.**
Model: sonnet. Estimated size: ~150k tokens.

## Problem

`SPEED_MOUNTED = 3` exists in the taxonomy and is unused. Nothing folds a mounted-speed aura into
effective run speed, and nothing tells the client about it — player movement is
client-authoritative, so a server-side field alone never speeds anyone up.

The mount display has the same gap. `mount_display_id` reaches the client only through
`build_taxi_presentation_values`, which deliberately couples the display with the taxi unit flag.
A land mount needs its own single-field relay, and the subscription diff must not fire both.

## Delivery

**1. Mounted-speed fold** (module side). An `A_MOD_SPEED` aura with `SPEED_MOUNTED` contributes to
effective run speed **only while the target has an active `A_MOUNTED` aura**. Dismounting returns
the player to base speed plus any still-active ordinary `SPEED_MOVE` modifiers — never a blind
force back to base. Base-point normalization produces the nominal integer 60% and 100% from the
vanilla stored 59 and 99. Slow mounts are +60%, fast mounts +100%.

**2. Standalone mount VALUES builder.** Add a single-field `UNIT_FIELD_MOUNTDISPLAYID` builder to
`gateway/src/codec/values.rs`, in the shape of the sibling single-field builders
(`build_health_values`, `build_unit_flags_values`, `build_dynamic_flags_values`, …). Do **not**
extend `build_taxi_presentation_values` (~line 187) — taxi presentation keeps updating the display
and the taxi unit flag atomically, which is exactly why it cannot serve a land mount.

**Crash-class rule: a partial VALUES mask must not include `OBJECT_FIELD_TYPE`.** Assert it.

**3. Subscription diffing.** In `gateway/src/stdb/subscriptions.rs`, relay a
`mount_display_id` change on the world-entity diff, in the same place as the existing per-field
diffs (`player_bytes_2`, `player_flags`, `power`, `run_speed_mult_bp`, ~lines 2240–2275). It must
**not double-fire with the taxi presentation relay**: a taxi activation or landing already emits
its own coupled update, so the land-mount diff has to exclude the taxi-owned transition. State how
you distinguish them in your report.

**4. Force-run-speed relay.** Extend the existing `run_speed_packet` path (call sites ~1365,
~1412, ~1444 in `subscriptions.rs`; `BASE_RUN_SPEED = 7.0`, shared with the GM `.speed`
`run_speed_mult_bp` diff at ~2267) so a change to relevant move-speed auras, mounted-speed auras,
**or mounted-state activation and removal** recomputes and relays the effective client run speed.

**5. CREATE.** The existing player CREATE encoding already emits nonzero `mount_display_id`. Reuse
it; a reconnecting mounted player must rebuild with the mount visible, and so must an observer's
peer CREATE. Confirm a warm shard handoff keeps the aura and the projection coherent (story 7).

## Acceptance criteria

Covers stories 4, 5, 6, 7, 8, 9, 10, 11, 30.

- [ ] A slow-mount fixture yields exactly +60%; a fast mount yields exactly +100%.
- [ ] The mounted-speed aura contributes nothing while `A_MOUNTED` is absent.
- [ ] Dismounting with an ordinary `SPEED_MOVE` buff still up returns to base plus that buff, not
      to bare base.
- [ ] Mounting and dismounting each emit exactly one `SMSG_FORCE_RUN_SPEED_CHANGE` with the
      expected value.
- [ ] Mounting emits a single-field mount-display VALUES update whose mask excludes
      `OBJECT_FIELD_TYPE`. Dismounting emits the same shape with display 0.
- [ ] A taxi activation and a taxi landing each still emit exactly one coupled taxi presentation
      update, with no extra land-mount update alongside it.
- [ ] Self CREATE after reconnect, and an observer's peer CREATE, both carry the active mount
      display.
- [ ] An observer already in range sees the mount appear and disappear through VALUES, without
      leaving and re-entering visibility.
- [ ] Codec tests extend the existing taxi mount-display CREATE and presentation VALUES prior art
      rather than duplicating it.

## Definition of done

`cargo fmt`, `cargo clippy`, `cargo test` clean for `lyracore-gateway` and `lyracore-module`. Push
to `feat/issue-22-mounts`. Report how the land-mount diff is separated from the taxi-owned
transition.
