# Browser deployment verification

Recorded on 2026-10-18. Account passwords and service credentials are excluded.

## Target

- Host / checkout: Argus, `159.69.88.70`, `/home/wave-defense/LyraCore`.
- Approved configuration source: the existing Wave Defense deployment and the user's request to
  host Manila with LyraCore accounts and the addon. Protected settings remain in
  `/etc/wave-defense/gateway.env` and `/etc/wave-defense/web.env`.
- SpacetimeDB node: `http://127.0.0.1:3001`, version 2.7.1, database `wave-defense`.
- Commit: Core `de503130` -> `718daae2`; browser host `c10b8da`; Packages `128e864`;
  native Benilla addon mirror `d6744fae`.
- Mode: update.

## Topology

- Expected: one cohosted Shard for Realm-core, Character state and the Instance Pool. Matches
  have separate Instances. The browser host relays to the existing Gateway.
- Discovered: the same topology, with the browser host at `127.0.0.1:8090` behind Nginx.
- Gateway configured: `wave-defense`, public logon port 3724 and world port 8085, advertised
  address `159.69.88.70:8085`. The replacement process environment matches the approved settings.

## Checks

| Check | Outcome | Evidence |
| --- | --- | --- |
| Core deployment gate | PASS | Bindings, importer, Module, Gateway, shared, test support, workspace build and preflight completed. `browser-core-gate.log` ends with `BROWSER_CORE_DEPLOY_GATE_PASS`. |
| Guarded publish | PASS | Explicit `wave-defense` target, no clear flag, repair after publish and Gateway build completed. `browser-core-publish.log` ends with `BROWSER_CORE_PUBLISH_PASS`. |
| Live connectivity | PASS | Published schema reachable, coordinator connected, both listeners present, no startup errors, credential warning, address mismatch or unresolved occupancy warning. |
| Standard topology parser | WARN | `browser-core-status.log` reports two failures because it expects split-Shard startup markers. The approved cohosted Shard has neither marker. Live browser login and world entry independently passed. |
| Host checks | PASS | 9 unit, 15 application and 2 session tests passed. Optional live MySQL coverage had no configured instance and is not evidence for a MySQL deployment. |
| Wasm artifact | PASS | Workstation build and installed bundle match SHA-256 `e841f9677302d7fa122da6f83ea9e52535ec37f44845ee4b7f30d1a4cd539102`. |
| Public HTTPS | PASS | Trusted certificate for IP `159.69.88.70`; public requests verify without a TLS bypass. ACME staging dry run and production issuance passed. |
| Browser runtime | PASS | Chrome reported a secure context and a WebGPU adapter. No JavaScript page errors occurred during the lobby check. |
| Access controls | PASS | Anonymous addon, asset, play and socket requests returned 401. A tester awaiting password change received 403 for addon access. Cross-site socket request returned 403. |
| Account provisioning | PASS | Admin setup and tester invitation created separate game Accounts through the create-only command. The tester changed the temporary web password before play. |
| Game login and lobby | PASS | Real SRP login, Character creation and world entry succeeded. Both test Characters were level 1 in Scarlet Monastery with WaveDefense loaded. |
| Addon controls | PASS | Specialization selection, queue admission and leaving the queue succeeded. Queueing before specialization selection was refused. |
| Fresh Character cinematic | PASS | A second fresh Character reached the visible lobby. The validated addon reply ended the opening cinematic. |
| Addon checks | PASS | Lua 5.0.3 UI flow, 13 protocol/request tests and the required 37-test Package API check passed. Installed addon hash matches Packages `128e864`. |
| Client archives | PASS | Health reports readable client data. Original archives and permissions are unchanged; the web service uses the documented archive view. |
| Cleanup | PASS | Tester signed out; subsequent play request returned 401. Browser closed and Gateway reported zero sessions. |
| Comment review | PASS | Final source, test, script and configuration changes reviewed against the comment policy. No unresolved findings. Documentation links resolve. |

Deployment logs are in `/home/wave-defense/deploy-logs`. The host build log is
`browser-host-final-build.log`. Certificate logs are `ip-certificate-staging.log` and
`ip-certificate-production.log`.

## Service state

- SpacetimeDB: `wave-defense-spacetimedb.service` active.
- Gateway: `wave-defense-gateway.service` active, PID 2107454, zero service restarts after update.
- Browser host: `wave-defense-web.service` active, PID 2102724, zero service restarts.
- Listeners: game ports 3724 and 8085; public IP ports 80 and 443; web host on loopback 8090.
  Tailscale retains HTTPS on its own addresses.
- Connected databases: `wave-defense` on the dedicated port-3001 node.
- Nginx, Discord signup and the certificate timer are active. Certificate renewal checks run
  every four hours. Health reports `backend: lyracore`, SQLite and client data ready, no
  client-data error, and null MySQL/SOAP status.

## Warnings

- The standard status parser assumes a split topology. Its two missing-marker failures need
  interpretation against this approved deployment until the parser supports a cohosted Shard.
- Browser verification covers login, world entry and lobby controls. An attended Party still
  needs to test combat, shops, bosses, balance and the intended thirty-minute round.
- The new realm and web account state have no off-host backup. Preserve the web state directory
  with its encryption key when adding one. Keep the old realm's backup timer disabled because
  its script restarts the old services.
- T3 preview snapshots timed out twice. Verification used an isolated Chrome session through
  Playwright. Optional missing-asset lookups occurred, but the lobby rendered successfully.

## Blockers

- None for browser tester access.

## Next actions

1. Retrieve the admin handoff using [the browser runbook](BROWSER.md), then invite testers at
   `/admin/users`.
2. Run an attended five-player match and record gameplay defects separately from this access check.
