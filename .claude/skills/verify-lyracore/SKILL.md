---
name: verify-lyracore
description: Verify a running LyraCore Realm with Benilla, the Headless Client and committed state. Use when a change needs evidence from a live protocol path or real client.
---

# Verify LyraCore

The Native Host accepts logon and world connections directly. Its Module owns gameplay, AOI and
protocol handling in one Shard. Use three sources of evidence:

1. Benilla exercises the actual UI, client state and build-5875 protocol.
2. The Headless Client tests native requests and replies without a display.
3. Narrow SQL reads prove the committed side effects.

Read the action and expected result from the task. A successful client exit or screenshot alone
does not prove that action succeeded.

When a change needs a real-client check for a feature, read [checks.md](checks.md).

## Choose the target

For local work, record `./lyracore dev status` and `./lyracore packages list` before startup. Native
`dev up` uses this checkout's `.lyracore/native-data` and one Shard, `lyracore`. The default HTTP,
logon and world ports are 3000, 3724 and 8085, so another checkout can still own those ports. Inspect
its process identity before starting or stopping anything.

A deployed Realm requires the user's authorization. Use the `lyracore-operator` skill and
[the native operations guide](../../../docs/native-operations.md) to establish its exact Host,
node, Shard, configuration and credentials. Do not use the local `dev` lifecycle to manage a
separate service. A login can replace an existing World Session, so use named test Accounts or
create dedicated fixtures within the authorized scope. Keep host access details and passwords out
of tracked files and published evidence.

## Start a local Realm

```bash
./lyracore dev status
./lyracore packages list
flock -o ~/.cache/lyracore-build.lock ./lyracore dev up
./lyracore dev status
```

Ready means the Native Host is running, both native listeners are present, and the Shard is
published. Inspect actual socket ownership as well. Native logs come from `./lyracore dev logs
spacetime`.

Provision a dedicated Account through `./lyracore account create NAME --password-stdin`. Creating
an existing Account resets its password. Use a new name unless that reset is intended. The seeded
`TEST` Account owns `Tester`, a level 1 Human Warrior in Northshire. Grant GM only if the specific
check needs it.

The content check must pass before treating a seeded fixture as a populated-world test. An import
needs a 1.12.1 client's `Data/` and the source dump. Import only when authorized and needed; it is
not a routine fix for a failed login. Report incompatible installed Packages rather than silently
replacing them.

## Drive Benilla

Set `BENILLA_DIR` to the client checkout and `WOW_DATA` to its 1.12.1 data. The client needs a
working display. `drive-benilla.sh` builds it and stores the evidence under `EVIDENCE`.

```bash
EVIDENCE=/tmp/lyracore-verify/movement \
WOW_USER=TEST WOW_PASS=test123 WOW_CHAR=Tester \
WOW_PROBE_KEY='W@15:2' \
WOW_PROBE_LUA='ProbeLog("zone=" .. GetZoneText())' WOW_PROBE_LUA_AT=25 \
WOW_LIVE_SHOT_AT=28 \
  .claude/skills/verify-lyracore/drive-benilla.sh 35
```

The example credentials belong only to the local fixture. Supply deployed credentials privately.
For a nondefault endpoint, set `WOW_HOST` to the authorized logon address. The advertised world
address must also be reachable, directly or through the test tunnel.

| Variable | Action |
|---|---|
| `WOW_PROBE_CHAT` | Submit semicolon-separated Speech, slash commands or GM commands. |
| `WOW_PROBE_LUA` | Run the stock UI API, such as `AcceptQuest()`, and log results with `ProbeLog`. |
| `WOW_PROBE_KEY` | Press timed keys, such as `W@15:2` or `Space@20`. |
| `WOW_LIVE_SHOT_AT` | Capture the rendered client at the specified time. |
| `WOW_MOVE_TRACE` | Record protocol evidence. Keep authentication redacted. |

Read the selected Benilla revision's `docs/CONTRIBUTING.md` for timing and additional controls.
Use separate Accounts, client state directories and evidence paths for simultaneous clients.
Inspect screenshots and recordings. A login screen, black frame, cinematic or busy UI can prevent
the intended action even when the process exits successfully.

Test both sides of controls. For example, receiving root or water-walk is insufficient: verify the
client's acknowledgement and a subsequent usable World Session. For a map change, verify worldport
acknowledgement, destination state and relevant notifications during loading.

## Headless Client and durable state

```bash
printf 'test123\n' | ./lyracore dev smoke TEST Tester --password-stdin
```

The local smoke check requires imported content. Its refusal before world entry is not evidence
that gameplay passed. For detailed isolated native regressions, use `native-host/check.py` as
[the native operations guide](../../../docs/native-operations.md) describes.

Query the same Shard for Character and Account state. Use the private publisher configuration and
explicit HTTP URI, for example:

```bash
spacetime --config-path .lyracore/native-cli.toml sql \
  --server http://127.0.0.1:3000 lyracore \
  "SELECT guid, name, map_id, x, y, z, online FROM game_character WHERE name = 'Tester'"
```

Live Character values can reside in `game_world_entity` until logout persists them. Check both the
live result and relogged durable state when persistence matters. Read field names from the current
schema. SQL has no `ORDER BY`, `IN` or subqueries. Do not dump authentication tables.

Debug reducers may stage a fixture or trigger a specified condition, such as death. Record those
calls separately from the client actions being tested. A debug cast does not prove the client's
cast path, and a debug teleport does not prove a travel UI.

## Evidence and cleanup

Keep the exact Core, CLI and Benilla commits, Host and Module hashes, action script, bounded logs,
redacted packet trace, before/after SQL and inspected recording together. State which steps used
fixture setup. For a Refusal, show the reply and unchanged durable state.

Stop only the client and display PIDs created by the check. Stop a local Realm only if this run
started it and still owns its recorded process. Leave an already-running deployment in its intended
state. Clean up only this run's test Accounts and temporary data when appropriate. Keep the evidence
and recovery backup.
