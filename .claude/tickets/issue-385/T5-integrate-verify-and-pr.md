# T5: Integrate, verify, file the Package follow-up, and open the PR

Parent: issue #385. **Runs last after T3 and T4 are integrated.**
Model: strongest. Estimated size: ~180k tokens.

## Problem

The preceding slices can pass separately while their union still has stale bindings, a tag-table
subscription race, duplicated refusal classifiers, missing state cleanup, or a reward path that
still consults the killing blow. The absent playerbots Package also needs an explicit owner and
contract instead of disappearing from the handoff.

## Delivery

1. Integrate T3 and T4 into the shared feature branch. Resolve conflicts by preserving the narrow
   seams from T1 and T2. Read the combined tag, death, loot, projection, and handler paths end to
   end before editing.
2. Search every creature reward, loot-open, item-take, money-take, skinning, entity-create, and
   dynamic-flags update entry point. Reconcile duplicated helpers and close any route that bypasses
   the canonical Gate.
3. Confirm bindings and schema tripwires match the Module. Existing table fields must be unchanged.
4. Add or strengthen cross-layer tests for the key stories: first player wins; foreign killing
   blow; tag-time party ceiling; leave; evade clear; tag-less death; viewer-relative live flags;
   viewer-relative corpse flags; and `DIDNT_KILL` with closed window.
5. Run focused tests, both affected crate suites, clippy for affected crates, and the Module wasm
   check. Record exact counts and any proven pre-existing failure.
6. Confirm the playerbots Package is absent. File one concise linked GitHub follow-up for stories
   20 through 22. State the final LyraCore table/Gate contract bots must consume, link issue #385,
   and do not claim LyraCore implements Package target selection.
7. Rebase the feature branch onto the latest `origin/main`, rerun risk-proportionate checks, and
   use the repository's `file-pr` skill to open one PR. Leave it open for a maintainer to merge.

## Acceptance criteria

1. There is one Module Loot Tag rule, one recipient calculation, and one corpse access Gate.
2. No reward or corpse action still grants rights from the killing blow or an empty eligibility
   fallback.
3. No Gateway path infers party ownership.
4. The stored and projected flag behavior is internally consistent across tag, death, combat exit,
   CREATE, and VALUES updates.
5. Ownership Refusals name actor and corpse in Module logs, map to `DIDNT_KILL`, and close the loot
   window without session failure.
6. Existing tables retain their row shape. Generated bindings and subscription lists are current.
7. The playerbots follow-up exists and links back to #385.
8. The feature branch is rebased on current main, checks are recorded, and one open PR links #385.

## File ownership

T5 may edit any file already touched by T1 through T4 to reconcile the union. New production files
outside that set require an explicit handoff explanation. Ticket notes and focused integration
tests are in scope. Realm configuration, deployment files, and live databases are not.

## Definition of done

The branch is pushed, the Package follow-up and LyraCore PR are open, no production or development
realm was touched, and the final report gives the DAG outcome, commits, tests, PR URL, follow-up
URL, and any remaining risk.
