//! Periodic per-shard load sampling (issue #78) — the ops half of Phase C.
//!
//! The gateway is the one component that can see the whole realm: every shard's own SpacetimeDB
//! node exposes `/v1/metrics` (the same endpoint this project's internal capacity-benchmark harness
//! reads for its writer-occupancy number), and every shard's coordinator connection already caches
//! its live player-session count. This module samples both on a timer and writes them to
//! realm-core via `record_shard_load` (`module/src/load.rs`), so "which shard is hot" is
//! answerable with `spacetime sql`.
//!
//! # Split: impure I/O vs. the testable driver
//!
//! [`OccupancySampler`] does the one genuinely impure thing here (an HTTP GET against a live node)
//! and is therefore NOT exercised by this crate's tests — same "KNOWN-UNCOVERED, name it" convention
//! already used elsewhere for `Coordinator::escrow_row`. Its Prometheus text
//! PARSING and its occupancy-percentage MATH are pure and pinned directly
//! (the wire harness's `src/bin/bench/metrics.rs` computes the identical thing; this is a
//! from-scratch reimplementation — see that file's own doc for why sharing it as a library was not
//! the fit: `lyracore-shared` cannot take an HTTP-capable dependency, it also compiles to wasm).
//!
//! [`sample_and_record`], the actual per-cycle DRIVER, takes the already-scraped occupancy numbers
//! as a plain `HashMap` and is generic over [`crate::realm_core::RealmDb`] — the exact same seam
//! `realm_core.rs` built for the realm-core split — so it runs under `fake::Handle` in this crate's
//! own tests, mutation-checked, with no live node at all.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

use crate::realm_core::RealmDb;

/// Default sampling cadence — matches the internal capacity-benchmark harness's own read of the same
/// node metric, and is short enough that a `SHARD_LOAD_RING` of 20
/// (`module/src/load.rs`) covers ~10 minutes of history.
const DEFAULT_SAMPLE_SECS: u64 = 30;

/// How long to sleep between samples — `LYRACORE_LOAD_SAMPLE_SECS`, default [`DEFAULT_SAMPLE_SECS`]. `0`
/// or unparseable falls back to the default rather than busy-looping.
pub(crate) fn sample_interval() -> Duration {
    let secs = std::env::var("LYRACORE_LOAD_SAMPLE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_SAMPLE_SECS);
    Duration::from_secs(secs)
}

/// `<shard>=<hex-identity-prefix>` pairs, comma/semicolon/newline separated, `#` comments — the
/// same separator convention `ShardMap::parse` uses for its rule text.
///
/// This mapping exists because SpacetimeDB's `/v1/metrics` labels samples by database IDENTITY
/// (`db="<hex>"`), not by the friendly name the gateway's own config uses
/// (the internal capacity-benchmark tool's `--db` flag takes the identical prefix, for the
/// identical reason). Discover a shard's identity the same way that tool does: a `--dry-run`
/// query against `/v1/metrics` lists every identity the node currently exposes.
pub(crate) fn parse_db_ids(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in text.split(['\n', ',', ';']) {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !v.is_empty() {
                out.insert(k.to_string(), v.to_string());
            }
        }
    }
    out
}

// ===================================================================================================
//  Prometheus scrape + parse — a from-scratch reimplementation of
//  the wire harness's `src/bin/bench/metrics.rs`'s identical computation, trimmed to the one family
//  this task needs (see the module doc for why it is not shared as a library).
// ===================================================================================================

/// One scrape, keyed on the verbatim `name{labels}` text of each sample line — same shape as the
/// capacity benchmark's `Snapshot`, and for the same reason: a key-wise lookup is all `sum` needs,
/// and the raw key keeps label filtering to a substring match.
#[derive(Clone, Debug, Default)]
struct Snapshot(BTreeMap<String, f64>);

/// The one metric family this task reads: "time spent executing a transaction, EXCLUDING time
/// spent waiting to acquire database locks" — i.e. the serialized writer's busy time.
const OCCUPANCY_FAMILY: &str = "spacetime_txn_cpu_time_sec_sum";

