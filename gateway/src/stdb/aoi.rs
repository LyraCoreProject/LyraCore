//! Area-of-Interest tracker — a per-player, grid-SCOPED `game_world_entity` subscription that recenters
//! (unsubscribe+resubscribe) as the player crosses grid cells, so the client receives only entities in
//! its 5×5 grid box instead of the whole world. The entity `on_insert`/`on_delete`/`on_update` callbacks
//! are registered connection-level in `subscriptions.rs`; they fire automatically for rows entering or
//! leaving this scoped set (the SDK's refcounted union cache), so this tracker only owns the SQL footprint.
//! Off by default (`config::aoi_enabled`, env `LYRACORE_AOI`).
//!
//! **#73 — cross-seam view-merge (rebuilt on the measured-cheap shape).** #190 measured that
//! per-cell subscriptions saturate the writer twice over — a per-cell equality query costs more to
//! REGISTER than a whole box range costs to EVALUATE — and its closing decision is the law for this
//! file: a straddling box splits into per-shard RANGE sub-boxes, never per-cell queries. Every
//! recenter, [`split_box_by_shard`] cuts the wanted box at whatever region boundaries the shard
//! resolver reports (usually zero, occasionally one straight line, at most a region corner), and
//! each shard's share — one or two [`GridRect`]s — is subscribed as ONE handle carrying the SAME 4
//! range queries [`box_queries`] always has ([`rect_queries`]), on THAT shard's own per-account
//! connection. Recenter is still subscribe-new-then-unsubscribe-old-on-applied, just done once per
//! shard instead of once for the whole box. A non-straddling box collapses to exactly one shard
//! (home) with exactly one rect (the box itself), and [`GridRect::from_box`] emits the identical
//! 4-tuple [`GridBox::bounds`] does — so the common case is BYTE-IDENTICAL to pre-#73 traffic, not
//! merely equivalent.
//!
//! `LYRACORE_VIEW_MERGE=0` (`config::view_merge_enabled`) collapses every box to home-only, same escape
//! hatch shape as `LYRACORE_AOI`. See `docs/region-sharding.md`'s view-merge section for the operator
//! story and the deliberately-out-of-scope list.
//!
//! **The away leg never blocks the movement thread.** Opening a foreign shard's per-account
//! connection is the one genuinely slow step in this whole file (`Coordinator::player_conn` builds a
//! fresh SDK connection, up to a 20s timeout) — #207's first fast-follow on the original #73 PR.
//! [`AreaOfInterestTracker::ensure_away`] kicks it off on a background thread and polls a shared slot;
//! until it resolves, that shard's cells are simply not covered THIS recenter (degrade = the far side
//! pops in late on the very next crossing, never a stall of the player's own packet processing).
//!
//! **The away leg mirrors two viewer-owned flags, never reads them cross-shard per row.** The peer-
//! visibility gates spirit-healer/ghost and stealth relaying already runs on the HOME leg need the
//! viewer's own GHOST player-flag (stealth itself is read off whatever connection holds the
//! CANDIDATE peer's aura rows — the away leg subscribes `game_aura` globally, same as home, so that
//! read is same-shard, not cross-shard). [`ViewerGates`] is the one small piece of shared state a
//! HOME-side callback writes and the away closures load — #207's second fast-follow.

use anyhow::{anyhow, Result};
use lyracore_shared::spatial::{GridBox, GridRect};
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use super::bindings::*;
use super::connection::{Coordinator, PlayerConn};
use super::subscriptions::{
    away_offer_peer_create, register_peer_view_relays, AWAY_VIEWER_INSTANCE,
};
use crate::world::SessionTx;

/// #184 (churn experiment): count every recenter (cell crossing → box re-subscribe) across every
/// session, cheaply — one `fetch_add` on the movement hot path, read back by a periodic log line
/// in `world/mod.rs` alongside `MOTIONSTAT`. Gateway-side only; no module change, no per-session
/// state, nothing that could perturb the race #109's diagnosis counters were built to avoid
/// touching. This is a COUNT, not a rate — the 10s log line in `world/mod.rs` takes the delta.
pub(crate) static AOI_RECENTERS: AtomicU64 = AtomicU64::new(0);

/// One `SELECT * FROM <table> WHERE map_id = .. AND grid_x/grid_y BETWEEN ..` range query — the
/// shared shape [`aoi_query`]/[`aoi_go_query`]/[`aoi_motion_query`]/[`aoi_spline_query`]/[`rect_queries`]
/// all build from, so a box's query and its post-split rect's query can never drift apart in format.
fn table_range_query(
    table: &str,
    map_id: u32,
    gx_min: i32,
    gx_max: i32,
    gy_min: i32,
    gy_max: i32,
) -> String {
    format!(
        "SELECT * FROM {table} WHERE map_id = {map_id} AND grid_x >= {gx_min} AND grid_x <= {gx_max} AND grid_y >= {gy_min} AND grid_y <= {gy_max}"
    )
}

/// The SQL the AOI subscription uses: `game_world_entity` rows whose cell falls in the 5×5 box — a range
/// on the `by_grid` btree `(map_id, grid_x, grid_y)`, so the server query is index-friendly. Pure →
/// unit-tested.
pub(crate) fn aoi_query(map_id: u32, b: &GridBox) -> String {
    let (gx_min, gx_max, gy_min, gy_max) = b.bounds();
    table_range_query("game_world_entity", map_id, gx_min, gx_max, gy_min, gy_max)
}

/// The gameobject twin of [`aoi_query`] (246): scope the GO subscription to the same grid box —
/// the un-scoped table shipped ~4.6k zone-wide GOs to every client (the felt frame hitch + the
/// login burst). Same enter/leave semantics: the union cache fires on_insert/on_delete only for
/// true box transitions, and the existing on_go_* callbacks relay CREATE/DESTROY.
pub(crate) fn aoi_go_query(map_id: u32, b: &GridBox) -> String {
    let (gx_min, gx_max, gy_min, gy_max) = b.bounds();
    table_range_query("game_gameobject", map_id, gx_min, gx_max, gy_min, gy_max)
}

/// The MOTION twin of [`aoi_query`] (perf catalog 2.1): the same 5×5 box, over the per-mover
/// `game_entity_motion` row. This is what replaces the per-recipient `game_movement_event` fan-out —
/// the writer stops computing recipients (one indexed UPDATE per heartbeat instead of one INSERT per
/// nearby player), and the subscription engine, which already evaluates this exact box for entities,
/// does the fan-out instead.
pub(crate) fn aoi_motion_query(map_id: u32, b: &GridBox) -> String {
    let (gx_min, gx_max, gy_min, gy_max) = b.bounds();
    table_range_query("game_entity_motion", map_id, gx_min, gx_max, gy_min, gy_max)
}

/// The CREATURE-MOVEMENT twin of [`aoi_query`]: the same 5×5 box over `game_creature_spline`.
///
/// Replaces a GLOBAL `SELECT * FROM game_creature_move_event` subscription — every creature leg in
/// the world was delivered to every connected session, which the gateway then discarded for any guid
/// not in `created`. Measured at 100 dispersed players: 121.7 legs/s fanned to all 100 sessions.
/// Scoping it to the box the observer already subscribes makes the delivery match the interest.
pub(crate) fn aoi_spline_query(map_id: u32, b: &GridBox) -> String {
    let (gx_min, gx_max, gy_min, gy_max) = b.bounds();
    table_range_query(
        "game_creature_spline",
        map_id,
        gx_min,
        gx_max,
        gy_min,
        gy_max,
    )
}

