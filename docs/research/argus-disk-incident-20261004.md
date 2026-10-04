# Argus disk exhaustion and write attribution

On 2026-10-04, Argus's Standalone had accumulated 3,218 automatic restarts. Its retained stderr
records `StorageFull` and failure to write `metadata.toml`. The daily pruner began at 00:16:48 UTC;
Standalone started at 00:16:55 and the pruner finished at 00:17:31. The pruner reported 58,039 MB
freed. That is its accounting, not an independent allocated-block measurement.

The Operator removed all 1,000 bots through `playerbots_despawn_all` at 05:55 UTC. All four Shards
had empty rosters afterwards; non-bot Characters and transfer intents were preserved. This incident
does not qualify the previous workload as a successful unattended endurance run.

## Measured write sources

The source under load was Core `da11dcfc71c235c0c4b7f7c34236b2c6b99bbd1d` and the previously installed
Package Collection `aeeec7ba5c10db1ac1917f26e0d1f295b9fd4efb`. SpacetimeDB is pinned to 2.7.1.

Read-only analysis sampled the oldest retained raw transaction segment on each World Shard. Each
commit's CRC32C was verified against its header and payload. All sampled commits held one
transaction. The reducer name came from its recorded inputs, without emitting argument or row
values. Framing follows the pinned upstream [commit format](https://github.com/clockworklabs/SpacetimeDB/blob/v2.7.1/crates/commitlog/src/commit.rs)
and [transaction format](https://github.com/clockworklabs/SpacetimeDB/blob/v2.7.1/crates/commitlog/src/payload/txdata.rs).

| World Shard | Sample bytes | Transactions | Bytes attributed to tick_creatures |
|---|---:|---:|---:|
| lyracore | 268,486,896 | 2,228 | 265,105,035 |
| lyracore-world-1 | 268,839,310 | 1,808 | 265,881,824 |

`tick_creatures` accounted for about 99% of bytes in both samples. It includes the Package's
decision pass, so the reducer name alone does not establish whether creatures or bots caused the
growth.

A second pass decoded row lengths in the first 16 MiB of each segment using the deployed schema.
Every transaction was consumed exactly, and its checksum verified. Only byte totals by reducer,
table and operation were retained.

| World Shard | Sample bytes | Runner row bytes, inserts and deletes | Share |
|---|---:|---:|---:|
| lyracore | 16,924,449 | 12,967,980 | 76.62% |
| lyracore-world-1 | 17,190,645 | 12,998,195 | 75.61% |

The leading table was `pkg_playerbots_runner`. `game_world_entity`, the bot roster, quest admission
and provisioning were smaller contributors. These figures measure serialized transaction bytes,
including old and new row images, rather than compressed disk growth or a whole-day average.

## Consequences

Frequent guarded retention and a disk reserve prevent another unattended fill. They do not remove
the cost of generating and compressing transaction history. The next optimization should measure
which Runner fields change per decision and separate necessary durable action progress from
diagnostic history that can be sampled less often. Do not simply slow movement or stop saving
recovery state to lower the byte count. That requires its own gameplay and recovery verification.

No new 1,000-bot population was created for this investigation. The current policy change leaves
bots removed. Retained evidence is under the Operator archive `argus-disk-policy-20261004`:
`commitlog-profile.json`, `table-write-profile.json`, schema and table identifiers, and the two
read-only analysis scripts. Raw authentication rows and reducer arguments were not exported.