fn http_get(url: &str) -> Result<String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("only http:// URLs are supported (got {url:?})"))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let addr = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:80")
    };
    let mut s = TcpStream::connect(&addr).with_context(|| format!("connect metrics {addr}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nAccept: text/plain\r\nConnection: close\r\n\r\n"
    )?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).context("read metrics response")?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response from {url}"))?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200") {
        bail!("metrics endpoint {url} returned {status:?}");
    }
    Ok(body.to_string())
}

fn parse(text: &str) -> Snapshot {
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.rsplit_once(' ') else {
            continue;
        };
        let Ok(v) = val.trim().parse::<f64>() else {
            continue;
        };
        m.insert(key.to_string(), v);
    }
    Snapshot(m)
}

fn split_key(key: &str) -> (&str, &str) {
    match key.find('{') {
        Some(i) => (&key[..i], &key[i..]),
        None => (key, ""),
    }
}

/// Substring filter selecting one database on the node: `db="<prefix>`.
fn db_filter(db_id: &str) -> String {
    format!("db=\"{db_id}")
}

impl Snapshot {
    /// Sum every sample of `name` whose label blob contains `filter` — same rule the capacity
    /// benchmark's `Snapshot::sum` uses, trimmed to a single filter (this task only ever filters by
    /// one shard's `db=` prefix at a time).
    fn sum(&self, name: &str, filter: &str) -> f64 {
        self.0
            .iter()
            .filter(|(k, _)| {
                let (n, labels) = split_key(k);
                n == name && labels.contains(filter)
            })
            .map(|(_, v)| *v)
            .sum()
    }
}

/// Occupancy percentage from a CPU-time delta over a wall-clock delta (the same computation the
/// internal capacity-benchmark tool uses). Pure, so it is the one piece of this
/// module pinned directly against concrete numbers.
///
/// Clamped to `[0, 100]`... deliberately NOT clamped to 100 — a brief scheduling skew can make a
/// window read slightly over 100%, and clamping that away would hide a real saturation signal
/// behind a false ceiling. Only the FLOOR is enforced: a negative delta (the node restarted or the
/// module republished mid-window, so the counter went backwards) reads as `0.0`, not as a negative
/// occupancy that would make no sense on a graph. This module does not attempt the capacity
/// benchmark's stronger counter-reset detection (`Snapshot::first_negative`, which VOIDS a whole
/// measured rung) — that is the right rigor for a benchmark used to grade a perf claim; this is a
/// background convenience gauge, and reading one 30s sample as "briefly idle" instead of flagging a
/// reset is an acceptable trade for not needing a scheduled reaper or a voided-sample state machine.
pub(crate) fn occupancy_pct(cpu_sec_delta: f64, wall_dt_secs: f64) -> f32 {
    if wall_dt_secs <= 0.0 || !cpu_sec_delta.is_finite() {
        return 0.0;
    }
    ((cpu_sec_delta / wall_dt_secs) * 100.0).max(0.0) as f32
}

/// Scrapes `metrics_url` on demand and turns the delta since the PREVIOUS scrape into an
/// occupancy-percentage-per-shard map, for every shard named in `db_ids`.
///
/// One scrape serves every configured shard (the node's `/v1/metrics` already reports every
/// database it hosts in one response) — this samples the endpoint ONCE per cycle and slices it by
/// `db=` label per shard, rather than re-scraping per shard.
pub(crate) struct OccupancySampler {
    metrics_url: String,
    /// shard name -> database identity hex prefix (`LYRACORE_METRICS_DB_IDS`).
    db_ids: HashMap<String, String>,
    /// The previous cycle's scrape + the instant it was taken, shared across every configured
    /// shard (they all delta against the SAME two scrapes).
    last: Mutex<Option<(Snapshot, Instant)>>,
}

