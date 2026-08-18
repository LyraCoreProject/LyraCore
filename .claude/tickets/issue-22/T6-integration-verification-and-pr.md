# T6 — mounts: integrate the union, write the verification doc, file the PR

Parent: issue #22. **Runs last, after T1–T5 are integrated on `feat/issue-22-mounts`.**
Model: opus. Estimated size: ~150k tokens.

## Problem

Five tickets can each pass while their union leaves a second dismount helper, an inconsistent
refusal path, stale documentation, or a story with no owner. This ticket closes that, produces the
verification artifact the issue's Testing Decisions demand, and files the single PR.

## Delivery

**1. Verify the union first, before writing anything.** Read the integrated `module/src/mount.rs`
end to end plus every call site, and confirm:

- **One** dismount operation. If a ticket introduced a parallel helper, a second projection write,
  or its own display-clearing code, reconcile it down to T1's `dismount` / `recompute_mount`.
- Every aura-removal site recomputes the mount projection. Re-run the crate-wide `game_aura`
  deletion search yourself; do not trust T1's list.
- Nothing in module or gateway runtime branches on a spell id or a spell name.
- Every gate still runs before any mutation, after T2 and T3 both added gates.
- Land-mount code never writes `mount_display_id` while a taxi flight owns it.

Report everything you reconcile.

**2. Walk all 34 user stories** and record which test or code path covers each. Any story with no
owner is a finding, not a silent gap — surface it in the PR description.

**3. Write `docs/mount-verification.md`.** The issue names the pinned build-5875 wire harness as
the authoritative automated seam, but that harness is a **separate maintainer-owned repository**
(`LyraCoreProject/wire-harness`, pinned by `.wire-harness-rev`). It is not a workspace member, and
a LyraCore pull request cannot add a scenario to it. Do not patch the ignored cache under
`.lyracore/wire-harness/<sha>/`.

Follow the established precedent in `docs/taxi-flight-verification.md` exactly: specify the
adapter scenario for the *next* harness release, in numbered steps, naming T2's reserved fixture
entries. The scenario must cover:

- Mount item use: item still present, both mount aura effects created, expected display written,
  one mount-display VALUES update, expected force-run-speed change.
- Self CREATE mask after reconnect carries the active mount display; an observer connection
  verifies peer CREATE and live VALUES.
- Failure cases: missing riding skill, insufficient rank, combat, dead, a real imported indoor
  WMO, and the submerged predicate. Each asserts the item and all mounted state are unchanged.
- Dismount by accepted attack start, accepted cast start, indoor transition, aura cancellation,
  replacement with another mount, and synthetic Dazed resolution. Each asserts aura removal,
  display clear, and the correct restored run speed.
- Ordinary direct and periodic damage leaves the player mounted. A Dazed spell that does not land
  causes no dismount.

Close with the same publish instruction the taxi doc uses: tag a harness release, bump
`.wire-harness-rev`, run the full adapter suite, then clear the PR's `needs-live-eyeball` marker.
State the attended gate separately — an unmodified 1.12.1 build-5875 client must confirm by eye
that the mount renders, moves at the expected speed, dismounts on buff cancellation and on action,
and stays up after ordinary damage.

**4. Refresh documentation.**

- `docs/schema.md`: `mount_display_id` currently reads as taxi-only (~lines 174–180). Rewrite it as
  the shared projection for taxi flight **and** land mounts, and say which owns it when.
- `docs/architecture.md`: the mount state model — aura is authoritative, field is projection, one
  dismount recompute — and the indoor-presence table's fail-open contract.
- If T3 changed the vmap indoor rule, check `docs/vmap-rollout.md` and `docs/data-ingestion.md` for
  statements that are now wrong.

**5. File the PR** with the `file-pr` skill. Rebase onto the latest `main` first. Title in
conventional-commit style, simple: `feat(mount): ...`. The description opens with the problem,
then the solution, in Simplified Technical English, using `CONTEXT.md` vocabulary, no em dashes.
Include the story coverage summary, anything reconciled in step 1, and the `needs-live-eyeball`
marker. End with the model and harness blurb.

## Acceptance criteria

Covers stories 23, 29, 34 at the union level, and the documentation decisions.

- [ ] Exactly one land-dismount operation exists; no parallel helper remains.
- [ ] Every `game_aura` deletion site recomputes the mount projection, verified by fresh search.
- [ ] No runtime spell-id or spell-name branch anywhere in `module/` or `gateway/`.
- [ ] All 34 stories have a named owner or a stated gap.
- [ ] `docs/mount-verification.md` exists in the taxi doc's shape and names real fixture entries.
- [ ] `docs/schema.md` describes `mount_display_id` as taxi plus land-mount projection.
- [ ] `cargo fmt`, `cargo clippy` and `cargo test` clean across all four crates.
- [ ] One PR, rebased onto latest `main`, carrying the whole slice.

## Non-goals

Do not add mount behavior. This ticket verifies, documents and ships. Flying mounts, vehicles,
mount collections, the full Dazed proc, `CMSG_MOUNTSPECIAL_ANIM`, pet handling while mounted and
Ahn'Qiraj mount-area exceptions are all explicitly out of scope in the issue.