/// THE box's query set — every table the player's 5×5 box scopes, in one place.
///
/// 109: login and recenter each built their own list, and 2.1/2.3's `game_entity_motion` +
/// `game_creature_spline` were added to the LOGIN one only. Recenter then subscribed 2 queries and
/// dropped the 4-query handle on apply, so the first cell crossing (~7 s at run speed) silently and
/// permanently killed peer movement AND creature legs for that session. Both call sites take the
/// list from here so the two can never drift again — adding a box-scoped table is one edit.
pub(crate) fn box_queries(box_: &GridBox) -> Vec<String> {
    vec![
        aoi_query(box_.map_id, box_),
        aoi_go_query(box_.map_id, box_),
        aoi_motion_query(box_.map_id, box_),
        aoi_spline_query(box_.map_id, box_),
    ]
}

/// The RECT twin of [`box_queries`] (#73): the same 4 tables, same query shape, any rectangle — not
/// necessarily anchored on a cell, because a straddling box's per-shard share generally isn't. Built
/// from the SAME [`table_range_query`] the 4 `aoi_*_query` functions use, which is why a rect built
/// from a whole box ([`GridRect::from_box`]) produces byte-identical SQL to [`box_queries`] — pinned
/// by `rect_queries_of_a_whole_box_are_byte_identical_to_box_queries` below.
pub(crate) fn rect_queries(rect: &GridRect) -> Vec<String> {
    let (gx_min, gx_max, gy_min, gy_max) = rect.bounds();
    [
        "game_world_entity",
        "game_gameobject",
        "game_entity_motion",
        "game_creature_spline",
    ]
    .into_iter()
    .map(|table| table_range_query(table, rect.map_id, gx_min, gx_max, gy_min, gy_max))
    .collect()
}

/// Build + apply a scoped `game_world_entity` subscription for `box_`, blocking until the server
/// acknowledges it (`on_applied`) so the caller can safely drop a prior subscription afterward.
fn subscribe_box(conn: &Arc<PlayerConn>, box_: &GridBox) -> Result<SubscriptionHandle> {
    let (atx, arx) = mpsc::channel::<std::result::Result<(), String>>();
    let atx_err = atx.clone();
    let sub = conn
        .conn
        .subscription_builder()
        .on_applied(move |_ctx| {
            let _ = atx.send(Ok(()));
        })
        .on_error(move |_ctx, err| {
            let _ = atx_err.send(Err(format!("{err}")));
        })
        .subscribe(box_queries(box_));
    match arx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => Ok(sub),
        Ok(Err(e)) => Err(anyhow!("AOI subscription error: {e}")),
        Err(_) => Err(anyhow!("AOI subscription not applied within 15s")),
    }
}

/// Split `box_` into per-shard grid-cell rectangles at whatever region boundaries `resolve` reports
/// (#73's rebuild). `resolve(gx, gy)` answers the SAME per-cell truth
/// `config::resolve_region_shard_for_cell` does (`None` = home) — this is the boundary-line splitter,
/// not a separately-materialized per-cell partition map: the resolve and the rectangle decomposition
/// are one pass, not two.
///
/// # Geometry
///
/// `lyracore_shared::region::Region` floors every side at 10 cells and a box is only 5 cells wide, so a
/// box can straddle AT MOST ONE region boundary per axis — never three regions in a row. The scan
/// below does not lean on that fact for CORRECTNESS (it is a general row-run-length + vertical-merge
/// rectangle decomposition of any per-cell partition, so a bigger box or a smaller region floor would
/// still produce a correct result), only for the ≤2-rects-per-shard bound the design doc promises for
/// TODAY's geometry:
///
/// - **No split**: every cell resolves the same → one shard, one rect (`box_` itself).
/// - **Edge split**: one straight boundary crosses the box → one home rect + one away rect, both
///   spanning the box's full height or width.
/// - **Corner split**: a region corner sits inside the box → up to 4 quadrants; a shard occupying two
///   DIAGONAL quadrants (never merge-able into one rectangle) gets exactly 2 disjoint rects — the
///   "L-shape" the design doc names. No shard ever needs more than 2 for a 5-wide box, because each
///   axis has at most one boundary to be on either side of.
///
/// # Byte-identity for the common case
///
/// A box entirely on one shard returns `vec![(shard, vec![GridRect::from_box(box_)])]` — the SAME
/// 4-tuple bounds [`box_queries`] already builds its SQL from, so a non-straddling recenter's rect
/// query text is identical to today's whole-box query, not merely equivalent.
pub(crate) fn split_box_by_shard(
    box_: &GridBox,
    resolve: impl Fn(i32, i32) -> Option<String>,
) -> Vec<(Option<String>, Vec<GridRect>)> {
    let (gx_min, gx_max, gy_min, gy_max) = box_.bounds();

    struct Run {
        shard: Option<String>,
        gx0: i32,
        gx1: i32,
    }
    struct Open {
        shard: Option<String>,
        gx0: i32,
        gx1: i32,
        gy0: i32,
        gy1: i32,
    }

    let mut open: Vec<Open> = Vec::new();
    let mut closed: Vec<Open> = Vec::new();

    for gy in gy_min..=gy_max {
        // This row's shard runs: contiguous gx spans that resolve to the same shard.
        let mut row: Vec<Run> = Vec::new();
        for gx in gx_min..=gx_max {
            let shard = resolve(gx, gy);
            match row.last_mut() {
                Some(run) if run.shard == shard => run.gx1 = gx,
                _ => row.push(Run {
                    shard,
                    gx0: gx,
                    gx1: gx,
                }),
            }
        }
        // Extend an OPEN rect whose gx-span and shard match this row's run and which ended exactly
        // one row above; close everything else that didn't extend, open fresh entries for the rest.
        let mut next_open: Vec<Open> = Vec::new();
        for run in row {
            if let Some(pos) = open.iter().position(|o| {
                o.shard == run.shard && o.gx0 == run.gx0 && o.gx1 == run.gx1 && o.gy1 == gy - 1
            }) {
                let mut o = open.remove(pos);
                o.gy1 = gy;
                next_open.push(o);
            } else {
                next_open.push(Open {
                    shard: run.shard,
                    gx0: run.gx0,
                    gx1: run.gx1,
                    gy0: gy,
                    gy1: gy,
                });
            }
        }
        closed.append(&mut open);
        open = next_open;
    }
    closed.extend(open);

    let mut out: Vec<(Option<String>, Vec<GridRect>)> = Vec::new();
    for o in closed {
        let rect = GridRect {
            map_id: box_.map_id,
            gx_min: o.gx0,
            gx_max: o.gx1,
            gy_min: o.gy0,
            gy_max: o.gy1,
        };
        match out.iter_mut().find(|(s, _)| *s == o.shard) {
            Some((_, rects)) => rects.push(rect),
            None => out.push((o.shard, vec![rect])),
        }
    }
    out
}

