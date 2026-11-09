# 0008. Migrate Characters from MaNGOS-family servers

## Status

Proposed, 2026-11-09.

## Context

Operators of vmangos and cmangos servers want to move their players to LyraCore. Both servers keep
Accounts in a realmd database and Characters in a characters database, in MySQL. Their tables
share one layout and differ mostly in column names. LyraCore imports its world from vmangos
([ADR 0004](./0004-import-world-data-from-vmangos.md)), so item, spell, quest, faction, skill and
taxi ids in a source Character name the same content on a LyraCore Realm. cmangos classic uses the
same 1.12 ids, except for custom content.

Both realmd schemas store the SRP6 salt and verifier, not a password. LyraCore keeps the same two
values. A migrated Account therefore keeps its password.

The Character Transfer blob cannot carry a migration. It is the BSATN encoding of the Module's
internal rows, so a tool outside the Module would break on every schema change, and its import
skips the reference checks a foreign source needs.

## Decision

1. The Module owns two Operator reducers, `migrate_account` and `migrate_character`. Their
   arguments are `SpacetimeType` structs in the vanilla shape of the data, not LyraCore rows. The
   Module translates bag positions, talent rank spells, earned reputation and taxi node ids,
   checks every reference against its catalogues and writes one Character in one transaction.
   Any refusal leaves no Account, Character or record.
2. A Character gets new guids. `game_character_migration` records its Migration Source label and
   source guid, and outlives the Character. A rerun of the same source Character changes nothing,
   even after the player deleted the migrated Character.
3. An Account name that already exists is the same Account only when its salt and verifier match.
   Otherwise the migration refuses. GM levels never migrate.
4. A separate tool reads the Migration Source through a read-only MySQL connection in one
   consistent snapshot. It never executes source SQL or scripts
   ([ADR 0003](./0003-parse-world-data-never-execute.md)). It checks each consumed column against
   a Source Profile, builds the reducer arguments, reports what it drops and why, and calls the
   reducers.
5. The first version supports a single-Shard Realm. A split Realm needs Realm Shard name
   reservations and routing to the World Shard that owns each map.

## Consequences

- The reducer arguments are a public contract. A change to them changes the tool in the same
  change.
- State that the Module cannot hold where the source kept it, such as Items in bank bags, moves to
  a free slot or refuses the Character. It is never dropped without a line in the report.
- Source state without a LyraCore equivalent, such as Auras, cooldowns and Instance bindings, is
  dropped and named in the report.
