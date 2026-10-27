# Classic baseline after creature source switching

The classic creatures import now clears `game_creature_level_stats`. Without that DELETE, a
classic re-import leaves vmangos level rows behind and suppresses classic rank scaling.
The maintainer authorized this SQL change for the Milestone 3 re-check.

The full dry-run outputs have these SHA-256 hashes. They use the pinned classic dump
`d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6` and the local build-5875 client.

| Artifact | Previous baseline | New baseline |
| --- | --- | --- |
| Full-world SQL | `b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40` | `fd06e44cb7a4bd3f8c529caeebc01d0f1db327448d048bb6d2e45c7a5ebaf980` |
| Alliance-single SQL | `f1e070079f8c984f7c124cadcd49eceb657428d3b551164290b6018079644d92` | `cdd4c4461307d230da5f2c71ae859ab668798cd18fdc9ac49e9bc50f1f2be244` |
| All 467 full-world reducer batches | `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098` | unchanged |

In both SQL outputs, the only new statement follows the creature-template DELETE:

```sql
DELETE FROM game_creature_level_stats WHERE id > 0;
```

The output also adds the normal blank-line separator and increments the world-plan statement
count by one. Removing that statement and separator and decrementing that count reproduces each
previous output byte for byte. All existing SQL statements and all reducer batches are unchanged.
The serialized reducer batches also compare byte for byte with the previous receipt.

The durable importer test `classic_reimport_restores_rank_health_and_melee_damage` loads a
hand-authored vmangos template through the importer, then re-imports its classic counterpart.
The spawned rank-three creature changes from 100 health and 20 melee damage to 400 health and
50 melee damage. This verifies the source switch through the Module, beyond the SQL comparison.
