# Danger zones — DO-NOT-TOUCH traps, tooling gotchas, deploy/verify

> **This document is authoritative** over every other document in this repository for anything about
> migrations, publishing, the deploy/verify procedure, and the traps in §1. Where
> [`architecture.md`](./architecture.md) or any other page explains the same mechanism, it explains
> *what it is*; this page says *what will bite you*, and this page wins. Nothing supersedes it
> without saying so here.

Every entry below is here because it cost real debugging time at least once. A change that crosses
any line in §1 needs a human review before it ships, whoever or whatever wrote it.

## 1. DO-NOT-TOUCH / high-risk traps (these have bitten before)

1. **`spacetime publish -c` is DESTRUCTIVE** — wipes the database and breaks login until
   `claim_operator` runs again. **Never run it.** Plain `spacetime publish` auto-migrates, or aborts
   safely without touching the data. `lyracore publish` refuses to forward `-c` (or any other
   flag-shaped argument) for exactly this reason; deploy through it.
2. **Migrations:**
   - New COLUMN → **END-append** the struct field + `#[default(...)]` → additive auto-migrate. Anywhere
     else = publish aborts.
   - New reducer ARG → change the module reducer **and** the gateway `call_reducer!` **in the same
     commit** (code lockstep, NOT a database migration).
   - New TABLE → **regenerate the gateway bindings**. Regeneration is the supported path here:
     `spacetime generate --lang rust --out-dir gateway/src/stdb/bindings --module-path module \
     --include-private --build-options='--features=debug_reducers' -y`. **`--include-private` is REQUIRED**
     (without it `game_account`/`game_session`/the schedule tables/`pkg_*` silently vanish and the
     gateway will not compile), and the generator snake-cases digit suffixes (`eff_p0`→`eff_p_0`,
     `cond_value1`→`cond_value_1`), so a regen breaks exactly those columns whose GATEWAY CODE uses
     the non-underscore name. Post-regen hand-patch (verified 2026-07-18 on the exploration-fog
     regen): put `gossip_option_type.rs` `cond_value1`/`2` and `aura_type.rs`/`mod.rs`
     `eff_p0`/`1`(`_kind`) back to the no-underscore spelling. BUT LEAVE `spell_effect_type.rs`
     `p_0`/`p_1` and `game_object_template_type.rs`/`debug_spawn_gameobject_reducer.rs`
     `data_0`/`data_1` AS GENERATED (with the underscore) — their gateway code uses the underscore
     names. The rule: match the COMMITTED (working) binding's field name, which is what the gateway
     code accesses; `cargo build` E0609 "no field X" tells you which way each one goes.
     Regeneration also removes the explanatory `facing` / `facing_angle` comment from
     `creature_spline_type.rs`; restore it from the committed binding after every regeneration.
     Regeneration needs a machine that can run `spacetimedb-standalone` — schema extraction shells
     out to it, so a sandbox without it cannot regenerate. Diff-review the regenerated tree before
     committing (it rewrites every file; drift deletions are expected and correct), then run the
     gateway tests. Do NOT hand-splice new tables into `bindings/mod.rs` — that hand-maintenance tax
     is what regeneration replaces.
     `gateway/tests/schema_parity.rs` structurally checks every subscribed table's binding against
     the module schema — run `cargo test -p lyracore-gateway` after ANY hand edit to a
     `<table>_type.rs` file to catch a missed, misordered, or mistyped column before it ships.
     **One documented exception:** `game_transfer_out` plus the seven transfer/instance reducer
     bindings were spliced BY HAND rather than regenerating all ~512 files — they are the only
     module→gateway data flow the cross-database transfer adds, and the regen's blast radius was
     worse than the splice. `transfer_out_type.rs` / `game_transfer_out_table.rs` say so in their
     headers, and `schema_parity.rs` covers the row shape. A future regen overwrites both with
     identical content; nothing needs undoing.
     **A second exception:** `record_shard_load`/`record_region_load` (and their `game_shard_load` /
     `game_region_load` tables) were also hand-spliced, because no live `spacetimedb-standalone` was
     available to run the generator. Only the REDUCER bindings were added
     (`record_shard_load_reducer.rs`, `record_region_load_reducer.rs`, mirroring
     `install_guid_range_reducer.rs`'s shape) — the gateway only ever WRITES those two tables (an
     operator reads them with `spacetime sql`), so no TABLE binding exists and none is needed. A
     future regen adds the two table bindings (harmless, since nothing subscribes them) and
     overwrites the reducer bindings with equivalent content; nothing needs undoing. This is also
     the case that establishes the general rule: **a table binding is only needed if the gateway
     subscribes to or reads that table.**

   **A defaulted column's default can be a VALID VALUE, not a sentinel — plan the backfill (#456).**
   `cell` was END-appended to the four AOI-scoped tables as `#[default(0i64)]`, and 0 is the legitimate
   packed id of grid cell (0, 0), not "unset". Every pre-existing row therefore claims to live in that
   one cell the instant the publish lands, and the AOI subscription probes `cell` by equality — so
   moving entities self-heal on their next heartbeat, but **static `game_gameobject` rows never
   re-stamp themselves and stay invisible world-wide until backfilled**. After publishing a change of
   this shape, run the sweep on every shard:
   `for db in lyracore lyracore-world-1 lyracore-instances lyracore-realm; do spacetime call $db debug_backfill_cell_ids; done`
   The general rule: when you END-append a defaulted column that something INDEXES or FILTERS on, ask
   what the default means as a value before asking whether the migration applies cleanly. It will apply
   cleanly and still be wrong.
