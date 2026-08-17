# Land mount verification

A land mount is an ordinary cancelable self aura. The `A_MOUNTED` aura row is the mounted state.
`WorldEntity.mount_display_id` is a projection of that aura set for `UNIT_FIELD_MOUNTDISPLAYID`, and
`run_speed_mult_bp` is a projection of the same aura set through the shared move-speed fold. Both are
re-derived by `mount::recompute_mount`, so every dismount trigger converges on one operation.

Module and Gateway tests prove the rules headlessly. They cannot prove that the unmodified client
renders the mount, moves at the higher speed, or stays connected. This document records the attended
procedure and specifies the Headless Client scenario for the next release of the pinned suite.

## 1. Reserved fixture data

Seeded by `seed::seed_mount_fixture` from `init`, and re-seeded by `debug_repair_after_publish` on an
already-migrated shard. All ids sit inside the reserved fixture ranges and never collide with an
imported world.

| Entity | Id | Detail |
|---|---:|---|
| Riding skill line | 762 | `game_skill_line`, name `Riding` |
| Mount spell | 50310 | `Test Riding Horse`, self cast, no cost, no GCD, permanent duration |
| Mount aura effect | | `A_MOUNTED`, `p0` = display 1147, `p0_kind` = `P_DISPLAY_ID` |
| Mounted speed effect | | `A_MOD_SPEED`, `p0` = `SPEED_MOUNTED`, amount +60 |
| Riding requirement | | `game_skill_ability` 5090020: spell 50310, line 762, `min_skill` 75, masks 0 |
| Mount item | 5090054 | `Test Riding Reins`, on-use spell 50310, max stack 1 |
| Dazed spell | 50311 | `Test Dazed`, range 30 yd, duration 4 s, targets an enemy |
| Dazed slow effect | | `A_MOD_SPEED`, `p0` = `SPEED_MOVE`, amount -50 |
| Dazed dismount effect | | `E_DISMOUNT`, no parameters |
| Riding trainer | 51007 | `Riding Trainer`, `trainer_type::MOUNTS`, seeded by `debug_seed_scenario_fixtures` only |
| Apprentice offering | 50132 | cost 100 copper, required level 1, `learn_skill_line` 762, cap 75 |
| Journeyman offering | 50133 | cost 1000 copper, required level 60, `learn_skill_line` 762, cap 150 |

The mount spell is data alone. Nothing in the Module or the Gateway branches on 50310, 50311, or on
any spell name. An imported mount works through the same taxonomy with no code change.

Expected speeds: `BASE_RUN_SPEED` is 7.0 yd/s. A +60% mount runs at 11.2 yd/s and writes
`run_speed_mult_bp` 16000. An unmounted rider with no other speed aura writes 10000.

Refusal texts, usable as assertions:

- `spell 50310 requires riding training`
- `you cannot mount while dead`
- `you cannot mount while in combat`
- `you cannot mount indoors`
- `you cannot mount in deep water`

## 2. Indoor presence is written at verification time only

`game_vmap_indoor_cell` is a Module-private per-cell marker table. It is computed inside
`verify_vmap_generation`, which already decodes every staged chunk once. `vmap::is_indoor` asks that
table first and only pays a ray cast for a cell that holds indoor geometry.

A missing row means outdoors. The whole indoor rule fails open: vmap disabled, no active generation,
no marker row, or no containing WMO group all read as outdoors.

**Operational consequence.** A vmap generation that was verified before this change carries no marker
rows. On such a shard the indoor gate and the indoor dismount stay inactive, and no refusal or
dismount will ever fire indoors. Re-run `verify_vmap_generation` on the generation, or re-import the
vmap data, before attempting steps 3.7 and 3.8 below.

## 3. Attended procedure

Run against a live shard with `debug_reducers` enabled and an unmodified 1.12.1 build-5875 client.
`<C>` is the character guid.

1. `debug_repair_after_publish` after publishing, so the mount fixture exists on a migrated shard.
   Then `debug_seed_scenario_fixtures` to seed the Riding Trainer and its two offerings.
2. `debug_grant_item(<C>, 5090054, 1)`. Log in and confirm the reins are in the bag.
3. Use the reins with no riding skill. Expect the training refusal, the item still in the bag, no
   aura, no display change, and no speed change.
4. `debug_spawn_at_feet(<C>, 51007, 3.0)`, open the trainer, and buy the Apprentice offering. The
   character needs 100 copper. Confirm the Riding skill appears at 75/75 in the skills pane.
   `debug_learn_riding(<C>, 75)` is the direct alternative when the trainer path is not under test.
5. Use the reins again. **Eyeball:** the character mounts and the horse renders. The buff appears in
   the aura bar. Movement is visibly faster. The reins are still in the bag.