/// Subscribe every rect in `new_rects` on `conn`, non-blocking, and unsubscribe every handle in `old`
/// once ALL of them have APPLIED — never on error. Generalizes the pre-#73 single-box recenter
/// (subscribe-new-then-unsubscribe-old-on-applied) from one handle to N, so a shard whose share of the
/// box decomposed into 2 rects (the corner case) still gets exactly this ordering: overlap rows are
/// never briefly covered by neither the old nor the new coverage.
///
/// **On error, `old` is deliberately never retired** — same philosophy the pre-#73 single-box path
/// documented: dropping `old` here would leave whatever that rect covered entity-blind. The tracker
/// keeps whatever new rects DID apply plus the errored one's (inert) handle; `old`'s remaining
/// handles are effectively leaked until a FUTURE recenter fully replaces this bucket. Accepted, not
/// fixed here — it is the exact tradeoff the box-only code already made, just generalized to N.
///
/// `on_applied_rect` fires once per NEWLY-issued subscription's apply, before the shared countdown
/// ticks — the away leg's resident-sweep hook (145-class, [`sweep_away_rect`]); the home leg passes a
/// no-op.
fn resubscribe_rects(
    conn: &Arc<PlayerConn>,
    label: &str,
    old: Vec<SubscriptionHandle>,
    new_rects: Vec<GridRect>,
    on_applied_rect: Arc<dyn Fn(GridRect) + Send + Sync>,
) -> Vec<SubscriptionHandle> {
    if new_rects.is_empty() {
        for h in old {
            let _ = h.unsubscribe();
        }
        return Vec::new();
    }
    let remaining = Arc::new(Mutex::new(new_rects.len()));
    let old = Arc::new(Mutex::new(Some(old)));
    let mut fresh = Vec::with_capacity(new_rects.len());
    for rect in new_rects {
        let remaining = remaining.clone();
        let old = old.clone();
        let on_applied_rect = on_applied_rect.clone();
        let label = label.to_string();
        let sub = conn
            .conn
            .subscription_builder()
            .on_applied(move |_ctx| {
                on_applied_rect(rect);
                let mut r = remaining.lock().unwrap();
                if *r > 0 {
                    *r -= 1;
                }
                if *r == 0 {
                    if let Some(old_handles) = old.lock().unwrap().take() {
                        for h in old_handles {
                            let _ = h.unsubscribe();
                        }
                    }
                }
            })
            .on_error(move |_ctx, err| {
                log::warn!(
                    "view-merge: {label} rect {rect:?} resubscribe failed (keeping the previous \
                     coverage for this shard — degraded, not broken; a later recenter retries): {err}"
                );
            })
            .subscribe(rect_queries(&rect));
        fresh.push(sub);
    }
    fresh
}

/// #73 / work-item 145 on the away leg: offer every row ALREADY resident in `rect` on `conn` through
/// the away peer-create gate. A subscription's very first apply does not reliably fire per-row
/// `on_insert` (the same reason `subscribe_player_events`'s login sweep exists) — an away shard's
/// first-ever rect subscription is exactly that situation on a brand-new connection. Cheap (bounded
/// by the rect's own resident row count) and idempotent via the shared `created` dedup, so running it
/// on every away rect's apply — not just a shard's first — costs a small scan instead of a
/// hard-to-prove "only the very first rect" branch.
#[allow(clippy::too_many_arguments)]
fn sweep_away_rect(
    conn: &Arc<PlayerConn>,
    coord: &Coordinator,
    tx: &SessionTx,
    created: &Arc<Mutex<HashSet<u64>>>,
    viewer_gates: &Arc<ViewerGates>,
    self_guid: u64,
    rect: &GridRect,
) {
    let (gx_min, gx_max, gy_min, gy_max) = rect.bounds();
    let resident: Vec<WorldEntity> = conn
        .conn
        .db
        .game_world_entity()
        .iter()
        .filter(|e| {
            e.map_id == rect.map_id
                && e.grid_x >= gx_min
                && e.grid_x <= gx_max
                && e.grid_y >= gy_min
                && e.grid_y <= gy_max
        })
        .collect();
    for row in &resident {
        away_offer_peer_create(
            &conn.conn.db,
            coord,
            tx,
            created,
            viewer_gates,
            self_guid,
            AWAY_VIEWER_INSTANCE,
            row,
        );
    }
}

/// The viewer-owned state the away leg's peer-visibility gate needs but cannot read off an away
/// connection (#207 fast-follow 2) — just the spirit-healer ghost-gate's `viewer_is_ghost`. Written
/// from a HOME-side callback (the existing ghost-flag edge-detect on the viewer's own row, in
/// `subscriptions::subscribe_player_events`) and loaded by the away closures — a plain atomic, never
/// a cross-shard read per row. The stealth gate needs no mirror at all: it reads the CANDIDATE peer's
/// own aura rows, which live on whichever shard the candidate is resident on (the away connection
/// subscribes `game_aura` globally, same as home), so that read is already same-shard.
#[derive(Default)]
pub(crate) struct ViewerGates {
    pub(crate) is_ghost: AtomicBool,
    /// #74 seam-chat: the viewer's own live `(map_id, x, y)`, mirrored on every processed movement
    /// packet (`AreaOfInterestTracker::update`/`new`) — same reasoning as `is_ghost` above: the
    /// viewer's own `game_world_entity` row lives ONLY on the home shard, so the away leg's chat/
    /// emote relay (`register_peer_view_relays`) needs a same-cost mirror to compute distance-to-
    /// speaker instead of a cross-shard read. `f32` has no lock-free atomic in std, so it rides as
    /// bits through an `AtomicU32` (`f32::to_bits`/`from_bits`, exact — no precision loss).
    map_id: AtomicU32,
    x_bits: AtomicU32,
    y_bits: AtomicU32,
}

impl ViewerGates {
    pub(crate) fn is_ghost(&self) -> bool {
        self.is_ghost.load(Ordering::Relaxed)
    }

    /// Mirror the viewer's own position (#74). Called on every movement packet (not just a
    /// recenter), so the away leg's chat range check is never more than one heartbeat stale.
    pub(crate) fn set_position(&self, map_id: u32, x: f32, y: f32) {
        self.map_id.store(map_id, Ordering::Relaxed);
        self.x_bits.store(x.to_bits(), Ordering::Relaxed);
        self.y_bits.store(y.to_bits(), Ordering::Relaxed);
    }

    /// The mirrored `(map_id, x, y)` — reads back whatever `set_position` last stored.
    pub(crate) fn position(&self) -> (u32, f32, f32) {
        (
            self.map_id.load(Ordering::Relaxed),
            f32::from_bits(self.x_bits.load(Ordering::Relaxed)),
            f32::from_bits(self.y_bits.load(Ordering::Relaxed)),
        )
    }
}

/// One foreign shard's AOI coverage (#73): its own per-account connection, its own peer-visibility
/// relay teardown, and the rect subscriptions currently applied there. Parked (connection open,
/// callbacks registered, `rects` empty) rather than torn down the instant the box stops straddling it
/// — see [`AreaOfInterestTracker::recenter_away`]'s doc comment for the async-unsubscribe-ack race
/// that parking avoids. Actually torn down only by `AreaOfInterestTracker`'s own `Drop`.
struct AwayShard {
    conn: Arc<PlayerConn>,
    coord: Coordinator,
    rects: Vec<SubscriptionHandle>,
    /// The rects `rects` currently covers — compared against a bucket's next `want` set
    /// (`rects_match`) so a heartbeat that re-asks without anything actually changing (a parked
    /// straddle, or a background connect still resolving) never re-issues an identical subscribe.
    want: Vec<GridRect>,
    teardown: Option<Box<dyn FnOnce() + Send>>,
}