3. **The 5875 partial-VALUES crash trap** — any partial `UNIT_FIELD_*` update MUST route through the
   `dirty_reset` path so it **never carries `OBJECT_FIELD_TYPE`**. Re-sending TYPE crashes the client
   (null+0x110). Copy `build_health_values` / `build_resistance_values` exactly; there is a
   wire-pinning test for each.
4. **`SMSG_SET_FACTION_STANDING` sends the rep-INDEX** (`Faction.dbc` `ReputationListID`), not the
   `faction_id` — sending the id indexes past the client's 64-slot array and takes the client down
   with ERROR #132.
5. **Don't gate features on client-display readback.** The server is authoritative; verify effects
   server-side. Client visuals are secondary — flag the residue for a human to look at.
6. **`#[default(...)]` cannot default a `String` column** (verified against SpacetimeDB 2.5's
   `spacetimedb-bindings-macro`): the macro type-checks every default expression inside a plain
   `const _: () = { let _check: T = EXPR; };` block, and dropping a `String` (or any type with a
   real `Drop` impl) in const-evaluated Rust is `error[E0493]` on stable — a hard Rust limitation,
   not a repo convention, so it blocks `#[default(String::new())]` and `#[default("")]` equally on
   ANY end-appended `String` column, regardless of the literal's content. Numeric and bool
   end-appends are unaffected (no `Drop`). Workaround: put the new `String` data in a **separate new
   table** — no existing rows means nothing to backfill, so no column needs a default at all. That is
   the same one-row-plus-child-rows shape this codebase already uses for one-to-many data
   (`game_creature_waypoint`, `game_creature_spell`). See `module/src/creatures/spawn.rs::NpcTextSlot`.
