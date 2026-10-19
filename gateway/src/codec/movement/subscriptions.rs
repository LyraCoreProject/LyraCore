use crate::codec::{self};
use crate::stdb::bindings::*;
use crate::stdb::subscriptions::{MOTION_CALLS, MOTION_SENT};
use crate::stdb::world_index::{CellKey, SessionId, ShardId};
use crate::stdb::world_view::*;
use crate::world::Outbound;
use anyhow::Result;
use lyracore_shared::spatial::BOX_HALF_SPAN;
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::Vector3d;

pub(crate) fn build_teleport_relay(
    still_here: bool,
    mover_guid: u64,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> Result<Outbound> {
    if still_here {
        let m = codec::build_teleport_ack(mover_guid, 0, x, y, z, o);
        Ok(Outbound::One(ServerOpcodeMessage::MSG_MOVE_TELEPORT_ACK(
            Box::new(m),
        )))
    } else {
        let [pending, new_world] = codec::build_cross_map_teleport(map_id, x, y, z, o)?;
        Ok(Outbound::Batch(vec![pending, new_world]))
    }
}

pub(crate) fn teleport_event_outbound(row: &TeleportEvent) -> Vec<Outbound> {
    match build_teleport_relay(
        !row.cross_map,
        row.mover_guid,
        row.map_id,
        row.x,
        row.y,
        row.z,
        row.orientation,
    ) {
        Ok(out) => vec![out],
        Err(error) => {
            log::warn!("teleport relay: {error} (guid {})", row.mover_guid);
            Vec::new()
        }
    }
}

#[cfg(test)]
pub(crate) fn motion_outbound(
    created: &Mutex<HashSet<u64>>,
    self_guid: u64,
    row: &EntityMotion,
) -> Vec<Outbound> {
    MOTION_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if row.guid == self_guid || !created.lock().unwrap().contains(&row.guid) {
        return Vec::new();
    }
    let out = movement_outbound(row);
    if !out.is_empty() {
        MOTION_SENT.fetch_add(1, Ordering::Relaxed);
    }
    out
}

fn movement_outbound(row: &EntityMotion) -> Vec<Outbound> {
    match codec::build_movement_relay_raw(row.opcode as u32, row.guid, &row.movement_info) {
        Some((opcode, body)) => {
            vec![Outbound::Raw { opcode, body }]
        }
        None if lyracore_shared::opcodes::movement::is_relayed_move(row.opcode as u32) => {
            log::warn!(
                "motion relay: malformed movement_info ({} bytes) for mover {}",
                row.movement_info.len(),
                row.guid
            );
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// The Module's movement gate uses the same rule.
fn holds_in_place(aura: &Aura) -> bool {
    lyracore_shared::aura::holds_in_place(aura.eff_kind, aura.eff_p_0)
}

fn moves_speed(aura: &Aura, speed: u8) -> bool {
    aura.eff_kind == lyracore_shared::spell::taxonomy::A_MOD_SPEED
        && aura.eff_p_0 == i32::from(speed)
}

/// cmangos: a slow lowers backward running too.
fn slows(aura: &Aura) -> bool {
    moves_speed(aura, lyracore_shared::spell::taxonomy::SPEED_MOVE)
        && lyracore_shared::aura::stacked_amount(aura.amount, aura.stacks) < 0
}

fn aura_amounts(auras: &[Aura]) -> impl Iterator<Item = lyracore_shared::aura::AuraAmount> + '_ {
    auras.iter().map(|aura| lyracore_shared::aura::AuraAmount {
        kind: aura.eff_kind,
        parameter: aura.eff_p_0,
        amount: aura.amount,
        stacks: aura.stacks,
    })
}

/// Only slows apply to backward running, with the run fold's floor.
pub(crate) fn run_back_speed(
    auras: impl IntoIterator<Item = lyracore_shared::aura::AuraAmount>,
) -> f32 {
    use lyracore_shared::spell::taxonomy::{A_MOD_SPEED, SPEED_MOVE};
    let slow = auras
        .into_iter()
        .filter(|aura| aura.kind == A_MOD_SPEED && aura.parameter == i32::from(SPEED_MOVE))
        .map(|aura| lyracore_shared::aura::stacked_amount(aura.amount, aura.stacks))
        .filter(|amount| *amount < 0)
        .sum();
    lyracore_shared::aura::move_speed_with_pct(lyracore_shared::constants::speeds::RUN_BACK, slow)
}

/// Controls the owner's client must hold; see docs/benilla-compatibility.md#movement-controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MoverControl {
    pub(crate) rooted: bool,
    pub(crate) water_walk: bool,
}

impl MoverControl {
    pub(crate) fn of(entity: Option<&WorldEntity>, auras: &[Aura]) -> Self {
        const GHOST: u32 = lyracore_shared::constants::player_flags::GHOST;
        let ghost = entity.is_some_and(|e| e.player_flags & GHOST != 0);
        let body = entity.is_some_and(|e| e.dead) && !ghost;
        Self {
            rooted: body || auras.iter().any(holds_in_place),
            water_walk: ghost,
        }
    }
}

fn root_outbound(guid: u64, rooted: bool, own_mover: bool) -> Outbound {
    let (opcode, body) = if own_mover {
        codec::build_force_move_root_raw(guid, rooted)
    } else {
        codec::build_spline_move_root_raw(guid, rooted)
    };
    Outbound::Raw { opcode, body }
}

/// `changed` holds the changed rows (all rows on reconnect); `current` is the post-change set.
pub(crate) fn aura_movement_outbound(
    entity: Option<&WorldEntity>,
    current: &[Aura],
    changed: &[&Aura],
    target_guid: u64,
    self_guid: u64,
) -> Vec<Outbound> {
    const RUN: f32 = 7.0; // the base `run_speed_packet` and the CREATE speed block use
    let touched = |rule: fn(&Aura) -> bool| changed.iter().any(|aura| rule(aura));
    let mut out = Vec::new();
    if target_guid == self_guid {
        if touched(holds_in_place) {
            let rooted = MoverControl::of(entity, current).rooted;
            out.push(root_outbound(self_guid, rooted, true));
        }
        if touched(slows) {
            out.push(Outbound::One(
                ServerOpcodeMessage::SMSG_FORCE_RUN_BACK_SPEED_CHANGE(Box::new(
                    codec::build_force_run_back_speed(
                        self_guid,
                        run_back_speed(aura_amounts(current)),
                    ),
                )),
            ));
        }
        return out;
    }
    if touched(holds_in_place) && matches!(target_guid >> 48, 0xF130 | 0xF140) {
        let rooted = current.iter().any(holds_in_place);
        out.push(root_outbound(target_guid, rooted, false));
    }
    let spline = |kind, speed| Outbound::One(codec::build_spline_speed(target_guid, kind, speed));
    if touched(|aura| lyracore_shared::aura::moves_run_speed(aura.eff_kind, aura.eff_p_0)) {
        let speed = lyracore_shared::aura::run_speed(RUN, aura_amounts(current));
        out.push(spline(codec::ObservedSpeed::Run, speed));
    }
    if touched(slows) {
        out.push(spline(
            codec::ObservedSpeed::RunBack,
            run_back_speed(aura_amounts(current)),
        ));
    }
    if touched(|aura| moves_speed(aura, lyracore_shared::spell::taxonomy::SPEED_SWIM)) {
        let speed = lyracore_shared::aura::swim_speed(
            lyracore_shared::constants::speeds::SWIM,
            aura_amounts(current),
        );
        out.push(spline(codec::ObservedSpeed::Swim, speed));
    }
    out
}

pub(crate) fn aura_movement_from_cache(
    coord: &crate::stdb::Coordinator,
    current: &[Aura],
    changed: &[&Aura],
    target_guid: u64,
    self_guid: u64,
) -> Vec<Outbound> {
    let entity = (target_guid == self_guid)
        .then(|| {
            coord
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&target_guid)
        })
        .flatten();
    aura_movement_outbound(entity.as_ref(), current, changed, target_guid, self_guid)
}

pub(crate) fn self_entity_movement_outbound(
    old: &WorldEntity,
    new: &WorldEntity,
    auras: &[Aura],
) -> Vec<Outbound> {
    let before = MoverControl::of(Some(old), auras);
    let after = MoverControl::of(Some(new), auras);
    let mut out = Vec::new();
    if old.dead != new.dead || before.rooted != after.rooted {
        out.push(root_outbound(new.guid, after.rooted, true));
    }
    if before.water_walk != after.water_walk {
        out.push(Outbound::One(codec::build_move_water_walk(
            new.guid,
            after.water_walk,
        )));
    }
    out
}

/// The self CREATE starts unrooted, on land and at base backward speed.
pub(crate) fn world_entry_movement_outbound(entity: &WorldEntity, auras: &[Aura]) -> Vec<Outbound> {
    let control = MoverControl::of(Some(entity), auras);
    let mut out = Vec::new();
    if control.rooted {
        out.push(root_outbound(entity.guid, true, true));
    }
    if control.water_walk {
        out.push(Outbound::One(codec::build_move_water_walk(
            entity.guid,
            true,
        )));
    }
    if auras.iter().any(slows) {
        out.push(Outbound::One(
            ServerOpcodeMessage::SMSG_FORCE_RUN_BACK_SPEED_CHANGE(Box::new(
                codec::build_force_run_back_speed(entity.guid, run_back_speed(aura_amounts(auras))),
            )),
        ));
    }
    out
}

pub(crate) fn creature_leg_outbound(
    created: &Mutex<HashSet<u64>>,
    row: &CreatureSpline,
) -> Vec<Outbound> {
    if !created.lock().unwrap().contains(&row.guid) {
        return Vec::new();
    }
    creature_spline_outbound(row)
}

fn creature_spline_outbound(row: &CreatureSpline) -> Vec<Outbound> {
    let start = Vector3d {
        x: row.sx,
        y: row.sy,
        z: row.sz,
    };
    if row.facing {
        return vec![Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(
            Box::new(codec::build_monster_move_facing(
                row.guid,
                start,
                row.facing_angle,
                row.spline_id,
            )),
        ))];
    }
    if let Some(path) = &row.path {
        let points = &path.points;
        let points: Vec<_> = points.iter().map(|p| (p.x, p.y, p.z)).collect();
        if let Some((opcode, body)) = codec::build_ground_path_raw(
            row.guid,
            start,
            &points,
            row.dur_ms,
            row.spline_id,
            row.run,
        ) {
            return vec![Outbound::Raw { opcode, body }];
        }
        log::warn!(
            "refused malformed creature path for guid {} spline {}",
            row.guid,
            row.spline_id
        );
        if [start.x, start.y, start.z].iter().all(|v| v.is_finite()) {
            return vec![Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(
                Box::new(codec::build_monster_move(
                    row.guid,
                    start,
                    start,
                    0,
                    row.spline_id,
                    false,
                )),
            ))];
        }
        return Vec::new();
    }
    let dest = Vector3d {
        x: row.dx,
        y: row.dy,
        z: row.dz,
    };
    vec![Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(
        Box::new(codec::build_monster_move(
            row.guid,
            start,
            dest,
            row.dur_ms,
            row.spline_id,
            row.run,
        )),
    ))]
}

