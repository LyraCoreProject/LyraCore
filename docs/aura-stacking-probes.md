# Aura stacking-family live probes

Deterministic tests already pin the stacking policy itself (`module/src/spell/stacking.rs`), and a
tripwire pins the single aura-insertion boundary. This page covers the layer those cannot reach: a
real `ReducerContext`, real `game_aura` rows, and the family decision as an operator sees it on a
development database.

Run these probes on a development database only. Every step writes gameplay state.

## Scope and prerequisites

- The module must be published with debug reducers. `./lyracore publish` bakes them in; a bare
  `spacetime publish` does not. Follow [`danger-zones.md`](./danger-zones.md) §3 for the deploy, and
  never pass `-c`.
- `scripts/publish-module.sh` calls `debug_repair_after_publish` after every publish. That reducer
  re-seeds the stacking families and the probe fixture, because `init` does not re-run on an
  auto-migrating publish. Call it by hand if you published another way:

  ```bash
  spacetime call -s local lyracore debug_repair_after_publish
  ```
- Every command below uses `-s local <database>`. Substitute your own server nickname and database.
- `spacetime sql` accepts no `ORDER BY`, no `IN`, and no subqueries. `spacetime call` truncates
  integers above 2^53, so pass creature GUIDs as quoted strings; character GUIDs are small.
- Record the module commit under test. A probe result without one cannot be reproduced.

## Fixture and characters

`seed_stacking_probe_fixture` (`module/src/seed/fixtures.rs`) supplies the four family members the
probes need, because a curated sandbox carries only rank 1 of each family and every one of those is
self-cast. Each spell is inserted only when the catalogue does not already hold it, so an imported
Spell.dbc keeps its own rows.

| Spell | Id | Family | Rule | Magnitude |
|---|---|---|---|---|
| Power Word: Fortitude rank 1 | 1243 | 2 | EXCLUSIVE_STRONGER | +3 stamina |
| Prayer of Fortitude rank 1 | 21562 | 2 | EXCLUSIVE_STRONGER | +26 stamina |
| Blessing of Might | 19740 | 3 | EXCLUSIVE_PER_CASTER | +20 attack power |
| Blessing of Wisdom | 19742 | 3 | EXCLUSIVE_PER_CASTER | +10 spirit |

Family 2 has `rank_is_comparable = false`, so its members are compared by effect magnitude, not by
rank number. The pair is drawn from the family's two different chains on purpose: `aura_apply`
displaces an active aura of the same spell NAME before any family policy runs, so two ranks of one
chain never reach the strength comparison. That precedence predates the stacking families and is
unchanged here. Confirm the data before probing:

```bash
spacetime sql -s local lyracore "SELECT spell_id, name FROM game_spell WHERE spell_id > 1242 AND spell_id < 1244"
spacetime sql -s local lyracore "SELECT spell_id, name FROM game_spell WHERE spell_id > 21561 AND spell_id < 21563"
spacetime sql -s local lyracore "SELECT group_id, rule, rank_is_comparable FROM game_spell_group_rule"
spacetime sql -s local lyracore "SELECT group_id, spell_id FROM game_spell_group WHERE group_id = 3"
```

Pick three characters — two casters and one target — and note their GUIDs:

```bash
spacetime sql -s local lyracore "SELECT guid, name, class, level FROM game_character"
```

Each of the three needs a live world entity, and the casters must stand within the 30-yard range of
the target:

```bash
spacetime call -s local lyracore debug_spawn_player_entity <caster_a>
spacetime call -s local lyracore debug_spawn_player_entity <caster_b>
spacetime call -s local lyracore debug_spawn_player_entity <target>
spacetime sql  -s local lyracore "SELECT guid, map_id, x, y, z FROM game_world_entity WHERE guid = <target>"
spacetime call -s local lyracore debug_teleport <caster_a> <map_id> <x> <y> <z> 0
spacetime call -s local lyracore debug_teleport <caster_b> <map_id> <x> <y> <z> 0
```

`debug_cast_at` resolves synchronously and bypasses the spellbook gate, so a caster of any class can
apply either family. Between probes, clear the target's auras — a probe-only direct write, and the
reason these steps belong on a development database:

```bash
spacetime sql -s local lyracore "DELETE FROM game_aura WHERE target_guid = <target>"
```

## Probe 1 — strongest family member survives, in both orders

Weaker first, then stronger. The stronger application must evict the weaker one:

```bash
spacetime call -s local lyracore debug_cast_at <caster_a> 1243 <target>
spacetime sql  -s local lyracore "SELECT id, spell_id, caster_guid, slot, amount, stacks FROM game_aura WHERE target_guid = <target>"
spacetime call -s local lyracore debug_cast_at <caster_b> 21562 <target>
spacetime sql  -s local lyracore "SELECT id, spell_id, caster_guid, slot, amount, stacks FROM game_aura WHERE target_guid = <target>"
```

