# Argus Wave Defense realm

The defense realm runs as Unix account `wave-defense` on Argus, `159.69.88.70`.
Cutover completed on 2026-10-18. See [verification](VERIFICATION.md) for checks and limits.
Browser hosting and admin access are described in [the browser runbook](BROWSER.md).
It uses one database, `wave-defense`, on its own SpacetimeDB node at `127.0.0.1:3001`.
Realm-core, Character state and the Instance Pool share that database. This topology is required
by the mode's Party admission rules. Every match still has a separate Instance.

## Sources and paths

| Component | Source or path |
| --- | --- |
| Core | `LyraCoreProject/LyraCore`, branch `t3/team-wave-defense-mode` |
| Packages | `LyraCoreProject/packages`, branch `t3/wave-defense` |
| Optional addon copy | `Mathih13/benilla`, branch `t3/wave-defense` |
| CLI bootstrap fix | `LyraCoreProject/lyracore-cli`, commit `7fbc8f0dbb45219c80662bca6c448335aeb67f24` |
| Core checkout | `/home/wave-defense/LyraCore` |
| Packages checkout | `/home/wave-defense/packages` |
| Database storage | `/var/lib/wave-defense/spacetimedb` |
| Node signing keys | `/var/lib/wave-defense/keys` |
| Gateway configuration | `/etc/wave-defense/gateway.env` |
| Signup configuration | `/etc/wave-defense/discord-bot.env` |
| Logs | `/var/log/wave-defense` and `/home/wave-defense/deploy-logs` |

The Gateway binds public ports 3724 and 8085 and advertises `159.69.88.70:8085`.
During staging it uses loopback ports 13724 and 18085. Its session limit is 100, with 128 blocking
threads, admission concurrency 10 and at most eight entering sessions. These are operating limits,
not measured capacity. AOI is enabled and `MALLOC_ARENA_MAX` is 2.

The environment files and CLI credentials stay on Argus. Do not commit or print them.
The addon requires no Benilla engine changes. Install it as described in the Benilla branch's
`addons/README.md` or the Package README.

## Services

The units beside this document run the database, Gateway, Discord signup bot, history pruning and
disk guard. The node uses the pinned SpacetimeDB 2.7.1 executable. Database and Gateway logs append
to files under `/var/log/wave-defense`.

The signup bot is copied from the existing Argus installation. Its configuration points both
account databases at `wave-defense`, clears the shard map and uses a separate account mapping file.
It starts with a fresh player account list. The old realm's accounts and Characters remain in its
original databases. Ordinary new accounts have Alpha Test Tools disabled.

The disk guard is copied from `/opt/lyracore/bin/lyracore-disk-guard.py`, with its prune-service name
changed to `wave-defense-prune.service`. Its configuration names only the new database and node
CLI. The timer renews Playerbots capacity while disk reserve remains available. History pruning
runs every fifteen minutes and retains the recovery history required by the existing prune tool.
Start the prune timer before the initial manual prune, so systemd retains the service's completion
timestamp for the disk guard. Resume the guard only after a successful prune and sufficient reserve.

## Deployment checks

Before publishing, run the complete gate in `docs/danger-zones.md`. Publish the explicit database
name through `./lyracore publish wave-defense`, with the new account's `local` server selector
pointing to port 3001. Never use a clear-publish flag or the old account's CLI configuration.

Use the Package's `content-notes.md` to generate authored creature and item Deltas from local game
data. Import DBC references, spells, talents, creatures, items, globals and the `scarlet-courtyard`
Vmap Slice. Prepare Navigation Coverage, enable collision and navigation, then complete the import
for map 189. Start the staging Gateway before calling `wave_defense_configure true`, so its
single-Shard certificate exists. Configuration validates routes, floors and required content.

This deployment imports the shared spell, talent, item and loot catalogues, plus character creation
data. World geometry and population are limited to the courtyard profile. The shared catalogues
contain more entries than the mode uses. A smaller import selection remains future work.
The shop-item Delta exceeds Linux's command argument limit. Send it as a JSON request body through
the [SpacetimeDB reducer HTTP endpoint](https://spacetimedb.com/docs/http/database/), loading the
Operator credential on Argus. Keep the authored content on Argus and the data-owning workstation.

The live wire check uses a dedicated account and proves authenticated login, level-one lobby,
queue admission, four labeled bots completing the Party, the initial draft, and return to the lobby.
It leaves before combat. An attended five-person run is still needed for client visuals, shops,
combat balance and the intended thirty-minute duration.

## Rollback

The old checkout, environment files and four databases are retained. Stop the new signup bot,
Gateway, disk guard and prune timer before restoring the old services. Stop the new node after its
Gateway. Start `spacetimedb-standalone.service`, wait for all four old databases to answer schema
requests on port 3000, then start `lyracore-gateway.service` and `lyracore-discord-bot.service`.
Restore their enabled state and the old backup, prune and disk-guard timers.

The old nightly cold-backup timer must stay disabled while Wave Defense owns the public ports:
its script restarts the previous realm. The new realm has database snapshots and retained recovery
history, but no separate off-host backup has been configured.