pub(crate) fn append_resident_creature_after_create(
    created_outbound: &mut Vec<Outbound>,
    created: &Mutex<HashSet<u64>>,
    viewer_instance_id: u64,
    row: Option<&CreatureSpline>,
    now_micros: u64,
) {
    let Some(row) = row else {
        return;
    };
    if row.instance_id != viewer_instance_id {
        return;
    }
    if row.facing {
        created_outbound.extend(creature_leg_outbound(created, row));
        return;
    }

    let duration_micros = u64::from(row.dur_ms) * 1_000;
    let elapsed_micros = now_micros.saturating_sub(row.start_micros);
    if duration_micros == 0 || elapsed_micros >= duration_micros {
        return;
    }

    let elapsed_fraction = elapsed_micros as f64 / duration_micros as f64;
    let current = |start: f32, destination: f32| {
        (f64::from(start) + (f64::from(destination) - f64::from(start)) * elapsed_fraction) as f32
    };
    let mut remaining = row.clone();
    remaining.sx = current(row.sx, row.dx);
    remaining.sy = current(row.sy, row.dy);
    remaining.sz = current(row.sz, row.dz);
    if let Some(path) = &row.path {
        let points = &path.points;
        let points: Vec<_> = points.iter().map(|p| (p.x, p.y, p.z)).collect();
        let (at, next) = lyracore_shared::movement_path::sample(
            (row.sx, row.sy, row.sz),
            &points,
            elapsed_fraction as f32,
        );
        (remaining.sx, remaining.sy, remaining.sz) = at;
        if let Some(path) = &mut remaining.path {
            path.points = path.points[next..].to_vec();
        }
    }
    remaining.dur_ms = (duration_micros - elapsed_micros).div_ceil(1_000) as u32;
    created_outbound.extend(creature_leg_outbound(created, &remaining));
}

