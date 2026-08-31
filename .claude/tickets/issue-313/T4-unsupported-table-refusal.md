# T4 — the precise refusal for an unsupported DBC table

Model: sonnet

Depends on: T3. Rebase onto T3 before starting.

## Goal

Satisfy the acceptance criterion "unsupported DBC tables fail with a precise message". Today a
claim on `game_area` gets the generic `unknown table` refusal, which lists the whole catalogue and
tells the author nothing about WHY that particular table is missing. It is not missing by accident.

## The problem with the generic refusal

`DeltaError::UnknownTable` prints:

```text
unknown table `game_area`; a Package Delta claims `game_spell`, `game_spell_effect`, …
```

For a typo that is the right answer. For a DBC table this project has deliberately closed, it is
the wrong one twice: the author reads a long list and concludes the feature is unfinished, and the
project's decision is nowhere the author can see it.

## The change

A closed list of KNOWN-BUT-EXCLUDED table names, each with its one-line reason, checked in
`Table::parse`'s failure path before `UnknownTable` is raised. A new refusal,
`DeltaError::DbcTableNotSupported { table: String, reason: &'static str }`, prints:

```text
`game_area` is a client DBC catalogue LyraCore does not open to Package Deltas: a zone's name, map
art and exploration slot live in the client's own AreaTable.dbc, so a server-side edit diverges from
what the player sees
```

Keep the list beside the `Table` catalogue in `crates/lyracore-package-delta/src/schema.rs`, because
it is the negative half of the same decision. A `const EXCLUDED_DBC_TABLES: &[(&str, &str)]` is
enough; do not build a second enum.

## The list

Verbatim from `.claude/tickets/issue-313/README.md`'s exclusion table. Seven entries:

| Name | Reason |
|---|---|
| `game_char_base_info` | its only non-key columns, `race` and `class`, are the components of its own key, so a claim could set nothing |
| `game_skill_availability` | no Module game logic reads it, so a claim would change no behaviour |
| `game_area` | a zone's name, map art and exploration slot live in the client's own `AreaTable.dbc`, so a server-side edit diverges from what the player sees |
| `game_area_trigger` | trigger volume geometry with no Module reader; the behaviour bound to a trigger is claimable through `game_areatrigger_teleport` under the `globals` family |
| `game_talent_tab` | the client draws the talent panes from its own `TalentTab.dbc` and sends the ids it read there |
| `game_talent` | the same, and retuning what a talent rank grants is better done on the spell itself, under the `spell` family |
| `game_start_item` | see below; this one is NOT a refusal |

**`game_start_item` must not reach this list.** It is DBC-loaded, but it already belongs to the
`globals` Import Family (#311), so `Table::parse` resolves it and no refusal fires. Add a test that
pins that: a `game_start_item` claim parses and reports family `globals`. It is on the README's
inventory so a reader can find it; it is not an exclusion.

`game_areatrigger_teleport` likewise resolves under `globals` and must not reach the list. It is not
DBC-loaded at all: only its key is a `AreaTrigger.dbc` identifier.

## Where the reason text lives

One place. The `&'static str` in the const IS the reason, and the README's table is prose that
quotes it. Do not write the sentence twice in code.

Keep each reason to one clause and lowercase-initial, so it reads as the tail of the refusal
sentence. No trailing period; the refusal supplies the shape.

## Files owned

- `crates/lyracore-package-delta/src/schema.rs`, `error.rs`, `lib.rs`
- `crates/lyracore-package-delta/tests/families.rs`
- `.claude/tickets/issue-313/README.md`, only if a reason's wording changes

## Out of scope

- Any new claimable table. The catalogue closed at T3.
- Changing the existing `UnknownTable` message. It still fires for a typo, and its text is asserted
  by existing tests.
- A refusal for a DBC file that has no `game_*` table at all (`SkillTiers.dbc`,
  `CreatureDisplayInfo.dbc`, `LockType.dbc`). A claim names a durable table, never a DBC file, so
  there is nothing for an author to spell wrong.

## Acceptance tests

1. Each of the six excluded names is refused with `DbcTableNotSupported`, and the refusal contains
   both the name and its reason.
2. A typo (`game_are`, `game_spel`) still gets `UnknownTable` with the full catalogue.
3. `game_start_item` parses and reports family `globals`; `game_areatrigger_teleport` parses and
   reports family `globals`.
4. A test that walks `EXCLUDED_DBC_TABLES` and asserts no entry is also in `Table::ALL`, so the two
   lists can never both claim a name.
5. A test that every reason is non-empty, lowercase-initial and has no trailing period, so the
   refusal sentence stays well formed as the list grows.
6. Crate suite passes; clippy and rustfmt clean on touched files.

## Definition of done

An author who claims a DBC table LyraCore does not open reads, in the refusal, why. The decision
lives in the code beside the catalogue it is the negative of, in exactly one place.