/// Do `a` and `b` cover the SAME set of rectangles, ignoring order? Both sides are ≤2 elements in
/// practice (a shard's box share never decomposes further for a 5-wide box), so a linear
/// mutual-containment check is simpler than sorting and just as cheap.
fn rects_match(a: &[GridRect], b: &[GridRect]) -> bool {
    a.len() == b.len() && a.iter().all(|r| b.contains(r))
}

impl Drop for AwayShard {
    fn drop(&mut self) {
        for h in self.rects.drain(..) {
            let _ = h.unsubscribe();
        }
        if let Some(td) = self.teardown.take() {
            td();
        }
    }
}

/// One foreign shard's connection-setup state (#207 fast-follow 1): either still connecting on a
/// background thread (a shared slot the thread fills exactly once) or ready to subscribe rects on.
enum Away {
    // The shared once-filled slot the away-shard connect thread writes: `Result<(conn, coordinator), String>` behind the Arc/Mutex/Option the hand-off needs.
    #[allow(clippy::type_complexity)]
    Connecting(Arc<Mutex<Option<std::result::Result<(Arc<PlayerConn>, Coordinator), String>>>>),
    Ready(AwayShard),
}

/// Owns the player's AOI coverage + its current box anchor — the HOME shard's rect(s) AND every
/// straddled AWAY shard's (#73). `Drop` unsubscribes everything (the session is ending — peers learn
/// of the leave via the entity delete on logout, not this).
pub struct AreaOfInterestTracker {
    conn: Arc<PlayerConn>,
    /// This session's HOME shard handle — the routing root #73 resolves every recenter's box against.
    coordinator: Coordinator,
    account_id: u64,
    self_guid: u64,
    /// Cloned into every away shard's relay registration so a peer seen from either shard renders
    /// through the same socket.
    tx: SessionTx,
    /// The SAME dedup set `subscribe_player_events` seeds with `self_guid` and hands to the home
    /// registration — shared, not owned, so a peer dedupes across shards.
    created: Arc<Mutex<HashSet<u64>>>,
    /// The viewer-state mirror the away leg's gate reads (#207 fast-follow 2). Cloned into every
    /// away shard's relay registration.
    viewer_gates: Arc<ViewerGates>,
    /// #74 seam-chat: the SAME per-table dedup sets `subscribe_player_events` seeds and hands to the
    /// home `game_chat_event`/`game_emote_event` registrations — shared, not owned, so a chat/emote
    /// row (living on exactly one physical database) can only ever cross this session's socket once,
    /// the same discipline `created` already gives peer CREATEs.
    chat_seen: Arc<Mutex<HashSet<u64>>>,
    emote_seen: Arc<Mutex<HashSet<u64>>>,
    /// Captured once at construction (house pattern: `config::aoi_enabled` is captured the same way
    /// in `subscribe_player_events`) — a mid-session env change was never a supported shape for any
    /// gate in this file.
    view_merge: bool,
    box_: GridBox,
    /// The HOME shard's currently-applied rect subscriptions. Exactly one (the whole box) on a
    /// non-straddling session — the byte-identical-to-pre-#73 case — up to two on a corner straddle.
    home_rects: Vec<SubscriptionHandle>,
    /// The rects `home_rects` currently covers — see [`rects_match`].
    home_want: Vec<GridRect>,
    /// One entry per FOREIGN shard the box has EVER touched this session, keyed by database name.
    /// Empty on every session that never straddles a seam — including every single-database realm
    /// and every un-imported seam menu, where `split_box_by_shard` never names a shard at all.
    away: HashMap<String, Away>,
}