7. **Never `.iter()` (or `.count()`) a SPATIAL table** (`game_world_entity`, `game_creature_spawn`,
   `game_gameobject`, `game_dynamic_object`) — including via the codebase's usual
   `let entities = ctx.db.game_world_entity();` handle, which is the same whole-world read one line
   later. Use the partition-scoped helpers instead:
   `helpers::entities_near(ctx, map_id, instance_id, x, y, radius)` for a radius search (grid-indexed,
   scoped to `(map_id, instance_id)`) and `helpers::in_same_partition(&e, map_id, instance_id)` for a
   membership test. **Why:** the realm is cut into instance / continent shards, each its own
   SpacetimeDB writer. A whole-table scan then returns only the rows on the caller's own shard — it
   does not error, it quietly returns a subset, and every feature built on "I can see the whole world"
   silently goes wrong. Going through the helpers means the read already asks the partition question,
   so the split is a no-op. This is also the performance story: a scan is O(world), the helpers are
   O(neighborhood). **Enforced** by `module/src/tripwires.rs::partition_discipline_tripwire` (#379
   moved this out of `lib.rs`) — a source scan over `module/src/**` (and the in-tree extension
   packages) with a per-file whitelist of today's legitimate scans. See `WHITELIST` in that file for
   the current set; it is a ratchet that only ever
   shrinks, so any number quoted here would be stale by design.
   A new scan fails `cargo test -p lyracore-module --lib`. Raise a budget only for a genuinely
   realm-wide or partition-managing read, with a one-line justification, and delete budget as scans
   are replaced with helpers — a budget larger than the real count fails its own test.

## 2. Tooling gotchas (will silently mislead you)

- **`spacetime sql` (2.5):** NO `ORDER BY`, NO `IN`/subqueries, NO `Timestamp`/`Identity` literal. A
  range filter on a SINGLE column can wrongly return 0 rows — use a histogram or `COUNT`. To load a
  `Timestamp` column, go through a reducer that stamps `ctx.timestamp`.
- **`spacetime call` mangles u64 guids > 2^53** (JSON number truncation) — pass big integers as
  **string** arguments.
- **`auto_inc` sequences sit BEHIND explicitly-numbered imported rows.** A content import writes
  quest / vendor / trainer / creature-loot rows with explicit ids WITHOUT advancing the table's
  sequence, so any later `id: 0` insert (from a reducer) or SQL `VALUES (0, …)` allocates a COLLIDING
  id: reducers PANIC with errno-12 and the whole transaction rolls back (a seed that "does nothing" is
  usually this), and SQL inserts fail silently. Fixture and scenario rows must therefore use FIXED
  reserved ids (the 509xxxx range) with a delete-first. Same class: `SpellEffect.id` is a
  deterministic primary key, `(spell_id << 2) | index` — never plain-insert one the importer may also
  write; use the fixtures' `upsert_effect`. Found live 2026-07-15.
- **`#[default(0)]` on a u64 is a real, deploy-only break.** It encodes four bytes where eight are
  expected ("data too short for u64: Expected 8, given 4") and no unit test sees it. `lyracore
  preflight` validates every `#[default(...)]` encoding offline — run it before every publish.
- **gtker vanilla has known wire gaps** (the update-mask wall, the loot-item codec, faction-standing
  width). If a packet looks right in a decoder but the real client misbehaves, suspect a
  gtker-versus-real-client gap before suspecting your own logic.
- **The gateway's logout persist is ASYNC (~1–3 s after the client's socket closes)** — anything that
  reads `game_character` progression (money, xp, level) immediately after a session ends races it and
  sees the PRE-session value. Poll until the expected value settles instead of reading once. An
  entire "money corruption" investigation turned out to be this plus a test's own teardown. Same
  class: an offline `debug_set_money`-style write placed BETWEEN two sessions gets overwritten by the
  first session's late persist.