pub(crate) fn unix_now_micros() -> u64 {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    micros.min(u128::from(u64::MAX)) as u64
}

pub(crate) struct MotionRows<T> {
    rows: FxHashMap<u64, (Arc<SharedMotion<T>>, usize)>,
    bytes: usize,
}

impl<T> Default for MotionRows<T> {
    fn default() -> Self {
        Self {
            rows: Default::default(),
            bytes: 0,
        }
    }
}

impl<T> MotionRows<T> {
    fn insert(&mut self, guid: u64, row: Arc<SharedMotion<T>>, bytes: usize) -> bool {
        let previous = self.rows.get(&guid).map_or(0, |(_, bytes)| *bytes);
        let next = self.bytes.saturating_sub(previous).saturating_add(bytes);
        if next > 2 * 1024 * 1024 || (previous == 0 && self.rows.len() >= 4096) {
            return false;
        }
        self.rows.insert(guid, (row, bytes));
        self.bytes = next;
        true
    }

    fn drain(&mut self) -> Vec<Arc<SharedMotion<T>>> {
        self.bytes = 0;
        self.rows.drain().map(|(_, (row, _))| row).collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) struct SharedMotion<T> {
    pub(crate) row: T,
    packets: OnceLock<std::result::Result<Vec<crate::world::egress::Packet>, String>>,
}

impl<T> SharedMotion<T> {
    fn new(row: T) -> Self {
        Self {
            row,
            packets: OnceLock::new(),
        }
    }