Pass: after the first cast exactly one row, `spell_id` 1243, `caster_guid` = caster A, `amount` 3.
After the second exactly one row, `spell_id` 21562, `caster_guid` = caster B, `amount` 26. The
weaker row is gone. Different casters do not make the family tolerate two members.

Now the reverse order. Clear the target, apply the stronger member first, then attempt the weaker:

```bash
spacetime sql  -s local lyracore "DELETE FROM game_aura WHERE target_guid = <target>"
spacetime call -s local lyracore debug_cast_at <caster_b> 21562 <target>
spacetime sql  -s local lyracore "SELECT id, spell_id, caster_guid, applied_at, expires_at, amount FROM game_aura WHERE target_guid = <target>"
spacetime call -s local lyracore debug_cast_at <caster_a> 1243 <target>
spacetime sql  -s local lyracore "SELECT id, spell_id, caster_guid, applied_at, expires_at, amount FROM game_aura WHERE target_guid = <target>"
```

Pass: the weaker application is refused. The two queries return the identical single row — same
`id`, same `caster_guid`, same `applied_at` and `expires_at`. A refusal must not refresh, re-stamp
or displace the stronger aura, and it must not create a row of its own. The module log records
`stacking group 2: spell 1243 onto <target> refused (weaker)`.

## Probe 2 — one Blessing per paladin, two paladins coexist

```bash
spacetime sql  -s local lyracore "DELETE FROM game_aura WHERE target_guid = <target>"
spacetime call -s local lyracore debug_cast_at <caster_a> 19740 <target>
spacetime call -s local lyracore debug_cast_at <caster_a> 19742 <target>
spacetime sql  -s local lyracore "SELECT id, spell_id, caster_guid, slot, amount FROM game_aura WHERE target_guid = <target>"
spacetime call -s local lyracore debug_cast_at <caster_b> 19740 <target>
spacetime sql  -s local lyracore "SELECT id, spell_id, caster_guid, slot, amount FROM game_aura WHERE target_guid = <target>"
```

Pass: after caster A's second Blessing exactly one row remains — `spell_id` 19742, `caster_guid` =
caster A. A paladin's new Blessing replaces that paladin's prior Blessing. After caster B's cast
there are two rows, 19742 from caster A and 19740 from caster B, in different slots. One paladin
never erases another paladin's contribution.

## Probe 3 — persisted rows agree with the visible decision

After each probe above, read the rows back and check three things against what the client shows:

```bash
spacetime sql -s local lyracore "SELECT id, spell_id, caster_guid, slot, eff_kind, amount, stacks, expires_at FROM game_aura WHERE target_guid = <target>"
spacetime sql -s local lyracore "SELECT guid, health, max_health FROM game_world_entity WHERE guid = <target>"
```

- The surviving `spell_id` and `caster_guid` are the ones the decision named.
- `amount` equals the surviving spell's magnitude, so the effect value the combat folds read agrees
  with the icon the player sees.
- A Fortitude swap moves `max_health`, because stamina auras re-derive vitals at the boundary.

## Probe 4 — a multi-effect spell is one icon and does not fight itself

Mark of the Wild (1126) carries three stat effects. It is self-cast in the curated catalogue, so
cast it on the caster itself:

```bash
spacetime sql  -s local lyracore "DELETE FROM game_aura WHERE target_guid = <caster_a>"
spacetime call -s local lyracore debug_cast_at <caster_a> 1126 <caster_a>
spacetime sql  -s local lyracore "SELECT id, spell_id, slot, eff_kind, eff_p0, amount FROM game_aura WHERE target_guid = <caster_a>"
```

Pass: three rows, one per stat effect, all carrying the same `slot`. One buff icon, three effects,
and no effect of the spell evicts or refuses a sibling effect of the same spell.

## Evidence template

```text
UTC date/time:
Operator:
Module commit under test:
Database and server:
Caster A / caster B / target GUIDs:

Family data check (group 2 rule/rank_is_comparable, group 3 rule):
Probe 1 weak-then-strong rows before/after:
Probe 1 strong-then-weak rows before/after (aura id, applied_at, expires_at):
Probe 1 module log line for the refusal:
Probe 2 rows after caster A's second Blessing:
Probe 2 rows after caster B's Blessing:
Probe 3 amount and max_health readings:
Probe 4 row count and shared slot value:

Verdict: PASS / FAIL / INCONCLUSIVE
Raw command output:
```
