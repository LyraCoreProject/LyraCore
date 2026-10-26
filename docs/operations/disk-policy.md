# Realm disk policy

SpacetimeDB 2.7.1 keeps every commit-log segment, snapshot and daily module log until someone
deletes them, and no release up to 2.11.0 adds retention. It only compresses older segments and
snapshots. On 2026-09-28 the `lyracore` database held 73 GB of history and the next deploy had no
room to build. This page owns the rules that prevent that. Install them before another sustained bot
run.

## History pruner

A restart needs only the newest valid snapshot and the segments after it. It falls back to an older
snapshot if the newest one fails to load, and a gap between segments stops the database from
starting. `deploy/spacetimedb-prune.sh` in the
[`lyracore-cli`](https://github.com/LyraCoreProject/lyracore-cli) repository runs every fifteen
minutes and follows those rules:

- It keeps the two newest valid snapshots, plus any snapshot that is incomplete or locked.
- It keeps every segment from the one that holds S2 + 1, where S2 is the older kept snapshot, and
  deletes only a run of the oldest segments, so no gap can open.
- It deletes module logs from before today (UTC), and skips files changed in the last 30 minutes.

The pruner is safe with the node running. Without `--apply` it only reports. Never delete files
under `replicas/` by hand, and never replace these rules with an age or size deletion there.
Deleting a segment in the middle, or the newest snapshot, makes the database unrecoverable. A
retention timer does not protect the Realm if pruning fails or the retained recovery history exceeds
the free space.

## Disk monitor

The host disk monitor runs every minute. It warns below 30 GiB free and suspends bots below 20 GiB.
It records the last sample, growth rate, estimated hours until the reserve and renewal failures for
the Shard in `/var/lib/lyracore-disk-guard/status.json`. Failed or overdue pruning also suspends
capacity and produces a failed monitor service. Journal priorities make warnings and errors visible
to host monitoring; these units do not send email or chat notifications.

While cleanup runs, the monitor uses its last observed successful completion from the same boot.
Starting cleanup does not extend that completion's thirty-minute deadline. Missing history, a
failed cleanup, or an overdue completion revokes capacity even if cleanup is still running.

Each successful sample renews a three-minute Bot Capacity Lease on the Shard through
`set_package_config`. Playerbots must include the Package capacity check before enabling this
monitor. Expiry refuses new bots and controller activation; the Runner freezes existing bots in
bounded batches and cancels their actions. A failed monitor therefore cannot leave a permanent
permission to spawn. The filesystem and the monitor must refer to the Standalone's actual data
directory, and the monitor configuration must name the Realm's one Shard, `lyracore`.

The monitor starts suspended when its status file is absent and latches low-disk or failed-pruning
suspension. After repairing the cause and restoring at least 30 GiB, run it with `--resume` as the
service account. This renews capacity but leaves bots Frozen. Select their controllers explicitly
after verifying the population and reserve. Do not remove the Package Config key to bypass the Gate.

## Install

The scripts and units live in the `deploy/` directory of the
[`lyracore-cli`](https://github.com/LyraCoreProject/lyracore-cli) repository. Run the install from a
`lyracore-cli` checkout.

Review `deploy/disk-guard.example.json` against the named host and approved node, then install that
configuration at `/etc/lyracore/disk-guard.json`. Its `databases` list must hold only `lyracore`;
the example may still list the retired World Shard, Instance Pool and Realm-core databases, and its
CLI path is pinned. No credential belongs in this file; the service account retains its existing
SpacetimeDB login.

```bash
sudo install -d /opt/lyracore/bin
sudo install -o root -g root -m 0755 deploy/lyracore-disk-guard.py deploy/lyracore-capture.py /opt/lyracore/bin/
sudo install -o root -g root -m 0755 deploy/spacetimedb-prune.sh /opt/lyracore/bin/spacetimedb-prune
sudo install -o root -g root -m 0644 deploy/systemd/lyracore-disk-guard.{service,timer} deploy/systemd/lyracore-capture-prune.{service,timer} /etc/systemd/system/
sudo install -o root -g root -m 0644 deploy/systemd/spacetimedb-prune.service deploy/systemd/spacetimedb-prune.timer /etc/systemd/system/
sudo install -d -o lyracore -g lyracore -m 0700 /var/lib/lyracore/routine-captures
sudo systemctl daemon-reload
sudo systemctl enable --now spacetimedb-prune.timer
sudo systemctl restart spacetimedb-prune.timer
sudo systemctl list-timers spacetimedb-prune.timer
sudo systemctl start spacetimedb-prune.service
sudo systemctl enable --now lyracore-disk-guard.timer lyracore-capture-prune.timer
sudo systemctl start lyracore-disk-guard.service
# Report only, before the first apply:
sudo -u lyracore /opt/lyracore/bin/spacetimedb-prune /var/lib/lyracore/spacetimedb
# After proving expiry refuses spawning on the Shard:
sudo -u lyracore python3 /opt/lyracore/bin/lyracore-disk-guard.py /etc/lyracore/disk-guard.json --resume
```

The Package uses an absent lease key only for unmanaged Realms. Installation must prove a lease
exists on the Shard and that an expired lease refuses spawning before calling the host protected.
Verify the guard timer, retention timer and actual service restart properties. The Standalone
Supervisor permits five starts in five minutes, waiting thirty seconds between attempts. After
repairing the failure, reset its failed state and start it manually.

## Diagnostic evidence

Keep ordinary command captures under `/var/lib/lyracore/routine-captures`. Use:

```bash
python3 /opt/lyracore/bin/lyracore-capture.py unique-run-name -- command arguments
```

Each capture keeps up to 256 MiB of initial output and a 64 KiB tail, plus exit status and
truncation counts. It continues draining output after reaching the limit so the command can finish.
Commands have a one-hour lifetime limit, adjustable with `--timeout-seconds` before the capture
name. Timeout terminates the command's process group and records exit 124. The shared lock refuses
concurrent captures instead of allowing the budget to race. Completed captures expire after seven
days or when their combined size exceeds 2 GiB, oldest first. An hourly timer expires them even when
no new capture starts. Incomplete and unrecognized files are preserved and consume the admission
budget; archive them before recording more. Only this wrapper's completed captures are automatically
deleted. Do not capture commands that print credentials.

Cap journald at 500M, because the Native Host logs continuously and the default cap is 4 GB. Put
`SystemMaxUse=500M` under `[Journal]` in `/etc/systemd/journald.conf.d/lyracore.conf` and restart
`systemd-journald`. Preserve durable incident summaries, revisions, hashes and selected
acceptance evidence outside routine retention. Historical `/home/lyracore/deploy-logs` directories
are not routine captures and are never deleted by these scripts. Copy closed evidence off host,
verify the copy by checksum, then retire only the verified local directory. Keep current captures
and evidence still needed for an active acceptance session.

## Verification and write investigation

Run the deployment-script tests on the build host. From a `lyracore-cli` checkout, where CI also
runs them:

```bash
python3 -m unittest discover -s deploy -p 'test_*.py'
bash deploy/spacetimedb-prune-test.sh
```

From this checkout, check the Standalone Supervisor unit and its runbook:

```bash
bash deploy/systemd/spacetimedb-standalone-artifact-test.sh
```

Sample physical transaction-log growth separately from logical write-byte counters. The pinned
Standalone's write-byte metric labels a transaction type rather than an individual reducer; do not
attribute it to a reducer from timing correlation alone. Use decoded retained transaction records
for attribution. Preserve only aggregate reducer and table totals, avoiding authentication rows and
raw argument payloads. Do not claim cleanup reduces the underlying gameplay write volume.
