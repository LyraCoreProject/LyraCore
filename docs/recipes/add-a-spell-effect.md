# Recipe: add a spell effect

The worked example is Distract (spell 1725, raw `Spell.dbc` effect 69), merged in `e3858119`. It
touched 24 files, three of them generated bindings. Read that diff next to this list:

```bash
git diff e3858119^1 e3858119 --stat
```

The steps run in this order because each one depends on the one before it. The spell effect
taxonomy has three hand-kept copies today, so step 2 and step 3 must agree exactly.

## 1. State the 1.12.1 behaviour

Write down the raw effect id, its implicit targets, base points, die sides, radius, range, cost,
cooldown and header attributes from the client's `Spell.dbc`, and what vmangos or cmangos do with
the effect. This is the **Expected (1.12.1)** part of the issue, and the tests in later steps assert
these values.

## 2. Add the kind to the Module taxonomy

File: `module/src/spell/taxonomy.rs`.

- Add `pub(crate) const E_<NAME>: u8` at the next free value, with a doc comment that names the raw
  effect and what its amount means. An instant kind sits below `0x80`. An aura kind sets
  `KIND_AURA_BIT`.
- The value is append-only. Stored `game_spell_effect` rows carry it, so never renumber a kind.
- Add the kind to `ALL_INSTANT_KINDS` or `ALL_AURA_KINDS`.

File: `module/src/spell/tests.rs`. Raise the count in `instant_kind_wire_values_exhaustive` (or its
aura twin) by one and pin the new value, for example `assert_eq!(E_DISTRACT, 0x25)`.

## 3. Map the raw effect in the importer

File: `importer/src/spell.rs`.

- Add the same `E_<NAME>` constant with the same value.
- Map the raw effect id in `instant_effect_to_kind` (or the aura mapping) and name the kind in
  `kind_name`.
- If the effect needs parameters, set them in `resolve_instant_params`. If it needs a header
  attribute (Distract keeps stealth), add it in `spell_flag_attributes`.
- Add a unit test for the mapping, and an ignored test that reads the real spell from
  `LYRACORE_TEST_DBC` and asserts the values from step 1.

## 4. Add the Gates

Files: `module/src/spell/cast/resolve.rs`, `module/src/spell/outcome.rs`.

- An effect-specific Gate runs in `resolve_cast_at_typed` after `check_cast_gates` and before the
  cost is charged, so a Refusal leaves power and cooldown untouched.
- A Gate returns a `CastRefusal`. Add a `CastRefusalKind` only when a caller must tell the new
  Refusal apart.
- The Gateway picks the client's `SpellCastResult` from the Refusal message
  (`cast_failure_reason_for` in `gateway/src/codec/combat.rs`). Reuse an existing phrase such as
  "out of range" or "line of sight", or add an arm there with a test.

## 5. Apply the effect

File: `module/src/spell/cast/targeting.rs`. Add one arm to `apply_effect` that calls the owning
family. The behaviour lives in that family's module, not in the arm. Distract calls
`crate::creatures::distraction::distract`, a new module declared in `module/src/creatures/mod.rs`.

## 6. Store the effect's state

Only when the effect outlives the cast. Add the table in the owning family's module.

- A new table needs no default. A new column on an existing table goes at the END of the struct
  with `#[default(...)]`. A `String` column cannot take a default, so string data goes in a new
  table. [`danger-zones.md`](../danger-zones.md) section 1 has the rules.
- End the state on every path that ends it. Distract clears its row on death
  (`module/src/combat/death.rs`), on a new Engagement (`module/src/combat/engage.rs`), when a pet
  despawns (`module/src/creatures/pet.rs`) and when a creature's lifecycle resets
  (`module/src/creatures/eventai/edges.rs`).
- Read it where behaviour changes. Distract holds idle movement in the creature behavior cycle's
  `IdleSink` (`module/src/creatures/cycle/ctx.rs`) and faces the point through
  `module/src/creatures/tick/mod.rs`.

## 7. Regenerate the Gateway bindings

Required for every new table or reducer, including a private table the Gateway never reads, because
the binding check compares the complete generated tree.

- Run the `spacetime generate` command in [`danger-zones.md`](../danger-zones.md) section 1.2, then
  restore the hand-patched names and the comment it lists.
- Run `scripts/check-gateway-bindings.py`. It must report no difference.
- Review the diff under `gateway/src/stdb/bindings/`. Distract added
  `creature_distraction_type.rs`, `game_creature_distraction_table.rs` and lines in `mod.rs`.
- If the Gateway subscribes to the table, add its query to `coordinator_queries` in
  `gateway/src/stdb/connection.rs`, and add a `parity_test!` and the table name to
  `MANIFEST_TABLES` in `gateway/tests/schema_parity.rs`.

## 8. Change the packets, if the client needs it

Most effects need no Gateway change. Distract did: a cast at a clicked ground point echoes the point
in the START and GO target blocks.

- Add the builders in `gateway/src/codec/combat.rs` and any new gtker type import in
  `gateway/src/codec/mod.rs`.
- Choose them in `gateway/src/world/handlers/cast/mod.rs`.
- Test the builders byte for byte, and the handler through the cast dispatcher's Fake.

## 9. Write the durable test

File: `module/tests/<effect>.rs`, `#[ignore]`d like its siblings. It publishes the Module to its own
standalone, writes the real spell's `game_spell` and `game_spell_effect` rows (the effect row id is
`(spell_id << 2) | effect_index`), casts through a debug or `gw_*` reducer, and reads the result
with SQL. Run it as [`testing.md`](../testing.md) shows. CI picks up a new target with no workflow
edit.

## 10. Record the words and the client check

- A new term goes in `CONTEXT.md`, and in `CORE_TERMS.md` if a first change meets it. Distract
  added **Distraction**.
- What only a real client shows goes in a write-up under
  [`docs/verification/`](../verification/README.md), with a row in its index. Distract added
  `distract-client-check.md`.

## 11. Run the checks

Run every command in [`testing.md`](../testing.md) that the change reaches: the unit tier, the
wasm check, the new durable target, and the binding check.