- **Test locations must be line-of-sight-probed.** Melee swings gate on `has_los`, which now defers
  to the exact vmap ray (`vmap::los_ray`, WMO-only) whenever `game_config.vmap_enabled` is on
  (#523), and only falls back to the grid-rasterized nav obs data when it's off — either way a spot
  that looks open can sit inside real/rasterized geometry, so fights stall forever with an armed
  row, `last_swing_ms=0`, and no error anywhere. Before parking a fixture, probe the ray that will
  actually gate it: `spacetime call lyracore -- debug_vmap_ray <map> <x0> <y0> <z0> <x1> <y1> <z1>`
  on a vmap-enabled map (require `los=None`), or `spacetime call lyracore -- debug_nav_leg <map>
  <x> <y> <x2> <y2>` on a grid-only one (require `has_los=true`). Also: a stationary automated
  client only reaches the 5 yd standstill melee range, so fight fixtures belong at ≤4 yd unless the
  client walks in. And a level-1 character parked in an imported world is a valid aggro target for
  the ambient spawns — disposable test characters default to level 5 so they are grey to them.
- **In an imported world, LOW ids are not yours.** Anything that seeds test data at real, low entry
  ids will corrupt imported content: deleting and reinserting faction-template rows 14/1 breaks the
  imported Monster/Player templates until the next import, item entries 50/52 are real imported items,
  and granting reputation on a real faction that was imported without a reputation bar silently does
  nothing. All fixture data must live at RESERVED ids (509xxxx items and factions, 50900 quests,
  5090x faction templates), and staging helpers must be import-aware — no-ops when the real rows
  exist. Teardown assertions must compare against a PRE-TEST count, never against zero.
- **`spacetime sql` prints whole floats WITHOUT a decimal point.** Any regex reading a coordinate back
  out of it needs `-?[0-9]+(\.[0-9]+)?`; a character parked at exactly `x=-8890.0` otherwise fails
  every "did it arrive" poll.

### Maintainer tooling referenced above

Several verification tools are **maintainer-side and not included in this repository**: the
wire-protocol test suite (a headless client that speaks the real 5875 protocol — SRP6 plus the
encrypted world stream — and decodes SMSG through gtker), the cross-shard catalogue check, and the
capacity benchmark. `lyracore-deploy`, which the CLI's production-realm refusal names (distinct from
the PID-identity refusal in [`development-cli.md`](./development-cli.md)), is maintainer-side too;
§3 below is the deploy procedure available here and does the same work by hand. (The world-import ETL is **not** one of these: `importer/scripts/` — including
`import-world.sh`, the ETL that builds a full zone from operator-supplied data — ships in this
repository; see [`data-ingestion.md`](./data-ingestion.md).) The traps above are written so they
still apply if you build equivalent tooling of your own; where one of them names a script that is
not in this repository, it is describing the maintainers' setup, not a command available here.

One property of the wire suite is worth knowing even if you cannot run it: a headless protocol client
confirms *"the server sent the right packet"* and nothing more. It shares gtker's reader, so it
**cannot** reproduce a client crash or lock-up. That class of bug needs a real 5875 client and a human
watching it.

### Reading `work-item N` in code comments

Comments written before 2026-07-30 sometimes cite `work-item N` (occasionally just a bare 3-digit
number) — an id from a retired internal task queue, not a GitHub issue or PR number. That queue's
index is maintainer-side and not included in this repository, so these ids don't resolve to anything
you can open here; treat them as an opaque historical label ("this behavior was decided while working
task N") rather than a link. **Do not assume `work-item N` and issue `#N` are the same thing** — the
two id ranges overlap, so for a number under a few hundred it is easy to land on an unrelated GitHub
issue by guessing. A comment's own `#N` references (this repository's actual issues/PRs) are a
separate, resolvable convention and are unaffected by this.

## 3. Deploy + verify procedure (use exactly this)

### SpacetimeDB standalone is a supervised production dependency

The gateway only recovers a coordinator connection when a healthy standalone node comes back. Run
that node under the committed `deploy/systemd/spacetimedb-standalone.service` unit, rather than in
`nohup`, `screen`, or a login shell. The unit restarts **every** standalone exit, gives every
restart `524288` file descriptors, and appends standalone stderr to
`/var/log/lyracore/spacetimedb-standalone.log`.

Install the exact 2.7.1 `spacetimedb-standalone` binary at
`/opt/lyracore/spacetimedb/spacetimedb-standalone`, then install and enable the unit as root. The
directories are deliberately owned by the non-login `lyracore` service account: the database data
and error log must survive both an executable upgrade and a standalone crash.