6. Attempt the Journeyman offering below level 60. Expect a level refusal. Raise the level, buy it,
   and confirm the skill reads 150. `debug_learn_riding_from_trainer(<C>, 150)` drives the same
   trainer path without the gossip UI.
7. Stand inside a real imported WMO interior on a re-verified vmap generation. Attempt to mount.
   Expect the indoor refusal and no state change.
8. Mount outdoors, then walk through the doorway of the same interior. **Eyeball:** the character
   dismounts within about one stride of crossing, and run speed drops back.
9. Mount, then swim until submerged and attempt to mount again after dismounting. Expect the deep
   water refusal. Wading in shallow water must not refuse.
10. Mount, then take melee, spell, periodic, fall, and environmental damage. **Eyeball:** the mount
    stays up through all five.
11. Mount, then right-click the mount buff off. **Eyeball:** the model reverts, the buff clears, and
    speed returns to normal in the same frame the model changes.
12. Mount, then start an auto-attack. Then mount again and start any active cast. Both dismount
    before the action takes effect. Casting the mount spell itself must not dismount first.
13. Mount, then have a second character cast 50311 at the rider. Expect a dismount plus the movement
    slow. Repeat against an immune or out-of-range target and confirm no dismount.
14. Mount, then relog. **Eyeball:** the character returns mounted, at mounted speed, with the buff
    present. Repeat across a Shard Boundary and confirm the same.
15. Mount, then take a taxi flight. The taxi Gate refuses while mounted. Dismount, fly, and confirm
    the flight presentation is unaffected by any land mount cleanup, and that landing clears cleanly.
16. Have a second client observe every mount and dismount above. **Eyeball:** the observer sees the
    same model and the same movement speed with no relog and no leaving visibility.

## 4. Headless Client scenario for the next release

The build-5875 suite is maintainer owned and is not stored in this repository. The checkout under
`.lyracore/wire-harness/<sha>/` is fetched from `.wire-harness-rev`; do not patch that ignored cache
from a LyraCore pull request.

For the next release, add a LyraCore adapter scenario that:

1. Seeds the fixture, logs `TEST` in outdoors, and grants item 5090054.
2. Uses the reins with no riding skill. Asserts the refusal, an unchanged item count, zero
   `game_aura` rows for 50310, `mount_display_id` 0, and `run_speed_mult_bp` 10000.
3. Grants Riding at rank 74 and repeats step 2. Asserts the same unchanged state, proving the rank
   threshold is inclusive at 75.
4. Grants Riding at rank 75 and uses the reins. Asserts the item is still present, both aura effects
   of 50310 exist, `mount_display_id` is 1147, exactly one mount-display VALUES update arrives
   without `OBJECT_FIELD_TYPE` in its mask, and one `SMSG_FORCE_RUN_SPEED_CHANGE` carries 11.2.
5. Reconnects and asserts the self CREATE mask carries `UNIT_FIELD_MOUNTDISPLAYID` 1147 and the
   mounted speed.
6. Attaches an observer connection and asserts the peer CREATE carries the same display, then asserts
   a live VALUES update on the next mount and dismount without a visibility change.
7. Enters combat and attempts a mount. Then repeats while dead, while a ghost, inside a real imported
   indoor WMO group on a re-verified generation, and while submerged. Each case asserts the refusal
   text and that the item, aura rows, display, speed, cooldowns, and combat state are unchanged.
8. Dismounts by each trigger in turn: accepted melee attack start, accepted ranged attack start,
   accepted active cast start, movement across an indoor boundary, `CMSG_CANCEL_AURA` on 50310, and
   resolution of 50311. Each asserts every 50310 aura row is gone, `mount_display_id` is 0, and
   `run_speed_mult_bp` is back to the correct value including any still-active ordinary speed aura.
9. Casts the mount spell while already mounted on a second mount spell. Asserts the prior mount's
   aura rows and display are replaced, never stacked, and that only one `A_MOUNTED` row exists.
10. Applies direct melee damage, direct spell damage, and periodic damage to a mounted player.
    Asserts the mount survives all three. Then casts 50311 at a target that resists or is immune and
    asserts no dismount.
11. Activates a taxi flight from an unmounted state, asserts the flight presentation, and asserts
    that a concurrent land dismount call leaves the flight display and the taxi unit flag intact.

Publish that change as a tagged release, update `.wire-harness-rev` to the tag and commit, and run
the full adapter suite before removing the pull request's `needs-live-eyeball` marker.

## 5. Attended gate

The attended gate remains separate. An unmodified 1.12.1 build-5875 client must confirm by eye that
the mount renders on the rider and on an observer, that movement is visibly faster, that the mount
comes off on buff cancellation and on an action start, that the mount survives ordinary damage, and
that the session stays stable across mount, dismount, relog, and a Shard Boundary crossing.
