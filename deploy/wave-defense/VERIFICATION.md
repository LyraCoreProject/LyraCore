# Argus deployment, 2026-10-18

## Target

- Host / checkout: `159.69.88.70`, `/home/wave-defense/LyraCore`.
- Approved configuration source: dedicated defense realm requested by the maintainer;
  installed `wave-defense-*.service` units and `/etc/wave-defense/gateway.env`.
- SpacetimeDB node: the new account's `local` selector, `http://127.0.0.1:3001`.
- Commit: old realm `5df1865ae2c0fb17c4dd5f78b054aa4f5c781b4a` to new realm binary
  `d71437d2`, with Packages `df14a41` and CLI `7fbc8f0dbb45219c80662bca6c448335aeb67f24`.
  Later documentation commits do not change the deployed binary.
- Mode: update, replacing a test realm with a separate dedicated realm.

## Topology

- Expected: one Shard, `wave-defense`, sharing Realm-core and Character state. Each match has its
  own Instance. The node listens only on loopback port 3001.
- Discovered: `wave-defense`, identity
  `c20082b042df30731db8b3e71e2bedbcb42a4728e6279594ed990414d41fb2b7`.
- Gateway configured: the expected Shard and node, AOI enabled, Owner Token present, public
  ports 3724 and 8085, advertised address `159.69.88.70:8085`. The process environment matches
  the service configuration. No split-topology variables are set.

## Checks

| Check | Outcome | Evidence |
| --- | --- | --- |
| Complete Linux deployment gate | PASS | `deploy-logs/build-gate.log`, `DEPLOY_GATE_PASS`; all required crate targets and preflight pass |
| CLI bootstrap fix | PASS | 813 unit tests, CLI smoke and eight Package tests pass on Linux |
| Guarded publish | PASS | `deploy-logs/publish.log`, `PUBLISH_PASS`; no clear-publish |
| Content and geometry | PASS | 51 authored shop items, active Vmap generation `5996660338920838971`, complete Navigation Coverage, mode configuration accepted |
| Staging login and matchmaking | PASS | `deploy-logs/defense-wire.log`, `DEFENSE_WIRE_PASS`; real SRP, lobby, 60-second queue, four labeled bots, draft choice, lobby return |
| Public client path | PASS | `deploy-logs/public-smoke.log`, authenticated world entry using the advertised public address; both ports reachable from the workstation |
| Service stability and cleanup | PASS | Second socket observation retains the same PIDs and listeners, zero restarts; abandoned test match reaped, zero active matches and sessions |
| Live configuration | PASS | All environment-file values match the Gateway process; credentials remain remote |
| Signup bot | PASS | `wave-defense-discord-bot.service` logs its ready state against the new configuration |
| Bot capacity and history retention | PASS | Expired lease refused a spawn; successful prune and resumed disk guard report healthy with no failed databases |
| Standard production-status parser | WARN | Connectivity, schema, listeners, startup errors and credentials pass; split-topology markers fail because the approved realm uses one Shard |
| Benilla addon compatibility | PASS | Stock FrameXML addon check reports zero load or session errors; no Benilla engine changes |
| Comment review | PASS | Deployment units and operational helper changes checked; no explanatory comments added |

Logs named above are under `/home/wave-defense` on Argus. Local content import evidence is under
`/tmp/wave-defense-argus-*.log` on the data-owning workstation. The first shop-item transfer hit the
Linux argument limit; retrying the Delta through the authenticated HTTP endpoint succeeded.
The first Headless Client attempt queued before lobby admission; the final run handles map-transfer
acknowledgements and waits for admission. Neither retry changed gameplay code.

## Service state

- SpacetimeDB: `wave-defense-spacetimedb.service` active and enabled; PID 2012929, zero restarts.
- Gateway: `wave-defense-gateway.service` active and enabled; public PID 2082429 replaces staging
  PID 2080455.
- Listeners: Gateway owns `0.0.0.0:3724` and `0.0.0.0:8085`; node owns `127.0.0.1:3001`.
- Connected databases: Coordinator subscriptions and all four call pipes connected to `wave-defense`.
- Signup, prune timer and disk-guard timer are active and enabled. Old Gateway, signup bot and node
  are stopped and disabled. Old backup, prune and disk-guard timers are stopped and disabled.
  Their previous states are recorded in `deploy-logs/old-unit-states.json`.

## Warnings

- Attended Benilla combat is unverified. Run five people through targeting, shops, deaths,
  resurrection, reconnect and bosses before judging balance or the intended thirty-minute duration.
- Limits of 100 sessions and 64 active matches are operating bounds, not measured capacity.
- The standard status parser assumes split topology. Its two topology failures do not describe this
  deliberately single-Shard realm; use the recorded configuration, connectivity and client evidence.
- There is no off-host backup for the new realm. Configure one before relying on durable player
  progress; do not re-enable the old backup job because it starts the old services.

## Blockers

- None for operating the prototype. Attended combat and capacity measurement remain unproved.

## Next actions

1. Install the WaveDefense addon on stock Benilla, create a fresh account through the Discord signup
   bot, and connect to `159.69.88.70` for an attended Party run.