    /// Appends this row's packets; returns whether it had any.
    fn relay(
        &self,
        tx: &crate::world::SessionTx,
        project: fn(&T) -> Vec<Outbound>,
        out: &mut Vec<Outbound>,
    ) -> bool {
        match self.packets.get_or_init(|| {
            crate::world::egress::encode(project(&self.row)).map_err(|error| error.to_string())
        }) {
            Ok(packets) => {
                out.extend(packets.iter().map(crate::world::egress::Packet::outbound));
                !packets.is_empty()
            }
            Err(error) => {
                log::warn!("movement Relay encoding failed: {error}");
                tx.close();
                false
            }
        }
    }
}

pub(crate) fn motion(
    view: &WorldView,
    shard: ShardId,
    row: &EntityMotion,
    delivery: &crate::stdb::movement_batch::MotionDelivery,
) {
    delivery.received();
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    let bytes = std::mem::size_of::<EntityMotion>() + row.movement_info.len() + 64;
    let row = Arc::new(SharedMotion::new(row.clone()));
    stage(view, shard, key, |batch, cell| {
        batch.entity.push(StagedRow {
            cell,
            guid: row.row.guid,
            bytes,
            row,
        })
    });
}

pub(crate) fn creature_leg(view: &WorldView, shard: ShardId, row: &CreatureSpline) {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    let bytes = std::mem::size_of::<CreatureSpline>()
        + 64
        + row
            .path
            .as_ref()
            .map_or(0, |path| std::mem::size_of_val(path.points.as_slice()));
    let row = Arc::new(SharedMotion::new(row.clone()));
    stage(view, shard, key, |batch, cell| {
        batch.creature.push(StagedRow {
            cell,
            guid: row.row.guid,
            bytes,
            row,
        })
    });
}

/// Movement and creature path rows from one SDK message, shared by every nearby viewer.
#[derive(Default)]
pub(crate) struct MotionBatch {
    cells: Vec<CellKey>,
    entity: Vec<StagedRow<EntityMotion>>,
    creature: Vec<StagedRow<CreatureSpline>>,
}

struct StagedRow<T> {
    cell: usize,
    guid: u64,
    bytes: usize,
    row: Arc<SharedMotion<T>>,
}

#[derive(Default)]
pub(crate) struct MotionStage {
    cells: FxHashMap<CellKey, usize>,
    batch: MotionBatch,
}

/// One staged batch and the indexes of its cells inside this viewer's audience box.
pub(crate) struct ViewerMotion {
    batch: Arc<MotionBatch>,
    cells: Vec<usize>,
}

type Staged<'a, T> = Vec<&'a StagedRow<T>>;

impl ViewerMotion {
    /// The rows this viewer is sent: inside its audience box, without its own Character.
    fn rows(&self, self_guid: u64) -> (Staged<'_, EntityMotion>, Staged<'_, CreatureSpline>) {
        let mut nearby = vec![false; self.batch.cells.len()];
        for &cell in &self.cells {
            nearby[cell] = true;
        }
        let entity = self.batch.entity.iter();
        let creature = self.batch.creature.iter();
        (
            // An echo of a Character's own movement fights its client's position authority.
            entity
                .filter(|staged| nearby[staged.cell] && staged.guid != self_guid)
                .collect(),
            creature.filter(|staged| nearby[staged.cell]).collect(),
        )
    }
}

enum PendingMotion {
    /// The usual case: one batch, read in place.
    Batch(ViewerMotion),
    Coalesced(
        Vec<Arc<SharedMotion<EntityMotion>>>,
        Vec<Arc<SharedMotion<CreatureSpline>>>,
    ),
}

// Fold pending batches into the coalesced rows when a writer falls this far behind; each
// pending batch keeps its whole SDK message alive.
const MAX_PENDING_BATCHES: usize = 16;

fn stage(
    view: &WorldView,
    shard: ShardId,
    key: CellKey,
    push: impl FnOnce(&mut MotionBatch, usize),
) {
    let mut staged = view.staged_motion.lock().unwrap();
    let stage = staged.entry(shard).or_default();
    let next = stage.cells.len();
    let cell = *stage.cells.entry(key).or_insert(next);
    if cell == next {
        stage.batch.cells.push(key);
    }
    push(&mut stage.batch, cell);
}

/// Hands the SDK message's staged rows to each nearby viewer and queues one flush per viewer.
pub(crate) fn flush_motion(view: &Arc<WorldView>, shard: ShardId) {
    let Some(stage) = view.staged_motion.lock().unwrap().remove(&shard) else {
        return;
    };
    let batch = Arc::new(stage.batch);
    let mut cells_of: FxHashMap<SessionId, Vec<usize>> = FxHashMap::default();
    for (index, cell) in batch.cells.iter().enumerate() {
        for session in view.spatial.viewers_within(*cell, BOX_HALF_SPAN) {
            cells_of.entry(session).or_default().push(index);
        }
    }
    for (viewer, cells) in view.viewers_on_shard_with(shard, cells_of) {
        let behind = {
            let mut batches = viewer.motion_pending.batches.lock().unwrap();
            batches.push(ViewerMotion {
                batch: batch.clone(),
                cells,
            });
            batches.len() > MAX_PENDING_BATCHES
        };
        if behind && !fold_batches(&viewer) {
            viewer.tx.close();
            continue;
        }
        let flush_queued = &viewer.motion_pending.flush_queued;
        // A load first keeps the shared cache line clean when a flush is already queued.
        if !flush_queued.load(Ordering::Acquire) && !flush_queued.swap(true, Ordering::AcqRel) {
            queue_motion_flush(view, shard, viewer);
        }
    }
}

/// Returns false when the coalesced rows would exceed their bounds.
fn fold_batches(viewer: &Viewer) -> bool {
    // Hold the rows while taking the batches so a concurrent fold cannot reorder them.
    let mut entity = viewer.motion_pending.entity.lock().unwrap();
    let mut creature = viewer.motion_pending.creature.lock().unwrap();
    let batches = std::mem::take(&mut *viewer.motion_pending.batches.lock().unwrap());
    fold_into(viewer.self_guid, &mut entity, &mut creature, batches)
}

/// Returns None when the coalesced rows would exceed their bounds.
fn take_motion(viewer: &Viewer) -> Option<PendingMotion> {
    let mut entity = viewer.motion_pending.entity.lock().unwrap();
    let mut creature = viewer.motion_pending.creature.lock().unwrap();
    let mut batches = std::mem::take(&mut *viewer.motion_pending.batches.lock().unwrap());
    // One batch is bounded by the encoded byte limit; the row bounds cap a backlog.
    if batches.len() == 1 && entity.is_empty() && creature.is_empty() {
        return batches.pop().map(PendingMotion::Batch);
    }
    fold_into(viewer.self_guid, &mut entity, &mut creature, batches)
        .then(|| PendingMotion::Coalesced(entity.drain(), creature.drain()))
}

fn fold_into(
    self_guid: u64,
    entity: &mut MotionRows<EntityMotion>,
    creature: &mut MotionRows<CreatureSpline>,
    batches: Vec<ViewerMotion>,
) -> bool {
    for motion in batches {
        let (moves, legs) = motion.rows(self_guid);
        for staged in moves {
            if !entity.insert(staged.guid, staged.row.clone(), staged.bytes) {
                return false;
            }
        }
        for staged in legs {
            if !creature.insert(staged.guid, staged.row.clone(), staged.bytes) {
                return false;
            }
        }
    }
    true
}

/// Appends shown rows' packets and unshown rows' guids; returns the count of rows relayed.
fn relay_rows<T>(
    viewer: &Viewer,
    mut rows: Vec<&SharedMotion<T>>,
    guid: fn(&T) -> u64,
    project: fn(&T) -> Vec<Outbound>,
    out: &mut Vec<Outbound>,
    missing: &mut Vec<u64>,
) -> u64 {
    // Release `created` before encoding; SDK pumps take it.
    {
        let created = viewer.created.lock().unwrap();
        rows.retain(|row| {
            let guid = guid(&row.row);
            created.contains(&guid) || {
                missing.push(guid);
                false
            }
        });
    }
    rows.into_iter()
        .map(|row| u64::from(row.relay(&viewer.tx, project, out)))
        .sum()
}

// ponytail: benchmark-only movement compression, never merged; LYRACORE_MOVE_COMPRESSION sets
// the zlib level (unset or 0 is off) and LYRACORE_MOVE_COMPRESSION_MIN the smallest packed run.
pub(crate) struct MoveCompression {
    level: u32,
    min_packets: usize,
}

pub(crate) static MOVE_COMPRESS_IN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static MOVE_COMPRESS_OUT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static MOVE_COMPRESS_RUNS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static MOVE_COMPRESS_NANOS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

const COMPRESSED_MOVES: u16 = 0x02fb;
// Keeps the compressed body well inside one frame.
const RUN_BYTES: usize = 32 * 1024;

fn move_compression() -> Option<&'static MoveCompression> {
    static CONFIG: OnceLock<Option<MoveCompression>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let level = std::env::var("LYRACORE_MOVE_COMPRESSION")
                .ok()?
                .parse::<u32>()
                .ok()
                .filter(|level| (1..=9).contains(level))?;
            let min_packets = std::env::var("LYRACORE_MOVE_COMPRESSION_MIN")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(8)
                .max(2);
            log::info!("movement compression: level {level}, runs of {min_packets}+ packets");
            Some(MoveCompression { level, min_packets })
        })
        .as_ref()
}

