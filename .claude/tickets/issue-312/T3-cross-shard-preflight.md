# T3: cross-Shard preflight for spatial claims

Parent: issue #312. **Depends on T2. Rebase onto it before starting.**
Model: sonnet.

**Also rebase onto the #311 chain head.** See the README.

## Goal

Answer the issue's fourth acceptance criterion: "cross-Shard preflight catches invalid map ownership
and references before writes."

T1 and T2 make a spatial Claim reach only the World Shards whose World Import Scope owns its map.
That is per-Shard, and it is correct: each import drops what it does not want. What no single import
can see is the whole picture, a Claim that lands on NO Shard at all, or a Package spawn whose
template nothing anywhere provides. Both are silent today: the operator gets a clean import and a
missing NPC.

This ticket adds one terminal importer mode that reads the enabled Packages and every World Import
Profile, and says which claims reach nothing.

## Read first

- `.claude/tickets/issue-312/README.md`
- T1's and T2's finished diffs: `ClaimScope`, the two `admits_*` predicates, the two-pass template
  rule, and the plan report's out-of-scope lines. This ticket reuses all four rather than restating
  them.
- `importer/src/main.rs`: `--spell-snapshot`'s handling around lines 1271 and 1366 (the worked
  example of a terminal mode that touches no database), `parse_args_from`'s validation block around
  line 1349
- `importer/src/world_import_scope.rs`: `WorldImportProfile::NAMES`, `WorldImportScope::canonical`
- `crates/lyracore-package-delta/src/bin/lyracore-delta-check.rs`: the existing database-free check,
  and why it is not enough on its own

## The mode

```
lyracore-importer --package-preflight <enabled packages root>
```

A terminal mode, like `--spell-snapshot`: it touches no database, needs no `--dump` and no `--dbc`,
and refuses to run beside `--apply` or any other mode flag. Say so in the refusal, in the voice
`--spell-snapshot`'s already uses.

It reads no dump on purpose. Checking a claim against the real imported template entry set needs
`--dump` and belongs to a later step; what this mode checks is the two things only a cross-Shard view
can see, and it can see them from the artifacts and the profile catalogue alone.

## What it does

For every name in `WorldImportProfile::NAMES`, build the canonical `WorldImportScope` and run T1's
and T2's filter over the enabled Packages' claims for both spatial families. Then report.

Refuse, naming the Package, the table and the key each time:

1. **A spatial claim no profile admits.** Its map belongs to no World Import Profile, or its position
   falls outside every Bounded Map Slice of the profiles that own its map. This is the "invalid map
   ownership" the criterion names, and it is the reason the mode exists: on any single Shard such a
   claim looks exactly like a claim meant for a different Shard.
2. **A spawn claim whose template no profile can supply.** The plan inserts no template at that
   entry, and the entry is not in the Package Creature Range or the Package Gameobject Range, so it
   must be a real dump template, which this dump-free mode cannot confirm and does not refuse. State
   that limit in the report rather than guessing.
   What it DOES refuse: a spawn whose `entry` sits inside a Package range and which no enabled
   Package inserts. That claim can never work on any Shard.
3. **A template or trap claim no kept spawn claim names, on any profile.** A Package template that
   nothing spawns reaches no Shard. It is more likely a forgotten spawn claim than a deliberate
   catalogue-only row, and the operator should hear about it.
4. **Every Claim Conflict**, across all families, the way `lyracore-delta-check` already does.

Exit non-zero on any of the four. Print the whole list, not the first: an operator fixing them wants
one pass.

## What it reports on success

One block per World Import Profile, then a summary:

```text
=== Package spatial preflight (packages/) ===
  alliance-eastern    12 creature spawns   3 creature templates   4 gameobject spawns
  alliance-kalimdor    0 creature spawns   0 creature templates   0 gameobject spawns
  alliance-single     12 creature spawns   3 creature templates   4 gameobject spawns
  instances            2 creature spawns   1 creature template    0 gameobject spawns
  every spatial claim reaches at least one World Import Profile
=============================================
```

A profile with zero claims is not a failure. A CLAIM with zero profiles is.

## Where the code goes

- `importer/src/package_delta.rs` gains a `pub(crate) fn preflight(root: &str) -> Result<()>`. It
  reuses `read_enabled`, `artifacts_for_family`, the T1/T2 filter and `trace`, so there is one
  definition of "does this scope want this claim" and this mode cannot drift from what an import
  actually does.
- `importer/src/main.rs` gains the flag, its validation, and one dispatch arm before the `--dump`
  path, next to `--spell-snapshot`'s.
- Nothing new goes in `world_import_scope.rs`: T1 and T2 already put both predicates there.

Do not add a Shard-to-database catalogue. `WorldImportProfile::NAMES` is the whole set of World
Import Profiles this build knows, and which database each is imported into is an operator fact that
lives in `importer/scripts/import-world.sh`. The issue forbids new routing concepts, and a profile
catalogue is not one; a Shard manifest would be.

## Files owned

- `importer/src/{main.rs,package_delta.rs}`
- `docs/`: one paragraph wherever the importer's modes are documented, if such a page exists
- `CONTEXT.md` only if a new term is needed. Prefer reusing World Import Profile and Claim.

## Out of scope

- A dump-aware preflight that resolves references against the real imported template entry set.
- Any change to what an import does. This mode reports; it never writes and never changes the filter.
- Generalizing `packages replay` or `lyracore-delta-check`. Named follow-ups.
- The `lyracore preflight` verb in the sibling `lyracore-cli` repository. Wiring this mode into it is
  a cross-repo change with its own pin bump.

## Acceptance tests

1. A claim on a map no profile owns is refused, naming the Package, the table and the key.
2. A creature spawn claim inside `alliance-kalimdor`'s Darkshore slice is accepted, and the report
   shows it under `alliance-kalimdor` and `alliance-single` and nowhere else.
3. A creature spawn claim on an owned map but outside every Bounded Map Slice of every profile is
   refused.
4. A spawn whose `entry` is in the Package Creature Range and which no enabled Package inserts is
   refused; the same spawn with the template insert present is accepted.
5. A Package template with no spawn claim anywhere is refused.
6. A Claim Conflict between two Packages is refused with both identities.
7. A clean set exits zero and prints one line per profile.
8. The mode refuses to run beside `--apply`, `--dump` or `--dbc`, with a message naming the conflict.
9. `cargo test -p lyracore-importer --bin lyracore-importer` passes.
10. `cargo clippy` clean on every touched file, `rustfmt` clean.

## Definition of done

`lyracore-importer --package-preflight packages/` reads no database and no dump, reports every
spatial claim against every World Import Profile, and refuses any claim that reaches nothing.
