# Verifying the aura-slot cap end to end (issue #155)

The vanilla 32-buff/16-debuff cap is enforced by `pick_aura_slot`
(`module/src/spell/cast/targeting.rs`) and its caller in `aura_apply`. Two rungs prove it, per
[`architecture.md` §8](./architecture.md#8-verification): pure policy is a `cargo test` vector, the
persisted/wire behavior needs a live stack. This page is the live rung's procedure — read it when you
need to re-run the probe (a regression, a reviewer request, or before touching `pick_aura_slot` or the
overflow relay).

## What's already covered by `cargo test`

`module/src/spell/cast/targeting.rs`'s `effect_handler_tests` module:

- `sixteen_debuffs_full_refuses_the_seventeenth_leaving_the_sixteen_untouched`
- `thirty_two_buffs_full_refuses_the_thirty_third_leaving_the_thirty_two_untouched`
- `buff_and_debuff_ranges_are_independent_when_full`
- `debuffs_take_the_upper_range_of_aura_slots_buffs_the_lower_range`

These exercise `pick_aura_slot` directly (pure function, no `ReducerContext`) and prove the slot
arithmetic: a full range refuses its own next distinct aura, existing entries are never disturbed, and
the two polarities never compete for each other's slots. What they **cannot** prove: that a live
`aura_apply` call actually leaves `game_aura` untouched, emits the module log line, and reaches a
connected client as `SMSG_SPELL_FAILURE`. That's this page.

## The live probe

### Prerequisites

```bash
./lyracore dev up --single      # or plain `dev up` — either topology works, single is simpler here
printf 'test123' | ./lyracore account create TEST --password-stdin   # once per fresh stack
```

The module must be built with `--features=debug_reducers` (`lyracore publish`/`dev up` always do
this) — the probe uses `debug_fill_aura_slots` and `debug_cast_at`, both debug-only reducers
(`docs/architecture.md` §8, "Debug reducers are compiled out by default").

Get a live entity into the world once, so its guid exists in `game_world_entity` (a fresh character is
not there until it logs in):

```bash
./lyracore dev smoke                    # logs TEST's fixture character ("Ginger", guid 2) in and out
```

### `debug_fill_aura_slots` — the setup lever

```
debug_fill_aura_slots(target_guid: u64, caster_guid: u64, debuff: bool, count: u8)
```

Inserts `count` SYNTHETIC filler auras straight into `target_guid`'s buff or debuff range (spell ids
`9_000_000 + slot`, so they never collide with a real probe spell), skipping `pick_aura_slot` and the
stacking-group boundary entirely — it's setup, not the thing under test. Errors if `count` exceeds the
range's real capacity (32 buff / 16 debuff).

### 1. Fill 16 debuff slots, prove the 17th is refused and the 16 survive

Open a listener on the caster's OWN wire connection first — `SMSG_SPELL_FAILURE` (opcode `0x0133` =
307) is a caster-private packet, so the listener must be logged in as the same character that casts:

```bash
bash .lyracore/wire-harness/*/adapters/lyracore/wire.sh TEST Ginger opcode-watch 307 60 &
```

Then, from a second shell, fill and probe (guid 2 = Ginger, self-cast/self-target for simplicity):

```bash
spacetime call -s local lyracore debug_fill_aura_slots 2 2 true 16
spacetime sql   -s local lyracore "SELECT slot, spell_id FROM game_aura WHERE target_guid = 2"
#   16 rows, slots 32..47 — the real debuff range, full.

spacetime call -s local lyracore debug_cast_at 2 50011 2   # "Test Snare" — a real, distinct debuff
```

Expect, within a couple seconds:

- the background listener prints `OPCODE-WATCH PASS ✓ opcode 0x133 arrived` — the caster-visible
  `SMSG_SPELL_FAILURE` reached the live wire connection through the unmodified gateway relay
  (`gateway/src/stdb/subscriptions.rs`'s `on_cast`, unchanged — it already fires for any
  `is_interrupted` row).
