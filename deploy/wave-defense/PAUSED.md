# Wave Defense pause and original realm restoration

Recorded on 2026-10-18 after the user requested a pause and restoration of the old test server.

## Target

- Host / checkout: Argus, `159.69.88.70`, `/home/lyracore/LyraCore`.
- Approved configuration source: preserved systemd units, `/etc/lyracore/gateway.env`, and
  `/home/wave-defense/deploy-logs/old-unit-states.json` from the original cutover.
- SpacetimeDB node: `http://127.0.0.1:3000`.
- Commit: `5df1865ae2c0fb17c4dd5f78b054aa4f5c781b4a`, unchanged.
- Mode: update of service state only. No build, publish, import or source update was required.

## Topology

- Expected: `lyracore`, `lyracore-world-1`, `lyracore-instances`, `lyracore-realm`.
- Discovered: all four databases retained and reachable on the original node.
- Gateway configured: all four databases, distinct Realm-core, original map routes, AOI enabled,
  original capacity limits, and advertised world address `159.69.88.70:8085`.

## Checks

| Check | Outcome | Evidence |
| --- | --- | --- |
| Inventory and startup | PASS | All four schema endpoints returned HTTP 200 before the Gateway started. |
| Production status | PASS | Pinned parser reports HEALTHY; all four Coordinator connections and Realm-core active. |
| Configuration | PASS | Every preserved environment-file value matches the running Gateway. Credentials stayed on Argus. |
| Client ports | PASS | Gateway owns ports 3724 and 8085; both accept public TCP connections from the workstation. |
| Stability | PASS | Second observation retains node, Gateway and signup PIDs with zero restarts. |
| Signup | PASS | Original Discord account bot reports ready. |
| Retention | PASS | Original prune completed successfully; maintenance and backup timers restored. |
| Disk guard | PASS | Resumed its suspension latch after successful pruning and more than 30 GiB free. Reports healthy with no failed databases; limits unchanged. |
| Wave Defense pause | PASS | Browser, signup, Gateway and node stopped and disabled; prune and disk-guard timers stopped and disabled. |
| Browser response | PASS | Nginx validation passed; public HTTPS returns an explicit pause message with HTTP 503. |

Status evidence is `/home/wave-defense/deploy-logs/old-realm-restored-status.log` on Argus.
The Gateway startup capture is `/var/log/lyracore/restored-gateway-20261018.log`.

## Service state

- SpacetimeDB: original service active and enabled, PID 2115861, loopback port 3000.
- Gateway: original service active and enabled, PID 2116379.
- Listeners: public game ports 3724 and 8085; no Wave Defense node or browser host listener.
- Connected databases: all four original databases.
- Original signup, prune, disk guard and nightly backup timers restored to their saved state.
- Wave Defense data, web accounts, credentials, client files and source checkouts remain intact.
  Nginx and the HTTPS certificate renewal timer remain active.

## Warnings

- This restoration verifies service health and public TCP access, not a fresh in-game login.
- Restoring the nightly backup timer does not prove a new backup has completed.

## Blockers

- None for the restored test realm.

## Next actions

1. Use the native client and existing test-realm accounts at `159.69.88.70`.
2. When the user requests Wave Defense again, stop the original signup, Gateway, node and their
   maintenance timers, including the backup timer that can restart the original services.
   Restore the Wave Defense node and verify its schema before starting its Gateway and signup.
   Restore its prune and disk-guard timers, then the web host. Restore Nginx from
   `/etc/nginx/sites-available/wave-defense.before-pause`, validate it and reload Nginx.
   Verify service configuration, browser access and game entry before declaring it resumed.
