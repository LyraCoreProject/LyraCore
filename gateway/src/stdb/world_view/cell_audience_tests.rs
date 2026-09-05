//! The cell-anchored relays against the whole-shard-then-gate path they replaced.
//!
//! Every relay here used to enqueue one job per viewer on the shard and let the job's gate reject
//! the row. Now the actor's cell picks the candidates first. The property under test, over
//! randomised realms: filtering the candidates through the same gate yields exactly the set the
//! whole-shard path yielded, and the candidates are fewer than the shard.

use super::*;
use crate::codec::property_tests::Rng;
use crate::stdb::bindings::{
    Aura, ChatEvent, CombatEvent, EmoteEvent, MeleeAttack, SpellCastEvent, SpellImpactEvent,
};
use crate::stdb::subscriptions::{chat_in_range, chat_range_yd, A_STEALTH};
use crate::world::{Outbound, SessionTx};
use std::sync::mpsc::Receiver;

const PLAYER_BASE: u64 = 0x1000;
const CREATURE_BASE: u64 = 0xF130_0000_0000_2000;
const YELL: u8 = 1;

/// A randomised realm: players and creatures at random positions inside a `span_yd` square, one
/// partition, every viewer's `created` set holding exactly what its box shows it plus itself.
struct Realm {
    view: Arc<WorldView>,
    viewers: Vec<Arc<Viewer>>,
    queues: Vec<Receiver<Outbound>>,
    positions: HashMap<u64, (f32, f32)>,
    creatures: Vec<u64>,
}

fn identity(guid: u64) -> spacetimedb_sdk::Identity {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&guid.to_le_bytes());
    spacetimedb_sdk::Identity::from_byte_array(bytes)
}

