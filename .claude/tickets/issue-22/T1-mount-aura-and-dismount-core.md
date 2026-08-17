# T1 — mounts: `A_MOUNTED`, `E_DISMOUNT`, and the shared dismount recompute (tracer)

Parent: issue #22. **Runs alone. Blocks T2–T6.**
Model: opus. Estimated size: ~200k tokens.

## Problem

Mount spells already carry mounted-speed residue and mount items already reference an on-use
spell, but no runtime effect establishes mounted state. There is no `A_MOUNTED` aura kind, no
`E_DISMOUNT` effect kind, and no operation that removes a mount coherently. Worse, the module has
no single aura-deletion boundary, so a naive "clear the display on cancel" would leave expiry,
dispel, channel-end and death cleanup writing partial state.

This ticket establishes the state model and the convergence mechanism. Nothing else in the slice
is safe to build until it exists.

## Delivery

**1. Taxonomy.** Add `A_MOUNTED = 0xB3`, `E_DISMOUNT = 0x23`, `P_DISPLAY_ID = 14` to
`module/src/spell/taxonomy.rs`, in the existing block style, and register them in the exhaustive
constant lists at the bottom of the file. `0x1E` stays reserved. `A_MOUNTED`'s frozen `p0` is the
resolved creature display id used for `UNIT_FIELD_MOUNTDISPLAYID`; `p0_kind` is `P_DISPLAY_ID`.

**2. New module `module/src/mount.rs`** holding the whole land-mount state model:

- `mount_aura_moves_mount(eff_kind: u8) -> bool` — true for `A_MOUNTED`, and for `A_MOD_SPEED`
  with `SPEED_MOUNTED` if the speed fold needs the signal. Pure, unit-testable.
- `recompute_mount(ctx, guid)` — derive `WorldEntity.mount_display_id` and the effective run speed
  from the target's current aura rows. Zero `A_MOUNTED` auras means display 0 and ordinary speed.
  **Recompute, never a delta.** This is what makes every trigger idempotent (story 34).
- `dismount(ctx, guid)` — the one shared land-dismount operation. Find the active `A_MOUNTED`
  aura, delete **every** aura row whose `spell_id` matches it (so the paired `SPEED_MOUNTED`
  effect goes too), then `recompute_mount`. No-op when not mounted, and no error.
- `active_mount_spell(ctx, guid) -> Option<u32>` for callers that need the test.

**Taxi safety.** `recompute_mount` and `dismount` must leave a taxi flight completely alone: no
write to `mount_display_id` and no aura deletion while `module/src/taxi.rs` owns the projection
for that player. Read the taxi state guard, do not duplicate it.

**3. Wire the apply side.** In `module/src/spell/cast/targeting.rs::aura_apply` — the single
`game_aura` insertion site — an `A_MOUNTED` application first calls `dismount` for the target
(story 28: two ground mounts never stack), then inserts, then recomputes.

**4. Wire every removal site.** Beside the existing `revitalize` / `resheet` collection-and-
recompute pattern, add the mount equivalent at each site that deletes `game_aura` rows:
`scheduler.rs::do_cancel_aura`, the expiry reap pass, the channel-end pass, dispel, death cleanup,
and `spellbook.rs`'s unlearn path. Search the crate for `game_aura` deletions; do not trust this
list to be complete.

**5. `E_DISMOUNT` resolve arm.** An instant effect that calls `dismount` for its resolved target.
It resolves like any other instant effect, so an unlanded cast changes nothing (story 27).

**6. Prove it end to end via `CMSG_CANCEL_AURA`.** `gw.rs::gw_cancel_aura` →
`scheduler.rs::do_cancel_aura` must clear the aura rows, the display projection and the speed
restoration in one reducer call.

## Out of scope for this ticket

Riding gates, combat/liquid/indoor gates, the item path, the importer, the gateway relays, and the
fixtures. A debug reducer or an existing debug cast path is enough to exercise `A_MOUNTED` here.

## Acceptance criteria

Covers stories 3, 23, 27, 28, 29, 34.

- [ ] `A_MOUNTED`, `E_DISMOUNT` and `P_DISPLAY_ID` exist at the values above and appear in the
      taxonomy's exhaustive lists. `0x1E` is still unused.
- [ ] Applying `A_MOUNTED` writes `mount_display_id` from the frozen `p0`.
- [ ] `CMSG_CANCEL_AURA` on the mount spell removes both mount effects, clears the display, and
      restores ordinary run speed.
- [ ] Natural expiry, dispel and death cleanup produce the same end state as manual cancellation.
- [ ] `dismount` called twice in a row, and called on an unmounted player, both succeed and
      change nothing the second time.
- [ ] Applying a second `A_MOUNTED` spell leaves exactly one mount spell's auras behind.
- [ ] `E_DISMOUNT` resolving against a mounted target dismounts it; against an unmounted target it
      is a silent no-op.
- [ ] With an active taxi flight, `dismount` and `recompute_mount` leave the flight, the taxi unit
      flag and `mount_display_id` untouched.
- [ ] `mount_aura_moves_mount` has direct unit coverage.

## Definition of done

`cargo fmt`, `cargo clippy -p lyracore-module`, `cargo test -p lyracore-module` clean. Push to
`feat/issue-22-mounts`. Report the final constant values and the exact list of aura-removal sites
you wired, so T2–T5 can rely on it.