/// Packs runs of small movement packets into SMSG_COMPRESSED_MOVES, keeping packet order.
fn pack_moves(out: Vec<Outbound>, config: &MoveCompression) -> Vec<Outbound> {
    if out.len() < config.min_packets {
        return out;
    }
    let mut packed = Vec::new();
    let mut run = Vec::new();
    let mut originals = Vec::new();
    for outbound in out {
        let Outbound::Shared { opcode, body } = &outbound else {
            flush_run(&mut run, &mut originals, config, &mut packed);
            packed.push(outbound);
            continue;
        };
        // The client reads each packed size as one byte.
        if body.len() + 2 > 0xFF {
            flush_run(&mut run, &mut originals, config, &mut packed);
            packed.push(outbound);
            continue;
        }
        if run.len() + body.len() + 3 > RUN_BYTES {
            flush_run(&mut run, &mut originals, config, &mut packed);
        }
        run.push((body.len() + 2) as u8);
        run.extend_from_slice(&opcode.to_le_bytes());
        run.extend_from_slice(body);
        originals.push(outbound);
    }
    flush_run(&mut run, &mut originals, config, &mut packed);
    packed
}

fn flush_run(
    run: &mut Vec<u8>,
    originals: &mut Vec<Outbound>,
    config: &MoveCompression,
    packed: &mut Vec<Outbound>,
) {
    let compressed = (originals.len() >= config.min_packets)
        .then(|| deflate(run, config.level))
        .flatten();
    match compressed {
        Some(body) => {
            MOVE_COMPRESS_IN.fetch_add((run.len() + originals.len()) as u64, Ordering::Relaxed);
            MOVE_COMPRESS_OUT.fetch_add(4 + body.len() as u64, Ordering::Relaxed);
            MOVE_COMPRESS_RUNS.fetch_add(1, Ordering::Relaxed);
            packed.push(Outbound::Raw {
                opcode: COMPRESSED_MOVES,
                body,
            });
            originals.clear();
        }
        None => packed.append(originals),
    }
    run.clear();
}