- `spacetime sql ... game_aura WHERE target_guid = 2` still returns exactly the same 16 rows
  (same slots, same spell ids) — the refused 17th created no row and disturbed none of the 16.
- `spacetime logs -s local lyracore | grep overflow` shows exactly one new line:
  `aura slot overflow: all 48 slots on 2 full — dropping spell 50011 (loud: relayed to caster 2)`.

### 2. Fill 32 buff slots too, prove the 33rd is refused and the 32 survive

Same target, same connection (open a fresh `opcode-watch` listener — the first one exits after its
one match):

```bash
bash .lyracore/wire-harness/*/adapters/lyracore/wire.sh TEST Ginger opcode-watch 307 60 &
spacetime call -s local lyracore debug_fill_aura_slots 2 2 false 32
spacetime call -s local lyracore debug_cast_at 2 6673 2    # Battle Shout — a real, distinct buff
```

Expect the same three signals: the listener catches opcode `0x133`, `game_aura` for guid 2 still
returns 48 rows total (the 16 debuffs from step 1 + the 32 buffs, untouched), and the module log gains
one more overflow line naming spell 6673.

### 3. Independence — a full range never blocks the OTHER polarity

Use a second live entity (guid 1, "Tester" — needs `debug_set_health 1 100` first if it's not alive)
so the ranges start empty:

```bash
spacetime call -s local lyracore debug_fill_aura_slots 1 1 true 16   # debuff range full, buff range empty
spacetime call -s local lyracore debug_cast_at 1 6673 1              # a fresh BUFF
spacetime sql  -s local lyracore "SELECT slot, spell_id FROM game_aura WHERE target_guid = 1"
#   17 rows: the 16 synthetic debuffs PLUS spell 6673 at slot 0 — a full debuff range didn't
#   consume buff capacity.
```

Clear guid 1's auras (`spacetime sql ... "DELETE FROM game_aura WHERE id = <id>"` per row — there is
no bulk-clear reducer, on purpose: production never wants one) and repeat in the other direction:

```bash
spacetime call -s local lyracore debug_fill_aura_slots 1 1 false 32  # buff range full, debuff range empty
spacetime call -s local lyracore debug_cast_at 1 50011 1             # a fresh DEBUFF
spacetime sql  -s local lyracore "SELECT slot, spell_id FROM game_aura WHERE target_guid = 1"
#   33 rows: the 32 synthetic buffs PLUS spell 50011 at slot 32 — a full buff range didn't
#   consume debuff capacity.
```

### Cleanup

The probe's rows are ordinary `game_aura` rows with a 1-hour expiry — they age out on their own via
`tick_auras`'s expiry pass, or delete them immediately per-id as shown above if you want a clean slate
for another run.

## Result recorded 2026-08-13

Run against a freshly published `lyracore` (single-database topology), TEST/Ginger (guid 2) and TEST's
second fixture character (guid 1):

| Assertion | Result |
|---|---|
| 16 debuffs full → 17th (spell 50011) creates no row | confirmed — `game_aura` unchanged (16 rows, same slots/ids) |
| …16 existing debuffs stay in their original slots | confirmed |
| 32 buffs full → 33rd (spell 6673) creates no row | confirmed — `game_aura` unchanged (48 rows total, same slots/ids) |
| …32 existing buffs stay in their original slots | confirmed |
| One module log line per refusal, naming aura-slot overflow | confirmed — one `aura slot overflow: …` line per cast, naming the dropped spell id |
| One caster-visible `SMSG_SPELL_FAILURE` per refusal | confirmed — `opcode-watch 307` caught exactly one `0x133` per probe cast |
| A full debuff range doesn't consume buff capacity | confirmed — a fresh buff still landed at slot 0 with 16 debuffs full |
| A full buff range doesn't consume debuff capacity | confirmed — a fresh debuff still landed at slot 32 with 32 buffs full |