```bash
sudo useradd --system --home-dir /var/lib/lyracore --shell /usr/sbin/nologin lyracore  # once
sudo install -d -o lyracore -g lyracore /opt/lyracore/spacetimedb /var/lib/lyracore/spacetimedb /var/log/lyracore
sudo install -o lyracore -g lyracore -m 0755 /path/to/spacetimedb-standalone /opt/lyracore/spacetimedb/spacetimedb-standalone
sudo install -o root -g root -m 0644 deploy/systemd/spacetimedb-standalone.service /etc/systemd/system/spacetimedb-standalone.service
sudo systemctl daemon-reload
sudo systemctl enable --now spacetimedb-standalone
```

Inspect the service and its durable stderr capture with:

```bash
sudo systemctl status spacetimedb-standalone
sudo journalctl -u spacetimedb-standalone --no-pager -n 100
sudo tail -n 100 /var/log/lyracore/spacetimedb-standalone.log
```

After replacing the standalone binary, run `sudo systemctl restart spacetimedb-standalone` and
repeat those inspections. Do not delete `/var/lib/lyracore/spacetimedb` during an upgrade: it is the
node's persistent database state. The unit binds standalone to loopback; expose it only through the
separately configured front door, never by changing this service's listen address casually.

### Live capacity-edge node-death validation (human-authorized)

This is the production validation for #83, tracked by #176. It is deliberately a **runbook**, not a
command to run from a development checkout: killing a database node disconnects real players. Run it
only in an approved maintenance window, with an operator who can stop the test, and only after #173,
#174, and #175 are deployed together. Do not mark #176 or #83 complete from code review, a unit test,
or this blank template.

Before the window, identify the gateway systemd unit as `<gateway-unit>` and the approved load driver
as `<load-driver>`. Use isolated, disposable test accounts and a test realm or an explicitly approved
production slice. The gateway configuration must provide at least 750 world-session and blocking-pool
slots, and set `LYRACORE_ADMIT_CONCURRENCY=25`; record the actual `LYRACORE_MAX_SESSIONS` and
`LYRACORE_MAX_BLOCKING_THREADS` values below. Do not quietly raise an operating limit just to make the
test pass — the change itself needs normal deployment approval.

1. Capture the gateway PID, its start time, and its active configuration before beginning. Capture the
   standalone unit's `ActiveState`, `MainPID`, restart count, stderr tail, and the effective descriptor
   limit:

   ```bash
   sudo systemctl show <gateway-unit> -p MainPID -p ActiveEnterTimestamp
   sudo systemctl show spacetimedb-standalone -p ActiveState -p MainPID -p NRestarts -p LimitNOFILE
   standalone_pid="$(sudo systemctl show spacetimedb-standalone -p MainPID --value)"
   sudo awk '/Max open files/ { print }' "/proc/${standalone_pid}/limits"
   sudo tail -n 100 /var/log/lyracore/spacetimedb-standalone.log
   ```

2. With `<load-driver>`, establish approximately 750 seated world sessions, then offer approximately
   1,000 additional authenticated world logins at **25 per second** and keep their sockets open. Save
   the driver's unabridged output and the gateway's `QUEUESTAT` lines. The pre-failure evidence must
   show the intended seated population and a non-empty queue; if it does not, stop and record the
   deviation rather than inducing node death.

3. Start continuous, timestamped capture of the gateway journal, standalone journal, standalone stderr,
   and load-driver output. An authorized operator must then perform the reviewed node-death action.
   Record the exact command and timestamp in the evidence template; do not substitute a gateway restart
   for node death. The gateway process must remain running throughout this step.

4. Observe until the affected world sessions have disconnected and the queue has drained according to
   the configured 25-per-second admission limit. Preserve the post-failure `QUEUESTAT` lines and a
   gateway thread/process snapshot. The pass condition is no persistently parked world-session threads
   and no retained queue seats: after the controlled clients disconnect, `QUEUESTAT` must converge to
   zero active and zero depth (or the documented baseline if other approved traffic was present).