impl AreaOfInterestTracker {
    /// Subscribe the initial 5×5 box around the player's login position; the already-registered
    /// entity `on_insert` callbacks deliver the in-box peers as CREATE on apply. Unchanged by #73 —
    /// login stays home-only (module doc above), so this does not resolve shards at all.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conn: Arc<PlayerConn>,
        coordinator: Coordinator,
        account_id: u64,
        self_guid: u64,
        tx: SessionTx,
        created: Arc<Mutex<HashSet<u64>>>,
        viewer_gates: Arc<ViewerGates>,
        chat_seen: Arc<Mutex<HashSet<u64>>>,
        emote_seen: Arc<Mutex<HashSet<u64>>>,
        map_id: u32,
        x: f32,
        y: f32,
    ) -> Result<Self> {
        let box_ = GridBox::around(map_id, x, y);
        let sub = subscribe_box(&conn, &box_)?;
        // #74 seam-chat: seed the mirror at login, before any `update()` call ever runs — an away
        // shard's chat relay must never read the all-zeros default for a player who straddled a
        // seam on their very first recenter.
        viewer_gates.set_position(map_id, x, y);
        Ok(Self {
            conn,
            coordinator,
            account_id,
            self_guid,
            tx,
            created,
            viewer_gates,
            chat_seen,
            emote_seen,
            view_merge: crate::config::view_merge_enabled(),
            box_,
            home_rects: vec![sub],
            home_want: vec![GridRect::from_box(&box_)],
            away: HashMap::new(),
        })
    }

    /// Drive from the player's movement — called on EVERY processed movement packet, not only a
    /// cell crossing (`world/mod.rs`'s `aoi_update`). If they've crossed out of the anchor cell,
    /// recenter — split the new box by shard, then resubscribe each touched bucket (home + every
    /// away shard the new split names, plus any away shard still parked from an earlier straddle)
    /// independently. NON-BLOCKING (253): every subscribe call below rides into its own
    /// `on_applied`/`on_error` callback on the SDK thread — the hot path never waits on a round
    /// trip, home OR away.
    ///
    /// Runs even WITHOUT a real crossing while an away shard is still `Connecting` (#207 fast-follow
    /// 1's other half): the background connect can resolve between two heartbeats with no cell
    /// crossing in between, and without this a player who stops walking exactly on the touching
    /// cell would never get the away rects it was waiting on — "the far side pops in late" would
    /// become "the far side never pops in at all". `recenter_home`/`recenter_away`'s own
    /// [`rects_match`] guard means this extra check costs nothing once every bucket is settled: no
    /// crossing + no pending connect + unchanged rects is the by-far-most-common per-heartbeat call,
    /// and it does no I/O at all.
    pub fn update(&mut self, x: f32, y: f32) {
        // #74 seam-chat: refresh the position mirror on EVERY movement packet, not only a recenter —
        // the away leg's chat/emote range check needs a live position between recenters too, and the
        // early-return below (no crossing, no pending connect) must not skip this.
        self.viewer_gates.set_position(self.box_.map_id, x, y);
        let recentering = self.box_.needs_recenter(self.box_.map_id, x, y);
        let pending_connect = self.away.values().any(|a| matches!(a, Away::Connecting(_)));
        if !recentering && !pending_connect {
            return;
        }
        if recentering {
            AOI_RECENTERS.fetch_add(1, Ordering::Relaxed);
        }
        let new_box = if recentering {
            GridBox::around(self.box_.map_id, x, y)
        } else {
            self.box_
        };
        let split = self.resolve_split(&new_box);

        let mut new_home: Vec<GridRect> = Vec::new();
        let mut new_away: HashMap<String, Vec<GridRect>> = HashMap::new();
        for (shard, rects) in split {
            match shard {
                None => new_home = rects,
                Some(name) => {
                    new_away.insert(name, rects);
                }
            }
        }

        self.recenter_home(new_home);

        // Every shard EITHER currently parked OR newly wanted — a shard dropping to zero wanted
        // rects still runs through `recenter_away` once, to unsubscribe every rect it held.
        let mut touched: HashSet<String> = self.away.keys().cloned().collect();
        touched.extend(new_away.keys().cloned());
        for shard in touched {
            let want = new_away.remove(&shard).unwrap_or_default();
            self.recenter_away(&shard, want);
        }

        self.box_ = new_box;
    }

    /// #73: resolve the new box's per-shard rectangle split, or the home-only identity split every
    /// degenerate case (view-merge off, an unresolvable instance) takes. Reads the viewer's CURRENT
    /// instance from the home connection's own cache (never a probe — the tracker is driven by a
    /// movement `(x, y)`, not an identity lookup), because region routing never applies inside an
    /// instance and an unknown instance must not be guessed at as "0".
    fn resolve_split(&self, box_: &GridBox) -> Vec<(Option<String>, Vec<GridRect>)> {
        let home_only = || vec![(None, vec![GridRect::from_box(box_)])];
        if !self.view_merge {
            return home_only();
        }
        let Some(instance_id) = self.current_instance_id() else {
            return home_only();
        };
        self.coordinator.split_box_by_shard(box_, instance_id)
    }

    /// The viewer's own live `instance_id`, read from the HOME connection's cache — `None` when the
    /// self row isn't resident there (mid-teardown, or a relay firing before the self CREATE lands).
    /// `resolve_split` treats `None` as "skip view-merge this recenter", never as "assume instance 0".
    fn current_instance_id(&self) -> Option<u64> {
        self.conn
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&self.self_guid)
            .map(|v| v.instance_id)
    }

    /// Resubscribe the HOME bucket to `new_rects` — one call, whether the box is straddling or not.
    /// A non-straddling recenter passes exactly one rect (the whole box), so this is textually the
    /// same subscribe-new-then-unsubscribe-old-on-applied sequence the pre-#73 single-box code ran.
    /// A no-op if `new_rects` is exactly what's already covering ([`rects_match`]) — the guard that
    /// makes a pending-connect heartbeat with no real crossing free once the home bucket is settled.
    fn recenter_home(&mut self, new_rects: Vec<GridRect>) {
        if rects_match(&self.home_want, &new_rects) {
            return;
        }
        let old = std::mem::take(&mut self.home_rects);
        self.home_rects = resubscribe_rects(
            &self.conn,
            "home",
            old,
            new_rects.clone(),
            Arc::new(|_rect| {}),
        );
        self.home_want = new_rects;
    }

    /// Resubscribe `shard`'s away bucket to `want` — opening (or reusing) its connection first via
    /// [`Self::ensure_away`]. If the connection is still connecting (or just started connecting this
    /// call), `want` is simply not covered THIS recenter: nothing is lost, because `want` is
    /// recomputed fresh from the box's geometry every time `update` runs, and either the next real
    /// crossing OR the next heartbeat's pending-connect check (see `update`'s doc comment) tries
    /// again with whatever `ensure_away` has resolved to by then (#207 fast-follow 1's degrade
    /// contract — the far side pops in late, the player's own packet processing never stalls). A
    /// no-op if `want` is exactly what's already covering ([`rects_match`]) once the shard IS ready —
    /// the guard that makes the pending-connect re-check above free for every OTHER already-settled
    /// away shard.
    fn recenter_away(&mut self, shard: &str, want: Vec<GridRect>) {
        if !self.ensure_away(shard) {
            return;
        }
        let away = match self.away.get_mut(shard) {
            Some(Away::Ready(a)) => a,
            _ => return, // ensure_away just proved this, but never trust it across a borrow split
        };
        if rects_match(&away.want, &want) {
            return;
        }
        let old = std::mem::take(&mut away.rects);
        let conn = away.conn.clone();
        let coord = away.coord.clone();
        let tx = self.tx.clone();
        let created = self.created.clone();
        let viewer_gates = self.viewer_gates.clone();
        let self_guid = self.self_guid;
        let on_applied: Arc<dyn Fn(GridRect) + Send + Sync> = Arc::new(move |rect| {
            sweep_away_rect(
                &conn,
                &coord,
                &tx,
                &created,
                &viewer_gates,
                self_guid,
                &rect,
            );
        });
        away.rects = resubscribe_rects(&away.conn.clone(), shard, old, want.clone(), on_applied);
        away.want = want;
    }

    /// Ensure `shard`'s away bucket is ready to subscribe rects on — `true` if it is (already
    /// `Ready`, or a background connect that JUST resolved), `false` if it is still connecting (a
    /// fresh attempt was just kicked off, or one already in flight hasn't answered yet).
    ///
    /// The connect itself (`Coordinator::player_conn`, which opens a brand-new SDK connection) runs
    /// on a plain background thread — #207 fast-follow 1: this is the ONE genuinely slow step in the
    /// whole file, and it must never run on the caller's (movement-packet) thread. A shared
    /// `Mutex<Option<Result<..>>>` slot is the whole state machine: the thread fills it exactly once,
    /// and this method drains it the next time it's asked about that shard — no polling loop, no
    /// timer, just "check when a recenter happens to ask again".
    fn ensure_away(&mut self, shard: &str) -> bool {
        match self.away.get(shard) {
            Some(Away::Ready(_)) => return true,
            Some(Away::Connecting(slot)) => {
                let resolved = slot.lock().unwrap().take();
                match resolved {
                    Some(Ok((conn, coord))) => {
                        let teardown = register_peer_view_relays(
                            &conn,
                            &coord,
                            self.tx.clone(),
                            self.created.clone(),
                            self.viewer_gates.clone(),
                            self.chat_seen.clone(),
                            self.emote_seen.clone(),
                            self.self_guid,
                        );
                        self.away.insert(
                            shard.to_string(),
                            Away::Ready(AwayShard {
                                conn,
                                coord,
                                rects: Vec::new(),
                                want: Vec::new(),
                                teardown: Some(teardown),
                            }),
                        );
                        return true;
                    }
                    Some(Err(e)) => {
                        log::warn!(
                            "view-merge: away shard {shard} connect failed ({e}) — this player's box \
                             stays uncovered there until a later recenter retries"
                        );
                        self.away.remove(shard); // a fresh attempt starts clean next time
                        return false;
                    }
                    None => return false, // still connecting
                }
            }
            None => {}
        }
        // Not seen before: kick off a background connect and come back later.
        let slot = Arc::new(Mutex::new(None));
        let slot2 = slot.clone();
        let coordinator = self.coordinator.clone();
        let account_id = self.account_id;
        let shard_owned = shard.to_string();
        let spawned = std::thread::Builder::new()
            .name(format!("view-merge-{shard_owned}"))
            .spawn(move || {
                let result = coordinator
                    .shard_handle(&shard_owned)
                    .ok_or_else(|| format!("shard {shard_owned} is not connected"))
                    .and_then(|away_coord| {
                        away_coord
                            .player_conn(account_id)
                            .map(|conn| (conn, away_coord))
                            .map_err(|e| e.to_string())
                    });
                *slot2.lock().unwrap() = Some(result);
            });
        match spawned {
            Ok(_) => {
                self.away.insert(shard.to_string(), Away::Connecting(slot));
            }
            Err(e) => {
                log::warn!("view-merge: failed to spawn the away-connect thread for {shard}: {e}");
            }
        }
        false
    }
}