fn viewer(session: SessionId, self_guid: u64, tx: SessionTx) -> Arc<Viewer> {
    Arc::new(Viewer {
        active: std::sync::atomic::AtomicBool::new(true),
        session,
        self_guid,
        bound_identity: identity(self_guid),
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

fn realm(rng: &mut Rng, players: usize, creatures: usize, span_yd: f32) -> Realm {
    let view = Arc::new(WorldView::new(true));
    let position = |rng: &mut Rng| {
        let x = rng.below(span_yd as usize) as f32;
        let y = rng.below(span_yd as usize) as f32;
        (x, y)
    };
    let mut positions = HashMap::new();
    let mut viewers = Vec::new();
    let mut queues = Vec::new();
    for i in 0..players {
        let guid = PLAYER_BASE + i as u64;
        let (x, y) = position(rng);
        let key = CellKey::of_position(0, 0, x, y);
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, guid, key, 0, 0);
        let (tx, rx) = SessionTx::with_depth(0);
        let v = viewer(view.next_session_id(), guid, tx);
        view.add_viewer_on_shard(v.clone(), key, 0);
        positions.insert(guid, (x, y));
        viewers.push(v);
        queues.push(rx);
    }
    let creatures: Vec<u64> = (0..creatures)
        .map(|i| {
            let guid = CREATURE_BASE + i as u64;
            let (x, y) = position(rng);
            view.spatial.upsert_entity(
                EntityLayer::WorldEntity,
                guid,
                CellKey::of_position(0, 0, x, y),
                0,
                0,
            );
            positions.insert(guid, (x, y));
            guid
        })
        .collect();
    for v in &viewers {
        let visible = view
            .spatial
            .visible_entities(EntityLayer::WorldEntity, v.session);
        v.created
            .lock()
            .unwrap()
            .extend(visible.into_iter().map(|(guid, _)| guid));
    }
    Realm {
        view,
        viewers,
        queues,
        positions,
        creatures,
    }
}

impl Realm {
    /// A random unit: a player two times in three, else a creature.
    fn actor(&self, rng: &mut Rng) -> u64 {
        if rng.below(3) < 2 {
            self.viewers[rng.below(self.viewers.len())].self_guid
        } else {
            self.creatures[rng.below(self.creatures.len())]
        }
    }

    fn grid(&self, guid: u64) -> (i32, i32) {
        let (x, y) = self.positions[&guid];
        lyracore_shared::spatial::grid_cell(x, y)
    }

    fn all(&self) -> HashSet<SessionId> {
        self.viewers.iter().map(|v| v.session).collect()
    }

    /// The whole-shard path: every viewer, filtered by `gate`.
    fn shard_then_gate(&self, gate: impl Fn(&Viewer) -> bool) -> HashSet<SessionId> {
        self.viewers
            .iter()
            .filter(|v| gate(v))
            .map(|v| v.session)
            .collect()
    }

    /// The sessions whose writer queue holds at least one job, drained.
    fn jobs_landed(&self) -> HashSet<SessionId> {
        let mut landed = HashSet::new();
        for (v, rx) in self.viewers.iter().zip(&self.queues) {
            while let Ok(out) = rx.try_recv() {
                assert!(
                    matches!(out, Outbound::Job(_)),
                    "the pump enqueues jobs only"
                );
                landed.insert(v.session);
            }
        }
        landed
    }

    fn owner(&self, guid: u64) -> Option<SessionId> {
        self.view.session_of_owner(guid)
    }
}

fn sessions(audience: Vec<Arc<Viewer>>) -> HashSet<SessionId> {
    audience.into_iter().map(|v| v.session).collect()
}

fn created_contains(v: &Viewer, guid: u64) -> bool {
    v.created.lock().unwrap().contains(&guid)
}

fn combat(realm: &Realm, attacker: u64, target: u64) -> CombatEvent {
    let (grid_x, grid_y) = realm.grid(attacker);
    CombatEvent {
        id: 1,
        attacker_guid: attacker,
        target_guid: target,
        damage: 12,
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
        grid_x,
        grid_y,
    }
}

fn melee(attacker: u64, target: u64) -> MeleeAttack {
    MeleeAttack {
        attacker_guid: attacker,
        target_guid: target,
        last_swing_ms: 0,
        ranged_spell_id: 0,
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
    }
}

fn cast(realm: &Realm, caster: u64, target: u64) -> SpellCastEvent {
    let (grid_x, grid_y) = realm.grid(caster);
    SpellCastEvent {
        id: 1,
        caster_guid: caster,
        spell_id: 133,
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        target_guid: target,
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
        grid_x,
        grid_y,
        failure_reason: 0,
    }
}

fn impact(realm: &Realm, caster: u64, target: u64) -> SpellImpactEvent {
    let (grid_x, grid_y) = realm.grid(caster);
    SpellImpactEvent {
        id: 1,
        caster_guid: caster,
        target_guid: target,
        spell_id: 116,
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        damage: 33,
        school: 4,
        is_crit: false,
        resisted: 0,
        absorbed: 0,
        map_id: 0,
        instance_id: 0,
        grid_x,
        grid_y,
    }
}

fn emote(realm: &Realm, sender: u64, target: u64) -> EmoteEvent {
    let (grid_x, grid_y) = realm.grid(sender);
    EmoteEvent {
        id: 1,
        sender_guid: sender,
        text_emote: 101,
        emote_anim: 3,
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        target_guid: target,
        map_id: 0,
        instance_id: 0,
        grid_x,
        grid_y,
    }
}

fn chat(sender: u64, chat_type: u8) -> ChatEvent {
    ChatEvent {
        id: 1,
        sender_guid: sender,
        chat_type,
        language: 7,
        message: "hello".to_string(),
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        target_guid: 0,
    }
}

fn aura(id: u64, target: u64, eff_kind: u8) -> Aura {
    Aura {
        id,
        target_guid: target,
        caster_guid: target,
        spell_id: 1784,
        slot: 0,
        level: 60,
        flags: 0x1f,
        applied_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        expires_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        effect_id: id,
        eff_kind,
        amount: 0,
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

/// 24 players and 40 creatures over a 600 yd square: twelve cells a side, so a box covers well
/// under half of it and the whole-shard path and the cell path differ on most events.
fn spread_realm(rng: &mut Rng) -> Realm {
    realm(rng, 24, 40, 600.0)
}

// ===============================================================================================
//  Gated relays: the candidates filtered by the job's gate equal the shard filtered by the gate.
// ===============================================================================================

/// A swing reaches exactly the viewers whose `created` set holds the attacker, and the pump
/// enqueues for the attacker's box and the two owners only.
#[test]
fn combat_candidates_gate_to_exactly_the_whole_shard_recipients() {
    let mut rng = Rng::new(0xC0A7_0001);
    let mut skipped_someone = false;
    for _ in 0..200 {
        let realm = spread_realm(&mut rng);
        let attacker = realm.actor(&mut rng);
        let target = realm.actor(&mut rng);
        let row = combat(&realm, attacker, target);
        let gate = |v: &Viewer| created_contains(v, attacker);

        let candidates = sessions(combat_audience(&realm.view, 0, &row));
        combat_event_appeared(&realm.view, 0, &row);
        assert_eq!(
            realm.jobs_landed(),
            candidates,
            "the dispatch enqueues for the audience"
        );

        let gated: HashSet<SessionId> = realm
            .viewers
            .iter()
            .filter(|v| candidates.contains(&v.session) && gate(v))
            .map(|v| v.session)
            .collect();
        assert_eq!(gated, realm.shard_then_gate(gate), "attacker {attacker:#x}");
        skipped_someone |= candidates.len() < realm.viewers.len();
    }
    assert!(
        skipped_someone,
        "a spread realm must leave some viewers out of the box"
    );
}

/// An engagement row has no cell of its own; the attacker's indexed cell stands in, and the
/// `created` gate still admits exactly the whole-shard recipients.
#[test]
fn melee_candidates_gate_to_exactly_the_whole_shard_recipients() {
    let mut rng = Rng::new(0x3E1E_E002);
    let mut skipped_someone = false;
    for _ in 0..200 {
        let realm = spread_realm(&mut rng);
        let attacker = realm.actor(&mut rng);
        let row = melee(attacker, realm.actor(&mut rng));
        let gate = |v: &Viewer| created_contains(v, attacker);

        let candidates = sessions(melee_audience(&realm.view, 0, &row));
        melee_engaged(&realm.view, 0, &row);
        assert_eq!(realm.jobs_landed(), candidates);
        melee_disengaged(&realm.view, 0, &row);
        assert_eq!(realm.jobs_landed(), candidates);

        let gated: HashSet<SessionId> = realm
            .viewers
            .iter()
            .filter(|v| candidates.contains(&v.session) && gate(v))
            .map(|v| v.session)
            .collect();
        assert_eq!(gated, realm.shard_then_gate(gate));
        skipped_someone |= candidates.len() < realm.viewers.len();
    }
    assert!(
        skipped_someone,
        "a spread realm must leave some viewers out of the box"
    );
}

/// An aura job passes its gate for the target itself, for a viewer showing the target, or (the
/// stealth reveal) for a viewer whose box covers the target. The target's cell admits all three.
#[test]
fn aura_candidates_gate_to_exactly_the_whole_shard_recipients() {
    let mut rng = Rng::new(0xA07A_0003);
    let mut skipped_someone = false;
    for trial in 0..200 {
        let realm = spread_realm(&mut rng);
        let target = realm.actor(&mut rng);
        let kind = if trial % 2 == 0 { A_STEALTH } else { 0xA0 };
        let row = aura(1, target, kind);
        let gate = |v: &Viewer| {
            v.self_guid == target
                || created_contains(v, target)
                || realm
                    .view
                    .spatial
                    .can_see(EntityLayer::WorldEntity, v.session, target)
        };

        let candidates = sessions(aura_audience(&realm.view, 0, &row));
        let gated: HashSet<SessionId> = realm
            .viewers
            .iter()
            .filter(|v| candidates.contains(&v.session) && gate(v))
            .map(|v| v.session)
            .collect();
        assert_eq!(gated, realm.shard_then_gate(gate), "target {target:#x}");
        skipped_someone |= candidates.len() < realm.viewers.len();
    }
    assert!(skipped_someone);
}

/// A unit that is already gone from the index (its row deleted in the same transaction) can be
/// on nobody's screen, so only the owner leg remains.
#[test]
fn an_aura_on_an_unindexed_unit_reaches_its_owner_only() {
    let mut rng = Rng::new(0xA07A_0004);
    let realm = spread_realm(&mut rng);
    let player = realm.viewers[3].self_guid;
    realm
        .view
        .spatial
        .remove_entity(EntityLayer::WorldEntity, player, 0);
    assert_eq!(
        sessions(aura_audience(&realm.view, 0, &aura(1, player, 0xA0))),
        HashSet::from([realm.viewers[3].session])
    );
    let creature = realm.creatures[0];
    realm
        .view
        .spatial
        .remove_entity(EntityLayer::WorldEntity, creature, 0);
    assert!(aura_audience(&realm.view, 0, &aura(2, creature, 0xA0)).is_empty());
}

// ===============================================================================================
//  Ungated relays: the audience is the actor's box plus the two named owners.
// ===============================================================================================

/// The viewers showing the actor (their `created` sets were seeded from the index's forward
/// direction) plus the actor's and target's own sessions, whether or not they can see the cell.
fn box_plus_owners(realm: &Realm, actor: u64, target: u64) -> HashSet<SessionId> {
    let mut want: HashSet<SessionId> = realm
        .viewers
        .iter()
        .filter(|v| created_contains(v, actor))
        .map(|v| v.session)
        .collect();
    want.extend(realm.owner(actor));
    want.extend(realm.owner(target));
    want
}

#[test]
fn cast_impact_and_emote_reach_the_actors_box_and_both_owners() {
    let mut rng = Rng::new(0xCA57_0005);
    let mut skipped_someone = false;
    for _ in 0..200 {
        let realm = spread_realm(&mut rng);
        let actor = realm.actor(&mut rng);
        let target = realm.actor(&mut rng);
        let want = box_plus_owners(&realm, actor, target);

        let row = cast(&realm, actor, target);
        assert_eq!(
            sessions(cast_audience(&realm.view, 0, &row)),
            want,
            "cast by {actor:#x}"
        );
        cast_event_appeared(&realm.view, 0, &row);
        assert_eq!(realm.jobs_landed(), want);

        let row = impact(&realm, actor, target);
        assert_eq!(
            sessions(impact_audience(&realm.view, 0, &row)),
            want,
            "impact by {actor:#x}"
        );
        impact_appeared(&realm.view, 0, &row);
        assert_eq!(realm.jobs_landed(), want);

        let row = emote(&realm, actor, target);
        assert_eq!(
            sessions(emote_audience(&realm.view, 0, &row)),
            want,
            "emote by {actor:#x}"
        );

        skipped_someone |= want.len() < realm.viewers.len();
    }
    assert!(skipped_someone);
}

// ===============================================================================================
//  Chat: the widened span never loses a listener the yard-range gate would admit.
// ===============================================================================================

/// The job's gate, over the positions the fixture placed the units at.
fn hears(realm: &Realm, speaker: u64, listener: &Viewer, chat_type: u8) -> bool {
    if listener.self_guid == speaker {
        return true;
    }
    let (sx, sy) = realm.positions[&speaker];
    let (lx, ly) = realm.positions[&listener.self_guid];
    let range = chat_range_yd(chat_type);
    chat_in_range(0, 0, sx, sy, 0, 0, lx, ly, range * range)
}

#[test]
fn say_and_yell_candidates_gate_to_exactly_the_whole_shard_listeners() {
    let mut rng = Rng::new(0x5A11_0006);
    let mut yell_skipped_someone = false;
    for trial in 0..300 {
        // 1200 yd a side: a yell (300 yd) reaches a corner of it, never all of it.
        let realm = realm(&mut rng, 30, 20, 1200.0);
        let speaker = realm.actor(&mut rng);
        let chat_type = if trial % 2 == 0 { YELL } else { 0 };
        let row = chat(speaker, chat_type);
        let gate = |v: &Viewer| hears(&realm, speaker, v, chat_type);

        let candidates = sessions(chat_audience(&realm.view, 0, &row));
        let gated: HashSet<SessionId> = realm
            .viewers
            .iter()
            .filter(|v| candidates.contains(&v.session) && gate(v))
            .map(|v| v.session)
            .collect();
        assert_eq!(
            gated,
            realm.shard_then_gate(gate),
            "speaker {speaker:#x} kind {chat_type}"
        );
        if chat_type == YELL {
            yell_skipped_someone |= candidates.len() < realm.viewers.len();
        } else {
            assert!(
                candidates.len() < realm.all().len(),
                "a say over a 1200 yd square never reaches everyone"
            );
        }
    }
    assert!(yell_skipped_someone);
}

/// A speaker with no indexed row cannot be placed, so nobody but the speaker hears it, which is
/// what the job's gate decides for the others too (no speaker row, no packet).
#[test]
fn chat_from_an_unindexed_speaker_reaches_the_speaker_only() {
    let mut rng = Rng::new(0x5A11_0007);
    let realm = spread_realm(&mut rng);
    let speaker = realm.viewers[0].self_guid;
    realm
        .view
        .spatial
        .remove_entity(EntityLayer::WorldEntity, speaker, 0);
    assert_eq!(
        sessions(chat_audience(&realm.view, 0, &chat(speaker, YELL))),
        HashSet::from([realm.viewers[0].session])
    );
}

// ===============================================================================================
//  The aura index
// ===============================================================================================

#[test]
fn aura_index_insert_update_delete_by_shard_and_id() {
    let index = AuraIndex::default();
    index.upsert(0, &aura(1, 77, A_STEALTH));
    index.upsert(0, &aura(2, 77, 0xA0));
    index.upsert(1, &aura(1, 78, 0xA0)); // shard 1 reuses id 1 for another unit
    assert_eq!(index.stealth_count(0, 77), 1);
    assert!(index.is_stealthed(0, 77));
    assert!(!index.is_stealthed(1, 78));
    assert_eq!(index.on_target(0, 77).len(), 2);
    assert_eq!(index.stats(), (3, 2));

    // An update lands on the same (shard, id) and replaces the row.
    let mut refreshed = aura(2, 77, 0xA0);
    refreshed.amount = 25;
    index.upsert(0, &refreshed);
    assert_eq!(index.on_target(0, 77).len(), 2);
    assert_eq!(
        index
            .on_target(0, 77)
            .iter()
            .find(|a| a.id == 2)
            .map(|a| a.amount),
        Some(25)
    );

    // A delete on shard 1 of id 1 must not touch shard 0's id 1.
    index.remove(1, &aura(1, 78, 0xA0));
    assert!(index.on_target(1, 78).is_empty());
    assert_eq!(index.stealth_count(0, 77), 1);
    index.remove(0, &aura(1, 77, A_STEALTH));
    assert_eq!(index.stealth_count(0, 77), 0);
    index.remove(0, &aura(1, 77, A_STEALTH)); // idempotent
    assert_eq!(index.on_target(0, 77).len(), 1);
    index.remove(0, &aura(2, 77, 0xA0));
    assert_eq!(index.stats(), (0, 0));
}

#[test]
fn aura_index_reseed_drops_rows_the_caches_no_longer_hold() {
    let index = AuraIndex::default();
    index.upsert(0, &aura(1, 77, 0xA0));
    index.upsert(0, &aura(2, 77, 0xA0));
    index.replace_shard(0, [aura(2, 77, 0xA0)]);
    index.replace_shard(1, [aura(9, 80, A_STEALTH)]);
    assert_eq!(index.on_target(0, 77).len(), 1);
    assert!(index.is_stealthed(1, 80));
    assert_eq!(index.stats(), (2, 2));
}

#[test]
fn a_transfer_keeps_each_shards_auras_independent() {
    let index = AuraIndex::default();
    let mut source = aura(1, PLAYER_BASE, A_STEALTH);
    source.amount = 50;
    let mut destination = aura(1, PLAYER_BASE, 0xA4);
    destination.amount = 25;
    index.upsert(0, &source);
    index.upsert(1, &destination);

    assert_eq!(index.on_target(1, PLAYER_BASE), vec![destination.clone()]);
    assert!(!index.is_stealthed(1, PLAYER_BASE));
    assert!(index.is_stealthed(0, PLAYER_BASE));
    index.replace_shard(0, []);
    index.remove(0, &source);
    assert_eq!(index.on_target(1, PLAYER_BASE), vec![destination]);
}

#[test]
fn a_reconnect_groups_aura_refreshes_by_target() {
    let index = AuraIndex::default();
    for id in 0..32 {
        index.upsert(0, &aura(id, PLAYER_BASE, 0xA0));
    }
    index.upsert(0, &aura(32, PLAYER_BASE + 1, A_STEALTH));
    let mut current: Vec<_> = (0..32).map(|id| aura(id, PLAYER_BASE, 0xA0)).collect();
    current.push(aura(33, PLAYER_BASE + 2, 0xA0));

    let refreshes = index.replace_shard(0, current);
    assert_eq!(refreshes.len(), 3, "32 buffs need one target refresh");
    assert_eq!(refreshes[&PLAYER_BASE].len(), 32);
    assert_eq!(refreshes[&(PLAYER_BASE + 1)].len(), 1);
    assert!(refreshes[&(PLAYER_BASE + 2)].is_empty());
    assert!(index.on_target(0, PLAYER_BASE + 1).is_empty());
}

#[test]
fn an_old_shard_cannot_move_or_change_skills_on_a_destination_viewer() {
    let view = WorldView::new(true);
    let (tx, rx) = SessionTx::with_depth(0);
    let destination = viewer(1, PLAYER_BASE, tx);
    destination.created.lock().unwrap().insert(CREATURE_BASE);
    view.add_viewer_on_shard(destination.clone(), CellKey::at(0, 0, 0, 0), 1);
    motion(
        &view,
        0,
        &EntityMotion {
            guid: CREATURE_BASE,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            opcode: 0,
            movement_info: vec![],
            seq: 1,
            cell: 0,
        },
        &super::super::movement_batch::MotionDelivery::default(),
    );
    creature_leg(
        &view,
        0,
        &CreatureSpline {
            guid: CREATURE_BASE,
            start_micros: 0,
            dur_ms: 100,
            sx: 0.0,
            sy: 0.0,
            sz: 0.0,
            dx: 1.0,
            dy: 0.0,
            dz: 0.0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            spline_id: 1,
            run: true,
            cell: 0,
            facing: false,
            facing_angle: 0.0,
        },
    );
    skill_changed(
        &view,
        0,
        &PlayerSkill {
            id: 1,
            character_guid: PLAYER_BASE,
            owner_identity: destination.bound_identity,
            skill_line: 43,
            current: 10,
            max_rank: 50,
        },
    );
    let spline = TaxiPassengerSpline {
        character_guid: PLAYER_BASE,
        map_id: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        start_x: 0.0,
        start_y: 0.0,
        start_z: 0.0,
        points: vec![1.0, 0.0, 0.0],
        duration_ms: 100,
        spline_id: 1,
    };
    taxi_spline(&view, 0, &spline);
    assert!(rx.try_recv().is_err());
    assert!(destination.motion_pending.entity.lock().unwrap().is_empty());
    assert!(destination
        .motion_pending
        .creature
        .lock()
        .unwrap()
        .is_empty());
    taxi_spline(&view, 1, &spline);
    let Outbound::Job(job) = rx.try_recv().unwrap() else {
        panic!("expected the current Shard's taxi Relay");
    };
    assert_eq!(
        job().len(),
        1,
        "the passenger still receives its own current spline"
    );
}

#[test]
fn old_shard_combat_cannot_address_a_transferred_character() {
    let mut rng = Rng::new(74);
    let realm = spread_realm(&mut rng);
    let source = &realm.viewers[0];
    let transferred = viewer(1000, source.self_guid, source.tx.clone());
    let cast = cast(&realm, transferred.self_guid, transferred.self_guid);
    let combat = combat(&realm, transferred.self_guid, transferred.self_guid);
    realm.view.remove_viewer(source.session);
    realm
        .view
        .add_viewer_on_shard(transferred.clone(), CellKey::at(1, 0, 4, 4), 1);
    realm.view.spatial.upsert_entity(
        EntityLayer::WorldEntity,
        transferred.self_guid,
        CellKey::at(1, 0, 4, 4),
        1,
        0,
    );

    cast_event_appeared(&realm.view, 0, &cast);
    combat_event_appeared(&realm.view, 0, &combat);
    let mut ranged = melee(transferred.self_guid, 0);
    ranged.ranged_spell_id = 75;
    melee_disengaged(&realm.view, 0, &ranged);
    assert!(realm.queues[0].try_recv().is_err());
    assert!(aura_audience(&realm.view, 0, &aura(1, transferred.self_guid, 0xA0)).is_empty());

    // A current-shard owner still receives its private stop with no indexed anchor.
    realm
        .view
        .spatial
        .remove_entity(EntityLayer::WorldEntity, transferred.self_guid, 1);
    melee_disengaged(&realm.view, 1, &ranged);
    let Outbound::Job(job) = realm.queues[0].try_recv().unwrap() else {
        panic!("expected a Relay job");
    };
    assert!(matches!(
        job().as_slice(),
        [Outbound::One(
            wow_world_messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_CANCEL_AUTO_REPEAT
        )]
    ));
}

#[test]
fn jobs_selected_before_worldport_cannot_reach_the_replacement_world_session() {
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

    let view = Arc::new(WorldView::new(true));
    let (tx, rx) = SessionTx::with_depth(0);
    let arrival = crate::codec::EntityView {
        guid: PLAYER_BASE,
        ..Default::default()
    };
    let old_registration = super::super::subscriptions::PlayerSubscriptions::registered_for_test(
        view.clone(),
        PLAYER_BASE,
        &arrival,
        tx.clone(),
    );
    let selected = view.viewer_of_owner(OwnerGuid(PLAYER_BASE)).unwrap();
    let row = melee(PLAYER_BASE, CREATURE_BASE);
    melee_engaged(&view, 0, &row);
    drop(old_registration);

    // WORLDPORT writes destination entry on the same socket before registering its fresh Viewer.
    tx.send(Outbound::Raw {
        opcode: 0xCAFE,
        body: vec![],
    })
    .unwrap();
    let _destination = super::super::subscriptions::PlayerSubscriptions::registered_for_test(
        view.clone(),
        PLAYER_BASE,
        &arrival,
        tx,
    );
    let delayed_row = row.clone();
    enqueue(selected, move |viewer| {
        super::super::subscriptions::melee_engage_outbound(&viewer.created, &delayed_row)
    });
    melee_engaged(&view, 0, &row);

    let Outbound::Job(before_entry) = rx.try_recv().unwrap() else {
        panic!("old Relay job");
    };
    assert!(before_entry().is_empty());
    assert!(matches!(
        rx.try_recv().unwrap(),
        Outbound::Raw { opcode: 0xCAFE, .. }
    ));
    let Outbound::Job(after_entry) = rx.try_recv().unwrap() else {
        panic!("late old Relay job");
    };
    assert!(after_entry().is_empty());
    let Outbound::Job(current) = rx.try_recv().unwrap() else {
        panic!("current Relay job");
    };
    assert!(matches!(
        current().as_slice(),
        [Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(_))]
    ));
}

#[test]
fn local_owner_events_cannot_reach_a_destination_world_session() {
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    let view = WorldView::new(true);
    let cell = CellKey::at(0, 0, 0, 0);
    let (old_tx, _old_rx) = SessionTx::with_depth(0);
    view.add_viewer_on_shard(viewer(1, PLAYER_BASE, old_tx), cell, 0);
    view.remove_viewer(1);
    let (tx, rx) = SessionTx::with_depth(0);
    view.add_viewer_on_shard(viewer(2, PLAYER_BASE, tx), cell, 1);
    let (other_tx, other_rx) = SessionTx::with_depth(0);
    view.add_viewer_on_shard(viewer(3, PLAYER_BASE + 1, other_tx), cell, 1);
    let rest = RestStateEvent {
        id: 1,
        character_guid: PLAYER_BASE,
        player_bytes_2: 0x0100_0000,
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
    };
    let breath = BreathRelayEvent {
        id: 2,
        character_guid: PLAYER_BASE,
        kind: 0,
        time_remaining_ms: 12000,
        duration_ms: 60000,
        damage: 0,
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
    };
    let resurrect = ResurrectRequest {
        target_guid: PLAYER_BASE,
        target_identity: identity(PLAYER_BASE),
        caster_guid: PLAYER_BASE + 1,
        caster_name: "Caster".into(),
        points: 50,
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
    };
    let trade = TradeEvent {
        id: 3,
        recipient_identity: identity(PLAYER_BASE),
        kind: lyracore_shared::trade::event_kind::TRADE_CANCELED,
        other_guid: PLAYER_BASE + 1,
        recipient_guid: PLAYER_BASE,
        payload: String::new(),
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
    };
    for shard in [0, 1] {
        rest_state_appeared(&view, shard, &rest);
        breath_relay_appeared(&view, shard, &breath);
        resurrect_offered(&view, shard, &resurrect);
        trade_event_appeared(&view, shard, &trade);
        assert!(
            other_rx.try_recv().is_err(),
            "owner rows must not reach a bystander"
        );
        if shard == 0 {
            assert!(
                rx.try_recv().is_err(),
                "late source rows must not reach the destination"
            );
        }
    }
    let packets: Vec<_> = rx
        .try_iter()
        .flat_map(|outbound| {
            let Outbound::Job(job) = outbound else {
                panic!("expected a Relay job")
            };
            job()
        })
        .collect();
    assert_eq!(packets.len(), 4);
    assert!(matches!(&packets[0], Outbound::Raw { opcode: 0xA9, body }
        if body.ends_with(&0x0100_0000_u32.to_le_bytes())));
    assert!(matches!(
        &packets[1],
        Outbound::One(ServerOpcodeMessage::SMSG_START_MIRROR_TIMER(_))
    ));
    assert!(matches!(
        &packets[2],
        Outbound::One(ServerOpcodeMessage::SMSG_RESURRECT_REQUEST(_))
    ));
    assert!(
        matches!(&packets[3], Outbound::One(ServerOpcodeMessage::SMSG_TRADE_STATUS(status))
        if matches!(status.as_ref(), wow_world_messages::vanilla::SMSG_TRADE_STATUS::TradeCanceled))
    );
}

#[test]
fn spatial_candidates_also_stay_on_the_event_shard() {
    let view = WorldView::new(true);
    let (tx, rx) = SessionTx::with_depth(0);
    let observer = viewer(1, PLAYER_BASE, tx);
    let cell = CellKey::at(0, 0, 0, 0);
    view.add_viewer_on_shard(observer, cell, 1);
    assert!(view
        .cell_audience(0, Some(cell), BOX_HALF_SPAN, &[])
        .is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn late_source_deletes_preserve_the_destination_entity_and_its_client_object() {
    let view = WorldView::new(true);
    let (tx, rx) = SessionTx::with_depth(0);
    let observer = viewer(1, PLAYER_BASE, tx);
    observer.created.lock().unwrap().insert(CREATURE_BASE);
    let source = CellKey::at(0, 0, 0, 0);
    let destination = CellKey::at(1, 0, 0, 0);
    view.add_viewer_on_shard(observer.clone(), destination, 1);
    for layer in [EntityLayer::WorldEntity, EntityLayer::GameObject] {
        view.spatial
            .upsert_entity(layer, CREATURE_BASE, source, 0, 0);
        view.spatial
            .upsert_entity(layer, CREATURE_BASE, destination, 1, 0);
    }

    entity_vanished(&view, 0, CREATURE_BASE, 0);
    gameobject_vanished(&view, 0, CREATURE_BASE);
    assert!(rx.try_recv().is_err());
    assert!(observer.created.lock().unwrap().contains(&CREATURE_BASE));
    assert!(view
        .spatial
        .can_see(EntityLayer::WorldEntity, observer.session, CREATURE_BASE));
    assert!(view
        .spatial
        .can_see(EntityLayer::GameObject, observer.session, CREATURE_BASE));
}

#[test]
fn reconnect_removes_missing_rows_through_the_registered_viewers_writer() {
    let view = Arc::new(WorldView::new(true));
    let (tx, rx) = SessionTx::with_depth(0);
    let arrival = crate::codec::EntityView {
        guid: PLAYER_BASE,
        ..Default::default()
    };
    let registration = super::super::subscriptions::PlayerSubscriptions::registered_for_test(
        view.clone(),
        PLAYER_BASE,
        &arrival,
        tx,
    );
    let observer = view.viewer_of_owner(OwnerGuid(PLAYER_BASE)).unwrap();
    let cell = CellKey::of_position(0, 0, 0.0, 0.0);
    for (layer, guid) in [
        (EntityLayer::WorldEntity, CREATURE_BASE),
        (EntityLayer::GameObject, CREATURE_BASE + 1),
    ] {
        view.spatial.upsert_entity(layer, guid, cell, 0, 0);
        if layer == EntityLayer::WorldEntity {
            observer.created.lock().unwrap().insert(guid);
        }
    }

    reconcile_shard(&view, 0, vec![], vec![], vec![]);
    let mut destroyed = HashSet::new();
    while let Ok(Outbound::Job(job)) = rx.try_recv() {
        for packet in job() {
            let Outbound::One(
                wow_world_messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                    message,
                ),
            ) = packet
            else {
                panic!("expected DESTROY for a missing snapshot row");
            };
            destroyed.insert(message.guid.guid());
        }
    }
    assert_eq!(destroyed, HashSet::from([CREATURE_BASE, CREATURE_BASE + 1]));
    assert_eq!(
        *observer.created.lock().unwrap(),
        HashSet::from([PLAYER_BASE])
    );
    assert_eq!(view.spatial.viewer_cell(observer.session), Some(cell));
    drop(registration);
    assert!(view.viewer_of_owner(OwnerGuid(PLAYER_BASE)).is_none());
    assert_eq!(view.spatial.viewer_cell(observer.session), None);
}

#[test]
fn a_gameobject_leaving_the_box_uses_its_own_destroy_path() {
    let view = WorldView::new(true);
    let (tx, rx) = SessionTx::with_depth(0);
    let observer = viewer(1, PLAYER_BASE, tx);
    view.add_viewer_on_shard(observer, CellKey::at(0, 0, 0, 0), 0);
    view.spatial.upsert_entity(
        EntityLayer::GameObject,
        CREATURE_BASE,
        CellKey::at(0, 0, 0, 0),
        0,
        0,
    );
    let row = GameObject {
        guid: CREATURE_BASE,
        template_entry: 1,
        map_id: 0,
        instance_id: 0,
        grid_x: 10,
        grid_y: 10,
        cell: 0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        orientation: 0.0,
        state: 0,
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        respawn_at_micros: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 1.0,
    };
    gameobject_appeared(&view, 0, &row);
    let Outbound::Job(job) = rx.try_recv().unwrap() else {
        panic!("expected the game object's Relay job");
    };
    assert!(matches!(job().as_slice(), [Outbound::One(
        wow_world_messages::vanilla::opcodes::ServerOpcodeMessage::SMSG_DESTROY_OBJECT(message)
    )] if message.guid.guid() == CREATURE_BASE));
}

#[test]
fn reconnect_clears_the_action_bar_of_a_pet_missing_from_the_snapshot() {
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

    let view = Arc::new(WorldView::new(true));
    let (tx, rx) = SessionTx::with_depth(0);
    let arrival = crate::codec::EntityView {
        guid: PLAYER_BASE,
        ..Default::default()
    };
    let _registration = super::super::subscriptions::PlayerSubscriptions::registered_for_test(
        view.clone(),
        PLAYER_BASE,
        &arrival,
        tx,
    );
    let observer = view.viewer_of_owner(OwnerGuid(PLAYER_BASE)).unwrap();
    observer.created.lock().unwrap().insert(CREATURE_BASE);
    view.spatial.upsert_entity(
        EntityLayer::WorldEntity,
        CREATURE_BASE,
        CellKey::of_position(0, 0, 0.0, 0.0),
        0,
        PLAYER_BASE,
    );

    reconcile_shard(&view, 0, vec![], vec![], vec![]);
    let Outbound::Job(job) = rx.try_recv().unwrap() else {
        panic!("expected the missing pet's Relay job");
    };
    assert!(matches!(
        job().as_slice(),
        [
            Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(_)),
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_)),
            Outbound::One(ServerOpcodeMessage::SMSG_PET_SPELLS(_)),
        ]
    ));
    assert_eq!(
        *observer.created.lock().unwrap(),
        HashSet::from([PLAYER_BASE])
    );
}

#[test]
fn distant_characters_do_not_add_jobs_to_a_local_combat_event() {
    let mut rng = Rng::new(810);
    let realm = realm(&mut rng, 3, 1, 1.0);
    let row = combat(&realm, realm.viewers[0].self_guid, realm.creatures[0]);
    combat_event_appeared(&realm.view, 0, &row);
    assert_eq!(
        realm
            .queues
            .iter()
            .filter(|rx| rx.try_recv().is_ok())
            .count(),
        3
    );

    let mut distant = Vec::new();
    for i in 0..2000 {
        let (tx, rx) = SessionTx::with_depth(0);
        let observer = viewer(100 + i, PLAYER_BASE + 100 + i, tx);
        realm
            .view
            .add_viewer_on_shard(observer, CellKey::at(0, 0, 100, 100), 0);
        distant.push(rx);
    }
    combat_event_appeared(&realm.view, 0, &row);
    assert_eq!(
        realm
            .queues
            .iter()
            .filter(|rx| rx.try_recv().is_ok())
            .count(),
        3
    );
    assert!(distant.iter().all(|rx| rx.try_recv().is_err()));
}

/// The aura audience follows the viewer's anchor: walking into the target's box adds the viewer,
/// walking out removes it, with nothing re-registered.
#[test]
fn aura_audience_follows_a_viewer_recenter() {
    let view = WorldView::new(true);
    let (tx, _rx) = SessionTx::with_depth(0);
    let watcher = viewer(1, PLAYER_BASE, tx);
    view.add_viewer_on_shard(watcher.clone(), CellKey::at(0, 0, 0, 0), 0);
    let stealther = CREATURE_BASE;
    view.spatial.upsert_entity(
        EntityLayer::WorldEntity,
        stealther,
        CellKey::at(0, 0, 10, 10),
        0,
        0,
    );
    let row = aura(1, stealther, A_STEALTH);

    assert!(aura_audience(&view, 0, &row).is_empty());
    view.spatial
        .move_viewer_delta(watcher.session, CellKey::at(0, 0, 9, 9));
    assert_eq!(sessions(aura_audience(&view, 0, &row)), HashSet::from([1]));
    view.spatial
        .move_viewer_delta(watcher.session, CellKey::at(0, 0, 20, 20));
    assert!(aura_audience(&view, 0, &row).is_empty());
    // The target moves under a resident viewer.
    view.spatial.upsert_entity(
        EntityLayer::WorldEntity,
        stealther,
        CellKey::at(0, 0, 21, 21),
        0,
        0,
    );
    assert_eq!(sessions(aura_audience(&view, 0, &row)), HashSet::from([1]));
}

/// Every aura read the relays make goes through the index; a whole-cache scan must not creep
/// back into a job or onto the pump.
#[test]
fn aura_relays_never_scan_the_aura_cache() {
    let subscriptions = include_str!("../subscriptions.rs");
    for signature in [
        "pub(crate) fn offer_peer_create_for",
        "pub(crate) fn aura_insert_outbound",
        "pub(crate) fn aura_update_outbound",
        "pub(crate) fn aura_delete_outbound",
    ] {
        let body = crate::test_scan::code_of(subscriptions, signature);
        assert!(
            !body.contains("game_aura()"),
            "{signature} scans the aura cache"
        );
        assert!(
            body.contains("view.auras."),
            "{signature} must read the aura index"
        );
    }
    let arm = crate::test_scan::code_of(
        include_str!("../world_view.rs"),
        "fn register_shard_callbacks",
    );
    assert!(
        !arm.contains(".game_aura().iter()"),
        "the pump scans the aura cache"
    );
    assert_eq!(arm.matches("view.auras.upsert(shard, row)").count(), 2);
    assert_eq!(arm.matches("view.auras.remove(shard, row)").count(), 1);
}

fn queue_motion(batch: &super::super::movement_batch::MovementBatch, seq: u32) -> EntityMotion {
    batch.push(GwMove {
        actor_guid: PLAYER_BASE,
        opcode: 0x00ee,
        movement_info: vec![],
        x: 0.0,
        y: 0.0,
        z: 0.0,
        o: 0.0,
        move_time_ms: seq,
    });
    EntityMotion {
        guid: PLAYER_BASE,
        map_id: 0,
        instance_id: 0,
        grid_x: 0,
        grid_y: 0,
        opcode: 0x00ee,
        movement_info: vec![],
        seq,
        cell: 0,
    }
}

#[test]
fn healthy_motion_without_a_peer_does_not_report_stopped_delivery() {
    let view = WorldView::new(true);
    let batch = super::super::movement_batch::MovementBatch::new();
    let (tx, rx) = SessionTx::with_depth(0);
    view.add_viewer_on_shard(viewer(1, PLAYER_BASE, tx), CellKey::at(0, 0, 0, 0), 0);
    let before = batch.delivery.snapshot();
    for seq in 1..=172 {
        motion(&view, 0, &queue_motion(&batch, seq), &batch.delivery);
    }

    assert!(
        rx.try_recv().is_err(),
        "the mover must not receive its own motion"
    );
    let delta = batch.delivery.snapshot().since(before);
    assert_eq!(delta.queued, 172);
    assert_eq!(delta.callbacks, 172);
    assert!(!delta.is_silent());
}

#[test]
fn motion_delivery_counts_coalesced_rows_before_a_stalled_peer_writer() {
    let view = WorldView::new(true);
    let batch = super::super::movement_batch::MovementBatch::new();
    let (tx, rx) = SessionTx::with_depth(0);
    let peer = viewer(1, PLAYER_BASE + 1, tx);
    peer.created.lock().unwrap().insert(PLAYER_BASE);
    view.add_viewer_on_shard(peer, CellKey::at(0, 0, 0, 0), 0);
    let before = batch.delivery.snapshot();
    let mut latest = queue_motion(&batch, 1);
    for seq in 2..=172 {
        latest = queue_motion(&batch, seq);
    }
    // The Module may coalesce several queued movements into one row. The writer has not run.
    motion(&view, 0, &latest, &batch.delivery);

    assert!(matches!(rx.try_recv().unwrap(), Outbound::Job(_)));
    assert!(rx.try_recv().is_err());
    let delta = batch.delivery.snapshot().since(before);
    assert_eq!(delta.queued, 172);
    assert_eq!(delta.callbacks, 1);
    assert!(!delta.is_silent());
}

#[test]
fn motion_delivery_silence_is_scoped_to_the_shard_and_current_window() {
    let view = WorldView::new(true);
    let stopped = super::super::movement_batch::MovementBatch::new();
    let healthy = super::super::movement_batch::MovementBatch::new();
    motion(&view, 0, &queue_motion(&stopped, 1), &stopped.delivery);
    let before = stopped.delivery.snapshot();
    for seq in 2..=173 {
        queue_motion(&stopped, seq);
        motion(&view, 1, &queue_motion(&healthy, seq), &healthy.delivery);
    }
    let silent = stopped.delivery.snapshot();
    assert!(silent.since(before).is_silent());
    assert!(!healthy.delivery.snapshot().is_silent());

    motion(&view, 0, &queue_motion(&stopped, 174), &stopped.delivery);
    let resumed = stopped.delivery.snapshot();
    assert!(!resumed.since(silent).is_silent());
    assert!(!stopped.delivery.snapshot().since(resumed).is_silent());
}
