//! Relay scaling harness: a synthetic realm in memory, no sockets, no database.
//!
//! Run with `cargo test -p lyracore-gateway --release relay_scaling -- --ignored --nocapture`.
//! It prints one row per population size: the pump cost of a burst of combat, cast and impact
//! events, the writer-side cost of draining every job those enqueued, the aura relay's audience
//! and array-sync cost, and the yell audience. Ignored by default because it is a measurement,
//! not a check.

use super::*;
use crate::codec::property_tests::Rng;
use crate::stdb::bindings::{Aura, ChatEvent, CombatEvent, SpellCastEvent, SpellImpactEvent};
use crate::world::{Outbound, SessionTx};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const PLAYERS_PER_CELL: usize = 8;
const CREATURES_PER_PLAYER: usize = 4;
const AURAS_PER_PLAYER: usize = 5;
const AURA_BURST: usize = 50;
const PLAYER_GUID_BASE: u64 = 0x0000_0000_0001_0000;
const CREATURE_GUID_BASE: u64 = 0xF130_0000_0001_0000;

struct SyntheticRealm {
    view: Arc<WorldView>,
    viewers: Vec<Arc<Viewer>>,
    queues: Vec<Receiver<Outbound>>,
    creatures: Vec<(u64, CellKey)>,
    auras: Vec<Aura>,
}

fn viewer(session: SessionId, self_guid: u64, tx: SessionTx) -> Arc<Viewer> {
    let mut identity = [0u8; 32];
    identity[..8].copy_from_slice(&self_guid.to_le_bytes());
    Arc::new(Viewer {
        session,
        self_guid,
        bound_identity: spacetimedb_sdk::Identity::from_byte_array(identity),
        map_id: 0,
        instance_id: 0,
        zone_id: 0.into(),
        tx,
        created: Arc::new(Mutex::new(HashSet::from([self_guid]))),
        gates: Arc::new(ViewerGates::default()),
        skill_slots: Arc::new(Mutex::new((HashMap::new(), 0))),
        explored: Mutex::new(ExplorationReplay::default()),
        motion_pending: Arc::new(MotionPending::default()),
    })
}

fn aura(id: u64, target_guid: u64, eff_kind: u8) -> Aura {
    Aura {
        id,
        target_guid,
        caster_guid: target_guid,
        spell_id: 1000 + (id % 40) as u32,
        slot: (id % 32) as u8,
        level: 60,
        flags: 0x1f,
        applied_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        expires_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        effect_id: id,
        eff_kind,
        amount: 10,
        eff_p0: 0,
        eff_p0_kind: 0,
        eff_p1: 0,
        period_ms: 0,
        amount_remaining: 0,
        stacks: 1,
        next_tick_micros: 0,
        channel_target: 0,
        enters_combat: false,
        proc_flags: 0,
        proc_chance: 0,
        proc_ppm: 0.0,
        proc_ex: 0,
        proc_school_mask: 0,
        proc_family_name: 0,
        proc_family_flags: 0,
        proc_charges: 0,
        proc_icd_ms: 0,
        proc_ready_micros: 0,
    }
}

/// `players` viewers and `4 x players` creatures spread over a square of cells at eight players
/// per cell, every viewer's `created` set seeded with what its box can see, and five auras per
/// player spread over every entity.
fn build(players: usize, rng: &mut Rng) -> SyntheticRealm {
    let side = ((players / PLAYERS_PER_CELL) as f64).sqrt().ceil() as i32;
    let cell = |rng: &mut Rng| {
        CellKey::at(
            0,
            0,
            rng.below(side as usize) as i32,
            rng.below(side as usize) as i32,
        )
    };
    let view = Arc::new(WorldView::new(true));
    let mut viewers = Vec::with_capacity(players);
    let mut queues = Vec::with_capacity(players);
    for i in 0..players {
        let guid = PLAYER_GUID_BASE + i as u64;
        let (tx, rx) = SessionTx::with_depth(0);
        let v = viewer(view.next_session_id(), guid, tx);
        let anchor = cell(rng);
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, guid, anchor, 0);
        view.add_viewer_on_shard(v.clone(), anchor, 0);
        viewers.push(v);
        queues.push(rx);
    }
    let creatures: Vec<(u64, CellKey)> = (0..players * CREATURES_PER_PLAYER)
        .map(|i| (CREATURE_GUID_BASE + i as u64, cell(rng)))
        .collect();
    for (guid, key) in &creatures {
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, *guid, *key, 0);
    }
    for v in &viewers {
        let visible = view
            .spatial
            .visible_entities(EntityLayer::WorldEntity, v.session);
        v.created
            .lock()
            .unwrap()
            .extend(visible.into_iter().map(|(guid, _)| guid));
    }
    let entities = players + creatures.len();
    let auras: Vec<Aura> = (0..players * AURAS_PER_PLAYER)
        .map(|i| {
            let pick = rng.below(entities);
            let target = if pick < players {
                PLAYER_GUID_BASE + pick as u64
            } else {
                creatures[pick - players].0
            };
            let kind = if i % 20 == 0 {
                super::super::subscriptions::A_STEALTH
            } else {
                0xA0
            };
            aura(1 + i as u64, target, kind)
        })
        .collect();
    view.auras
        .replace_all(auras.iter().map(|row| (0, row.clone())));
    SyntheticRealm {
        view,
        viewers,
        queues,
        creatures,
        auras,
    }
}