5. Verify that systemd restarted standalone, that the restarted process still has `524288` open-file
   descriptors available, and that its stderr append log contains both sides of the event. Confirm the
   gateway PID and start time are unchanged. Finally, use fresh disposable accounts to prove that new
   logins complete after coordinator recovery; save the driver result and relevant gateway journal span.

6. Paste the completed evidence template below into a comment on #83 and #176. Any missing capture,
   deviation, timeout, stale seat, parked session, failed login, or gateway restart is a failed or
   inconclusive result — leave both issues open and link the evidence before scheduling a retry.

#### Evidence template — do not pre-fill

```text
Validation date/time (UTC):
Operator / approval / maintenance window:
Gateway unit and host:
Standalone host:
Gateway build/commit:
Standalone binary version:

Prerequisites deployed: #173 / #174 / #175 (commit or release identifiers):
Configured LYRACORE_MAX_SESSIONS:
Configured LYRACORE_MAX_BLOCKING_THREADS:
Configured LYRACORE_ADMIT_CONCURRENCY (must be 25):
Load driver, version, and test-account range:
Approved traffic isolation / baseline:

Gateway PID + start time before / after (must match):
Standalone MainPID before / after:
Standalone ActiveState, NRestarts, LimitNOFILE:
Effective `Max open files` from /proc/<pid>/limits after restart:
Standalone stderr-log excerpts/attachment:

Pre-failure seated sessions (target ~750):
Pre-failure offered queued logins (target ~1000 at 25/s):
Pre-failure QUEUESTAT evidence:
Reviewed node-death command and timestamp:
Post-failure gateway journal / thread-snapshot attachment:
Post-failure QUEUESTAT evidence (active/depth converge to baseline):
Coordinator-recovery evidence and fresh-login result:

Deviations, failures, or missing evidence:
Verdict: PASS / FAIL / INCONCLUSIVE
Links to #83 and #176 comments containing the raw evidence:
```

`lyracore dev up` is the one deliberate exception to the production topology below: it is a
contributor fixture on a loopback node, four databases since #108 (`lyracore`, `lyracore-kalimdor`,
`lyracore-instances`, `lyracore-realm`) and one under `dev up --single`. It **owns** the topology
variables rather than inheriting them — `LYRACORE_SHARD_MAP`, `LYRACORE_SHARD_MAP_FILE` and
`LYRACORE_REALM_CORE` are set to the fixture's own values in the sharded mode and actively unset in
`--single` — so having the recipe below exported in your shell cannot turn the fixture into a
gateway pointed at databases it never published. It also never adopts or stops a SpacetimeDB node it
did not start.

Its listeners are loopback-only unless you ask otherwise: `lyracore dev up --lan <private IP>`
moves **the logon and world listeners alone** to that address (and makes the realm list advertise
it, via `LYRACORE_REALM_ADDRESS`) so a client on your LAN can connect. SpacetimeDB stays on
`127.0.0.1:3000` in every mode, and only RFC1918 addresses are accepted — `0.0.0.0` and public
addresses are refused rather than bound.

**Do not use it to launch or repair a production/sharded realm** — its database list is the
fixture's, not production's (`lyracore-kalimdor` where production has `lyracore-world-1` and every
world shard after it), so it would leave those un-republished after a schema change, which is exactly
the partial-publish failure §1.2 warns about. The multi-database recipe below stays authoritative
there.

