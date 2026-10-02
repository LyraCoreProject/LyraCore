# T4: Integrate, verify across shards, document, open the PR

Parent: issue #532, meeting stones. **Runs last, after T1, T2 and T3.**
Model: Opus. Estimated size: ~150k tokens.

## Problem

T1, T2 and T3 each prove their own rung. Nothing yet proves the realm-wide claim: Seekers on
different World Shards meet in one queue, the stone forms one party on Realm-core, and every World
Shard mirror carries it. The relay's event order inside one transaction is also unproven, and the
docs do not know meeting stones exist.

## Delivery

1. Merge T1, then T2, then T3 on the feature branch. T3 adds a schedule table, so regenerate
   bindings once more from the merged module with the `danger-zones.md` §1 item 2 command and
   hand-patches. Never hand-merge generated files.
2. Cross-shard durable proof, `gateway/src/stdb/subscriptions_meeting_stone_durable_tests.rs`, the
   `subscriptions_character_gone_durable_tests.rs` shape: Realm-core plus two World Shards, a
   Coordinator with the Roster Revision Relay spawned. Stage a Deadmines stone and five Characters,
   three on one shard and two on the other, with their Account Claims on Realm-core and classes that
   fill tank, healer and three damage. Drive `dispatch_meeting_stone_action` for each through the
   real Coordinator. T1's fixtures stage one database; extend them for the split if needed.
3. Event order. Assert that one recipient's events from one transaction reach its session in row-id
   order. If the relay can reorder them, sort per recipient before enqueueing in
   `world_view::group_event_appeared` and say so in the PR.
4. CI. Write the exact `cargo test -p lyracore-gateway --bin lyracore-gateway <path> -- --ignored
   --exact` lines for this test and T2's durable test. The agents' token cannot push
   `.github/workflows/*`; follow the maintainer's answer to the README question. Until then, put the
   lines in the PR description.
5. `docs/meeting-stone-client-check.md`, the `auction-house-client-check.md` shape, status
   outstanding, for a human with a real 5875 client. Check: right-click sends `JOIN`, not
   `GAMEOBJ_USE`; the tooltip shows the level range; the client refuses an out-of-range level itself;
   the minimap button appears and its tooltip names the dungeon; each `ERR_MEETING_STONE_*` line for
   each status; `INFO` after a world port keeps the button; leaving through the minimap confirm;
   converting a queued party to a raid drops the button; a stone-formed party's frame, `/p` and XP
   split across two shards.
6. Docs:
   - `docs/architecture.md`: add the Meeting Stone Queue to Realm-core's holdings in §1 and §3.1;
     add the class and race facts to the §2.3 Gateway read list beside presence; add the Roster
     Revision Relay to §5.3.
   - `docs/schema.md`: the new tables in the §2 inventory and the recount.
   - `CONTEXT.md`: check the T1, T2 and T3 entries read as one section and use the `_Avoid_` words
     nowhere in new code or prose.
   - `CHANGELOG.md`: one line under `[Unreleased]` → Added.
7. Open the PR with the `file-pr` skill after rebasing on `main`. Title in the repo's style, for
   example `feat(meeting-stones): queue and group Seekers realm-wide`. The body states the problem,
   the solution, that 1.12.1 has no LFG window, the two settled maintainer decisions (stones only,
   humans only), the raid-convert dequeue that departs from both cores, the CI state, and the model
   and harness blurb. Use `Refs #532`, not `Closes`, while the client check is outstanding.

## Acceptance criteria

1. The five staged Characters end in one party on Realm-core with five members, the tank, healer and
   damage classes placed, and no Seeker or party row left.
2. Both World Shards' `game_group_member` mirrors hold the five members, and each shard's
   `game_group_roster_revision` equals Realm-core's, without any party op from those Characters.
3. Each Character's session receives its stone events in the order the Module inserted them, from
   its first `SETQUEUE(area, JOINED)` to the closing `COMPLETE` then `SETQUEUE(0, NONE)`.
4. A Seeker who crosses into the Instance Pool after queueing stays queued and gets
   `SETQUEUE(area, JOINED)` for `INFO` on arrival.
5. Every workspace check below is clean, or each exception is proven to fail on the base commit.
6. The client check document exists with outstanding status and exact steps.

## Verification rungs

- Format touched files individually, `cargo clippy` and `cargo test` for `lyracore-shared`,
  `lyracore-importer`, `lyracore-module`, `lyracore-gateway` and `lyracore-package-delta`.
- The Module wasm check, `scripts/check-gateway-bindings.py`, `lyracore preflight`.
- The durable Module tests from T1 and T3, the durable Gateway tests from T2 and this ticket.
- The real-client check is the named human rung. A Headless Client run is optional.

## CONTEXT.md terms

None new. Consolidate.

## File ownership

The union after merge, the new durable test, the docs listed above and the PR.

## Definition of done

The PR is open against `main`, rebased, green, and links the client check document as the one
outstanding human step.
