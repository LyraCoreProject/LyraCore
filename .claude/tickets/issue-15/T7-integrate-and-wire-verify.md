# T7 — Integrate, wire-verify and document

Parent: issue #15. Depends on T2, T3, T4, T5, T6. **Runs alone, last.**
Model: Opus. Estimated size: ~150k tokens.

## Problem

T2 through T6 each appended to the same four files from separate worktrees, each proving its own
slice in isolation against canned reads. Nothing has yet proved the **union**: that a guild can be
created, invited into, chatted in, rostered, retitled and disbanded in one continuous session
against a real database over a real encrypted socket.

The issue's acceptance note requires exactly that: "all wire-verified headlessly". Seam tests do not
satisfy it on their own. They prove branches; they do not prove the packets a real client receives.

## Delivery

**Reconcile.** Read the merged `module/src/guild.rs`,
`gateway/src/world/handlers/guild.rs`, `gateway/src/world/guild.rs` and
`crates/lyracore-shared/src/guild.rs` end to end and fix what parallel authorship left:

- duplicated helpers written twice under different names by two tickets
- op tags or event kinds that collide or skip numbers in the shared contract
- gate logic copy-pasted per op that should be one function
- error classification that drifted from `handlers/item.rs`'s rule
- dead code a later ticket orphaned
- comment density that crept toward the legacy `group.rs` essay style

**Wire-verify headlessly.** Using the pinned harness (`.wire-harness-rev`, see
`docs/development-cli.md` "dev smoke"), drive one continuous scenario against a live module:

create → invite a second character → accept → roster → `/g` both ways → set MOTD → kick → leave →
disband, asserting the actual opcodes on the wire at each step.

Then run the same scenario across a **sharded** topology with the two characters on different
shards, which is the configuration every cross-database decision in this folder exists for.

**Document.** Update `docs/guild-system.md` from spec to shipped state: mark the slices done, record
what changed during implementation, and keep the deferred list accurate. Add the guild vocabulary to
`CONTEXT.md` under a new `### Guilds` heading — Guild, Guild Master, Guild Rank, Guild Invite, Guild
Chat, each with its `_Avoid_` line, matching the existing entries' shape. Note explicitly that
"Guild Master" is not "GM", which in this codebase means game master.

## Acceptance criteria

1. The full lifecycle scenario above passes headlessly against a live module, asserting opcodes on
   the wire, with no manual steps and no `needs-live-eyeball` dependency.
2. The same scenario passes with the two characters on **different shards**.
3. `cargo test` is green across the workspace, not only the two crates the tickets touched.
4. `cargo clippy` is clean workspace-wide with no new allows introduced to silence it.
5. A `spacetime publish` over an existing database **applies the migration cleanly**. This is the
   one check nothing in `cargo test` or `cargo check` performs, and the `#[default(0u64)]` trap in
   the README is exactly the kind of failure it catches. Do not skip it.
6. Zero duplicated helpers and zero unused code across the four merged files.
7. The shared contract's op tags and event kinds are contiguous and collision-free.
8. `docs/guild-system.md` matches what shipped, and `CONTEXT.md` carries the guild vocabulary.
9. Every "no guild system yet" comment in the codebase is gone, and the four holes listed in the
   README's state-of-the-world table are closed.

## Tests

- The headless wire-harness scenarios in criteria 1 and 2.
- Whatever seam tests the reconciliation invalidates get rewritten, not deleted, unless a ticket's
  test genuinely duplicates another's.
- Do not add breadth for its own sake. The parallel tickets already cover their branches; your job
  is the union and the wire, not a second pass of the same assertions.

## File ownership

All of it. Nothing else is running.

## Non-goals

- Do not implement anything deferred: rank permissions, promote/demote, charters and petitions,
  tabards and emblems, officer chat, guild bank, faction gating. File follow-up issues instead, one
  per deferred item, each stating what it covers and what it depends on.
- Do not touch issue #15 on GitHub. Tickets stay local.

## Definition of done

Criteria 1 through 9 all met, on a branch rebased onto latest `main`, ready for the `file-pr` skill.