fn deflate(run: &[u8], level: u32) -> Option<Vec<u8>> {
    use flate2::{Compress, Compression, FlushCompress, Status};
    thread_local! {
        static DEFLATE: std::cell::RefCell<Option<(u32, Compress)>> =
            const { std::cell::RefCell::new(None) };
    }
    let started = std::time::Instant::now();
    let mut body = Vec::with_capacity(4 + run.len() + run.len() / 1000 + 64);
    body.extend_from_slice(&(run.len() as u32).to_le_bytes());
    let status = DEFLATE.with_borrow_mut(|deflate| {
        let compress = match deflate {
            Some((cached, compress)) if *cached == level => {
                compress.reset();
                compress
            }
            _ => {
                &mut deflate
                    .insert((level, Compress::new(Compression::new(level), true)))
                    .1
            }
        };
        compress.compress_vec(run, &mut body, FlushCompress::Finish)
    });
    MOVE_COMPRESS_NANOS.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    matches!(status, Ok(Status::StreamEnd)).then_some(body)
}

#[cfg(test)]
mod compression_tests {
    use super::*;

    #[test]
    fn packed_runs_decompress_to_the_original_packets_in_order() {
        let config = MoveCompression {
            level: 1,
            min_packets: 2,
        };
        let shared = |opcode: u16, len: usize| Outbound::Shared {
            opcode,
            body: vec![opcode as u8; len].into(),
        };
        let out = vec![
            shared(0xb5, 30),
            shared(0xb6, 31),
            shared(0xdd, 300),
            shared(0xee, 32),
            shared(0xee, 33),
            shared(0xee, 34),
        ];
        let packed = pack_moves(out, &config);
        let [Outbound::Raw {
            opcode: 0x02fb,
            body: first,
        }, Outbound::Shared { opcode: 0xdd, .. }, Outbound::Raw {
            opcode: 0x02fb,
            body: second,
        }] = packed.as_slice()
        else {
            panic!("two packed runs around the oversized packet");
        };
        let inflate = |body: &[u8]| {
            let mut plain = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&body[4..]), &mut plain)
                .unwrap();
            assert_eq!(
                u32::from_le_bytes(body[..4].try_into().unwrap()) as usize,
                plain.len()
            );
            let mut packets = Vec::new();
            let mut rest = plain.as_slice();
            while let [size, lo, hi, tail @ ..] = rest {
                let len = usize::from(*size) - 2;
                packets.push((u16::from_le_bytes([*lo, *hi]), tail[..len].to_vec()));
                rest = &tail[len..];
            }
            packets
        };
        assert_eq!(
            inflate(first),
            vec![(0xb5, vec![0xb5; 30]), (0xb6, vec![0xb6; 31])]
        );
        assert_eq!(
            inflate(second),
            vec![
                (0xee, vec![0xee; 32]),
                (0xee, vec![0xee; 33]),
                (0xee, vec![0xee; 34])
            ]
        );
    }
}