impl Drop for AreaOfInterestTracker {
    fn drop(&mut self) {
        for h in self.home_rects.drain(..) {
            let _ = h.unsubscribe();
        }
        self.away.clear(); // each Away::Ready(AwayShard)'s own Drop unsubscribes + tears down relays
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_match_is_order_independent_and_sensitive_to_every_field() {
        let a = GridRect {
            map_id: 0,
            gx_min: 3,
            gx_max: 5,
            gy_min: 3,
            gy_max: 7,
        };
        let b = GridRect {
            map_id: 0,
            gx_min: 6,
            gx_max: 7,
            gy_min: 3,
            gy_max: 7,
        };
        assert!(rects_match(&[a, b], &[b, a]), "order must not matter");
        assert!(rects_match(&[], &[]));
        assert!(
            !rects_match(&[a], &[a, b]),
            "a length mismatch is never a match"
        );
        assert!(
            !rects_match(&[a], &[b]),
            "a different rect is never a match"
        );
        let a_shifted = GridRect {
            gx_min: a.gx_min + 1,
            ..a
        };
        assert!(
            !rects_match(&[a], &[a_shifted]),
            "a single differing bound must be caught"
        );
        let a_other_map = GridRect { map_id: 1, ..a };
        assert!(
            !rects_match(&[a], &[a_other_map]),
            "map_id is part of the identity too"
        );
    }

    #[test]
    fn aoi_query_is_an_indexed_grid_box_range() {
        let b = GridBox {
            map_id: 0,
            gx: 5,
            gy: 7,
        };
        let q = aoi_query(0, &b);
        assert!(q.starts_with("SELECT * FROM game_world_entity WHERE"));
        assert!(q.contains("map_id = 0"));
        assert!(q.contains("grid_x >= 3") && q.contains("grid_x <= 7")); // anchor gx 5 ± BOX_HALF_SPAN(2)
        assert!(q.contains("grid_y >= 5") && q.contains("grid_y <= 9")); // anchor gy 7 ± BOX_HALF_SPAN(2)
    }

    /// 109: the box scopes FOUR tables, and dropping any of them from a subscription is silent —
    /// the session keeps running, just blind to that table forever.
    #[test]
    fn the_box_query_set_covers_every_box_scoped_table() {
        let qs = box_queries(&GridBox {
            map_id: 0,
            gx: 5,
            gy: 7,
        });
        for table in [
            "game_world_entity",
            "game_gameobject",
            "game_entity_motion", // peer movement (2.1) — its ONLY relay when LYRACORE_AOI=1
            "game_creature_spline", // creature legs (2.3)
        ] {
            assert!(
                qs.iter().any(|q| q.contains(table)),
                "the box query set must scope {table}; it has {qs:#?}"
            );
        }
        assert_eq!(qs.len(), 4, "a table was added without extending this test");
    }

    /// #73's twin of the test above: `rect_queries` must scope the SAME four tables — a rect is the
    /// per-shard unit a straddling recenter actually subscribes, so a table missing here reopens
    /// 109's class on the view-merge path specifically.
    #[test]
    fn rect_queries_covers_every_box_scoped_table() {
        let rect = GridRect {
            map_id: 0,
            gx_min: 3,
            gx_max: 7,
            gy_min: 5,
            gy_max: 9,
        };
        let qs = rect_queries(&rect);
        for table in [
            "game_world_entity",
            "game_gameobject",
            "game_entity_motion",
            "game_creature_spline",
        ] {
            assert!(qs.iter().any(|q| q.contains(table)), "{qs:#?}");
        }
        assert_eq!(qs.len(), 4);
    }

    /// THE byte-identity property the module doc promises: a non-straddling recenter's home rect is
    /// the whole box, and its query text must be IDENTICAL (not merely equivalent) to what the
    /// pre-#73 single-box path sent — anything else is observable on the wire as a behavior change
    /// for the overwhelmingly common (no seam in view) case.
    #[test]
    fn rect_queries_of_a_whole_box_are_byte_identical_to_box_queries() {
        let b = GridBox {
            map_id: 3,
            gx: -12,
            gy: 40,
        };
        let rect = GridRect::from_box(&b);
        assert_eq!(rect_queries(&rect), box_queries(&b));
    }

    /// The WIRING, not the helper (playbook §8): 109 was a green suite with a correct
    /// `subscribe_box` and a recenter that quietly subscribed a SHORTER list. Every `.subscribe(` in
    /// this file must route through `box_queries` (the login path) or `rect_queries` (the one generic
    /// resubscribe both home and every away bucket share) — an inlined `vec![..]` at either call site
    /// reopens the class of bug.
    #[test]
    fn every_subscription_in_this_file_routes_through_box_queries_or_rect_queries() {
        // Scan the code ABOVE `mod tests` only: this test's own literals would self-match.
        let src = include_str!("aoi.rs");
        let code = src
            .split_once("mod tests")
            .expect("this test lives in `mod tests`")
            .0;
        let calls: Vec<&str> = code
            .match_indices(".subscribe(")
            .map(|(i, _)| &code[i..])
            .collect();
        assert_eq!(
            calls.len(),
            2,
            "expected exactly the login box-subscribe (`subscribe_box`) and the one generic \
             rect-resubscribe (`resubscribe_rects`) call site; got {calls:#?}"
        );
        for call in calls {
            assert!(
                call.starts_with(".subscribe(box_queries(") || call.starts_with(".subscribe(rect_queries("),
                "every AOI subscription must pass `box_queries(..)` or `rect_queries(..)` — this one \
                 starts `{}`",
                &call[..call.len().min(60)]
            );
        }
    }

    // ==========================================================================================
    //  #73 view-merge: `split_box_by_shard`, the pure boundary-line box splitter.
    // ==========================================================================================

    fn anchor_box() -> GridBox {
        GridBox {
            map_id: 0,
            gx: 5,
            gy: 5,
        } // bounds: gx 3..=7, gy 3..=7 (5×5)
    }

    fn total_rects(split: &[(Option<String>, Vec<GridRect>)]) -> usize {
        split.iter().map(|(_, r)| r.len()).sum()
    }

    fn cells_covered(split: &[(Option<String>, Vec<GridRect>)]) -> HashSet<(i32, i32)> {
        let mut out = HashSet::new();
        for (_, rects) in split {
            for r in rects {
                for gx in r.gx_min..=r.gx_max {
                    for gy in r.gy_min..=r.gy_max {
                        assert!(
                            out.insert((gx, gy)),
                            "cell ({gx},{gy}) covered twice — overlap"
                        );
                    }
                }
            }
        }
        out
    }

    /// Fixture 1 — **no split**: every cell resolves home. Must collapse to ONE entry whose ONE rect
    /// is byte-identical to the box's own bounds (the property that makes "no seam in view" free).
    #[test]
    fn split_no_seam_in_view_collapses_to_one_home_rect_matching_the_box() {
        let b = anchor_box();
        let split = split_box_by_shard(&b, |_gx, _gy| None);
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].0, None);
        assert_eq!(split[0].1, vec![GridRect::from_box(&b)]);
    }

    /// Fixture 2 — **region-0 / no menu**: same shape as fixture 1, but exercised through a resolver
    /// that mimics `resolve_region_shard_for_cell`'s own "DEFAULT_REGION never routes" collapse — a
    /// distinct scenario in intent (a menu IS imported, this box just isn't inside any drawn region)
    /// even though the observable result is identical.
    #[test]
    fn split_region_zero_is_always_home_even_with_a_menu_imported() {
        let b = anchor_box();
        // A resolver that would route gx>=100 (nowhere near this box) — this box sits entirely in
        // the undrawn "rest of the map".
        let split = split_box_by_shard(&b, |gx, _gy| (gx >= 100).then(|| "pool-a".to_string()));
        assert_eq!(split, vec![(None, vec![GridRect::from_box(&b)])]);
    }

    /// Fixture 3 — **edge split**: one straight vertical boundary crosses the box. Exactly one home
    /// rect + one away rect, EACH spanning the box's full height — the design doc's literal example.
    #[test]
    fn split_edge_yields_one_home_rect_and_one_away_rect_spanning_the_full_height() {
        let b = anchor_box(); // gx 3..=7, gy 3..=7
        let split = split_box_by_shard(&b, |gx, _gy| (gx >= 6).then(|| "world-2".to_string()));
        assert_eq!(split.len(), 2, "{split:#?}");
        let home = split
            .iter()
            .find(|(s, _)| s.is_none())
            .expect("a home bucket");
        let away = split
            .iter()
            .find(|(s, _)| s.as_deref() == Some("world-2"))
            .expect("an away bucket");
        assert_eq!(
            home.1,
            vec![GridRect {
                map_id: 0,
                gx_min: 3,
                gx_max: 5,
                gy_min: 3,
                gy_max: 7
            }]
        );
        assert_eq!(
            away.1,
            vec![GridRect {
                map_id: 0,
                gx_min: 6,
                gx_max: 7,
                gy_min: 3,
                gy_max: 7
            }]
        );
        assert_eq!(
            total_rects(&split),
            2,
            "one rect per shard — never per cell"
        );
        assert_eq!(
            cells_covered(&split).len(),
            25,
            "every cell of the 5×5 box is covered exactly once"
        );
    }

    /// Fixture 4 — **corner split**: a region corner sits inside the box, so ONE shard's share is an
    /// L (2 disjoint rects: a full-width top band + a partial-width bottom band), and the OTHER
    /// shard's share is a single rect (the bottom-right slice). Pins the ≤2-rects-per-shard bound the
    /// design doc promises for this box size.
    #[test]
    fn split_corner_yields_an_l_shaped_shard_and_a_single_rect_shard() {
        let b = anchor_box(); // gx 3..=7, gy 3..=7
                              // Top two rows (gy 3,4): entirely home. Bottom three rows (gy 5,6,7): gx 3..=5 home,
                              // gx 6..=7 away — i.e. an away region whose corner (gx=6, gy=5) sits inside the box.
        let split = split_box_by_shard(&b, |gx, gy| {
            (gy >= 5 && gx >= 6).then(|| "world-2".to_string())
        });
        let home = split
            .iter()
            .find(|(s, _)| s.is_none())
            .expect("a home bucket");
        let away = split
            .iter()
            .find(|(s, _)| s.as_deref() == Some("world-2"))
            .expect("an away bucket");
        assert_eq!(home.1.len(), 2, "home's share is an L: {:#?}", home.1);
        assert_eq!(
            away.1.len(),
            1,
            "away's share is a single rect: {:#?}",
            away.1
        );
        assert!(
            home.1.contains(&GridRect {
                map_id: 0,
                gx_min: 3,
                gx_max: 7,
                gy_min: 3,
                gy_max: 4
            }),
            "home's top band: {:#?}",
            home.1
        );
        assert!(
            home.1.contains(&GridRect {
                map_id: 0,
                gx_min: 3,
                gx_max: 5,
                gy_min: 5,
                gy_max: 7
            }),
            "home's bottom-left band: {:#?}",
            home.1
        );
        assert_eq!(
            away.1[0],
            GridRect {
                map_id: 0,
                gx_min: 6,
                gx_max: 7,
                gy_min: 5,
                gy_max: 7
            }
        );
        assert_eq!(cells_covered(&split).len(), 25, "no overlap, no gap");
        for (_, rects) in &split {
            assert!(
                rects.len() <= 2,
                "no shard needs more than 2 rects for a 5-wide box: {rects:#?}"
            );
        }
    }

    /// A DIAGONAL variant of the corner case: home occupies two quadrants that share no edge (NW +
    /// SE), so they can never merge into one rectangle — still exactly 2 rects, never per-cell.
    #[test]
    fn split_diagonal_corner_gives_the_diagonal_shard_two_disjoint_rects() {
        let b = anchor_box(); // gx 3..=7, gy 3..=7 → midlines at gx=6 (3-5 vs 6-7), gy=6 (3-5 vs 6-7)
        let split = split_box_by_shard(&b, |gx, gy| {
            let west = gx <= 5;
            let north = gy <= 5;
            match (west, north) {
                (true, true) | (false, false) => None, // NW + SE → home (diagonal)
                _ => Some("world-2".to_string()),      // NE + SW → away
            }
        });
        let home = split
            .iter()
            .find(|(s, _)| s.is_none())
            .expect("a home bucket");
        let away = split
            .iter()
            .find(|(s, _)| s.as_deref() == Some("world-2"))
            .expect("an away bucket");
        assert_eq!(home.1.len(), 2, "NW and SE never touch: {:#?}", home.1);
        assert_eq!(
            away.1.len(),
            2,
            "NE and SW never touch either: {:#?}",
            away.1
        );
        assert_eq!(cells_covered(&split).len(), 25);
    }

    /// Fixture 5 — **no-menu collapse via the production resolver shape**: a resolver built the same
    /// way `Coordinator::split_box_by_shard` builds one from `config::resolve_region_shard_for_cell`
    /// against an EMPTY `RegionMap` must answer every cell home — the integration point, pinned here
    /// without a live connection.
    #[test]
    fn split_via_the_production_resolver_shape_with_no_menu_is_all_home() {
        use lyracore_shared::region::RegionMap;
        let regions = RegionMap::default();
        let assignments: Vec<crate::config::RegionAssignment> = Vec::new();
        let b = anchor_box();
        let split = split_box_by_shard(&b, |gx, gy| {
            crate::config::resolve_region_shard_for_cell(
                &regions,
                &assignments,
                b.map_id,
                0,
                gx,
                gy,
                |_| true,
            )
            .map(str::to_string)
        });
        assert_eq!(split, vec![(None, vec![GridRect::from_box(&b)])]);
    }

    /// The same production-resolver shape, now WITH a real menu + assignment, split across a real
    /// seam — proves `split_box_by_shard` composes correctly with `config::resolve_region_shard_for_cell`
    /// end to end (still no live connection: `Coordinator::split_box_by_shard` only adds the
    /// `map_regions()`/`region_assignments()` reads on top of this).
    #[test]
    fn split_via_the_production_resolver_shape_with_a_real_seam() {
        use lyracore_shared::region::RegionMap;
        let b = anchor_box(); // gx 3..=7, gy 3..=7
        let (regions, rejected) = RegionMap::parse("0:1 = -20..5, 0..20\n0:2 = 6..30, 0..20");
        assert!(rejected.is_empty(), "{rejected:?}");
        let assignments = vec![
            crate::config::RegionAssignment {
                map_id: 0,
                region_id: 1,
                shard: String::new(),
                epoch: 1,
            },
            crate::config::RegionAssignment {
                map_id: 0,
                region_id: 2,
                shard: "world-2".to_string(),
                epoch: 1,
            },
        ];
        let split = split_box_by_shard(&b, |gx, gy| {
            crate::config::resolve_region_shard_for_cell(
                &regions,
                &assignments,
                b.map_id,
                0,
                gx,
                gy,
                |_| true,
            )
            .map(str::to_string)
        });
        let home = split
            .iter()
            .find(|(s, _)| s.is_none())
            .expect("a home bucket");
        let away = split
            .iter()
            .find(|(s, _)| s.as_deref() == Some("world-2"))
            .expect("an away bucket");
        assert_eq!(
            home.1,
            vec![GridRect {
                map_id: 0,
                gx_min: 3,
                gx_max: 5,
                gy_min: 3,
                gy_max: 7
            }]
        );
        assert_eq!(
            away.1,
            vec![GridRect {
                map_id: 0,
                gx_min: 6,
                gx_max: 7,
                gy_min: 3,
                gy_max: 7
            }]
        );
    }

    /// An unconnected away shard degrades the SPLIT itself to home (the resolver's own contract,
    /// `config::resolve_region_shard_for_cell`'s `connected` argument) — the splitter never has to
    /// know why a cell came back `None`, which is what makes "shard unreachable" and "no seam here"
    /// indistinguishable to it, on purpose.
    #[test]
    // The two arms are deliberately identical: they encode two DIFFERENT reasons the resolver hands
    // back `None` (cell is away-but-unreachable vs. cell has no seam at all), and the point of the
    // test is that the splitter cannot tell them apart. Collapsing them would delete that statement.
    #[allow(clippy::if_same_then_else)]
    fn split_degrades_to_home_when_the_resolver_reports_the_away_shard_unreachable() {
        let b = anchor_box();
        let split = split_box_by_shard(&b, |gx, _gy| {
            if gx >= 6 {
                None // the resolver already folded "unreachable" into home before calling us
            } else {
                None
            }
        });
        assert_eq!(split, vec![(None, vec![GridRect::from_box(&b)])]);
    }

    /// The WIRING tripwire (playbook §8): none of `resolve_split`, `ensure_away`, `recenter_home`, or
    /// `recenter_away` can be driven by a test without a live SpacetimeDB connection (each ultimately
    /// calls `Coordinator::split_box_by_shard` / `shard_handle` / `player_conn`, all network-backed) —
    /// so the pure functions above (`split_box_by_shard`, `resubscribe_rects`'s countdown shape) are
    /// proven correct, but nothing here proves `update()` still CALLS them. This scan is the
    /// next-best thing.
    #[test]
    fn the_view_merge_wiring_between_update_and_its_shard_aware_helpers_is_intact() {
        let src = include_str!("aoi.rs");
        let code = src
            .split_once("mod tests")
            .expect("this test lives in `mod tests`")
            .0;

        let fn_body = |name: &str| -> &str {
            let start = code
                .find(&format!("fn {name}("))
                .unwrap_or_else(|| panic!("no `fn {name}(` found"));
            let after = &code[start..];
            let next = after[1..]
                .find("\n    fn ")
                .map(|i| i + 1)
                .unwrap_or(after.len());
            &after[..next]
        };

        assert!(
            fn_body("update").contains("self.resolve_split("),
            "update() must resolve the new box's shard split every recenter"
        );
        assert!(
            fn_body("update").contains("self.recenter_home("),
            "update() must resubscribe the home bucket — the home-only pre-#73 shape must not \
             silently come back"
        );
        assert!(
            fn_body("update").contains("self.recenter_away("),
            "update() must resubscribe every touched away bucket, or a straddling box's far side \
             would silently stop updating"
        );
        assert!(
            fn_body("update").contains("pending_connect")
                && fn_body("update").contains("Away::Connecting"),
            "update() must re-check a still-connecting away shard even WITHOUT a real crossing, or \
             a player who stops walking exactly on the touching cell would never get its away \
             rects — 'pops in late' would silently become 'never pops in'"
        );
        assert!(
            fn_body("recenter_away").contains("self.ensure_away("),
            "recenter_away() must open (or reuse) an away shard's connection before subscribing any \
             of its rects — a straddling box with no away registration is a silent hole"
        );
        assert!(
            fn_body("resolve_split").contains("self.view_merge"),
            "resolve_split() must consult the LYRACORE_VIEW_MERGE gate — losing this call would make the \
             escape hatch a no-op"
        );
        assert!(
            fn_body("ensure_away").contains("std::thread::Builder::new()")
                && fn_body("ensure_away").contains(".spawn("),
            "ensure_away() must open the away connection on a BACKGROUND thread — inlining \
             `player_conn` here would reintroduce the movement-thread stall #207 fast-follow 1 fixed"
        );
    }

    /// The WIRING tripwire (playbook §8) for #74's position mirror: `ViewerGates::set_position`
    /// itself is pure state (no test needed), but `new()`/`update()` actually CALLING it on login and
    /// every movement packet cannot be driven without a live SDK connection — same reason the view-
    /// merge wiring above gets a source scan instead of a behavioural test.
    #[test]
    fn seam_chat_position_mirror_is_seeded_at_login_and_refreshed_every_movement_packet() {
        let src = include_str!("aoi.rs");
        let code = src
            .split_once("mod tests")
            .expect("this test lives in `mod tests`")
            .0;

        let fn_body = |name: &str| -> &str {
            let start = code
                .find(&format!("fn {name}("))
                .unwrap_or_else(|| panic!("no `fn {name}(` found"));
            let after = &code[start..];
            let next = after[1..]
                .find("\n    fn ")
                .map(|i| i + 1)
                .unwrap_or(after.len());
            &after[..next]
        };

        assert!(
            fn_body("new").contains("viewer_gates.set_position("),
            "AreaOfInterestTracker::new() must seed the position mirror at login — otherwise a \
             player who straddles a seam on their very FIRST recenter would have the away leg's chat \
             range check read the all-zeros default instead of their real login position"
        );
        assert!(
            fn_body("update").contains("self.viewer_gates.set_position("),
            "update() must refresh the position mirror on EVERY movement packet, not only a \
             recenter — the away leg's chat range check needs a live position between recenters too, \
             and update()'s own early-return (no crossing, no pending connect) must not skip this call"
        );
    }
}