impl SyntheticRealm {
    fn player_cell(&self, i: usize) -> CellKey {
        self.view
            .spatial
            .entity_cell(EntityLayer::WorldEntity, self.viewers[i].self_guid)
            .expect("every synthetic player is indexed")
    }

    fn combat_row(&self, attacker: usize, rng: &mut Rng) -> CombatEvent {
        let key = self.player_cell(attacker);
        let (gx, gy) = lyracore_shared::spatial::grid_cell_of_id(key.cell);
        CombatEvent {
            id: attacker as u64,
            attacker_guid: self.viewers[attacker].self_guid,
            target_guid: self.creatures[rng.below(self.creatures.len())].0,
            damage: 17,
            hit_info: 0,
            killing_blow: false,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            blocked_amount: 0,
            ranged_spell_id: 0,
            ammo_display_id: 0,
            spell_swing: false,
            impact_delay_ms: 0,
            map_id: 0,
            instance_id: 0,
            grid_x: gx,
            grid_y: gy,
        }
    }

    fn cast_row(&self, caster: usize, rng: &mut Rng) -> SpellCastEvent {
        let key = self.player_cell(caster);
        let (gx, gy) = lyracore_shared::spatial::grid_cell_of_id(key.cell);
        SpellCastEvent {
            id: caster as u64,
            caster_guid: self.viewers[caster].self_guid,
            spell_id: 133,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            target_guid: self.creatures[rng.below(self.creatures.len())].0,
            cast_time_ms: 0,
            is_completion: true,
            damage: 40,
            school: 2,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            is_interrupted: false,
            cooldown_ms: 0,
            delay_ms: 0,
            healed: 0,
            is_proc_log: false,
            swing_hit_info: 0,
            client_initiated: false,
            map_id: 0,
            instance_id: 0,
            grid_x: gx,
            grid_y: gy,
            failure_reason: 0,
        }
    }

    fn impact_row(&self, caster: usize, rng: &mut Rng) -> SpellImpactEvent {
        let key = self.player_cell(caster);
        let (gx, gy) = lyracore_shared::spatial::grid_cell_of_id(key.cell);
        SpellImpactEvent {
            id: caster as u64,
            caster_guid: self.viewers[caster].self_guid,
            target_guid: self.creatures[rng.below(self.creatures.len())].0,
            spell_id: 116,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            damage: 33,
            school: 4,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            map_id: 0,
            instance_id: 0,
            grid_x: gx,
            grid_y: gy,
        }
    }

    fn yell_row(&self, speaker: usize) -> ChatEvent {
        ChatEvent {
            id: speaker as u64,
            sender_guid: self.viewers[speaker].self_guid,
            chat_type: 1,
            language: 7,
            message: "LFG".to_string(),
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            target_guid: 0,
        }
    }

    /// Run every queued job on every writer queue: `(jobs run, packets produced)`.
    fn drain(&self) -> (usize, usize) {
        let (mut jobs, mut packets) = (0, 0);
        for rx in &self.queues {
            while let Ok(out) = rx.try_recv() {
                match out {
                    Outbound::Job(job) => {
                        jobs += 1;
                        packets += job().len();
                    }
                    _ => packets += 1,
                }
            }
        }
        (jobs, packets)
    }
}

struct Burst {
    pump: Duration,
    drain: Duration,
    jobs: usize,
    packets: usize,
}

