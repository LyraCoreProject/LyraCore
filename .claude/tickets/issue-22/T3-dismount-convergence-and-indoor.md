# T3 — mounts: dismount convergence, vmap indoor correctness, indoor-presence table

Parent: issue #22. **After T1. LATE — see the rebase constraint below.**
Model: opus. Estimated size: ~200k tokens.

## Rebase constraint (read first)

This ticket edits `module/src/vmap.rs::verify_vmap_generation`, which the in-flight issue #195
branch (`feat/issue-195-derived-coverage`) also modifies — #195 adds coverage derivation to the
same reducer, which already decodes every staged chunk once. **Land after the #195 PR merges, or
rebase `feat/issue-22-mounts` onto it before starting.** Check with `gh pr list` before you begin.
If #195 has landed, the two derivations share one decode pass; say so in your report.

## Problem

After T1 a mount can be applied and cancelled, but nothing removes it when the player does
something a mounted player cannot do. Accepted attacks, accepted casts, teleports and crossing
into an indoor WMO group must all converge on T1's shared `dismount`.

The indoor test itself is currently wrong. `crates/lyracore-shared/src/vmap.rs` (~line 205) tests
`mogp_flags & 0x2000 != 0` and calls that indoor. Mangos's rule is the opposite: a found WMO group
is **outdoor only when MOGP flag `0x8000` is set**. Wrong bit, wrong polarity. Shipping an indoor
dismount on top of that would auto-dismount players outdoors.

## Delivery

**1. Fix the vmap indoor interpretation** in `crates/lyracore-shared/src/vmap.rs`:

- Replace the `WMO_GROUP_INDOOR_FLAG = 0x2000` test with the mangos rule: outdoor when
  `mogp_flags & 0x8000 != 0`, indoor otherwise, for a found WMO group. Update the doc comment and
  the codec tests that assume the old bit for their "typical interior group" fixture.
- Add a short `AREA_PROBE_DOWN_YD` (~10 yd) used by `cast_ray_area`. The 200 yd floor probe is a
  ray-cast budget and is far too long for an area query — it finds geometry the point is nowhere
  near.
- Hoist the duplicated `game_vmap_generation.by_map_state` scan; `vmap_enabled` and `fetcher` both
  perform it today.

**2. Per-cell indoor-presence table** (module-private), keyed `(generation_id, cell_key)`:

- Computed **inside `verify_vmap_generation`**, which already decodes every staged chunk exactly
  once. A cell gets a row when any of its triangles belongs to a WMO group that is not marked
  outdoor. Zero extra decode cost.
- **No manifest or digest change.** The table is derived, private, and rebuildable.
- Heartbeat does one indexed find. Only cells with indoor geometry pay a raycast.
- **A missing row means outdoors. Fail open.** So does `vmap_enabled` off, and so does no ACTIVE
  generation. The mount feature must be fully correct and shippable with vmap off — an operator
  without vmap data simply never gets indoor dismounts, and never gets false ones.

**3. Heartbeat check.** In `module/src/world.rs::apply_movement_update`, add a mounted-player-only
indoor check gated at 100 ms off the *client* movement clock:
`move_time_ms / 100 != old_move_ms / 100`. This is the same stateless gate shape as the existing
1 Hz rest and breath gates in the same function (~lines 1365 and 1374). Taxi passengers never
reach it: `crate::taxi::movement_is_suppressed` early-returns upstream (~line 1272). An indoor
edge calls T1's `dismount`. Unmounted movement must pay nothing beyond the mounted test.

**4. Teleport path.** `teleport_player` writes position outside the heartbeat, so it calls the
same shared indoor check and the same `dismount`.

**5. Accepted-action dismounts.** Both call T1's `dismount`, both after acceptance:

- **Attack start.** An accepted melee or ranged attack start dismounts *before* the engagement is
  armed. An invalid attack packet that fails normal target validation changes nothing.
- **Cast start.** An accepted non-passive, non-triggered active cast dismounts *before* cast
  resolution. **The mount cast itself is exempt.** Triggered spell effects and passive aura
  application never dismount the owner.

**6. Damage does not dismount.** Verify, do not implement. The vanilla Brown Horse spell's DBC
aura-interrupt value is the underwater-cancel bit, not the damage bit, and the existing
`breaks_on_damage` machinery is already driven by that import. Confirm direct melee, direct spell,
periodic, fall and environmental damage all leave the mount up, and write no mount-specific damage
code. If the imported interrupt data is wrong, report it to T5 rather than special-casing here.

## Acceptance criteria

Covers stories 19, 21, 22, 23, 25, 34.

- [ ] A point inside a real imported indoor WMO reports indoor; a point outdoors under an outdoor
      group reports outdoor. The old `0x2000` polarity would have inverted both.
- [ ] `AREA_PROBE_DOWN_YD` is used for area queries; the long floor probe stays on the ray path.
- [ ] The `by_map_state` scan appears once.
- [ ] Verifying a generation populates the indoor-presence table with no manifest or digest change,
      and no second decode pass.
- [ ] A cell with no row never raycasts and never dismounts.
- [ ] `vmap_enabled` off, or no ACTIVE generation: mounting and staying mounted work everywhere,
      and nothing panics.
- [ ] A mounted player who walks indoors dismounts once, within one 100 ms gate window. An
      unmounted player walking the same path incurs no area query.
- [ ] A taxi passenger flying over indoor geometry keeps flying.
- [ ] Teleporting a mounted player into an indoor position dismounts them.
- [ ] Accepted attack start and accepted cast start each dismount; a rejected attack packet and a
      passive or triggered application do not.
- [ ] The mount cast itself does not dismount the caster before it resolves.
- [ ] Direct melee, direct spell, periodic, fall and environmental damage all leave the player
      mounted, proven without any mount-specific damage code in the diff.

## Definition of done

`cargo fmt`, `cargo clippy`, `cargo test` clean for `lyracore-module` and `lyracore-shared`. Push
to `feat/issue-22-mounts`. Report the #195 rebase state and whether the decode pass is shared.