impl OccupancySampler {
    /// `stdb_base_url` is `LYRACORE_SPACETIMEDB_URL` (the same node every shard's coordinator connection
    /// targets) — `/v1/metrics` is appended here. Reads `LYRACORE_METRICS_DB_IDS` once; warns (loudly,
    /// once, at startup — the AC's "fails loudly when [manual config] disagrees with the topology")
    /// if it names nothing, since every shard then samples sessions but never occupancy.
    pub(crate) fn from_env(stdb_base_url: &str) -> Self {
        let db_ids = parse_db_ids(&std::env::var("LYRACORE_METRICS_DB_IDS").unwrap_or_default());
        if db_ids.is_empty() {
            log::warn!(
                "LYRACORE_METRICS_DB_IDS is unset — SHARDLOAD will sample session counts but report \
                 writer occupancy as unmeasured for every shard (issue #78). Discover each shard's \
                 database identity with `\"$(bash scripts/wire-harness.sh --bin vanilla-wire-bench)\" --dry-run 1`, \
                 then set LYRACORE_METRICS_DB_IDS=\"<shard>=<hex>,...\"."
            );
        }
        Self {
            metrics_url: format!("{}/v1/metrics", stdb_base_url.trim_end_matches('/')),
            db_ids,
            last: Mutex::new(None),
        }
    }

    /// One sampling cycle: scrape, then return `{shard -> occupancy_pct}` for every shard with a
    /// configured identity AND a prior baseline to delta against. A shard absent from the result is
    /// UNMEASURED this cycle (no configured id, this is the first-ever scrape, or the scrape
    /// itself failed) — never a measured zero; the caller must not conflate the two (the same care
    /// the internal capacity-benchmark tool takes over the identical trap).
    pub(crate) fn sample_all(&self) -> HashMap<String, f32> {
        if self.db_ids.is_empty() {
            return HashMap::new();
        }
        let snap = match http_get(&self.metrics_url).map(|t| parse(&t)) {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "SHARDLOAD: metrics scrape failed ({e:#}) — occupancy unmeasured this cycle"
                );
                return HashMap::new();
            }
        };
        let now = Instant::now();
        let mut guard = self.last.lock().unwrap();
        let out = match &*guard {
            Some((prev_snap, prev_at)) => {
                let dt = now.duration_since(*prev_at).as_secs_f64();
                self.db_ids
                    .iter()
                    .map(|(shard, id)| {
                        let filter = db_filter(id);
                        let delta = snap.sum(OCCUPANCY_FAMILY, &filter)
                            - prev_snap.sum(OCCUPANCY_FAMILY, &filter);
                        (shard.clone(), occupancy_pct(delta, dt))
                    })
                    .collect()
            }
            // First scrape ever: no baseline, so nothing can be DELTA'd yet — every shard is
            // unmeasured for exactly this one cycle.
            None => HashMap::new(),
        };
        *guard = Some((snap, now));
        out
    }
}

// ===================================================================================================
//  The driver — pure given the RealmDb abstraction, tested against `realm_core::fake::Handle`.
// ===================================================================================================