fn burst(realm: &SyntheticRealm, dispatch: impl Fn(usize)) -> Burst {
    let started = Instant::now();
    for i in 0..realm.viewers.len() {
        dispatch(i);
    }
    let pump = started.elapsed();
    let started = Instant::now();
    let (jobs, packets) = realm.drain();
    Burst {
        pump,
        drain: started.elapsed(),
        jobs,
        packets,
    }
}

fn micros_per(total: Duration, count: usize) -> f64 {
    total.as_secs_f64() * 1e6 / count as f64
}

/// The aura relay, event by event: audience selection on the pump, then the per-recipient job
/// gate and the aura array sync each surviving job builds.
fn aura_burst(realm: &SyntheticRealm, rng: &mut Rng) -> (Duration, usize, Duration, usize) {
    let mut audience = Duration::ZERO;
    let mut sync = Duration::ZERO;
    let (mut candidates, mut synced) = (0, 0);
    for _ in 0..AURA_BURST {
        let row = &realm.auras[rng.below(realm.auras.len())];
        let started = Instant::now();
        let recipients = aura_audience(&realm.view, row);
        audience += started.elapsed();
        candidates += recipients.len();
        for viewer in recipients {
            let visible = row.target_guid == viewer.self_guid
                || viewer.created.lock().unwrap().contains(&row.target_guid);
            if !visible {
                continue;
            }
            let started = Instant::now();
            let _ = super::super::subscriptions::aura_sync(
                realm.view.auras.on_target(row.target_guid).into_iter(),
                row.target_guid,
            );
            sync += started.elapsed();
            synced += 1;
        }
    }
    (audience, candidates, sync, synced)
}

#[test]
#[ignore]
fn relay_scaling() {
    println!();
    println!(
        "| players | creatures | auras | relay | pump us/event | jobs/event | drain us/event | packets/event |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for players in [500usize, 1000, 2000] {
        let mut rng = Rng::new(0x5CA1_AB1E_0000_0001 + players as u64);
        let realm = build(players, &mut rng);
        let n = players;
        let combat = {
            let rows: Vec<CombatEvent> = (0..n).map(|i| realm.combat_row(i, &mut rng)).collect();
            burst(&realm, |i| combat_event_appeared(&realm.view, &rows[i]))
        };
        let cast = {
            let rows: Vec<SpellCastEvent> = (0..n).map(|i| realm.cast_row(i, &mut rng)).collect();
            burst(&realm, |i| cast_event_appeared(&realm.view, &rows[i]))
        };
        let impact = {
            let rows: Vec<SpellImpactEvent> =
                (0..n).map(|i| realm.impact_row(i, &mut rng)).collect();
            burst(&realm, |i| impact_appeared(&realm.view, &rows[i]))
        };
        for (name, b) in [("combat", combat), ("cast", cast), ("impact", impact)] {
            println!(
                "| {players} | {} | {} | {name} | {:.1} | {:.0} | {:.1} | {:.1} |",
                realm.creatures.len(),
                realm.auras.len(),
                micros_per(b.pump, n),
                b.jobs as f64 / n as f64,
                micros_per(b.drain, n),
                b.packets as f64 / n as f64,
            );
        }
        let (audience, candidates, sync, synced) = aura_burst(&realm, &mut rng);
        println!(
            "| {players} | {} | {} | aura | {:.1} | {:.0} | {:.1} | {:.1} |",
            realm.creatures.len(),
            realm.auras.len(),
            micros_per(audience, AURA_BURST),
            candidates as f64 / AURA_BURST as f64,
            micros_per(sync, AURA_BURST),
            synced as f64 / AURA_BURST as f64,
        );
        let yells: Vec<ChatEvent> = (0..n).map(|i| realm.yell_row(i)).collect();
        let started = Instant::now();
        let candidates: usize = yells
            .iter()
            .map(|row| chat_audience(&realm.view, row).len())
            .sum();
        let audience = started.elapsed();
        println!(
            "| {players} | {} | {} | yell | {:.1} | {:.0} | n/a | n/a |",
            realm.creatures.len(),
            realm.auras.len(),
            micros_per(audience, n),
            candidates as f64 / n as f64,
        );
    }
    println!();
    println!("aura columns: pump = audience selection, jobs = candidate viewers, drain = array syncs built, packets = viewers past the visibility gate");
    println!(
        "yell columns: pump = audience selection over the widened span, jobs = candidate viewers"
    );
}
