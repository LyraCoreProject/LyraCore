# Crowd-control diminishing-returns probe

This is the live acceptance for
[#156](https://github.com/LyraCoreProject/LyraCore/issues/156): the duration ladder, the persisted
per-target category state, removal-based window timing, temporary immunity, the creature exemption,
and reaping of lapsed rows, all observed against real entities on a development database.

The probe uses a hostile creature as the caster, so it needs neither a duel nor the player-versus-player
permission model. The pure timing policy is covered separately by the module's own tests
(`spell::stacking::tests::player_target_dr_timeline_end_to_end_on_a_twenty_second_control` and its
neighbours); this document covers what only a live database can show — persisted `game_dr_state` rows,
the aura expiry timestamps that must agree with them, and the event-reaper pass.

## Prerequisites

- A development database published with `--features=debug_reducers`. Never run this against a realm
  carrying real players: it applies crowd control to a live character and deletes rows.
- The curated fixture content: `Test Poly` (spell 50023, 10 s, `A_CONTROL`/`M_POLY`), `Dispel Magic`
  (spell 527), and the `Test Wolf` creature template (51000, hostile, zero melee damage). Reseed with
  `debug_repair_after_publish` if any is missing.
- A character to receive the control. The commands below use guid 1; set `PLAYER_GUID` otherwise.
- `spacetime call` truncates u64 arguments above 2^53 when they are quoted as strings — pass creature
  guids as bare numbers, exactly as the script does.

## Running the probe

```bash
#!/usr/bin/env bash
set -u
SRV="${SPACETIME_SERVER:-local}"
DB="${LYRACORE_DB:-lyracore}"
PLAYER="${PLAYER_GUID:-1}"

sql()  { spacetime sql  --server "$SRV" "$DB" "$1" 2>/dev/null; }
call() { spacetime call --server "$SRV" "$DB" "$@" 2>&1 | grep -v 'WARNING'; }
show() {
  echo "--- $1 (t+$SECONDS s)"
  sql "SELECT id, caster_guid, spell_id, applied_at, expires_at FROM game_aura WHERE target_guid = $PLAYER" | tail -n +2
  sql "SELECT id, target_guid, category, level, window_expires_micros FROM game_dr_state" | tail -n +2
}

call debug_spawn_player_entity "$PLAYER" >/dev/null
call debug_spawn_at_feet "$PLAYER" 51000 3 >/dev/null
CASTER=$(spacetime logs --server "$SRV" "$DB" -n 20 2>/dev/null | grep -o 'spawned entry 51000 as guid [0-9]*' | tail -1 | grep -o '[0-9]*$')
call debug_spawn_at_feet "$PLAYER" 51000 5 >/dev/null
VICTIM=$(spacetime logs --server "$SRV" "$DB" -n 20 2>/dev/null | grep -o 'spawned entry 51000 as guid [0-9]*' | tail -1 | grep -o '[0-9]*$')
echo "caster creature: $CASTER   creature target: $VICTIM   player target: $PLAYER"
sql "DELETE FROM game_aura WHERE target_guid = $PLAYER" >/dev/null
sql "DELETE FROM game_dr_state" >/dev/null

echo "=== creature target"
call debug_force_cast_at "$PLAYER" 50023 "$VICTIM"
sql "SELECT id, applied_at, expires_at FROM game_aura WHERE target_guid = $VICTIM" | tail -n +2
sleep 2
call debug_force_cast_at "$PLAYER" 50023 "$VICTIM"
sql "SELECT id, applied_at, expires_at FROM game_aura WHERE target_guid = $VICTIM" | tail -n +2
sql "SELECT id, target_guid, level FROM game_dr_state" | tail -n +2
sql "DELETE FROM game_aura WHERE target_guid = $VICTIM" >/dev/null

echo "=== player target"
SECONDS=0
call debug_force_cast_at "$CASTER" 50023 "$PLAYER"; show "cast 1 — expect 100% (10s), level 1"
sleep 2
call debug_force_cast_at "$CASTER" 50023 "$PLAYER"; show "cast 2 while still active — expect 50% (5s), level 2"
sleep 2
call debug_force_cast_at "$CASTER" 527 "$PLAYER";   show "dispel — early removal, window from the actual removal"
sleep 2
call debug_force_cast_at "$CASTER" 50023 "$PLAYER"; show "cast 3 — expect 25% (2.5s), level 3"
sleep 2
call debug_force_cast_at "$CASTER" 50023 "$PLAYER"; show "cast 4 while still active — expect refused (immune), row unchanged"
sleep 3
show "after natural expiry — window re-stamped at the reaper's removal instant"
call debug_force_cast_at "$CASTER" 50023 "$PLAYER"; show "cast 5 inside the window — expect refused (immune)"
sleep 14
call debug_force_cast_at "$CASTER" 50023 "$PLAYER"; show "cast 6 after the window lapsed — expect 100% (10s), level 1"
sleep 2
call debug_force_cast_at "$CASTER" 527 "$PLAYER";   show "dispel — shorten the window so the reaper pass is observable"
sleep 17
show "after the reaper pass — expect no DR row"
```

The 2-second gaps are the global cooldown between two casts by the same caster. The 14- and
17-second waits are the 15-second window plus the event reaper's own cadence.

## What each step must show

| Step | Aura row | `game_dr_state` |
| --- | --- | --- |
| cast 1 | `expires_at - applied_at` = 10 s | level 1, window = aura expiry + 15 s |
| cast 2 (prior aura still active) | 5 s | level 2, window = new expiry + 15 s |
| dispel | aura gone | level unchanged, window = dispel instant + 15 s, **earlier** than before |
| cast 3 | 2.5 s | level 3 |
| cast 4 (prior aura still active) | aura row byte-identical | level and window byte-identical |
| natural expiry | aura gone | level unchanged, window = the reaper's removal instant + 15 s, **later** than the value stamped at apply |
| cast 5 | no aura | unchanged |
| cast 6 (window lapsed) | 10 s | level 1 — a fresh chain |
| after the reaper pass | — | no row |
| creature target, both casts | 10 s each | no row for the creature at any point |

The dispel row is the load-bearing one: it proves the window starts at the *actual* removal rather
than the scheduled expiry, because it moves the window earlier than the value the application had
already stamped. Break-on-damage removal stamps the window through the same call
(`spell::control::break_auras_on_damage`); the curated fixture control spells carry
`aura_interrupt = 0`, so dispel is the early-removal path this probe drives.

## Recorded run

Development database `lyracore` on the local standalone node, 2026-08-13, target character guid 1,
caster creature guid 17379391817660760067. The creature-target lines are from a second run of that
section alone, against creature guid 17379391817660760069.

```text
creature target, cast 1  applied 12:38:00.600 expires 12:38:10.600   (10.000 s)
creature target, cast 2  applied 12:38:02.646 expires 12:38:12.646   (10.000 s)
creature target          no game_dr_state row at any point

cast 1     applied 12:35:40.975 expires 12:35:50.975  (10.000 s)  level 1  window 1786624565975197
cast 2     applied 12:35:43.073 expires 12:35:48.073  ( 5.000 s)  level 2  window 1786624563073387
dispel     aura removed at      12:35:45.125                      level 2  window 1786624560125557
cast 3     applied 12:35:47.221 expires 12:35:49.721  ( 2.500 s)  level 3  window 1786624564721226
cast 4     aura row unchanged (id 4, expires 12:35:49.721)         level 3  window 1786624564721226
expiry     aura removed at      12:35:50.094                       level 3  window 1786624565094331
cast 5     no aura placed                                          level 3  window 1786624565094331
cast 6     applied 12:36:06.464 expires 12:36:16.464  (10.000 s)  level 1  window 1786624591464695
dispel     aura removed at      12:36:08.520                       level 1  window 1786624583520890
reaper     no game_dr_state row
```

Every window value is exactly the stamping instant plus 15,000,000 µs. The dispel at 12:35:45.125
moved the window 2.95 s *earlier* than the value cast 2 had stamped from its scheduled expiry. The
natural expiry moved it 0.37 s *later* than the value cast 3 had stamped, that being the aura
reaper's cadence between the scheduled expiry and the removal it observed. Cast 6 carries a new row
id (3, where the earlier chain held id 2), so the reaper had already dropped the lapsed row before
the fresh chain began.