```bash
cd <repo-root>

# Build + test BEFORE deploying — every workspace member with tests, every time. This list used to
# omit lyracore-importer, and two broken tests sat unnoticed through roughly ten "all green" PRs.
cargo test -p lyracore-importer
cargo test -p lyracore-module --lib --features=debug_reducers
cargo test -p lyracore-gateway
cargo test -p lyracore-shared
cargo build
# ⚠ Each of these prints SEVERAL `test result:` lines (unit target, integration target, doctests) and
# some of them are legitimately `0 tests`. SUM them; do not report the last line. Reading a single
# line has repeatedly produced conclusions like "lyracore-shared has no tests" (it has 75).
# Expected shape on a green main:
#   importer 128 | module 603 | gateway 673 + 63 (schema parity) | lyracore-shared 75
# These numbers drift UP as tests are added; a LOWER count is the signal worth chasing, not an exact
# match.

# Publish-shaped PREFLIGHT — the deploy-only break class the test suites cannot see.
# Fully offline: no node, no database, no publish/call/sql. Three checks: the SpacetimeDB version
# gate (an exact pin, see quickstart), an offline module schema extraction, and RLS-filter
# validation. It compiles the wasm on the way through, so it replaces a separate build step, and it
# validates every `#[default(...)]` encoding — the same code path that rejects `#[default(0)]` on a
# u64. Budget: about a second idle, about sixteen seconds after a module edit. Run it every time.
./lyracore preflight
# Deliberately, preflight does NOT run a workspace-wide `cargo test` and should not gain one. A full
# workspace pass takes tens of seconds to minutes and duplicates the per-crate lines above; folding
# it in would make preflight slow enough that people stop running it, which defeats a check whose
# whole value is being cheap enough to run before every publish.

# Deploy the MODULE (only if the module changed). `lyracore publish` runs preflight first, bakes in
# `--features=debug_reducers` (a bare `spacetime publish` omits it, drops the feature-gated debug
# tables, and then false-aborts on a bogus "Removed table: game_debug_readout"), passes `--yes`, and
# REFUSES `-c` and any other flag-shaped argument. Plain publish = safe auto-migrate. NEVER -c.
./lyracore publish
# ⚠ That publishes the default database ONLY. A SCHEMA change must reach EVERY shard or the gateway
# refuses logons on the ones left behind — and a partial publish can present as an unrelated
# mid-session hang rather than a loud "no such table", because only the default-shard connect
# fails loudly; a stale extra shard degrades silently. Pass them all in one command:
./lyracore publish lyracore lyracore-world-1 lyracore-instances lyracore-realm

# Rebuild + restart the GATEWAY (always, if the gateway changed)
#
# ⚠ THIS IS THE PRODUCTION TOPOLOGY. A recipe carrying only LYRACORE_AOI and the token starts a
# SINGLE-DATABASE gateway — no Kalimdor, no instances, no realm-core — and every one of
# those degrades SILENTLY rather than refusing to start. Anything that omits a variable below is the
# single-database dev config, which is not what a sharded realm runs.
cargo build -p lyracore-gateway
TOKEN=$(grep -oP 'spacetimedb_token = "\K[^"]+' ~/.config/spacetime/cli.toml)
# ⚠ `lyracore-gatewa` is NOT a typo — see the trap note under this block.
pkill -x lyracore-gatewa; sleep 1     # NOT pkill -f "target/debug/lyracore-gateway" (self-matches the shell → exit 144)
setsid nohup env \
  LYRACORE_AOI=1 \
  LYRACORE_COORDINATOR_TOKEN="$TOKEN" \
  LYRACORE_SHARD_MAP="36:*=lyracore-instances, 1:*=lyracore-world-1" \
  LYRACORE_REALM_CORE=lyracore-realm \
  MALLOC_ARENA_MAX=2 \
  RUST_LOG=info \
  ./target/debug/lyracore-gateway </dev/null >/tmp/gw.log 2>&1 &
sleep 4
./lyracore production status \
  --gateway-log /tmp/gw.log \
  --realm-core lyracore-realm \
  lyracore lyracore-world-1 lyracore-instances lyracore-realm
# Also inspect the process identity and real sockets. Log listener markers do not prove the process
# still owns them.

