# Recipe: add a spell effect

The shared taxonomy supplies the codes used by the Module and Importer. Native protocol handlers
and Relays run in the Module; reusable packet builders live in the protocol crate.

## 1. State the 1.12.1 behaviour

Write down the raw effect id, its implicit targets, base points, die sides, radius, range, cost,
cooldown and header attributes from the client's `Spell.dbc`, and what vmangos or cmangos do with
the effect. This is the **Expected (1.12.1)** part of the issue, and the tests in later steps assert
these values.

## 2. Add the kind to the shared taxonomy

File: `crates/lyracore-shared/src/spell/taxonomy.rs`.

- Add the byte name, enum variant and code to `effect_kinds!`. An instant kind sits below `0x80`;
  an aura kind sets `KIND_AURA_BIT`.
- Codes are append-only. Stored `game_spell_effect` rows retain their byte encoding.
- Add a literal encoding assertion for the new code. The macro supplies conversion and iteration;
  no separate kind list or count needs an edit.

## 3. Map the raw effect in the importer

File: `importer/src/spell.rs`.

- Import the new byte name from `lyracore_shared::spell::taxonomy`.
- Map the raw effect id in `instant_effect_to_kind`, or for an aura the raw aura id in
  `aura_mod_to_kind`, and name the kind in `kind_name`.
- If the effect needs parameters, set them in `resolve_instant_params` (`resolve_aura_params` for an
  aura). If it needs a header
  attribute (Distract keeps stealth), add it in `spell_flag_attributes`.
- Add a unit test for the mapping, and an ignored test that reads the real spell from
  `LYRACORE_TEST_DBC` and asserts the values from step 1.

## 4. Add the Gates

Files: `module/src/spell/cast/resolve.rs`, `module/src/spell/outcome.rs`.

- An effect-specific Gate runs in `resolve_cast_at_typed` after `check_cast_gates` and before the
  cost is charged, so a Refusal leaves power and cooldown untouched.
- A Gate returns a `CastGateRefusal` with a `CastRefusalReason` (`module/src/spell/outcome.rs`).
  Add a `CastRefusalKind` only when a Package caller must tell the new Refusal apart.
- `cast_result` in `module/src/native/cast.rs` maps each `CastRefusalReason` to the client's
  `SpellCastResult`. Reuse an existing reason such as `OutOfRange` or `NoLineOfSight`, or add a
  variant, its arm in `cast_result` and a row in that file's table test.

## 5. Apply the effect

File: `module/src/spell/cast/targeting.rs`. The exhaustive `EffectKind` match in `apply_effect`
requires an arm for the new variant. An aura kind joins the arm that calls `apply_aura_effect`.
The behaviour lives in the owning family's module, not in the arm. Distract calls
`crate::creatures::distraction::distract`, a new module declared in `module/src/creatures/mod.rs`.

## 6. Store the effect's state

Only when the effect outlives the cast. Add the table in the owning family's module.

- A new table needs no default. A new column on an existing table goes at the END of the struct
  with `#[default(...)]`. A `String` column cannot take a default, so string data goes in a new
  table. [`danger-zones.md`](../danger-zones.md) has the rules.
- End the state on every path that ends it. Distract clears its row on death
  (`module/src/combat/death.rs`), on a new Engagement (`module/src/combat/engage.rs`), when a pet
  despawns (`module/src/creatures/pet.rs`) and when a creature's lifecycle resets
  (`module/src/creatures/eventai/edges.rs`).
- Read it where behaviour changes. Distract holds idle movement in the creature behavior cycle's
  `IdleSink` (`module/src/creatures/cycle/ctx.rs`) and faces the point through
  `module/src/creatures/tick/mod.rs`.

## 7. Check schema and delivery

Run `./lyracore preflight` for a table or reducer change. If the effect changes persistent state,
update its native Relay and any world-entry replay that restores it.

## 8. Change the packets when needed

- Add reusable builders in `crates/lyracore-protocol/src/codec/combat.rs`.
- Call them from `module/src/native/spell.rs` or the owning native Protocol Family.
- Test exact packet bytes, recipient selection and the client's acknowledgement when applicable.
- Ask a separate scenario author to record the gameplay Behavior, then make its tier 1 comparison
  pass in core. [Testing](../testing.md#conformance) defines the roles and the `check.py` boundary.

## 9. Write the durable test

File: `module/tests/<effect>.rs`, `#[ignore]`d like its siblings. It publishes the Module to its own
standalone, writes the real spell's `game_spell` and `game_spell_effect` rows (the effect row id is
`(spell_id << 2) | effect_index`), casts through a debug or `gw_*` reducer such as
`debug_cast_spell_at`, and reads the result with SQL. [`testing.md`](../testing.md) shows how to run
it and how CI picks up a new target.

## 10. Record the words and the client check

- A new term goes in `CONTEXT.md`, and in `CORE_TERMS.md` if a first change meets it. Distract
  added **Distraction**.
- What only a real client shows goes in a section of
  [`checks.md`](../../.claude/skills/verify-lyracore/checks.md). Distract added its Distract section.

## 11. Run the checks

Run every tier in [`testing.md`](../testing.md) that the change reaches.