/// One sampling cycle: for every connected world shard, record its occupancy (if measured) +
/// session count onto realm-core. Returns one "SHARDLOAD ..."
/// line per shard for the caller to log at the sample cadence (the QUEUESTAT/AOISTAT convention,
/// `gateway/src/world/mod.rs`) — visible without `spacetime sql`.
///
/// `occupancy_by_shard` is whatever [`OccupancySampler::sample_all`] returned THIS cycle — passed
/// in rather than sampled here so this function stays generic over [`RealmDb`] and therefore
/// testable without a live node or a live HTTP scrape.
pub(crate) fn sample_and_record<D: RealmDb>(
    db: &D,
    occupancy_by_shard: &HashMap<String, f32>,
) -> Vec<String> {
    let rc = match db.realm_core() {
        Ok(rc) => rc,
        Err(e) => {
            return vec![format!(
                "SHARDLOAD: realm-core unreachable this cycle ({e:#}) — skipped"
            )]
        }
    };
    let mut lines = Vec::with_capacity(db.world_shards().len());
    for (name, shard) in db.world_shards() {
        let sessions = shard.session_count() as u32;
        match occupancy_by_shard.get(&name) {
            Some(&pct) => {
                if let Err(e) = rc.record_shard_load(&name, pct, sessions) {
                    log::warn!("SHARDLOAD: record_shard_load({name}) failed: {e:#}");
                }
                lines.push(format!(
                    "SHARDLOAD shard={name} occupancy={pct:.1}% sessions={sessions}"
                ));
            }
            None => {
                lines.push(format!(
                    "SHARDLOAD shard={name} occupancy=unmeasured sessions={sessions} \
                     (set LYRACORE_METRICS_DB_IDS to enable occupancy)"
                ));
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------------------
    //  Pure math: parsing, filtering, occupancy.
    // -------------------------------------------------------------------------------------

    const SAMPLE: &str = r#"
# HELP spacetime_txn_cpu_time_sec_sum the writer's busy time
spacetime_txn_cpu_time_sec_sum{db="abc123",txn_type="Reducer"} 10.0
spacetime_txn_cpu_time_sec_sum{db="abc123",txn_type="Subscribe"} 2.0
spacetime_txn_cpu_time_sec_sum{db="zzz999",txn_type="Reducer"} 99.0
"#;

    #[test]
    fn parse_reads_labelled_samples_and_skips_comments() {
        let s = parse(SAMPLE);
        assert_eq!(s.0.len(), 3);
    }

    #[test]
    fn sum_filters_by_db_label_and_aggregates_every_txn_type() {
        let s = parse(SAMPLE);
        assert_eq!(
            s.sum(OCCUPANCY_FAMILY, &db_filter("abc123")),
            12.0,
            "reducer + subscribe CPU"
        );
        assert_eq!(s.sum(OCCUPANCY_FAMILY, &db_filter("zzz999")), 99.0);
        assert_eq!(s.sum(OCCUPANCY_FAMILY, &db_filter("nonexistent")), 0.0);
    }

    #[test]
    fn occupancy_pct_is_cpu_delta_over_wall_delta_times_100() {
        assert_eq!(occupancy_pct(15.0, 30.0), 50.0);
        assert_eq!(occupancy_pct(30.0, 30.0), 100.0);
        assert_eq!(occupancy_pct(0.0, 30.0), 0.0);
    }

    #[test]
    fn occupancy_pct_does_not_clamp_a_brief_overrun_to_100() {
        // A real momentary skew can read slightly over 100% — clamping it away would hide a real
        // saturation signal behind a false ceiling. See the function's own doc.
        assert_eq!(occupancy_pct(33.0, 30.0), 110.0);
    }

    #[test]
    fn occupancy_pct_floors_a_negative_delta_at_zero_instead_of_going_negative() {
        // A counter reset (node restart / republish mid-window) makes the delta negative; reading
        // that as a negative percentage would make no sense on a graph.
        assert_eq!(occupancy_pct(-5.0, 30.0), 0.0);
    }

    #[test]
    fn occupancy_pct_is_zero_for_a_non_positive_or_non_finite_input() {
        assert_eq!(
            occupancy_pct(15.0, 0.0),
            0.0,
            "no wall-clock delta means nothing to divide by"
        );
        assert_eq!(occupancy_pct(15.0, -1.0), 0.0);
        assert_eq!(occupancy_pct(f64::NAN, 30.0), 0.0);
        assert_eq!(occupancy_pct(f64::INFINITY, 30.0), 0.0);
    }

    #[test]
    fn parse_db_ids_reads_key_value_pairs_and_skips_comments_and_blanks() {
        let m = parse_db_ids(
            "lyracore=abc123\n\
             # a comment line\n\
             lyracore-world-1 = def456 ; lyracore-world-2=ghi789\n\
             \n\
             malformed-no-equals\n",
        );
        assert_eq!(m.len(), 3);
        assert_eq!(m.get("lyracore"), Some(&"abc123".to_string()));
        assert_eq!(
            m.get("lyracore-world-1"),
            Some(&"def456".to_string()),
            "whitespace around = is trimmed"
        );
        assert_eq!(
            m.get("lyracore-world-2"),
            Some(&"ghi789".to_string()),
            "; also separates entries"
        );
    }

    #[test]
    fn parse_db_ids_of_an_empty_string_is_empty() {
        assert!(parse_db_ids("").is_empty());
        assert!(parse_db_ids("   \n # just a comment\n").is_empty());
    }

    // -------------------------------------------------------------------------------------
    //  The driver, wired against the realm_core fake — no live node, mutation-checked.
    // -------------------------------------------------------------------------------------

    use crate::realm_core::fake::{realm, Handle};

    const WORLD: &str = "world";
    const CORE: &str = "lyracore-realm";

    fn one_shard_realm() -> Handle {
        realm(&[WORLD, CORE], "", Some(CORE))
    }

    #[test]
    fn sample_and_record_forwards_occupancy_and_sessions_to_realm_core() {
        let h = one_shard_realm();
        *h.db_at(WORLD).open_sessions.lock().unwrap() = 7;
        let occ = HashMap::from([(WORLD.to_string(), 42.5f32)]);

        let lines = sample_and_record(&h, &occ);

        assert_eq!(
            h.db_at(CORE).recorded_shard_loads.lock().unwrap().clone(),
            vec![(WORLD.to_string(), 42.5, 7)],
            "the world shard's occupancy + session sample must land on REALM-CORE, not on itself"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("SHARDLOAD")
                && l.contains("occupancy=42.5%")
                && l.contains("sessions=7")),
            "a SHARDLOAD line must be emitted for the cycle: {lines:?}"
        );
    }

    #[test]
    fn sample_and_record_marks_a_shard_with_no_occupancy_entry_as_unmeasured_not_zero() {
        let h = one_shard_realm();
        *h.db_at(WORLD).open_sessions.lock().unwrap() = 3;
        let occ = HashMap::new(); // nothing measured this cycle

        let lines = sample_and_record(&h, &occ);

        assert!(
            h.db_at(CORE)
                .recorded_shard_loads
                .lock()
                .unwrap()
                .is_empty(),
            "an unmeasured shard must not write a fabricated occupancy row"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("occupancy=unmeasured") && l.contains("sessions=3")),
            "the line must say UNMEASURED, never a silent 0%: {lines:?}"
        );
    }

    #[test]
    fn sample_and_record_samples_every_connected_world_shard_not_just_the_first() {
        // `world_shards()` is the default database plus every RULE's target (`ShardMap::shards`'s
        // own doc) — a database that is merely connected, with no rule naming it, is not one of
        // them. "1:*=world-b" is what actually puts world-b in the set, mirroring how a real
        // second world shard is declared.
        let h = realm(&["world-a", "world-b", CORE], "1:*=world-b", Some(CORE));
        *h.db_at("world-a").open_sessions.lock().unwrap() = 10;
        *h.db_at("world-b").open_sessions.lock().unwrap() = 20;
        let occ = HashMap::from([
            ("world-a".to_string(), 10.0f32),
            ("world-b".to_string(), 90.0f32),
        ]);

        sample_and_record(&h, &occ);

        let mut recorded = h.db_at(CORE).recorded_shard_loads.lock().unwrap().clone();
        recorded.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            recorded,
            vec![
                ("world-a".to_string(), 10.0, 10),
                ("world-b".to_string(), 90.0, 20),
            ],
            "both world shards must be sampled, each under its OWN name and numbers"
        );
    }

    #[test]
    fn sample_and_record_skips_the_cycle_cleanly_when_realm_core_is_unreachable() {
        use crate::realm_core::fake::realm_with_dead_core;
        let h = realm_with_dead_core(&[WORLD, CORE], "", CORE);
        let lines = sample_and_record(&h, &HashMap::from([(WORLD.to_string(), 5.0f32)]));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("realm-core unreachable"), "{lines:?}");
    }

    #[test]
    fn an_unconfigured_single_database_gateway_samples_and_records_onto_itself() {
        // No LYRACORE_REALM_CORE: `realm_core()` answers `self`, so the sample still lands SOMEWHERE
        // sensible rather than silently doing nothing.
        let h = realm(&[WORLD], "", None);
        *h.db_at(WORLD).open_sessions.lock().unwrap() = 4;
        sample_and_record(&h, &HashMap::from([(WORLD.to_string(), 33.0f32)]));
        assert_eq!(
            h.db_at(WORLD).recorded_shard_loads.lock().unwrap().clone(),
            vec![(WORLD.to_string(), 33.0, 4)]
        );
    }
}