# What each one buys, and how it fails if you leave it out — all three fail QUIETLY:
#   LYRACORE_SHARD_MAP      one database. Kalimdor and the instance pool are simply not there.
#                     ⚠ Keep the `36:*` rule — dropping it does not degrade, the gateway refuses to start.
#   LYRACORE_REALM_CORE     auth/sessions/parties fall back to the world database instead of realm-core.
#   MALLOC_ARENA_MAX  4x the RSS per connection (8.75 MB vs 2.16). Not baked into the binary.
#
# Optional, but the one whose absence you will notice later, not now:
#   LYRACORE_METRICS_DB_IDS="<shard>=<hex-identity-prefix>,..."  — without it, writer occupancy is
#     simply not sampled (sessions still are), so `game_shard_load` has no
#     rows for that shard and the "which shard is hot" query answers nothing. The gateway warns
#     once at startup naming the gap.
# The full environment-variable table, with every default and every silent-failure mode, is in
# docs/architecture.md §3.2.
#
# RUST_LOG=info deliberately, not info,gateway::world=debug: since the raw-bytes relay, every packet
# logs a line, which floods at scale.

# `scripts/publish-module.sh` already calls `debug_repair_after_publish` after every publish above
# (#378) — it re-arms the creature tick + aura/ground-area/instance-reaper schedules and re-seeds
# every fixture family `init` seeds but a plain republish doesn't re-run. No manual call needed here
# unless you bypassed the script.
# If you changed world DATA (spawns/quests/items): re-run the content import.

# After EVERY shard is published or re-imported, prove the replicated catalogues (spells, items, the
# DBC reference tables) did not skew. This needs live databases, so unlike preflight it is its own
# step, run last: a partial re-import that only reached some shards must not be reported as a
# success. The maintainers run a cross-shard catalogue check here; it is not in this repository.
```

### ⚠ Status distinguishes configuration from connectivity

`./lyracore production status` parses these latest-start markers separately. It avoids a fragile
connection count, but it cannot prove a process still owns a port; retain the socket inspection in
the recipe above.

Two facts that have made a broken startup look fine:

1. It is printed behind an `if conns.len() > 1` guard (`gateway/src/stdb/connection.rs`), so a
   **single-database gateway prints no database list at all**. An empty grep result is exactly what
   you get when a topology variable was dropped — and also exactly what you get from a correct
   single-database dev run, and also what a wrong grep pattern gives you. Empty is not "no news".
2. It prints `map.databases()` — the **configured** set. A shard that failed to connect is still
   listed there; its failure is a separate `log::error!`. Only the default database is fatal:
   realm-core down is an auth outage, and any other shard degrades to the default, both logged and
   both non-fatal.

`coordinator connected to shard <db>` (`connection.rs`) is printed once per **successful**
connection and is the line that actually proves connectivity. Count it.

### ⚠ `pkill -x lyracore-gatewa` — the 15-character truncation, not a typo

`pkill -x` / `pgrep -x` match `/proc/<pid>/comm`, which the kernel truncates to `TASK_COMM_LEN - 1`
= **15 characters**. `lyracore-gateway` is **16**, so the obvious command matches nothing and says so
only if you look:

```
$ pgrep -x lyracore-gateway
pgrep: pattern that searches for process name longer than 15 characters will result in zero matches
$ pgrep -x lyracore-gatewa      # ← drop the trailing y
993724
```

`pkill` prints the same warning but its **exit status is 1** either way — indistinguishable from
"nothing was running". A restart recipe that "worked" while the old gateway is still bound to :8085
is the failure this note exists to prevent. Always use the 15-character form, and confirm with
`pgrep -x lyracore-gatewa` after. `pkill -f` remains forbidden: it self-matches the launching shell
(exit 144). The binary keeps its full 16-character name deliberately; the truncation is a property of
`pkill`, not of the name.

### Where a gateway-only change stops

A gateway change alone needs only a rebuild and a restart — no publish, and no re-import.