// A nearby mover the viewer does not show gets at most one CREATE offer per interval.
const MISSING_CREATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

fn queue_motion_flush(view: &Arc<WorldView>, shard: ShardId, viewer: Arc<Viewer>) {
    use std::sync::atomic::Ordering as AtOrd;
    if crate::stdb::subscriptions::shed_motion_at_depth(viewer.tx.depth()) {
        crate::stdb::subscriptions::MOTION_DROPPED.fetch_add(1, Ordering::Relaxed);
        if !fold_batches(&viewer) {
            viewer.tx.close();
        }
        viewer
            .motion_pending
            .flush_queued
            .store(false, AtOrd::Release);
        return;
    }
    // The writer runs this flush inline; it must not wait on a lock held across an SDK message.
    let tx = viewer.tx.clone();
    let view = view.clone();
    let flush = crate::stdb::world_view::viewer_job(viewer, move |viewer| {
        // Clear before draining so a racing batch can queue another flush.
        viewer
            .motion_pending
            .flush_queued
            .store(false, AtOrd::Release);
        let Some(pending) = take_motion(&viewer) else {
            viewer.tx.close();
            return Vec::new();
        };
        type Rows<'a, T> = Vec<&'a SharedMotion<T>>;
        let (moves, legs): (Rows<EntityMotion>, Rows<CreatureSpline>) = match &pending {
            PendingMotion::Batch(motion) => {
                let (moves, legs) = motion.rows(viewer.self_guid);
                (
                    moves.into_iter().map(|staged| &*staged.row).collect(),
                    legs.into_iter().map(|staged| &*staged.row).collect(),
                )
            }
            PendingMotion::Coalesced(moves, legs) => (
                moves.iter().map(|row| &**row).collect(),
                legs.iter().map(|row| &**row).collect(),
            ),
        };
        let calls = moves.len() as u64;
        let mut out = Vec::new();
        let mut missing = Vec::new();
        let sent = relay_rows(
            &viewer,
            moves,
            |row| row.guid,
            movement_outbound,
            &mut out,
            &mut missing,
        );
        relay_rows(
            &viewer,
            legs,
            |row| row.guid,
            creature_spline_outbound,
            &mut out,
            &mut missing,
        );
        // One add per flush; a per-row add contends across every writer.
        MOTION_CALLS.fetch_add(calls, Ordering::Relaxed);
        MOTION_SENT.fetch_add(sent, Ordering::Relaxed);
        offer_missing_creates(&view, shard, &viewer, missing);
        match move_compression() {
            Some(config) => pack_moves(out, config),
            None => out,
        }
    });
    let _ = tx.send(Outbound::Motion(flush));
}

