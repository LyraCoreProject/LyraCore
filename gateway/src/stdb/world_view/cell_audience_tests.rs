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
            .upsert_entity(EntityLayer::WorldEntity, guid, key, 0);
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

        let candidates = sessions(combat_audience(&realm.view, &row));
        combat_event_appeared(&realm.view, &row);
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
    for _ in 0..200 {
        let realm = spread_realm(&mut rng);
        let attacker = realm.actor(&mut rng);
        let row = melee(attacker, realm.actor(&mut rng));
        let gate = |v: &Viewer| created_contains(v, attacker);

        let candidates = sessions(melee_audience(&realm.view, &row));
        melee_engaged(&realm.view, &row);
        assert_eq!(realm.jobs_landed(), candidates);
        melee_disengaged(&realm.view, &row);
        assert_eq!(realm.jobs_landed(), candidates);

        let gated: HashSet<SessionId> = realm
            .viewers
            .iter()
            .filter(|v| candidates.contains(&v.session) && gate(v))
            .map(|v| v.session)
            .collect();
        assert_eq!(gated, realm.shard_then_gate(gate));
    }
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

        let candidates = sessions(aura_audience(&realm.view, &row));
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
        .remove_entity(EntityLayer::WorldEntity, player);
    assert_eq!(
        sessions(aura_audience(&realm.view, &aura(1, player, 0xA0))),
        HashSet::from([realm.viewers[3].session])
    );
    let creature = realm.creatures[0];
    realm
        .view
        .spatial
        .remove_entity(EntityLayer::WorldEntity, creature);
    assert!(aura_audience(&realm.view, &aura(2, creature, 0xA0)).is_empty());
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
            sessions(cast_audience(&realm.view, &row)),
            want,
            "cast by {actor:#x}"
        );
        cast_event_appeared(&realm.view, &row);
        assert_eq!(realm.jobs_landed(), want);

        let row = impact(&realm, actor, target);
        assert_eq!(
            sessions(impact_audience(&realm.view, &row)),
            want,
            "impact by {actor:#x}"
        );
        impact_appeared(&realm.view, &row);
        assert_eq!(realm.jobs_landed(), want);

        let row = emote(&realm, actor, target);
        assert_eq!(
            sessions(emote_audience(&realm.view, &row)),
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

        let candidates = sessions(chat_audience(&realm.view, &row));
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
        .remove_entity(EntityLayer::WorldEntity, speaker);
    assert_eq!(
        sessions(chat_audience(&realm.view, &chat(speaker, YELL))),
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
    assert_eq!(index.stealth_count(77), 1);
    assert!(index.is_stealthed(77));
    assert!(!index.is_stealthed(78));
    assert_eq!(index.on_target(77).len(), 2);
    assert_eq!(index.stats(), (3, 2));

    // An update lands on the same (shard, id) and replaces the row.
    let mut refreshed = aura(2, 77, 0xA0);
    refreshed.amount = 25;
    index.upsert(0, &refreshed);
    assert_eq!(index.on_target(77).len(), 2);
    assert_eq!(
        index
            .on_target(77)
            .iter()
            .find(|a| a.id == 2)
            .map(|a| a.amount),
        Some(25)
    );

    // A delete on shard 1 of id 1 must not touch shard 0's id 1.
    index.remove(1, &aura(1, 78, 0xA0));
    assert!(index.on_target(78).is_empty());
    assert_eq!(index.stealth_count(77), 1);
    index.remove(0, &aura(1, 77, A_STEALTH));
    assert_eq!(index.stealth_count(77), 0);
    index.remove(0, &aura(1, 77, A_STEALTH)); // idempotent
    assert_eq!(index.on_target(77).len(), 1);
    index.remove(0, &aura(2, 77, 0xA0));
    assert_eq!(index.stats(), (0, 0));
}

#[test]
fn aura_index_reseed_drops_rows_the_caches_no_longer_hold() {
    let index = AuraIndex::default();
    index.upsert(0, &aura(1, 77, 0xA0));
    index.upsert(0, &aura(2, 77, 0xA0));
    index.replace_all([(0, aura(2, 77, 0xA0)), (1, aura(9, 80, A_STEALTH))]);
    assert_eq!(index.on_target(77).len(), 1);
    assert!(index.is_stealthed(80));
    assert_eq!(index.stats(), (2, 2));
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
    );
    let row = aura(1, stealther, A_STEALTH);

    assert!(aura_audience(&view, &row).is_empty());
    view.spatial
        .move_viewer_delta(watcher.session, CellKey::at(0, 0, 9, 9));
    assert_eq!(sessions(aura_audience(&view, &row)), HashSet::from([1]));
    view.spatial
        .move_viewer_delta(watcher.session, CellKey::at(0, 0, 20, 20));
    assert!(aura_audience(&view, &row).is_empty());
    // The target moves under a resident viewer.
    view.spatial.upsert_entity(
        EntityLayer::WorldEntity,
        stealther,
        CellKey::at(0, 0, 21, 21),
        0,
    );
    assert_eq!(sessions(aura_audience(&view, &row)), HashSet::from([1]));
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
    let arm =
        crate::test_scan::code_of(include_str!("../world_view.rs"), "pub(crate) fn arm_shard");
    assert!(
        !arm.contains(".game_aura().iter()"),
        "the pump scans the aura cache"
    );
    assert_eq!(arm.matches("view.auras.upsert(shard, row)").count(), 2);
    assert_eq!(arm.matches("view.auras.remove(shard, row)").count(), 1);
}