fn offer_missing_creates(
    view: &Arc<WorldView>,
    shard: ShardId,
    viewer: &Arc<Viewer>,
    guids: Vec<u64>,
) {
    if guids.is_empty() {
        return;
    }
    let now = std::time::Instant::now();
    let mut offered = viewer.motion_pending.create_offers.lock().unwrap();
    offered.retain(|_, at| now.duration_since(*at) < MISSING_CREATE_INTERVAL);
    for guid in guids {
        if offered.insert(guid, now).is_none() {
            crate::codec::entity::subscriptions::offer_missing_create(
                view,
                shard,
                viewer.clone(),
                guid,
            );
        }
    }
}

pub(crate) fn teleport_appeared(view: &WorldView, shard: ShardId, row: &TeleportEvent) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.mover_guid)) else {
        return;
    };
    let row = row.clone();
    enqueue(viewer.clone(), move |viewer| {
        if !row.cross_map {
            viewer.gates.teleport_pending.store(true, Ordering::Release);
        }
        crate::codec::movement::subscriptions::teleport_event_outbound(&row)
    });
}

crate::stdb::relays::declare_relays! {
    game_teleport_event [false] {
        insert(Live) => |context, _event, row| teleport_appeared(&context.view, context.shard, row);
    }
    game_entity_motion [false] {
        insert(Snapshot) => |context, _event, row| motion(&context.view, context.shard, row, &context.coord.0.motion_batch.delivery);
        update(Snapshot) => |context, _event, _old, row| motion(&context.view, context.shard, row, &context.coord.0.motion_batch.delivery);
    }
    game_creature_spline [false] {
        insert(Snapshot) => |context, _event, row| creature_leg(&context.view, context.shard, row);
        update(Snapshot) => |context, _event, _old, row| creature_leg(&context.view, context.shard, row);
    }
}

#[cfg(test)]
mod control_tests;

#[cfg(test)]
mod packet_tests;

#[cfg(test)]
mod audience_tests;
