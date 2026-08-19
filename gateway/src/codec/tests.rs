//! Unit + golden-vector tests for the wire codec. Hoisted out of `mod.rs` (was an 819-line
//! trailing `#[cfg(test)] mod tests` block) so the production codec reads on its own; this is
//! pure code-motion, byte-identical to the previous inline module.

use super::*;
// Only the loot byte-match test needs gtker's typed loot response (the runtime path is raw).
use wow_world_messages::vanilla::ServerMessage;
use wow_world_messages::vanilla::{
    EnvironmentalDamageType, SMSG_LOOT_RESPONSE_LootMethod, SMSG_LOOT_RESPONSE, TimerType,
};
// Group loot methods: the vote-kind byte constants + wire RollVote enum.
use lyracore_shared::loot_roll::vote_kind;
use wow_world_messages::vanilla::RollVote;

fn warrior_entity() -> EntityView {
    EntityView {
        guid: 1,
        map_id: 0,
        zone_id: 12,
        x: -8949.95,
        y: -132.493,
        z: 83.5312,
        scale_x: 1.0,
        type_mask: 0x19,
        health: 60,
        max_health: 60,
        power: 0,
        max_power: 1000,
        level: 1,
        run_speed_mult_bp: 10_000,
        faction_template: 1,
        unit_bytes_0: 1 | (1 << 8) | (1 << 24), // human warrior male rage
        display_id: 49,
        native_display_id: 49,
        ..Default::default()
    }
}

/// Reputation relay: a known faction id maps to its `Faction` enum value and the signed standing
/// survives the wire `u32` bit-cast (the client reads it back as `i32`); an unknown faction id yields
/// `None` so the relay skips silently instead of sending garbage.
#[test]
fn set_faction_standing_raw_sends_the_rep_index_not_the_faction_id() {
    // 5875 layout: count(u32) + ReputationListID(u32) + standing(u32) = 12 bytes. The CRASH FIX: the middle
    // field is the rep-list INDEX (Stormwind = 19), NOT the faction id (72). Sending 72 indexed past the
    // client's 64-slot rep array → null deref → ERROR #132. The caller passes game_faction.reputation_index.
    let (opcode, body) =
        build_set_faction_standing_raw(19, 500).expect("a valid rep-index always builds");
    assert_eq!(opcode, 0x0124);
    assert_eq!(
        body.len(),
        12,
        "count(u32) + rep_index(u32) + standing(u32)"
    );
    assert_eq!(
        u32::from_le_bytes(body[0..4].try_into().unwrap()),
        1,
        "count = 1"
    );
    assert_eq!(
        u32::from_le_bytes(body[4..8].try_into().unwrap()),
        19,
        "the rep-index (19), NOT faction_id 72"
    );
    assert_eq!(
        u32::from_le_bytes(body[8..12].try_into().unwrap()) as i32,
        500,
        "standing survives"
    );
    // Negative standing (e.g. Hostile) bit-casts through the u32 unchanged.
    let (_, neg) = build_set_faction_standing_raw(19, -42000).unwrap();
    assert_eq!(
        u32::from_le_bytes(neg[8..12].try_into().unwrap()) as i32,
        -42000
    );
}

/// The raw-send envelope is byte-identical to gtker's: build the SAME slot-0 aura VALUES update
/// both ways (gtker's typed `UpdatePlayer` slot-0 aura builder vs our `build_values_update_raw`
/// fed the equivalent mask) and assert the opcode + the whole body match. Pins the block-count /
/// has_transport / update-type / packed-guid envelope + the SMSG_UPDATE_OBJECT opcode so the raw
/// path can carry a multi-slot mask the typed builder can't express, without ever drifting from the
/// real wire shape.
#[test]
fn raw_values_body_matches_gtker_envelope() {
    use super::update_mask::{self, idx, AuraSlot};
    let (guid, spell_id, level, flags) = (0x42u64, 6673u32, 1u8, 0x09u8);

    // gtker's typed slot-0 aura VALUES update, with the same finalize→dirty_reset→re-set-only-changed
    // discipline the live builders use so OBJECT_FIELD_TYPE never leaks (stripped by dirty_reset).
    let mut player = UpdatePlayer::builder()
        .set_unit_aura(spell_id as i32)
        .set_unit_auraflags(flags, 0, 0, 0)
        .set_unit_auralevels(level, 0, 0, 0)
        .set_unit_auraapplications(0, 0, 0, 0) // stack count 1 is stored as 0
        .finalize();
    player.dirty_reset();
    player.set_unit_aura(spell_id as i32);
    player.set_unit_auraflags(flags, 0, 0, 0);
    player.set_unit_auralevels(level, 0, 0, 0);
    player.set_unit_auraapplications(0, 0, 0, 0);
    let mut gtker = Vec::new();
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Player(player),
        }],
    }
    .write_unencrypted_server(&mut gtker)
    .unwrap();
    let gtker_opcode = u16::from_le_bytes([gtker[2], gtker[3]]);
    let gtker_body = &gtker[4..]; // strip [size:2 BE][opcode:2 LE]

    let mut mask = update_mask::UpdateMaskValues::new();
    update_mask::write_auras(
        &mut mask,
        &[AuraSlot {
            slot: 0,
            spell_id,
            flags,
            level,
        }],
    );
    mask.set_u32(idx::UNIT_AURAAPPLICATIONS, 0); // gtker's slot-0 aura builder emits this present-zero word
    let (our_opcode, our_body) = build_values_update_raw(guid, &mask);

    assert_eq!(
        our_opcode, gtker_opcode,
        "SMSG_UPDATE_OBJECT opcode mismatch"
    );
    assert_eq!(
        our_body, gtker_body,
        "raw VALUES body must byte-match gtker's envelope+mask\n  ours:  {our_body:?}\n  gtker: {gtker_body:?}"
    );
}

#[test]
fn movement_info_bytes_roundtrip() {
    use wow_world_messages::vanilla::{MovementInfo_MovementFlags, Vector3d};
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 0x0102_0304,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.5,
        },
        orientation: 1.25,
        fall_time: 0.0,
    };
    let bytes = movement_info_to_bytes(&info).unwrap();
    // A vanilla MovementInfo with no flags is a fixed 28-byte body (flags u32 + time u32 +
    // xyz 3×f32 + orientation f32 + fall_time f32).
    assert_eq!(bytes.len(), 28);
    let back = bytes_to_movement_info(&bytes).unwrap();
    assert_eq!(back, info);
}

#[test]
fn name_query_response_maps_seeded_warrior() {
    let c = CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,   // Human
        class: 1,  // Warrior
        gender: 0, // Male
        ..Default::default()
    };
    let r = build_name_query_response(&c).unwrap();
    assert_eq!(r.guid.guid(), 1);
    assert_eq!(r.character_name, "Tester");
    assert!(r.realm_name.is_empty());
    assert_eq!(r.race, Race::Human);
    assert_eq!(r.class, Class::Warrior);
    assert_eq!(r.gender, Gender::Male);
}

#[test]
fn health_values_update_is_health_only_no_object_type() {
    // Combat-crash regression (2026-06-16): a partial VALUES update must carry ONLY
    // UNIT_FIELD_HEALTH (descriptor bit 22). The gtker builder seeds OBJECT_FIELD_TYPE (bit 2)
    // dirty; `build_health_values` strips it via dirty_reset, because re-sending TYPE in a
    // partial update crashes the 1.12 client (it overwrites the cached object type, stripping the
    // PLAYER bit on the player's own object → null deref at 0x0060655A / null+0x110).
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let msg = build_health_values(guid, 20);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    // The update mask is the LAST thing written, so the body tail is exactly:
    //   block_count(u8)=1 | mask word 0x00400000 LE (bit 22 only, NO bit 2) | value = health LE.
    // A re-sent OBJECT_FIELD_TYPE would make the mask word 0x00400004 with a 0x09 value word
    // ahead of the health word — this assertion fails in that case, guarding the regression.
    // (We deliberately do NOT round-trip: gtker's strict reader rejects a TYPE-less mask, but the
    // real 5875 client — the only consumer — requires TYPE's absence, like cmangos/vmangos.)
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        buf.ends_with(&[0x01, 0x00, 0x00, 0x40, 0x00, 20, 0, 0, 0]),
        "VALUES mask must be health-only (bit 22) with no OBJECT_FIELD_TYPE; tail was {:02x?}",
        &buf[buf.len().saturating_sub(16)..]
    );
}

#[test]
fn resistance_values_is_unit_only_no_object_type() {
    // Live armor (Demon Skin / gear): the EFFECTIVE-armor relay must be a Unit VALUES mask carrying ONLY
    // UNIT_FIELD_RESISTANCES[0] (descriptor index 155 → bit 27 of mask block 4 → word 0x08000000), never
    // re-sending OBJECT_FIELD_TYPE (bit 2, value 0x09 = OBJECT|UNIT) whose re-send strips the PLAYER bit
    // → 5875 null+0x110 crash. Same dirty_reset discipline as build_health_values.
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let armor = 0x1234u32; // 4660 — a recognizable LE pattern distinct from any mask word
    let msg = build_resistance_values(guid, armor);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    // The update mask is the LAST thing written: block_count=5 (descriptor 155 lives in block 4), then 5
    // mask words (only block 4 set = 0x08000000 = bit 27), then the armor value LE. A leaked
    // OBJECT_FIELD_TYPE would set bit 2 in block 0 and add a 0x09 value word ahead of the armor word, so
    // this exact-tail assertion fails in that case (guarding the crash regression).
    assert!(
        buf.ends_with(&[
            0x05, // block_count = 5 (the mask spans blocks 0..=4)
            0, 0, 0, 0, // block 0 (NO OBJECT_FIELD_TYPE bit 2)
            0, 0, 0, 0, // block 1
            0, 0, 0, 0, // block 2
            0, 0, 0, 0, // block 3
            0x00, 0x00, 0x00,
            0x08, // block 4 = bit 27 only (descriptor 155 = UNIT_FIELD_RESISTANCES[0])
            0x34, 0x12, 0x00, 0x00, // armor value 0x1234 LE
        ]),
        "resistance VALUES must carry only index 155 (no OBJECT_FIELD_TYPE); tail was {:02x?}",
        &buf[buf.len().saturating_sub(28)..]
    );
    // Belt-and-suspenders: the OBJECT_FIELD_TYPE value word (0x09 = OBJECT|UNIT) must be absent anywhere.
    let contains = |needle: &[u8]| buf.windows(needle.len()).any(|w| w == needle);
    assert!(
        !contains(&[0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the resistance VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn dynamic_flags_values_is_unit_only_no_object_type() {
    // Corpse render bit (slice 2): the killing-blow dynamic_flags update must be a Unit VALUES
    // mask carrying ONLY UNIT_DYNAMIC_FLAGS — never re-sending OBJECT_FIELD_TYPE (bit 2, value
    // 0x09 = OBJECT|UNIT), which crashes the 1.12 client. Same dirty_reset discipline as
    // build_health_values; this guards that the new builder didn't regress it.
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let msg = build_dynamic_flags_values(guid, 0x20); // UNIT_DYNFLAG_DEAD
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    // The dynamic_flags value word (0x20 LE) must be present; the OBJECT_FIELD_TYPE value word
    // (0x09 LE) must be absent — its presence would mean TYPE leaked into the partial mask.
    let contains = |needle: &[u8]| buf.windows(needle.len()).any(|w| w == needle);
    assert!(
        contains(&[0x20, 0x00, 0x00, 0x00]),
        "expected dynamic_flags value; buf {:02x?}",
        buf
    );
    assert!(
        !contains(&[0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in a partial VALUES update; buf {:02x?}",
        buf
    );
}

#[test]
fn coinage_values_is_player_mask_without_object_type() {
    // The live purse update (slice 3) must be a Player VALUES mask carrying ONLY
    // PLAYER_FIELD_COINAGE — never re-sending OBJECT_FIELD_TYPE (0x09). Same dirty_reset guard.
    let msg = build_coinage_values(1, 0x111); // 273 copper, a recognizable LE pattern
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), 1);
            assert!(
                matches!(mask1, UpdateMask::Player(_)),
                "coinage update must be a Player mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let contains = |needle: &[u8]| buf.windows(needle.len()).any(|w| w == needle);
    assert!(
        contains(&[0x11, 0x01, 0x00, 0x00]),
        "expected coinage value; buf {:02x?}",
        buf
    );
    assert!(
        !contains(&[0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the coinage VALUES update; buf {:02x?}",
        buf
    );
}

#[test]
fn aura_values_is_player_mask_without_object_type() {
    // The aura tracer's buff-icon update goes out on the LIVE relay path (stdb → full_aura_mask +
    // build_values_update_raw), the multi-slot encoder that replaced the slot-0-only typed builder.
    // The mask must carry the aura fields but NEVER OBJECT_FIELD_TYPE (0x09), whose re-send crashes
    // the 5875 client at null+0x110 — the make-or-break guard for the whole spell tier. Here the
    // trap is structural (full_aura_mask never inserts index 2), and build_values_update_raw also
    // debug-asserts its absence; this pins it on the wire bytes regardless.
    use super::update_mask::{self, idx, AuraSlot};
    let win = |b: &[u8], needle: &[u8]| b.windows(needle.len()).any(|w| w == needle);

    let mask = update_mask::full_aura_mask(&[AuraSlot {
        slot: 0,
        spell_id: 6673,
        flags: 0x09,
        level: 1,
    }]);
    assert!(
        mask.get(idx::OBJECT_TYPE).is_none(),
        "live aura mask must never carry OBJECT_FIELD_TYPE"
    );
    let (_op, body) = build_values_update_raw(1, &mask);
    assert!(
        win(&body, &[0x11, 0x1a, 0x00, 0x00]),
        "expected aura spell-id 6673; body {:02x?}",
        body
    );

    // OBJECT_FIELD_TYPE-leak check via the CLEAR path: with no auras present every aura field is 0,
    // so any `09 00 00 00` word would be a leaked OBJECT_FIELD_TYPE (0x09 = OBJECT|UNIT). (We can't
    // search the apply path for it — auraflags=0x09 legitimately produces the same value word.)
    let clear = update_mask::full_aura_mask(&[]);
    assert!(
        clear.get(idx::OBJECT_TYPE).is_none(),
        "cleared aura mask must never carry OBJECT_FIELD_TYPE"
    );
    let (_op, clear_body) = build_values_update_raw(1, &clear);
    assert!(
        !win(&clear_body, &[0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not appear in the cleared aura VALUES update; clear body {:02x?}",
        clear_body
    );
}

#[test]
fn loot_response_envelope_matches_gtker() {
    // The RAW loot response must reuse gtker's exact money-only ENVELOPE (guid u64, loot_method
    // u8, gold u32, count u8) — only the per-item bytes are ours (gtker's item codec is short).
    // Pin it: our raw body (no items) must equal gtker's SMSG_LOOT_RESPONSE body byte-for-byte,
    // and the opcode must match. (Same discipline as the iter-14 raw-aura envelope test.)
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let (opcode, body) = build_loot_response_raw(guid, 13, &[]);
    let gtker = ServerOpcodeMessage::SMSG_LOOT_RESPONSE(Box::new(SMSG_LOOT_RESPONSE {
        guid: Guid::new(guid),
        loot_method: SMSG_LOOT_RESPONSE_LootMethod::Corpse,
        gold: Gold::new(13),
        items: Vec::new(),
    }));
    let mut framed = Vec::new();
    gtker.write_unencrypted_server(&mut framed).unwrap();
    // Strip gtker's 4-byte server header ([size:u16 BE][opcode:u16 LE]) to get the body.
    let gtker_opcode = u16::from_le_bytes([framed[2], framed[3]]);
    assert_eq!(
        opcode, gtker_opcode,
        "raw opcode must match gtker's SMSG_LOOT_RESPONSE"
    );
    assert_eq!(
        body,
        framed[4..],
        "raw envelope must byte-match gtker's money-only loot"
    );
}

#[test]
fn loot_response_item_layout_is_full_vanilla() {
    // One item must serialize the FULL vanilla 22-byte layout (slot, id, count, display, 0,0, ty),
    // NOT gtker's short 6-byte form. Spot-check the offsets + total length.
    let guid = 0xF130_0000_0000_0001;
    let (_op, body) = build_loot_response_raw(guid, 5, &[(0, 25, 2, 1542)]);
    // envelope = 8 (guid) + 1 (method) + 4 (gold) + 1 (count) = 14; one item = 22 → 36 total.
    assert_eq!(
        body.len(),
        14 + 22,
        "one item must add 22 bytes (full vanilla layout)"
    );
    assert_eq!(body[13], 1, "amount_of_items == 1");
    let it = &body[14..];
    assert_eq!(it[0], 0, "loot slot index");
    assert_eq!(
        u32::from_le_bytes([it[1], it[2], it[3], it[4]]),
        25,
        "item id (entry)"
    );
    assert_eq!(
        u32::from_le_bytes([it[5], it[6], it[7], it[8]]),
        2,
        "stack count"
    );
    assert_eq!(
        u32::from_le_bytes([it[9], it[10], it[11], it[12]]),
        1542,
        "display id"
    );
    assert_eq!(it[21], 0, "slot_type = TypeAllowLoot");
}

// ---- Group loot methods — codec pins for the roll messages ----

#[test]
fn loot_start_roll_carries_the_countdown_and_item() {
    let m = build_loot_start_roll(0xF130_0000_0000_0007, 3, 1234, 60_000);
    assert_eq!(m.creature.guid(), 0xF130_0000_0000_0007);
    assert_eq!(m.loot_slot, 3);
    assert_eq!(m.item, 1234);
    assert_eq!(m.countdown_time, std::time::Duration::from_millis(60_000));
    // Round-trips through the real vanilla wire encode without error (opcode 0x02A1).
    let msg = ServerOpcodeMessage::SMSG_LOOT_START_ROLL(Box::new(m));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn wire_roll_number_signals_pass_as_128_and_passes_a_real_roll_through() {
    // vanilla's SMSG_LOOT_ROLL has NO separate auto_pass field (confirmed against the vendored
    // crate — a TBC/WRATH-only addition); a PASS is instead `roll_number > 127` per the field's own
    // doc comment. `[V]`: the exact sentinel byte (128 vs some other >127 value) is unverified
    // against a real client — 128 matches cmangos/vmangos.
    assert_eq!(wire_roll_number(vote_kind::PASS, 0), 128);
    assert_eq!(
        wire_roll_number(vote_kind::NEED, 87),
        87,
        "a real roll passes through unchanged"
    );
    assert_eq!(wire_roll_number(vote_kind::GREED, 42), 42);
}

#[test]
fn loot_roll_carries_the_voter_item_and_vote() {
    let m = build_loot_roll(
        0xF130_0000_0000_0007,
        3,
        55,
        1234,
        87,
        vote_kind::NEED,
        false,
    );
    assert_eq!(m.creature.guid(), 0xF130_0000_0000_0007);
    assert_eq!(m.loot_slot, 3);
    assert_eq!(m.player.guid(), 55);
    assert_eq!(m.item, 1234);
    assert_eq!(m.roll_number, 87);
    assert_eq!(m.vote, RollVote::Need);
    // A PASS vote's roll_number folds to the >127 sentinel regardless of the stored `rolled` (0).
    let pass = build_loot_roll(0xF130_0000_0000_0007, 3, 55, 1234, 0, vote_kind::PASS, true);
    assert_eq!(pass.roll_number, 128);
    assert_eq!(pass.vote, RollVote::Pass);
}

#[test]
fn loot_roll_won_carries_the_winner_roll_and_winning_tier() {
    let m = build_loot_roll_won(0xF130_0000_0000_0007, 3, 1234, 55, 96, vote_kind::NEED);
    assert_eq!(m.looted_target.guid(), 0xF130_0000_0000_0007);
    assert_eq!(m.loot_slot, 3);
    assert_eq!(m.item, 1234);
    assert_eq!(m.winning_player.guid(), 55);
    assert_eq!(m.winning_roll, 96);
    assert_eq!(m.vote, RollVote::Need);
    // A greed-only contest reports Greed — the tier is threaded, not hardcoded.
    let g = build_loot_roll_won(0xF130_0000_0000_0007, 3, 1234, 55, 42, vote_kind::GREED);
    assert_eq!(g.vote, RollVote::Greed);
}

#[test]
fn loot_master_list_carries_every_eligible_guid() {
    let m = build_loot_master_list(&[10, 20, 30]);
    assert_eq!(m.guids.len(), 3);
    assert_eq!(m.guids[0].guid(), 10);
    assert_eq!(m.guids[1].guid(), 20);
    assert_eq!(m.guids[2].guid(), 30);
    // Round-trips through the real vanilla wire encode without error (opcode 0x02A4).
    let msg = ServerOpcodeMessage::SMSG_LOOT_MASTER_LIST(Box::new(m));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

// ---- Money-loot split — codec pin for the MONEY_SHARE relay ----

#[test]
fn loot_money_notify_carries_the_share_not_a_hardcoded_total() {
    // The gateway's grouped-split relay decodes a `MONEY_SHARE` event payload and rebuilds
    // `SMSG_LOOT_MONEY_NOTIFY` straight from it (`stdb/subscriptions.rs`) — pin that the wire
    // builder's `amount` field is exactly the decoded share, for a value that would be wrong if the
    // relay ever passed the corpse's TOTAL instead of the per-recipient share.
    let share = lyracore_shared::loot_roll::decode_money_share(
        &lyracore_shared::loot_roll::encode_money_share(17),
    )
    .unwrap();
    let m = build_loot_money_notify(share);
    assert_eq!(m.amount, 17);
    // Round-trips through the real vanilla wire encode without error (opcode 0x0154).
    let msg = ServerOpcodeMessage::SMSG_LOOT_MONEY_NOTIFY(m);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn ghost_values_is_player_mask_without_object_type() {
    // The ghost transition relay (slice 5) carries PLAYER_FLAGS (GHOST) + UNIT_FIELD_BYTES_1 only,
    // and must NOT re-send OBJECT_FIELD_TYPE (0x09) — same dirty_reset discipline as the other
    // partial-VALUES builders (re-sending TYPE crashes the 5875 client).
    let guid = 1;
    let msg = build_ghost_values(guid, 0x10, 0x0100_0000); // PLAYER_FLAGS_GHOST + vis-ghost byte 3
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Player(_)),
                "ghost update must be a Player mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let contains = |n: &[u8]| buf.windows(n.len()).any(|w| w == n);
    assert!(
        contains(&[0x10, 0x00, 0x00, 0x00]),
        "expected PLAYER_FLAGS_GHOST; buf {:02x?}",
        buf
    );
    assert!(
        contains(&[0x00, 0x00, 0x00, 0x01]),
        "expected unit_bytes_1 vis-ghost (byte 3); buf {:02x?}",
        buf
    );
    assert!(
        !contains(&[0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the ghost VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn corpse_create_is_a_corpse_object_with_race_in_bytes1() {
    // The crash regression (slice 5): the client null-derefs if CORPSE_FIELD_BYTES_1 doesn't carry
    // the RACE in byte 1. The module packs bytes_1 = 0 | race<<8 | gender<<16 | skin<<24; this
    // confirms the codec emits a CORPSE object and passes that word through with race in byte 1.
    let corpse = CorpseView {
        guid: (0xF101u64 << 48) | 1,
        owner_guid: 1,
        x: -8949.0,
        y: -132.0,
        z: 83.0,
        orientation: 0.0,
        display_id: 49,
        bytes_1: (1u32 << 8) | (1u32 << 16) | (5u32 << 24), // race=1, gender=1, skin=5
        bytes_2: 0,
        is_bones: false,
    };
    let msg = build_corpse_create_object(&corpse);
    match &msg.objects[0] {
        Object::CreateObject2 {
            object_type, mask2, ..
        } => {
            assert!(
                matches!(object_type, ObjectType::Corpse),
                "must be a CORPSE object"
            );
            assert!(
                matches!(mask2, UpdateMask::Corpse(_)),
                "must carry an UpdateCorpse mask"
            );
        }
        other => panic!("expected Object::CreateObject2, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    // CORPSE_FIELD_BYTES_1 value word = 0x05010100 → LE [00, 01, 01, 05]: byte1=race(1), byte2=gender(1).
    assert!(
        buf.windows(4).any(|w| w == [0x00, 0x01, 0x01, 0x05]),
        "corpse BYTES_1 must carry race in byte 1 (not the player_bytes layout); buf {:02x?}",
        buf
    );
}

#[test]
fn corpse_create_object_flags_bones_state_work_item_201() {
    // A live body carries CORPSE_FLAG_UNK2 (0x04, what every real vanilla body sends); once the body
    // has decayed to bones (`Corpse::is_bones`), the object should instead carry
    // CORPSE_FLAG_BONE (0x01) so a real client renders bare bones instead of the dead body.
    // UNVERIFIED-until-observed — no live client has confirmed this bit actually drives the bones
    // render; the flag mapping is inferred from mangos' `CORPSE_FLAG_BONE` naming.
    let live = CorpseView {
        guid: 1,
        owner_guid: 1,
        display_id: 49,
        is_bones: false,
        ..Default::default()
    };
    let msg = build_corpse_create_object(&live);
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(
                m.corpse_flags(),
                Some(0x04),
                "a live body must carry CORPSE_FLAG_UNK2 (0x04)"
            );
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }

    let bones = CorpseView {
        is_bones: true,
        ..live
    };
    let msg2 = build_corpse_create_object(&bones);
    match &msg2.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(
                m.corpse_flags(),
                Some(0x01),
                "decayed bones must carry CORPSE_FLAG_BONE (0x01)"
            );
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }
}

#[test]
fn corpse_query_found_carries_position_else_not_found() {
    match build_corpse_query_response(Some((0, -8949.0, -132.0, 83.0))).unwrap() {
        crate::codec::MSG_CORPSE_QUERY_Server::Found { position, .. } => {
            assert_eq!(position.x, -8949.0);
        }
        other => panic!("expected Found, got {other:?}"),
    }
    assert!(matches!(
        build_corpse_query_response(None).unwrap(),
        crate::codec::MSG_CORPSE_QUERY_Server::NotFound
    ));
}

#[test]
fn resurrect_request_names_the_caster_and_flags_player() {
    // The offer must name the CASTER (the client's "<Name> requests to resurrect you" prompt)
    // and echo the caster's guid — the dead target answers CMSG_RESURRECT_RESPONSE against THIS guid,
    // not their own. `player` is always true (the module only ever offers this to a player target).
    let msg = build_resurrect_request(42, "Priestly".to_string());
    assert_eq!(msg.guid.guid(), 42);
    assert_eq!(msg.name, "Priestly");
    assert!(msg.player);
}

#[test]
fn levelup_values_is_player_mask_without_object_type() {
    // The ding descriptor update (UNIT_FIELD_LEVEL + maxhealth + health + xp) must be a Player
    // mask and must NOT re-send OBJECT_FIELD_TYPE (bit 2 / value 0x09) — same dirty_reset
    // discipline as build_health_values (see that test for the byte-level guard on the pattern).
    let msg = build_levelup_values(1, 2, 75, 75, 0, 900, 0, 1000);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), 1);
            assert!(
                matches!(mask1, UpdateMask::Player(_)),
                "ding update must be a Player mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    // OBJECT_FIELD_TYPE (bit 2) lives in mask block 0; its value word is 0x09 (OBJECT|UNIT) — none
    // of our fields are 9, so its absence in the body proves TYPE wasn't re-sent (level=2,
    // health/maxhealth=75, xp=0, next=900 never serialize as 0x09_00_00_00).
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let type_word = [0x09u8, 0x00, 0x00, 0x00];
    assert!(
        !buf.windows(4).any(|w| w == type_word),
        "build_levelup_values must not re-send OBJECT_FIELD_TYPE (0x09 word present); body {:02x?}",
        buf
    );
}

#[test]
fn levelup_values_sets_character_points1_at_l10() {
    // A mid-session ding to L10 must include PLAYER_CHARACTER_POINTS1=1 in the VALUES packet so the
    // talent pane shows 1 free point immediately (without relog). Verify by looking for the value
    // word [0x01, 0x00, 0x00, 0x00] in the wire body — at L10 it MUST be present.
    // NOTE: we use level=10 (not 9) to avoid the collision where the level value itself serializes
    // as [0x09, 0x00, 0x00, 0x00] (the same pattern as OBJECT_FIELD_TYPE). The OBJECT_FIELD_TYPE
    // guard is already proven by levelup_values_is_player_mask_without_object_type at level=2.
    let msg_l10 = build_levelup_values(1, 10, 100, 100, 0, 1500, 0, 1500);
    let mut buf = Vec::new();
    msg_l10.write_unencrypted_server(&mut buf).unwrap();
    let one_word = [0x01u8, 0x00, 0x00, 0x00];
    assert!(
        buf.windows(4).any(|w| w == one_word),
        "build_levelup_values(level=10) must emit PLAYER_CHARACTER_POINTS1=1 word; body {:02x?}",
        buf
    );
}

#[test]
fn attack_start_carries_attacker_and_victim() {
    let player = 1u64;
    let chicken = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let msg = build_attack_start(player, chicken);
    assert_eq!(msg.attacker.guid(), player);
    assert_eq!(msg.victim.guid(), chicken);
    // Round-trip through the wire codec.
    let server: ServerOpcodeMessage = ServerOpcodeMessage::SMSG_ATTACKSTART(Box::new(msg));
    let mut buf = Vec::new();
    server.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKSTART(m) => {
            assert_eq!(m.attacker.guid(), player);
            assert_eq!(m.victim.guid(), chicken);
        }
        other => panic!("expected SMSG_ATTACKSTART, got {other}"),
    }
}

#[test]
fn breath_mirror_timer_edges_roundtrip_on_the_vanilla_wire() {
    let start = build_breath_timer_start(57_000, 60_000);
    let mut start_bytes = Vec::new();
    start.write_unencrypted_server(&mut start_bytes).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut start_bytes.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_START_MIRROR_TIMER(msg) => {
            assert_eq!(msg.timer, TimerType::Breath);
            assert_eq!(msg.time_remaining, 57_000);
            assert_eq!(msg.duration, 60_000);
            assert_eq!(msg.scale, u32::MAX, "wire bits for signed -1 drain scale");
            assert!(!msg.is_frozen);
        }
        other => panic!("expected SMSG_START_MIRROR_TIMER, got {other}"),
    }

    let stop = build_breath_timer_stop();
    let mut stop_bytes = Vec::new();
    stop.write_unencrypted_server(&mut stop_bytes).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut stop_bytes.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_STOP_MIRROR_TIMER(msg) => assert_eq!(msg.timer, TimerType::Breath),
        other => panic!("expected SMSG_STOP_MIRROR_TIMER, got {other}"),
    }
}

#[test]
fn drowning_damage_log_roundtrips_with_the_actual_damage() {
    let msg = build_drowning_damage_log(0xF00D, 19);
    let mut bytes = Vec::new();
    msg.write_unencrypted_server(&mut bytes).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut bytes.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ENVIRONMENTAL_DAMAGE_LOG(log) => {
            assert_eq!(log.guid.guid(), 0xF00D);
            assert_eq!(log.damage_type, EnvironmentalDamageType::Drowning);
            assert_eq!(log.damage, 19);
        }
        other => panic!("expected SMSG_ENVIRONMENTAL_DAMAGE_LOG, got {other}"),
    }
}

#[test]
fn attacker_state_update_is_one_physical_normal_swing() {
    let player = 1u64;
    let chicken = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let msg = build_attacker_state_update(player, chicken, 7, 0, 0, 0);
    // A landed normal swing must carry AffectsVictim (0x02) + VictimState NORMAL (1) so the
    // 5875 client renders damage, not "MISS" (the symptom of VictimState=UNAFFECTED).
    assert_eq!(msg.hit_info, HitInfo::AffectsVictim);
    assert_eq!(msg.damage_state, 1);
    assert_eq!(msg.attacker.guid(), player);
    assert_eq!(msg.target.guid(), chicken);
    assert_eq!(msg.total_damage, 7);
    assert_eq!(msg.damages.len(), 1);
    assert_eq!(msg.damages[0].spell_school_mask, 0);
    assert_eq!(msg.damages[0].damage_uint, 7);
    assert_eq!(msg.damages[0].damage_float, 7.0);
    // Round-trip through the wire codec (the Vec<DamageInfo> + HitInfo must survive).
    let server = ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(msg));
    let mut buf = Vec::new();
    server.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(m) => {
            assert_eq!(m.total_damage, 7);
            assert_eq!(m.damages[0].damage_uint, 7);
        }
        other => panic!("expected SMSG_ATTACKERSTATEUPDATE, got {other}"),
    }
}

#[test]
fn attacker_state_update_maps_hit_info_discriminant() {
    // module hit_info: 0 normal, 1 crit, 2 miss → SMSG HitInfo enum + VictimState (damage_state).
    // Landed hits get VictimState NORMAL (1); a miss gets UNAFFECTED (0) — matching mangos so the
    // client shows the swing as a hit (normal/crit) vs. "MISS".
    let normal = build_attacker_state_update(1, 2, 7, 0, 0, 0);
    assert_eq!(normal.hit_info, HitInfo::AffectsVictim);
    assert_eq!(normal.damage_state, 1);
    assert_eq!(normal.blocked_amount, 0);
    let crit = build_attacker_state_update(1, 2, 14, 1, 0, 0);
    assert_eq!(crit.hit_info, HitInfo::CriticalHit);
    assert_eq!(crit.damage_state, 1);
    let miss = build_attacker_state_update(1, 2, 0, 2, 0, 0);
    assert_eq!(miss.hit_info, HitInfo::Miss);
    assert_eq!(miss.damage_state, 0);
    assert_eq!(miss.total_damage, 0);
    // Dodge/parry (3/4): zero damage, "Dodge"/"Parry" from VictimState (2/3), no HitInfo flag
    // (NormalSwing clears AFFECTS_VICTIM so no being-hit animation plays on the avoided swing).
    let dodge = build_attacker_state_update(1, 2, 0, 3, 0, 0);
    assert_eq!(dodge.hit_info, HitInfo::NormalSwing);
    assert_eq!(dodge.damage_state, 2);
    assert_eq!(dodge.total_damage, 0);
    let parry = build_attacker_state_update(1, 2, 0, 4, 0, 0);
    assert_eq!(parry.hit_info, HitInfo::NormalSwing);
    assert_eq!(parry.damage_state, 3);
    // Glancing (5): a landed hit with reduced damage — HitInfo::Glancing, VictimState NORMAL (1).
    let glancing = build_attacker_state_update(1, 2, 4, 5, 0, 0);
    assert_eq!(glancing.hit_info, HitInfo::Glancing);
    assert_eq!(glancing.damage_state, 1);
    assert_eq!(glancing.total_damage, 4);
    // Block (7): a landed hit whose damage a shield reduced — AffectsVictim (the swing connected, so the
    // being-hit animation plays) + VictimState BLOCK (4) + blocked_amount, the vanilla "Block N" path.
    let block = build_attacker_state_update(1, 2, 5, 7, 25, 0);
    assert_eq!(block.hit_info, HitInfo::AffectsVictim);
    assert_eq!(block.damage_state, 4);
    assert_eq!(block.total_damage, 5);
    assert_eq!(block.blocked_amount, 25);
    // Ranged: a normal shot carries its spell id (75 Auto Shot) in spell_id so the client attributes
    // it to the ability; HitInfo is identical to a melee normal hit (vanilla has no ranged HitInfo flag).
    let ranged = build_attacker_state_update(1, 2, 9, 0, 0, 75);
    assert_eq!(ranged.spell_id, 75);
    assert_eq!(ranged.hit_info, HitInfo::AffectsVictim);
    assert_eq!(ranged.total_damage, 9);
    // The blocked_amount must survive the wire round-trip (a message-level u32, after damage_state).
    let server = ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(block));
    let mut buf = Vec::new();
    server.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(m) => {
            assert_eq!(m.blocked_amount, 25);
            assert_eq!(m.damage_state, 4);
        }
        other => panic!("expected SMSG_ATTACKERSTATEUPDATE, got {other}"),
    }
}

#[test]
fn create_object_builds_unit_for_a_creature() {
    // A creature row (type Unit, no PLAYER bit) must yield ObjectType::Unit + UpdateMask::Unit,
    // not the player descriptor block.
    let chicken = EntityView {
        guid: (0xF130u64 << 48) | (620u64 << 24) | 1,
        map_id: 0,
        type_mask: lyracore_shared::constants::type_mask::CREATURE,
        entry: 620,
        scale_x: 1.0,
        health: 42,
        max_health: 42,
        level: 1,
        run_speed_mult_bp: 10_000,
        faction_template: 31,
        unit_bytes_0: 0x0101, // dummy-but-valid Human/Warrior/Male/Mana
        display_id: 304,
        native_display_id: 304,
        ..Default::default()
    };
    let msg = build_create_object(&chicken, CreateKind::Peer, &[], &[]).unwrap();
    assert_eq!(msg.objects.len(), 1);
    match &msg.objects[0] {
        Object::CreateObject2 {
            guid3,
            object_type,
            mask2,
            ..
        } => {
            assert_eq!(guid3.guid(), chicken.guid);
            assert_eq!(*object_type, ObjectType::Unit);
            assert!(
                matches!(mask2, UpdateMask::Unit(_)),
                "expected a Unit update mask"
            );
        }
        other => panic!("expected CreateObject2, got {other:?}"),
    }
}

#[test]
fn creature_create_carries_npc_flags() {
    // A gossip+vendor NPC (UNIT_NPC_FLAGS 0x1 gossip | 0x4 vendor — vanilla 1.12 numbering, where
    // VENDOR is 0x4, not the 0x80 of later cores) must surface those flags in the Unit mask so the
    // client shows the gossip-eye / vendor cursor (item 6). The packet must also serialize end-to-end.
    let vendor = EntityView {
        guid: (0xF130u64 << 48) | (6741u64 << 24) | 1,
        map_id: 0,
        type_mask: lyracore_shared::constants::type_mask::CREATURE,
        entry: 6741,
        scale_x: 1.0,
        health: 100,
        max_health: 100,
        level: 30,
        run_speed_mult_bp: 10_000,
        faction_template: 12,
        unit_bytes_0: 0x0101,
        display_id: 2068,
        native_display_id: 2068,
        npc_flags: 0x1 | 0x4, // gossip + vendor
        ..Default::default()
    };
    let msg = build_create_object(&vendor, CreateKind::Peer, &[], &[]).unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Unit(u),
            ..
        } => {
            assert_eq!(u.unit_npc_flags(), Some(0x1 | 0x4));
        }
        other => panic!("expected a Unit CreateObject2, got {other:?}"),
    }
    // A player carries no NPC flags (npc_flags 0; the player arm never sets the field).
    let player = build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &player.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.unit_npc_flags(), None);
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_carries_guid_position_speeds() {
    let e = warrior_entity();
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    assert_eq!(msg.objects.len(), 1);
    match &msg.objects[0] {
        Object::CreateObject2 {
            guid3,
            object_type,
            movement2,
            ..
        } => {
            assert_eq!(guid3.guid(), 1);
            assert_eq!(*object_type, ObjectType::Player);
            let living = movement2.update_flag.get_living().expect("living block");
            let MovementBlock_UpdateFlag_Living::Living {
                living_position,
                turn_rate,
                running_speed,
                ..
            } = living
            else {
                panic!("expected Living variant");
            };
            assert_eq!(living_position.x, e.x);
            assert_eq!(*turn_rate, speeds::TURN_RATE);
            assert_eq!(*running_speed, speeds::RUN);
        }
        other => panic!("expected CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_uses_entity_run_speed_multiplier() {
    let mut e = warrior_entity();
    e.run_speed_mult_bp = 30_000; // `.speed 3` survives a destination-shard entity rebuild
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 { movement2, .. } => {
            let living = movement2.update_flag.get_living().expect("living block");
            let MovementBlock_UpdateFlag_Living::Living { running_speed, .. } = living else {
                panic!("expected Living variant");
            };
            assert_eq!(*running_speed, 21.0, "3× the vanilla 7 yd/s run speed");
        }
        other => panic!("expected CreateObject2, got {other:?}"),
    }

    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.is_empty(),
        "the multiplier-derived CREATE must serialize to the wire"
    );
}

#[test]
fn self_and_peer_create_carry_the_self_flag_on_only_the_self_side() {
    // Was WEAK (only asserted `me != peer`, never which side carries SELF). The movement SELF flag
    // is what tells the client "this is MY character" (drives the local-player camera/controls); a
    // peer create must NOT carry it, or the client would fight over which object is itself.
    let e = warrior_entity();
    let me = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    let peer = build_create_object(&e, CreateKind::Peer, &[], &[]).unwrap();
    let self_flag = |msg: &SMSG_UPDATE_OBJECT| match &msg.objects[0] {
        Object::CreateObject2 { movement2, .. } => movement2.update_flag.get_self(),
        other => panic!("expected CreateObject2, got {other:?}"),
    };
    assert!(
        self_flag(&me),
        "a SelfPlayer create must carry the movement SELF flag"
    );
    assert!(
        !self_flag(&peer),
        "a Peer create must NOT carry the SELF flag"
    );
    // Only the movement SELF flag differs; the descriptor mask itself is unaffected.
    assert_ne!(me, peer);
}

#[test]
fn warrior_create_carries_skills_and_serializes() {
    // Skill lines give the spellbook its class tabs; Battle Shout (6673) lives in Fury (256), so
    // without Fury the client throws SpellBookFrame.lua:342 "invalid spell slot". The existing
    // CREATE tests only pattern-match, so SERIALIZE here to actually run the writer over the new
    // PLAYER_SKILL_INFO words (a malformed skill mask would otherwise slip past the suite).
    let msg = build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[]).unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_skill_info(SkillInfoIndex::Index1),
                Some(SkillInfo::new(Skill::Fury, 0, 1, 1, 0, 0)),
                "Fury (256) must anchor the Battle Shout spellbook tab"
            );
            // All five base attributes (UNIT_FIELD_STAT0..4) must survive the serializer so the
            // character sheet reads the cmangos curve values, not zero.
            let e = warrior_entity();
            assert_eq!(p.unit_strength(), Some(e.strength as i32));
            assert_eq!(p.unit_agility(), Some(e.agility as i32));
            assert_eq!(p.unit_stamina(), Some(e.stamina as i32));
            assert_eq!(p.unit_intellect(), Some(e.intellect as i32));
            assert_eq!(p.unit_spirit(), Some(e.spirit as i32));
            // EFFECTIVE armor (UNIT_FIELD_RESISTANCES[0]) is read from `entity.effective_armor` (base +
            // worn gear, folded at the self-login call site; the fixture has no gear → effective == base).
            assert_eq!(p.unit_normal_resistance(), Some(e.effective_armor as i32));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn char_enum_maps_seeded_warrior() {
    let view = CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,
        class: 1,
        gender: 0,
        level: 1,
        map_id: 0,
        zone_id: 12,
        ..Default::default()
    };
    let msg = build_char_enum(&[view]).unwrap();
    assert_eq!(msg.characters.len(), 1);
    assert_eq!(msg.characters[0].name, "Tester");
    assert_eq!(msg.characters[0].guid.guid(), 1);
}

#[test]
fn movement_relay_roundtrips_under_same_opcode() {
    use wow_world_messages::vanilla::MovementInfo_MovementFlags;

    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 99,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        fall_time: 0.0,
    };
    let relay =
        build_movement_relay(movement_opcodes::MSG_MOVE_START_FORWARD, 7, info.clone()).unwrap();

    let mut buf = Vec::new();
    relay.write_unencrypted_server(&mut buf).unwrap();

    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::MSG_MOVE_START_FORWARD(m) => {
            assert_eq!(m.guid.guid(), 7); // mover guid, packed on the wire
            assert_eq!(m.info.position.x, 1.0);
            assert_eq!(m.info.timestamp, 99);
        }
        other => panic!("expected relayed MSG_MOVE_START_FORWARD, got {other}"),
    }

    // A non-relayed opcode yields nothing.
    assert!(build_movement_relay(0x9999, 7, info).is_none());
}

/// Every `MovementInfo` shape the wire can carry: the plain 28-byte block, plus each optional block
/// the flags can switch on (they change the body LENGTH and ORDER, which is exactly what a
/// memcpy-based relay would get wrong if the layout assumption were false).
#[cfg(test)]
fn movement_info_cases() -> Vec<(&'static str, MovementInfo)> {
    use wow_world_messages::vanilla::{
        MovementInfo_MovementFlags, MovementInfo_MovementFlags_Jumping,
        MovementInfo_MovementFlags_OnTransport, MovementInfo_MovementFlags_SplineElevation,
        MovementInfo_MovementFlags_Swimming, TransportInfo,
    };
    let base = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 0x1122_3344,
        position: Vector3d {
            x: -8949.95,
            y: -132.493,
            z: 83.5312,
        },
        orientation: 1.75,
        fall_time: 0.25,
    };
    vec![
        ("still", base.clone()),
        (
            "running forward",
            MovementInfo {
                flags: MovementInfo_MovementFlags::new_forward(),
                ..base.clone()
            },
        ),
        (
            "jumping (a 16-byte block AFTER fall_time)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty()
                    .set_forward()
                    .set_jumping(MovementInfo_MovementFlags_Jumping {
                        cos_angle: 0.5,
                        sin_angle: -0.5,
                        xy_speed: 7.0,
                        z_speed: 3.0,
                    }),
                fall_time: 900.0,
                ..base.clone()
            },
        ),
        (
            "swimming (a 4-byte pitch BEFORE fall_time)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty()
                    .set_swimming(MovementInfo_MovementFlags_Swimming { pitch: -0.3 }),
                ..base.clone()
            },
        ),
        (
            "on a transport (a packed guid nested mid-block)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty().set_on_transport(
                    MovementInfo_MovementFlags_OnTransport {
                        transport: TransportInfo {
                            guid: Guid::new(0x0000_1234_5678_0000),
                            position: Vector3d {
                                x: 1.0,
                                y: 2.0,
                                z: 3.0,
                            },
                            orientation: 0.5,
                            timestamp: 77,
                        },
                    },
                ),
                ..base.clone()
            },
        ),
        (
            "spline elevation (a trailing 4-byte block)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty().set_spline_elevation(
                    MovementInfo_MovementFlags_SplineElevation {
                        spline_elevation: 12.5,
                    },
                ),
                ..base.clone()
            },
        ),
        (
            "swimming + jumping + spline elevation (all three optional blocks at once)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty()
                    .set_forward()
                    .set_swimming(MovementInfo_MovementFlags_Swimming { pitch: 0.9 })
                    .set_jumping(MovementInfo_MovementFlags_Jumping {
                        cos_angle: 1.0,
                        sin_angle: 0.0,
                        xy_speed: 4.5,
                        z_speed: -2.0,
                    })
                    .set_spline_elevation(MovementInfo_MovementFlags_SplineElevation {
                        spline_elevation: -3.25,
                    }),
                fall_time: 1234.5,
                ..base
            },
        ),
    ]
}

/// **The peer-motion relay's load-bearing premise, asserted rather than argued.** The peer-motion relay no
/// longer decodes `game_entity_motion.movement_info` and re-encodes it through gtker; it writes a
/// packed guid and memcpys the stored block. That is only safe if the resulting `(opcode, body)` is
/// what `build_movement_relay` + gtker's own serializer produced — a wrong layout here does not
/// error anywhere, it silently desyncs every peer's position on every client.
///
/// So: for EVERY relayed opcode × every `MovementInfo` shape × a set of guids chosen to exercise the
/// packed-guid encoding (1 byte, high bytes only, all 8 bytes, and the zero pattern), serialize the
/// typed message the old path sent, strip its 4-byte header, and demand the raw builder's opcode and
/// body match byte for byte.
#[test]
fn raw_movement_relay_is_byte_identical_to_the_typed_path() {
    // 1 = one low byte; 0x0100 = one HIGH byte (a zero low byte must be skipped, not written);
    // u64::MAX = all eight; 0 = the empty bit pattern; the last is a realistic player guid.
    const GUIDS: &[u64] = &[1, 0x0100, u64::MAX, 0, 0x0000_00F3_0000_0042];
    let mut compared = 0usize;
    for (label, info) in movement_info_cases() {
        // The bytes the module stores are produced by exactly this call (`WorldStore::movement_update`).
        let stored = movement_info_to_bytes(&info).unwrap();
        for &opcode in movement_opcodes::SLICE_MOVE_OPCODES {
            for &guid in GUIDS {
                let typed = build_movement_relay(opcode, guid, info.clone())
                    .unwrap_or_else(|| panic!("typed relay missing for opcode 0x{opcode:03X}"));
                let mut framed = Vec::new();
                typed.write_unencrypted_server(&mut framed).unwrap();
                // gtker's server framing: [size u16 BE][opcode u16 LE][body...] — the same 4 bytes
                // `Outbound::Raw`'s writer re-creates from `(opcode, body.len())`.
                let typed_opcode = u16::from_le_bytes([framed[2], framed[3]]);
                let typed_body = &framed[4..];
                assert_eq!(
                    u16::from_be_bytes([framed[0], framed[1]]) as usize,
                    2 + typed_body.len(),
                    "gtker's size field is opcode(2)+body — the assumption Outbound::Raw's writer makes"
                );

                let (raw_opcode, raw_body) = build_movement_relay_raw(opcode, guid, &stored)
                    .unwrap_or_else(|| panic!("raw relay missing for opcode 0x{opcode:03X}"));
                assert_eq!(
                    raw_opcode, typed_opcode,
                    "opcode mismatch for 0x{opcode:03X} ({label}, guid {guid:#X})"
                );
                assert_eq!(
                    raw_body, typed_body,
                    "BODY MISMATCH for 0x{opcode:03X} ({label}, guid {guid:#X})\n  raw:   {raw_body:?}\n  typed: {typed_body:?}"
                );
                compared += 1;
            }
        }
    }
    assert_eq!(
        compared,
        7 * 17 * 5,
        "every case × opcode × guid must have been compared"
    );
}

/// The other half of the equivalence: the two builders must accept and REJECT the same opcodes.
/// Exhaustive over the whole u16 opcode space rather than a sample, because the raw path re-derives
/// the accepted set from `is_slice_move` while the typed path spells it out as match arms — two
/// lists that can drift, and a drift means a movement opcode silently stops relaying (peers freeze
/// mid-strafe) or a non-movement one gets framed as movement.
#[test]
fn raw_and_typed_movement_relays_accept_exactly_the_same_opcodes() {
    let info = movement_info_cases()[0].1.clone();
    let stored = movement_info_to_bytes(&info).unwrap();
    for opcode in 0u32..=0xFFFF {
        assert_eq!(
            build_movement_relay(opcode, 7, info.clone()).is_some(),
            build_movement_relay_raw(opcode, 7, &stored).is_some(),
            "the typed and raw movement relays disagree about opcode 0x{opcode:04X}"
        );
    }
}

/// A row whose `movement_info` is too short to be a `MovementInfo` must produce NO packet. The typed
/// path got this for free (the decode failed and logged); the raw path memcpys, so without the
/// length floor a truncated block would be framed with a `size` the client trusts — it would read
/// past the body and desync the header stream for every subsequent packet.
#[test]
fn a_truncated_movement_block_is_never_framed() {
    let info = movement_info_cases()[0].1.clone();
    let stored = movement_info_to_bytes(&info).unwrap();
    assert_eq!(stored.len(), 28, "the minimum MovementInfo body");
    let op = movement_opcodes::MSG_MOVE_HEARTBEAT;
    assert!(
        build_movement_relay_raw(op, 7, &stored).is_some(),
        "a full block relays"
    );
    assert!(
        build_movement_relay_raw(op, 7, &stored[..27]).is_none(),
        "one byte short must not relay"
    );
    assert!(
        build_movement_relay_raw(op, 7, &[]).is_none(),
        "an empty block must not relay"
    );
}

#[test]
fn destroy_object_carries_guid() {
    let msg = build_destroy_object(42);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_DESTROY_OBJECT(m) => assert_eq!(m.guid.guid(), 42),
        other => panic!("expected SMSG_DESTROY_OBJECT, got {other}"),
    }
}

// ---- Items slice-1 ---------------------------------------------------------------------------

/// The hand-authored "Worn Shortsword" template the gateway maps to the wire (mirrors the
/// module seed: a Poor one-hand sword, mainhand-equip, durability 20).
fn worn_shortsword() -> ItemTemplateView {
    ItemTemplateView {
        entry: 25,
        class: 2,    // Weapon
        subclass: 7, // Sword (one-hand)
        sheath: 3,   // the REAL dump value for entry 25 (hip posture on the live client)
        name: "Worn Shortsword".to_string(),
        display_id: 1542,
        quality: 0,         // Poor
        inventory_type: 21, // WeaponMainHand
        item_level: 2,
        required_level: 1,
        max_durability: 20,
        buy_price: 35,
        sell_price: 7,
        max_stack: 1,
        damage_min: 1.0,
        damage_max: 3.0,
        delay_ms: 1900,
        // No stat bonuses on a plain weapon — all stat fields remain zero.
        ..Default::default()
    }
}

/// A simple chest armor with known stat values — used to pin that stat_armor + equip stats
/// reach the wire response (not zeroed out by the codec).
fn blackrock_gauntlets() -> ItemTemplateView {
    ItemTemplateView {
        entry: 1448,
        class: 4,    // Armor
        subclass: 1, // Leather
        name: "Blackrock Gauntlets".to_string(),
        display_id: 7836,
        quality: 1,         // Common
        inventory_type: 10, // INVTYPE_HAND
        item_level: 20,
        required_level: 15,
        max_durability: 30,
        buy_price: 2025,
        sell_price: 405,
        max_stack: 1,
        stat_armor: 105,
        stat_strength: 3,
        bonding: 2, // Bind on Equip (BoE) — greens/blues at 1-10 bind when worn
        ..Default::default()
    }
}

/// A hand-authored BoP (Bind on Pickup) template — a quest reward / starter-kit analogue — so the
/// `bonding=1` wire value is pinned independently of the BoE fixture above.
fn bound_on_pickup_relic() -> ItemTemplateView {
    ItemTemplateView {
        entry: 90001,
        class: 12, // Quest
        subclass: 0,
        name: "Ancient Relic".to_string(),
        display_id: 1542,
        quality: 1,
        inventory_type: 0,
        item_level: 1,
        required_level: 1,
        max_durability: 0,
        buy_price: 0,
        sell_price: 0,
        max_stack: 1,
        bonding: 1, // Bind on Pickup (BoP)
        ..Default::default()
    }
}

#[test]
fn item_query_response_maps_worn_shortsword_and_serializes() {
    // The item query is NOT independently live-verifiable (the 1.12 client only queries an item
    // it already holds an object for), so the typed mapping is pinned here: serialize + read back
    // through the opcode so a malformed field (a bad enum cast, a wrong array len) is caught.
    let t = worn_shortsword();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            assert_eq!(m.item, 25);
            let found = m.found.expect("Worn Shortsword must be found");
            assert_eq!(found.name1, "Worn Shortsword");
            assert_eq!(
                found.class_and_sub_class,
                ItemClassAndSubClass::OneHandedSword
            );
            assert_eq!(found.quality, ItemQuality::Poor);
            assert_eq!(found.inventory_type, InventoryType::WeaponMainHand);
            assert_eq!(found.max_durability, 20);
            assert_eq!(found.item_level, Level::new(2));
            // The per-item dump value rides through (wire 3) instead of the old blanket
            // MainHand — gtker NAMES it LargeWeaponLeft, but the value is what the client reads.
            assert_eq!(found.sheathe_type, SheatheType::LargeWeaponLeft);
            assert_eq!(found.sheathe_type.as_int(), 3);
            assert_eq!(found.damages[0].damage_minimum, 1.0);
            assert_eq!(found.damages[0].damage_maximum, 3.0);
            assert_eq!(found.delay, 1900);
            // RangedModRange: the vanilla-default 100% (NOT the struct Default of 0.0, which zeroed a
            // ranged weapon's effective range → Auto Shot "out of range" everywhere).
            assert_eq!(found.ranged_range_modification, 100.0);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_armor_and_stats_reach_wire() {
    // Pins that stat_armor and equip stats are NOT zeroed out by the codec path — the fix for
    // ItemTemplateView having dropped the stat/armor/block columns. Serialize + read back
    // through the opcode so the full round-trip is verified.
    use wow_world_messages::vanilla::ItemStatType;
    let t = blackrock_gauntlets();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Blackrock Gauntlets must be found");
            assert_eq!(
                found.armor, 105,
                "stat_armor must reach the wire (was 0 before fix)"
            );
            // First stat slot must carry Strength=3; remaining slots padded with Mana=0.
            assert_eq!(
                found.stats[0].stat_type,
                ItemStatType::Strength,
                "first stat pair must be Strength"
            );
            assert_eq!(found.stats[0].value, 3, "Strength value must be 3");
            assert_eq!(
                found.stats[1].value, 0,
                "second stat slot must be padded to 0"
            );
            assert_eq!(found.block, 0, "non-shield block must be 0");
            assert_eq!(
                found.bonding,
                Bonding::Equip,
                "BoE template must carry bonding=2 (Equip) over the wire"
            );
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

/// Item binding: a BoP template's item query carries `bonding=1` (Bonding::PickUp)
/// over the wire — the "Done when" criterion for the feature, pinned as a serialize+read round-trip
/// like the other item-query tests, independent of the BoE fixture above.
#[test]
fn item_query_response_bop_template_carries_bonding_pickup() {
    let t = bound_on_pickup_relic();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Ancient Relic must be found");
            assert_eq!(
                found.bonding,
                Bonding::PickUp,
                "BoP template must carry bonding=1 (PickUp) over the wire"
            );
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

/// Sheathe posture is a RAW passthrough, not a remap: a shield's dump byte (Dented Buckler,
/// entry 1166, `sheath=4`) must reach the wire as 4. gtker names that variant `LargeWeaponRight`,
/// which is why the assertion pins `as_int()` too — the name is noise, the byte is the contract.
/// Pairs with the entry-25 (`sheath=3`) leg in `item_query_response_maps_worn_shortsword_and_serializes`.
#[test]
fn item_query_response_shield_carries_its_own_sheathe_byte() {
    let t = ItemTemplateView {
        entry: 1166,
        class: 4,           // Armor
        subclass: 6,        // Shield
        inventory_type: 14, // Shield
        sheath: 4,          // the REAL dump value for entry 1166
        name: "Dented Buckler".to_string(),
        ..worn_shortsword()
    };
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Dented Buckler must be found");
            assert_eq!(
                found.sheathe_type.as_int(),
                4,
                "shield sheath byte must ride through unremapped"
            );
            assert_eq!(found.sheathe_type, SheatheType::LargeWeaponRight);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

/// The default (unbound) template — every item that predates these columns — must still carry
/// `bonding=0` (NoBind), baseline-safe: an unbound item's tooltip renders no binding line.
#[test]
fn item_query_response_default_bonding_is_no_bind() {
    let t = worn_shortsword(); // bonding left at its ..Default::default() value (0)
    assert_eq!(t.bonding, 0);
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Worn Shortsword must be found");
            assert_eq!(found.bonding, Bonding::NoBind);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

/// A hand-authored resist ring exercising every one of these item-query columns at once: 6 resistances, spell
/// slots 3-5, the skill/reputation gates, maxcount/flags/page_text/startquest/bag_family. Mirrors
/// a real vanilla "... of Fire Resistance" ring.
fn ring_of_fire_resistance() -> ItemTemplateView {
    ItemTemplateView {
        entry: 90002,
        class: 4,    // Armor
        subclass: 0, // Miscellaneous (rings/trinkets/necks)
        name: "Ring of Fire Resistance".to_string(),
        display_id: 5001,
        quality: 2,         // Uncommon (green)
        inventory_type: 11, // INVTYPE_FINGER
        item_level: 15,
        required_level: 10,
        max_durability: 0,
        buy_price: 500,
        sell_price: 100,
        max_stack: 1,
        holy_res: 0,
        fire_res: 12,
        nature_res: 0,
        frost_res: 0,
        shadow_res: 0,
        arcane_res: 0,
        spellid_3: 12345,
        spelltrigger_3: 1, // on-equip
        spellid_4: 12346,
        spelltrigger_4: 2, // chance-on-hit
        spellid_5: 12347,
        spelltrigger_5: 0,   // on-use
        required_skill: 165, // Leatherworking (arbitrary nonzero marker)
        required_skill_rank: 150,
        required_reputation_faction: 69,
        required_reputation_rank: 3,
        max_count: 1,
        item_flags: 0x200, // ItemFlag::WRAPPER bit — any nonzero marker
        page_text: 999,
        start_quest: 777,
        bag_family: 6, // Herbs
        allowed_class: 0x8000_0002,
        allowed_race: 0x8000_0004,
        ..Default::default()
    }
}

/// The "free gateway win" columns: the resistances, spells 3-5, skill/rep gates, and
/// maxcount/flags/page_text/startquest/bag_family the item-query response already had wire slots
/// for (previously hardcoded 0/default) now carry the template's real values end-to-end through a
/// serialize+read round-trip.
#[test]
fn item_query_response_carries_the_work_item_213_columns() {
    let t = ring_of_fire_resistance();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Ring of Fire Resistance must be found");
            assert_eq!(found.holy_resistance, 0);
            assert_eq!(found.fire_resistance, 12, "fire_res must reach the wire");
            assert_eq!(found.nature_resistance, 0);
            assert_eq!(found.frost_resistance, 0);
            assert_eq!(found.shadow_resistance, 0);
            assert_eq!(found.arcane_resistance, 0);
            assert_eq!(
                found.spells[2].spell, 12345,
                "spell slot 3 must reach the wire"
            );
            assert_eq!(found.spells[2].spell_trigger, SpellTriggerType::OnEquip);
            assert_eq!(
                found.spells[3].spell, 12346,
                "spell slot 4 must reach the wire"
            );
            assert_eq!(found.spells[3].spell_trigger, SpellTriggerType::ChanceOnHit);
            assert_eq!(
                found.spells[4].spell, 12347,
                "spell slot 5 must reach the wire"
            );
            assert_eq!(found.spells[4].spell_trigger, SpellTriggerType::OnUse);
            assert_eq!(found.required_skill, Skill::Leatherworking);
            assert_eq!(found.required_skill_rank, 150);
            assert_eq!(
                found.required_faction,
                Faction::Darnassus,
                "required_faction (69) must reach the wire"
            );
            assert_eq!(found.required_faction_rank, 3);
            assert_eq!(
                found.max_count, 1,
                "max_count must reach the wire (was hardcoded 0 before)"
            );
            assert_eq!(
                found.flags,
                ItemFlag::new(0x200),
                "item_flags must reach the wire"
            );
            assert_eq!(found.page_text, 999);
            assert_eq!(found.start_quest, 777);
            assert_eq!(found.bag_family, BagFamily::Herbs);
            assert_eq!(found.allowed_class, AllowedClass::new(0x8000_0002));
            assert_eq!(found.allowed_race, AllowedRace::new(0x8000_0004));
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

/// The default (all-zero) template — every item that predates these columns — keeps every new
/// item-query wire field at its default/empty value: baseline-safe, byte-identical to the
/// response shape before these columns existed.
#[test]
fn item_query_response_default_template_keeps_the_work_item_213_columns_zero() {
    let t = worn_shortsword(); // every new field left at its ..Default::default() value (0)
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Worn Shortsword must be found");
            assert_eq!(found.holy_resistance, 0);
            assert_eq!(found.fire_resistance, 0);
            assert_eq!(found.nature_resistance, 0);
            assert_eq!(found.frost_resistance, 0);
            assert_eq!(found.shadow_resistance, 0);
            assert_eq!(found.arcane_resistance, 0);
            assert_eq!(found.spells[2].spell, 0);
            assert_eq!(found.spells[3].spell, 0);
            assert_eq!(found.spells[4].spell, 0);
            assert_eq!(found.required_skill, Skill::None);
            assert_eq!(found.required_skill_rank, 0);
            assert_eq!(found.max_count, 0);
            assert_eq!(found.flags, ItemFlag::empty());
            assert_eq!(found.page_text, 0);
            assert_eq!(found.start_quest, 0);
            assert_eq!(found.bag_family, BagFamily::None);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_unknown_entry_is_not_found() {
    let msg = build_item_query_response(9999, None);
    assert_eq!(msg.item, 9999);
    assert!(
        msg.found.is_none(),
        "an unknown item must reply NotFound (found: None)"
    );
    // Must still serialize (a NotFound is a valid one-field reply).
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn item_create_object_is_item_typed_and_serializes() {
    let inst = ItemInstanceView {
        guid: (0x4000u64 << 48) | (1 << 8) | 23, // HIGHGUID_ITEM | char-derived low | slot 23
        entry: 25,
        owner_guid: 1,
        slot: 23,
        stack_count: 1,
        durability: 20,
        max_durability: 20,
        container_slots: 0,
    };
    let msg = build_item_create_object(&inst);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            object_type,
            mask2: UpdateMask::Item(it),
            ..
        } => {
            assert_eq!(*object_type, ObjectType::Item);
            assert_eq!(it.object_entry(), Some(25));
            assert_eq!(it.item_owner(), Some(Guid::new(1)));
            assert_eq!(it.item_contained(), Some(Guid::new(1)));
            assert_eq!(it.item_maxdurability(), Some(20));
        }
        other => panic!("expected an Item CreateObject2, got {other:?}"),
    }
}

#[test]
fn player_create_seeds_inventory_slot_descriptor() {
    // A backpack item's guid must land in its inventory slot (ItemSlot::Inventory0 == 23) of the
    // player's PLAYER_FIELD_INV_SLOT descriptors, and a NON-equipment slot carries NO visible item.
    let item_guid = (0x4000u64 << 48) | (1 << 8) | 23;
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[(23, item_guid, 25)],
        &[],
    )
    .unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_field_inv(ItemSlot::Inventory0),
                Some(Guid::new(item_guid)),
                "the item must occupy backpack slot 0"
            );
            assert_eq!(
                p.player_visible_item(VisibleItemIndex::Index15),
                None,
                "a backpack item must NOT set any equipment VISIBLE_ITEM"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn skill_block_reads_learned_rows_override_and_append() {
    // A learned row for a line already in the static class base (Defense 95) OVERRIDES it, a
    // learned profession (Mining 186) APPENDS at its real current/max, an unknown line id is
    // skipped, and the old Cooking fixture is GONE unless actually learned.
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[],
        &[(95, 37, 75), (186, 12, 75), (999_999, 1, 1)],
    )
    .unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            let infos: Vec<_> = (0..=20u8)
                .filter_map(|i| SkillInfoIndex::try_from(i).ok())
                .filter_map(|idx| p.player_skill_info(idx))
                .collect();
            let find = |sk: Skill| infos.iter().find(|si| si.skill == sk);
            let def = find(Skill::Defense).expect("Defense stays in the block");
            assert_eq!(
                (def.minimum, def.maximum),
                (37, 75),
                "learned row overrides the static Defense 1/5"
            );
            let mining = find(Skill::Mining).expect("learned Mining appends");
            assert_eq!((mining.minimum, mining.maximum), (12, 75));
            assert!(
                find(Skill::Cooking).is_none(),
                "the Cooking fixture is gone unless learned"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_carries_the_sheath_state_for_player_and_creature() {
    // #101: the CREATE is how a peer entering AOI range learns a unit's sheath state. Omit
    // UNIT_FIELD_BYTES_2 and everyone who walks up to a player with a drawn sword sees them
    // empty-handed until the next toggle. Byte 0 is the state; bytes 1-3 ride along untouched.
    let mut e = warrior_entity();
    e.unit_bytes_2 = 0xAB_CD_EF_01; // MELEE in byte 0, neighbours occupied
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => assert_eq!(
            p.unit_bytes_2(),
            Some((0x01, 0xEF, 0xCD, 0xAB)),
            "player CREATE must carry UNIT_FIELD_BYTES_2 with the sheath state in byte 0"
        ),
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }

    // Same field on the Unit branch — a creature draws its weapon on engage too.
    let mut c = warrior_entity();
    c.type_mask = lyracore_shared::constants::type_mask::CREATURE;
    c.unit_bytes_2 = 1;
    let msg = build_create_object(&c, CreateKind::Peer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Unit(u),
            ..
        } => assert_eq!(
            u.unit_bytes_2(),
            Some((1, 0, 0, 0)),
            "creature CREATE must carry the sheath state too"
        ),
        other => panic!("expected a Unit CreateObject2, got {other:?}"),
    }
}

#[test]
fn player_create_equips_mainhand_renders_visible_item() {
    // Slice-2: a weapon in the main-hand equipment slot (15) must set BOTH the inv-slot guid AND
    // PLAYER_VISIBLE_ITEM[15] to the item ENTRY, so the 3D model renders the weapon.
    let item_guid = (0x4000u64 << 48) | (1 << 8) | 15;
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[(15, item_guid, 25)],
        &[],
    )
    .unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_field_inv(ItemSlot::MainHand),
                Some(Guid::new(item_guid)),
                "the weapon must occupy the main-hand equipment slot"
            );
            let vis = p
                .player_visible_item(VisibleItemIndex::Index15)
                .expect("main-hand VISIBLE_ITEM must be set so the model renders the weapon");
            assert_eq!(
                vis.item, 25,
                "VISIBLE_ITEM must carry the item entry (drives the model)"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn gossip_message_echoes_guid_with_a_greeting_option() {
    let menu = |is_vendor, is_innkeeper| gossip_menu_options(Vec::new(), is_vendor, is_innkeeper);
    let m = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(false, false),
    );
    assert_eq!(m.guid.guid(), 0xF130_0000_0000_0001); // echoes the NPC the player talked to
    assert_eq!(m.title_text_id, GOSSIP_GREETING_TEXT_ID); // resolved via CMSG_NPC_TEXT_QUERY
                                                          // Guard the McBride fix: the npc_text id MUST be low/in-range. A high synthetic id (top byte 0xFF,
                                                          // the old 0xFFFF_FF01) makes the 5875 client silently drop the gossip packet (no GOSSIP_SHOW).
    const {
        assert!(
            GOSSIP_GREETING_TEXT_ID <= 0xFF_FFFF,
            "title_text_id must be a low, in-range npc_text id"
        )
    };
    assert_eq!(m.gossips.len(), 1); // non-vendor: one clickable line so the window isn't empty
    assert_eq!(m.gossips[0].message, "Farewell.");
    assert!(!m.gossips[0].coded);
    // A vendor NPC gets a "browse goods" option FIRST (index 0), then Farewell.
    let v = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(true, false),
    );
    assert_eq!(v.gossips.len(), 2);
    assert_eq!(v.gossips[0].id, 0);
    assert_eq!(v.gossips[0].item_icon, 1); // vendor bag icon
    assert_eq!(v.gossips[1].message, "Farewell.");
    assert!(m.quests.is_empty()); // a plain gossip NPC (no quests passed) is unchanged

    // INNKEEPER bit numbering (cmangos 1.12): the gate reads `npc_flags & 0x80`. Innkeeper Farley(295)
    // ships npc_flags=135; assert the corrected constant gates true while VENDOR (0x4) still passes — the
    // old WRONG literal (0x10000) made `135 & 0x10000 == 0`, silently dropping the "make home" option.
    use lyracore_shared::constants::npc_flags;
    assert_eq!(
        npc_flags::INNKEEPER,
        0x80,
        "cmangos 1.12 INNKEEPER is 0x80, not 0x10000"
    );
    const {
        assert!(
            135 & npc_flags::INNKEEPER != 0,
            "Farley(295) npc_flags=135 must gate as an innkeeper"
        )
    };
    assert_eq!(
        135 & npc_flags::VENDOR,
        0x4,
        "the VENDOR (0x4) bit still passes — no regression"
    );

    // Innkeeper "make home" lands at slot 0 on a non-vendor, and each id matches its position (the
    // value the client echoes back).
    let inn = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(false, true),
    );
    assert_eq!(inn.gossips[0].message, "Make this inn your home.");
    assert_eq!(inn.gossips[0].id, 0);
    // Vendor + innkeeper → browse goods (0), make home (1), Farewell (2).
    let both = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(true, true),
    );
    assert_eq!(both.gossips.len(), 3);
    assert_eq!(both.gossips[1].message, "Make this inn your home.");
    assert_eq!(both.gossips[1].id, 1);
}

/// Byte-parity pin: an NPC with NO imported options must produce EXACTLY the same gossip window as
/// before the option-import feature existed — the flag-derived synthesis must never silently diverge.
#[test]
fn gossip_message_with_no_imports_is_byte_identical_to_pre_217() {
    let guid = 0xF130_0000_0000_0042u64;
    let m = build_gossip_message(
        guid,
        4,
        Vec::new(),
        &gossip_menu_options(Vec::new(), true, true),
    );
    assert_eq!(m.gossips.len(), 3); // vendor + innkeeper + Farewell, exactly as pre-import
    assert_eq!(m.gossips[0].message, "I'd like to browse your goods.");
    assert_eq!(m.gossips[1].message, "Make this inn your home.");
    assert_eq!(m.gossips[2].message, "Farewell.");
}

#[test]
fn gossip_message_renders_imported_options_verbatim_with_a_trailing_farewell() {
    use lyracore_shared::constants::gossip_option;
    // An imported innkeeper NPC whose menu already covers both flags: browse goods + make-home + chat,
    // rendered VERBATIM from the dump. Nothing is synthesized on top.
    let opt = |icon, text: &str, action| GossipOptionView {
        icon,
        text: text.into(),
        action,
        ..Default::default()
    };
    let opts = vec![
        opt(0, "Care for a chat?", gossip_option::GOSSIP),
        opt(1, "Let me see your goods.", gossip_option::VENDOR),
        opt(0, "I'd like to stay here a while.", gossip_option::INNKEEPER),
    ];
    let m = build_gossip_message(
        1,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &gossip_menu_options(opts, true, true),
    );
    assert_eq!(m.gossips.len(), 4, "3 imported + a trailing Farewell");
    assert_eq!(m.gossips[0].message, "Care for a chat?");
    assert_eq!(m.gossips[0].id, 0);
    assert_eq!(m.gossips[1].message, "Let me see your goods.");
    assert_eq!(m.gossips[1].item_icon, 1);
    assert_eq!(m.gossips[2].message, "I'd like to stay here a while.");
    assert_eq!(m.gossips[3].message, "Farewell.");
    assert_eq!(m.gossips[3].id, 3);
}

/// LIVE VICTIMS of the old imports-suppress-synthesis rule: Orphan Matron Nightingale (stock, no
/// vendor row in her menu) and Katie Hunter (innkeeper, no bind row). The synthesized line must be
/// APPENDED to an imported menu that lacks it, and never duplicate one that has it.
#[test]
fn gossip_menu_options_appends_only_the_missing_flag_lines() {
    use lyracore_shared::constants::gossip_option;
    let chat = GossipOptionView {
        row_id: 7,
        text: "Care for a chat?".into(),
        action: gossip_option::GOSSIP,
        ..Default::default()
    };
    // Vendor with stock but no vendor row → browse-goods appended, marked as synthesized.
    let merged = gossip_menu_options(vec![chat.clone()], true, false);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].row_id, 7, "the imported row keeps its identity");
    assert_eq!(merged[1].text, "I'd like to browse your goods.");
    assert_eq!(merged[1].action, gossip_option::VENDOR);
    assert_eq!(merged[1].row_id, SYNTHESIZED_ROW_ID);
    // Innkeeper with no bind row → make-home appended.
    let merged = gossip_menu_options(vec![chat.clone()], false, true);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].action, gossip_option::INNKEEPER);
    // A menu that already carries the row is untouched — no duplicate line.
    let has_vendor = GossipOptionView {
        row_id: 9,
        text: "Let me see your goods.".into(),
        action: gossip_option::VENDOR,
        ..Default::default()
    };
    let merged = gossip_menu_options(vec![chat, has_vendor], true, false);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].text, "Let me see your goods.");
}

#[test]
fn option_condition_holds_gates_on_quest_status_and_fails_open_for_unknown_types() {
    use lyracore_shared::constants::gossip_condition;
    assert!(option_condition_holds(gossip_condition::NONE, false, false));
    assert!(option_condition_holds(
        gossip_condition::QUEST_TAKEN,
        true,
        false
    ));
    assert!(!option_condition_holds(
        gossip_condition::QUEST_TAKEN,
        false,
        false
    ));
    assert!(option_condition_holds(
        gossip_condition::QUEST_REWARDED,
        false,
        true
    ));
    assert!(!option_condition_holds(
        gossip_condition::QUEST_REWARDED,
        true,
        false
    ));
    // An unrecognized cond_type must fail OPEN (never silently hide an option).
    assert!(option_condition_holds(99, false, false));
}

#[test]
fn cast_result_failed_body_and_reason_map() {
    // The raw failure body is spell(u32 LE) + 0x02 (CAST_FAILED) + the CastFailureReason byte.
    assert_eq!(
        build_cast_result_failed(100, 0x4D),
        vec![100, 0, 0, 0, 0x02, 0x4D]
    );
    // The substring map covers the module's stable gate phrasings; unknown text falls back to
    // DontReport (0x17 — silent reset, never a wrong red line).
    assert_eq!(
        cast_failure_reason_for("not enough power: have 0, need 150"),
        0x4D
    );
    assert_eq!(
        cast_failure_reason_for("spell not ready (global cooldown)"),
        0x3C
    );
    assert_eq!(
        cast_failure_reason_for("target out of range (32.0 yd > 30 + 0.0 yd leeway)"),
        0x59
    );
    assert_eq!(
        cast_failure_reason_for("spell 53 must be cast from BEHIND the target (99)"),
        0x33
    );
    assert_eq!(
        cast_failure_reason_for("spell 1784 requires stealth (caster 9 is not stealthed)"),
        0x57
    );
    assert_eq!(
        cast_failure_reason_for("spell 100 cannot be cast in the caster's current stance (1)"),
        0x3D
    );
    assert_eq!(
        cast_failure_reason_for(
            "spell 7384 requires an Overpower window (the target must have dodged your swing)"
        ),
        0x12
    );
    assert_eq!(cast_failure_reason_for("some future gate wording"), 0x17);
}

#[test]
fn spell_damage_log_renders_crit_and_breakdown() {
    // A crit cast → hit_info encodes 0x2 (SPELL_HIT_TYPE_CRIT — gtker's AffectsVictim variant; the
    // MELEE-named CriticalHit encodes 0x80, which the client ignores in THIS packet — the 114 live
    // find: a 22-damage Heroic Strike crit rendered plain). Breakdown fields propagate unchanged.
    let crit = build_spell_non_melee_damage_log(0x42, 0x7, 133, 30, 4, true, 5, 3);
    assert_eq!(crit.hit_info, HitInfo::AffectsVictim);
    assert_eq!(crit.hit_info.as_int(), 0x2); // the wire truth this test actually guards
    assert_eq!(crit.damage, 30);
    assert_eq!(crit.resisted, 5);
    assert_eq!(crit.absorbed_damage, 3);
    assert_eq!(crit.blocked, 0); // spells aren't blocked in vanilla
    assert_eq!(crit.school, SpellSchool::Frost); // school index 4 = Frost
                                                 // A normal (non-crit) cast stays NormalSwing with a zeroed breakdown.
    let normal = build_spell_non_melee_damage_log(0x42, 0x7, 133, 20, 0, false, 0, 0);
    assert_eq!(normal.hit_info, HitInfo::NormalSwing);
    assert_eq!(normal.resisted, 0);
    assert_eq!(normal.absorbed_damage, 0);
    assert_eq!(normal.school, SpellSchool::Normal); // index 0 → Normal/physical
}

#[test]
fn gossip_message_body_is_exact_5875_layout() {
    // Pin the EXACT 5875 SMSG_GOSSIP_MESSAGE body for a gossip with 1 option + 1 quest, proving the
    // gtker typed message is byte-faithful to vanilla 1.12.1 (no RAW builder needed — unlike loot,
    // gtker's vanilla GossipItem is the correct shape; the TBC BoxMoney/BoxText fields are absent).
    // The McBride bug was the title_text_id VALUE (0xFFFF_FF01, an out-of-range npc_text id the client
    // refuses), not the wire shape. This locks the layout so a future gtker bump that adds the TBC
    // fields would fail here. Body (all LE), per the 1.12 gossip-menu wire layout:
    //   guid u64 | title_text_id u32 | gossip_count u32 |
    //   per gossip: id u32, icon u8, coded u8, message CString |
    //   quest_count u32 | per quest: quest_id u32, quest_icon u32, level u32, title CString
    let guid = 0xF130_0000_0000_0001u64;
    let msg = SMSG_GOSSIP_MESSAGE {
        guid: Guid::new(guid),
        title_text_id: GOSSIP_GREETING_TEXT_ID, // == 1 after the fix (low, in-range npc_text id)
        gossips: vec![GossipItem {
            id: 0,
            item_icon: 1,
            coded: false,
            message: "Hi".to_string(),
        }],
        quests: vec![QuestItem {
            quest_id: 0x29A, // 666
            quest_icon: 7,   // available/turn-in icon
            level: Level::new(3),
            title: "Q".to_string(),
        }],
    };
    // Frame it the way the writer does, then strip the 4-byte server header to get the body.
    let mut framed = Vec::new();
    ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(Box::new(msg))
        .write_unencrypted_server(&mut framed)
        .unwrap();
    assert_eq!(
        u16::from_le_bytes([framed[2], framed[3]]),
        0x017D,
        "opcode == SMSG_GOSSIP_MESSAGE"
    );
    let body = &framed[4..];

    let mut want: Vec<u8> = Vec::new();
    want.extend_from_slice(&guid.to_le_bytes()); // guid: u64
    want.extend_from_slice(&1u32.to_le_bytes()); // title_text_id: u32 (== 1, the fix)
    want.extend_from_slice(&1u32.to_le_bytes()); // amount_of_gossip_items: u32
                                                 // gossip item: id u32 | icon u8 | coded u8 | message CString — NO BoxMoney/BoxText (those are TBC)
    want.extend_from_slice(&0u32.to_le_bytes()); // id
    want.push(1u8); // item_icon
    want.push(0u8); // coded (false)
    want.extend_from_slice(b"Hi\0"); // message CString
    want.extend_from_slice(&1u32.to_le_bytes()); // amount_of_quests: u32
                                                 // quest item: quest_id u32 | quest_icon u32 | level u32 (full, not u8) | title CString
    want.extend_from_slice(&0x29Au32.to_le_bytes()); // quest_id
    want.extend_from_slice(&7u32.to_le_bytes()); // quest_icon
    want.extend_from_slice(&3u32.to_le_bytes()); // level: Level32 (a full u32, like SMSG_QUESTGIVER_QUEST_LIST)
    want.extend_from_slice(b"Q\0"); // title CString

    assert_eq!(
        body,
        want.as_slice(),
        "gossip body must be the exact 5875 layout\n got {:02x?}\nwant {:02x?}",
        body,
        want
    );
}

#[test]
fn npc_text_update_carries_the_greeting_in_slot_0() {
    // None → generic fallback
    let t = build_npc_text_update(GOSSIP_GREETING_TEXT_ID, None);
    assert_eq!(t.text_id, GOSSIP_GREETING_TEXT_ID);
    assert_eq!(t.texts.len(), 8); // fixed-size NpcText array
    assert!(t.texts[0].texts[0].contains("Greetings")); // the greeting lives in slot 0...
    assert_eq!(t.texts[0].probability, 1.0); // ...at probability 1.0 so the client picks it
    assert_eq!(t.texts[1].probability, 0.0); // the other 7 slots are default/empty
                                             // Some(imported view) → overrides the generic greeting, single-slot shape (back-compat).
    let mut view = NpcTextView::default();
    view.slots[0] = (
        "Hey, citizen!".to_string(),
        "Hey, citizen!".to_string(),
        1.0,
    );
    let t2 = build_npc_text_update(4938, Some(&view));
    assert_eq!(t2.text_id, 4938);
    assert_eq!(t2.texts[0].texts[0], "Hey, citizen!");
    assert_eq!(t2.texts[0].probability, 1.0);
}

/// All 8 weighted slots ship verbatim (male/female/probability each), not just slot 0 —
/// the CLIENT does the weighted pick, so the server must never collapse this to a single string.
#[test]
fn npc_text_update_ships_every_populated_slot_with_its_own_probability() {
    let mut view = NpcTextView::default();
    view.slots[0] = (
        "Well met.".to_string(),
        "Well met, traveler.".to_string(),
        0.6,
    );
    view.slots[3] = (
        "Watch yourself.".to_string(),
        "Watch yourself.".to_string(),
        0.4,
    );
    let t = build_npc_text_update(77, Some(&view));
    assert_eq!(
        t.texts[0].texts,
        ["Well met.".to_string(), "Well met, traveler.".to_string()]
    );
    assert_eq!(t.texts[0].probability, 0.6);
    assert_eq!(
        t.texts[3].texts,
        ["Watch yourself.".to_string(), "Watch yourself.".to_string()]
    );
    assert_eq!(t.texts[3].probability, 0.4);
    // Untouched slots stay default (empty text, 0 probability) — the client never selects them.
    assert_eq!(t.texts[1].probability, 0.0);
    assert!(t.texts[1].texts[0].is_empty());
}

#[test]
fn list_inventory_raw_body_is_exact_vanilla_layout() {
    // SMSG_LIST_INVENTORY is RAW (gtker's typed message is the tbc/wrath shape, with an extra
    // ExtendedCost u32 per item the 1.12 client doesn't read). Pin the opcode + the exact body for
    // a vendor + one item, built by hand from the vanilla layout (all LE):
    //   vendor_guid u64 | count u8 | per item: muid u32 (1-based) | item_entry u32 | display u32 |
    //   max_count u32 (0 stored → 0xFFFF_FFFF unlimited) | buy_price u32 | max_durability u32 |
    //   buy_count u32 (=1).
    let vendor_guid = 0xF130_0000_0000_0001u64;
    let item = VendorItemView {
        item_entry: 25,
        display_id: 1542,
        buy_price: 35,
        max_durability: 20,
        max_count: 0,
        buy_count: 1, // → 0xFFFF_FFFF on the wire (unlimited)
    };
    let (opcode, body) = build_list_inventory_raw(vendor_guid, &[item]);
    assert_eq!(opcode, 0x019F, "SMSG_LIST_INVENTORY opcode (vanilla 5875)");
    assert_eq!(opcode, SMSG_LIST_INVENTORY_OPCODE);

    let expected: Vec<u8> = vec![
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, // vendor_guid u64 LE
        0x01, // count = 1
        0x01, 0x00, 0x00, 0x00, // muid = 1 (1-based slot index)
        0x19, 0x00, 0x00, 0x00, // item_entry = 25
        0x06, 0x06, 0x00, 0x00, // display_id = 1542
        0xFF, 0xFF, 0xFF, 0xFF, // max_count: 0 → unlimited (0xFFFF_FFFF)
        0x23, 0x00, 0x00, 0x00, // buy_price = 35
        0x14, 0x00, 0x00, 0x00, // max_durability = 20
        0x01, 0x00, 0x00, 0x00, // buy_count = 1
    ];
    assert_eq!(
        body, expected,
        "raw SMSG_LIST_INVENTORY body must match the vanilla layout byte-for-byte"
    );

    // An empty stock is the vendor guid + a zero count + the error byte the client reads INSTEAD of
    // an item block. Without that trailing byte it parses past the end and shows no window at all.
    let (_op, empty) = build_list_inventory_raw(vendor_guid, &[]);
    assert_eq!(
        empty,
        vec![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, 0x00, 0x00],
        "empty vendor body is guid + count 0 + error byte"
    );
}

#[test]
fn list_inventory_raw_truncates_stock_to_255_items() {
    // The wire count is a u8; feeding more stock lines than that must never silently wrap the count
    // byte (300 % 256 == 44) — the slice is capped so the count and the item bytes always agree.
    let items: Vec<VendorItemView> = (0..300u32)
        .map(|i| VendorItemView {
            item_entry: i,
            display_id: 1,
            buy_price: 1,
            max_durability: 1,
            max_count: 1,
            buy_count: 1,
        })
        .collect();
    let (_op, body) = build_list_inventory_raw(1, &items);
    assert_eq!(body[8], 255, "the count byte must cap at 255");
    assert_eq!(
        body.len(),
        9 + 255 * 28,
        "envelope(9) + 255 items * 28 bytes each"
    );
}

// ---- P1: values.rs OBJECT_FIELD_TYPE-absence pins (the null+0x110 crash class) -----------------
//
// The remaining `build_*_values` builders share the same `dirty_reset` discipline as
// `build_health_values` above (finalize seeds OBJECT_FIELD_TYPE dirty; `dirty_reset` strips it; only
// the changed field(s) are re-applied). Each of these was previously unpinned.

#[test]
fn unit_flags_values_is_unit_only_no_object_type() {
    // The combat-indicator relay (UNIT_FLAG_IN_COMBAT toggling live) must ride a Unit VALUES mask
    // carrying ONLY UNIT_FIELD_FLAGS — never re-sending OBJECT_FIELD_TYPE (the 5875 crash field).
    let guid = 7u64;
    let flags = 0x0008_0000u32; // a recognizable flag value, distinct from the TYPE value word (0x09)
    let msg = build_unit_flags_values(guid, flags);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let contains = |n: &[u8]| buf.windows(n.len()).any(|w| w == n);
    assert!(
        contains(&[0x00, 0x00, 0x08, 0x00]),
        "expected the unit_flags value; buf {:02x?}",
        buf
    );
    assert!(
        !contains(&[0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the unit_flags VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn power_values_selects_the_descriptor_by_power_byte() {
    // power_b picks which UNIT_FIELD_POWERn the current-power value lands on, matching
    // build_create_object's own power_b -> descriptor mapping (mana=0, rage=1, focus=2, energy=3,
    // happiness=4).
    let cases: [(u8, i32); 5] = [(0, 111), (1, 222), (2, 333), (3, 444), (4, 555)];
    for (power_b, cur) in cases {
        let msg = build_power_values(1, power_b, cur as u32);
        match &msg.objects[0] {
            Object::Values {
                mask1: UpdateMask::Player(p),
                ..
            } => {
                let got = match power_b {
                    1 => p.unit_power2(),
                    2 => p.unit_power3(),
                    3 => p.unit_power4(),
                    4 => p.unit_power5(),
                    _ => p.unit_power1(),
                };
                assert_eq!(
                    got,
                    Some(cur),
                    "power_b={power_b} must land on its own descriptor"
                );
            }
            other => panic!("expected a Player Values update, got {other:?}"),
        }
    }
    let msg = build_power_values(1, 1, 222);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.windows(4).any(|w| w == [0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the power VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn target_values_is_unit_only_no_object_type() {
    let (guid, target) = (1u64, 555u64);
    let msg = build_target_values(guid, target);
    match &msg.objects[0] {
        Object::Values {
            guid1,
            mask1: UpdateMask::Unit(u),
        } => {
            assert_eq!(guid1.guid(), guid);
            assert_eq!(u.unit_target(), Some(Guid::new(target)));
        }
        other => panic!("expected a Unit Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.windows(4).any(|w| w == [0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the target VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn max_vitals_values_sets_maxhealth_and_selects_max_power_descriptor() {
    let cases: [(u8, i32); 5] = [(0, 1001), (1, 1002), (2, 1003), (3, 1004), (4, 1005)];
    for (power_b, max_power) in cases {
        let msg = build_max_vitals_values(1, 777, power_b, max_power as u32);
        match &msg.objects[0] {
            Object::Values {
                mask1: UpdateMask::Unit(u),
                ..
            } => {
                assert_eq!(u.unit_maxhealth(), Some(777));
                let got = match power_b {
                    1 => u.unit_maxpower2(),
                    2 => u.unit_maxpower3(),
                    3 => u.unit_maxpower4(),
                    4 => u.unit_maxpower5(),
                    _ => u.unit_maxpower1(),
                };
                assert_eq!(
                    got,
                    Some(max_power),
                    "power_b={power_b} must land on its own max-power descriptor"
                );
            }
            other => panic!("expected a Unit Values update, got {other:?}"),
        }
    }
    let msg = build_max_vitals_values(1, 777, 0, 1001);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.windows(4).any(|w| w == [0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the max-vitals VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn player_ammo_id_values_is_player_only_no_object_type() {
    let msg = build_player_ammo_id_values(1, 2512); // an arrow entry
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.player_ammo_id(), Some(2512));
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.windows(4).any(|w| w == [0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the ammo VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn player_xp_values_carries_xp_and_next_level_no_object_type() {
    let msg = build_player_xp_values(1, 450, 1000);
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.player_xp(), Some(450));
            assert_eq!(p.player_next_level_xp(), Some(1000));
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.windows(4).any(|w| w == [0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the xp VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn item_values_clamps_zero_stack_count_to_one_and_carries_durability() {
    // A stack count of 0 is nonsensical (an item object always represents at least one item) —
    // clamp it to 1 rather than emit a wire value the client would render as an empty stack.
    let msg = build_item_values(1, 0, 15);
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Item(it),
            ..
        } => {
            assert_eq!(
                it.item_stack_count(),
                Some(1),
                "a zero stack count must clamp to 1"
            );
            assert_eq!(it.item_durability(), Some(15));
        }
        other => panic!("expected an Item Values update, got {other:?}"),
    }
    // A real stack count survives unclamped.
    let stacked = build_item_values(1, 5, 15);
    match &stacked.objects[0] {
        Object::Values {
            mask1: UpdateMask::Item(it),
            ..
        } => assert_eq!(it.item_stack_count(), Some(5)),
        other => panic!("expected an Item Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.windows(4).any(|w| w == [0x09, 0x00, 0x00, 0x00]),
        "OBJECT_FIELD_TYPE (0x09) must not be re-sent in the item VALUES; buf {:02x?}",
        buf
    );
}

#[test]
fn container_slot_values_computes_the_field_index_from_the_bag_slot() {
    // CONTAINER_FIELD_SLOT_1 = 50, each slot spans 2 field indices (a u64 guid) — slot 2 must land
    // at field 50 + 2*2 = 54. Cross-checked against the raw envelope directly, the same discipline
    // as raw_values_body_matches_gtker_envelope.
    use super::update_mask::{self, idx};
    let (bag_guid, item_guid) = (0x4000_0000_0000_0001u64, 0x4000_0000_0000_0099u64);
    let (opcode, body) = build_container_slot_values(bag_guid, 2, item_guid);
    assert_eq!(opcode, 0x00A9, "SMSG_UPDATE_OBJECT opcode");

    let mut want_mask = update_mask::UpdateMaskValues::new();
    want_mask.set_u64(idx::CONTAINER_FIELD_SLOT_1 + 2 * 2, item_guid);
    let (_op, want_body) = build_values_update_raw(bag_guid, &want_mask);
    assert_eq!(
        body, want_body,
        "slot 2 must set field index 54 (CONTAINER_FIELD_SLOT_1 + 2*2)"
    );

    // Passing 0 CLEARS the slot pointer — a real zero guid on the wire, not a no-op.
    let (_op2, cleared) = build_container_slot_values(bag_guid, 2, 0);
    let mut clear_mask = update_mask::UpdateMaskValues::new();
    clear_mask.set_u64(idx::CONTAINER_FIELD_SLOT_1 + 2 * 2, 0);
    let (_op3, want_cleared) = build_values_update_raw(bag_guid, &clear_mask);
    assert_eq!(cleared, want_cleared);
}

#[test]
fn inv_slot_values_sets_the_slot_pointer_or_none_for_an_invalid_slot() {
    let item_guid = (0x4000u64 << 48) | (1 << 8) | 23;
    let msg = build_inv_slot_values(1, 23, item_guid).expect("slot 23 (Inventory0) is valid");
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_field_inv(ItemSlot::Inventory0),
                Some(Guid::new(item_guid))
            );
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    // Clearing a slot passes guid 0 — a real (zeroed) pointer, not a skipped update.
    let cleared = build_inv_slot_values(1, 23, 0).unwrap();
    match &cleared.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.player_field_inv(ItemSlot::Inventory0), Some(Guid::new(0)));
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    // An out-of-range slot ordinal degrades to None rather than panicking.
    assert!(build_inv_slot_values(1, 255, item_guid).is_none());
}

#[test]
fn aura_duration_carries_the_slot_and_remaining_window() {
    let msg = build_aura_duration(3, 12_000);
    assert_eq!(msg.aura_slot, 3);
    assert_eq!(msg.aura_duration, 12_000);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

// ---- P1: entity.rs login_sequence_messages + build_create_object derived combat numbers --------

#[test]
fn login_sequence_dedupes_known_spells_and_builds_the_action_bar() {
    // ATTACK_SPELL (6603) is always granted first; a duplicate of it (and of a learned id) must not
    // appear twice in either the spellbook or the action bar.
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[6603, 100, 100, 200],
        &[],
        &[],
        WorldEntry::FreshLogin,
    )
    .unwrap();
    let spells = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_INITIAL_SPELLS(s) => Some(s),
            _ => None,
        })
        .expect("SMSG_INITIAL_SPELLS must be in the sequence");
    let ids: Vec<u16> = spells.initial_spells.iter().map(|s| s.spell_id).collect();
    assert_eq!(
        ids,
        vec![6603, 100, 200],
        "duplicates of ATTACK_SPELL and a learned id must be deduped"
    );

    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .expect("SMSG_ACTION_BUTTONS must be in the sequence");
    assert_eq!(&bar.data[0..3], &[6603, 100, 200]);
    assert!(
        bar.data[3..].iter().all(|&b| b == 0),
        "unused action-bar slots stay zero"
    );
}

#[test]
fn login_sequence_sends_verify_world_at_fresh_login_only() {
    // A fresh login starts with SMSG_LOGIN_VERIFY_WORLD; a world-port re-entry must not carry it
    // (a resend commands a second map load). Everything else is identical.
    let fresh =
        login_sequence_messages(&warrior_entity(), &[], &[], &[], WorldEntry::FreshLogin).unwrap();
    assert!(
        matches!(
            fresh.first(),
            Some(ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(_))
        ),
        "a fresh login's sequence must START with SMSG_LOGIN_VERIFY_WORLD"
    );

    let port =
        login_sequence_messages(&warrior_entity(), &[], &[], &[], WorldEntry::WorldPort).unwrap();
    assert!(
        !port
            .iter()
            .any(|m| matches!(m, ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(_))),
        "a world-port re-entry must never resend SMSG_LOGIN_VERIFY_WORLD"
    );
    assert_eq!(
        port.len(),
        fresh.len() - 1,
        "the ONLY difference between the two sequences is the dropped verify-world"
    );
}

#[test]
fn login_sequence_caps_the_action_bar_at_120_slots_but_not_the_spellbook() {
    // 130 distinct learned spells (+ the always-granted ATTACK_SPELL) push the known-spell count past
    // the fixed 120-slot action bar array; SMSG_INITIAL_SPELLS must still list every one of them, while
    // SMSG_ACTION_BUTTONS truncates to the first 120 (no out-of-bounds write into the fixed array).
    let learned: Vec<u32> = (1000..1130).collect(); // 130 unique ids
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &learned,
        &[],
        &[],
        WorldEntry::FreshLogin,
    )
    .unwrap();
    let spells = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_INITIAL_SPELLS(s) => Some(s),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        spells.initial_spells.len(),
        131,
        "ATTACK_SPELL + all 130 learned spells must be known"
    );

    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        bar.data.len(),
        120,
        "the action-bar array is fixed at 120 slots"
    );
    assert_eq!(bar.data[0], 6603, "slot 0 is always the Attack toggle");
    assert_eq!(
        bar.data[119], 1118,
        "slot 119 is the 120th known spell (known[119] = learned[118])"
    );
}

#[test]
fn login_sequence_folds_reputation_by_index_and_ignores_out_of_range_slots() {
    // reputation_index must address the SAME Faction.dbc ReputationListID slot the live
    // SET_FACTION_STANDING relay uses (0..63); an index >= 64 has no slot and must be dropped
    // silently rather than panicking (a corrupt/import-drifted row must never crash login).
    let reps = [(5, 100, false), (63, -250, true), (64, 999, false)];
    let msgs = login_sequence_messages(&warrior_entity(), &[], &reps, &[], WorldEntry::FreshLogin)
        .unwrap();
    let factions = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(f) => Some(f),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        factions.factions.len(),
        64,
        "the client's fixed 64-slot rep array"
    );
    assert_eq!(factions.factions[5].standing, 100);
    assert_eq!(
        factions.factions[63].standing as i32, -250,
        "a negative standing bit-cast through u32"
    );
    // The persisted At-War checkbox rides the flag byte — VISIBLE|AT_WAR (0x03) on the
    // flagged slot, empty everywhere else (a false at_war never writes flags).
    assert!(
        factions.factions[63].flag.is_at_war(),
        "slot 63 carries AT_WAR"
    );
    assert!(
        factions.factions[63].flag.is_visible(),
        "AT_WAR rides with VISIBLE (the pane row must render)"
    );
    assert!(
        !factions.factions[5].flag.is_at_war(),
        "an at_war=false row leaves the flag byte empty"
    );
    for i in (0..64).filter(|&i| i != 5 && i != 63) {
        assert_eq!(
            factions.factions[i].standing, 0,
            "slot {i} must stay at the default 0 standing"
        );
    }
}

// ---- Imported action-bar rows vs. the synth fallback ----------------------------

/// The BYTE-IDENTICAL synth fallback pin: with NO `game_player_action` rows (pre-import, the state
/// every existing deployment is in today), the action bar must build EXACTLY as it did before this
/// feature — Attack (6603) in slot 0, then each known spell, zeros elsewhere. This is a regression
/// pin on `login_sequence_dedupes_known_spells_and_builds_the_action_bar` above (same fixture, spelled
/// out again here so a future edit to the imported-bar branch can't silently start also touching the
/// fallback branch it must leave alone).
#[test]
fn login_sequence_empty_player_actions_is_byte_identical_to_the_pre_212_synth() {
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[100, 200],
        &[],
        &[],
        WorldEntry::FreshLogin,
    )
    .unwrap();
    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .expect("SMSG_ACTION_BUTTONS must be in the sequence");
    assert_eq!(
        &bar.data[0..3],
        &[6603, 100, 200],
        "Attack + known spells, unpacked (type 0 implicit)"
    );
    assert!(bar.data[3..].iter().all(|&b| b == 0));
}

/// Once the character has IMPORTED `game_player_action` rows, the bar builds ENTIRELY from them —
/// packed as `action | (action_type << 24)` — and the synth (Attack + spellbook) is not consulted at
/// all, so a button the import omits (e.g. no button 0 row) stays 0, not auto-filled with Attack.
#[test]
fn login_sequence_builds_the_bar_from_imported_player_actions_packing_the_type_byte() {
    let rows = [
        (0u8, 6603u32, 0u8), // button 0: Attack, type 0 (spell) — packs as the raw id
        (1u8, 78u32, 0u8),   // button 1: Heroic Strike, type 0
        (5u8, 1234u32, 4u8), // button 5: some non-spell action, type 4 — must pack into the high byte
    ];
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[100, 200],
        &[],
        &rows,
        WorldEntry::FreshLogin,
    )
    .unwrap();
    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .expect("SMSG_ACTION_BUTTONS must be in the sequence");
    assert_eq!(bar.data[0], 6603); // action 6603 | (0 << 24) == 6603
    assert_eq!(bar.data[1], 78);
    assert_eq!(bar.data[5], 1234 | (4 << 24));
    // Slots the import didn't mention (incl. 2-4) stay zero — the synth's "100, 200" known spells are
    // NEVER consulted once imported rows exist, confirming there's no merge between the two sources.
    for slot in [2usize, 3, 4, 6, 7, 119] {
        assert_eq!(
            bar.data[slot], 0,
            "slot {slot} must stay 0 — no synth fallback fill-in"
        );
    }
}

/// A button index >= 120 (a corrupt/out-of-range import row) must be dropped, not panic on an
/// out-of-bounds array write — mirrors the reputation fold's out-of-range-slot handling above.
#[test]
fn login_sequence_drops_an_out_of_range_imported_button() {
    let rows = [(255u8, 999u32, 0u8)];
    let msgs = login_sequence_messages(&warrior_entity(), &[], &[], &rows, WorldEntry::FreshLogin)
        .unwrap();
    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .unwrap();
    assert!(
        bar.data.iter().all(|&b| b == 0),
        "an out-of-range button must be dropped silently"
    );
}

#[test]
fn create_object_derives_attack_power_and_weapon_damage_from_level_and_strength() {
    // PARITY with the module's combat::tables: attack_power = (level*3 + str*2) - 20 (saturating),
    // and the unarmed swing range folds in the AP*attack_time/(14*1000) bonus.
    let e = EntityView {
        level: 10,
        strength: 20,
        base_attack_time_ms: 2800,
        ..warrior_entity()
    };
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            // attack_power = (10*3 + 20*2) - 20 = 50
            assert_eq!(p.unit_attack_power(), Some(50));
            // dmg_bonus = 50 * 2800 / 14000 = 10; min = 1+10 = 11, max = 3+10 = 13
            assert_eq!(p.unit_mindamage(), Some(11.0));
            assert_eq!(p.unit_maxdamage(), Some(13.0));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_attack_power_saturates_at_zero_for_a_fresh_level_one() {
    // level 1, str 0: a plain `(1*3 + 0) - 20` would underflow; saturating_sub floors it at 0
    // instead of wrapping a u32 subtraction into a huge value.
    let msg = build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.unit_attack_power(), Some(0));
            assert_eq!(p.unit_mindamage(), Some(1.0));
            assert_eq!(p.unit_maxdamage(), Some(3.0));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn active_taxi_mount_is_present_in_the_player_create_mask() {
    let mut entity = warrior_entity();
    entity.mount_display_id = 1147;
    entity.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;

    let msg = build_create_object(&entity, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(player),
            ..
        } => {
            assert_eq!(player.unit_mountdisplayid(), Some(1147));
            assert_eq!(player.unit_flags(), Some(entity.unit_flags as i32));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }

    let unmounted = build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[])
        .unwrap();
    match &unmounted.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(player),
            ..
        } => assert_eq!(player.unit_mountdisplayid(), None),
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

/// A LAND mount writes the same field with no taxi flag, and an observer's CREATE carries it (story
/// 5): a peer entering visibility must see the rider on the mount without waiting for a VALUES
/// update. `unit_flags` stays 0, so the client never renders a land mount as a flight.
#[test]
fn a_land_mount_display_is_present_in_the_peer_create_mask() {
    let mut entity = warrior_entity();
    entity.mount_display_id = 1147;

    for kind in [CreateKind::Peer, CreateKind::SelfPlayer] {
        let msg = build_create_object(&entity, kind, &[], &[]).unwrap();
        match &msg.objects[0] {
            Object::CreateObject2 {
                mask2: UpdateMask::Player(player),
                ..
            } => {
                assert_eq!(player.unit_mountdisplayid(), Some(1147), "{kind:?}");
                assert_eq!(
                    player.unit_flags(),
                    Some(0),
                    "a land mount must not set TAXI_FLIGHT ({kind:?})"
                );
            }
            other => panic!("expected a Player CreateObject2, got {other:?}"),
        }
    }
}

/// The standalone land-mount VALUES builder (story 11) carries the display it is handed, and writes
/// an explicit 0 on dismount rather than omitting the field, or the client keeps the model. The
/// partial mask's freedom from `OBJECT_FIELD_TYPE` is asserted at the wire level by
/// `values::lint_tests::every_values_builder_serializes_without_the_object_type_bit`.
#[test]
fn mount_display_values_carries_the_display_it_is_given() {
    for display in [1147u32, 0] {
        let msg = crate::codec::build_mount_display_values(0x1234, display);
        match &msg.objects[0] {
            Object::Values {
                mask1: UpdateMask::Unit(unit),
                ..
            } => assert_eq!(unit.unit_mountdisplayid(), Some(display as i32)),
            other => panic!("expected a Unit Values object, got {other:?}"),
        }
    }
}

// ---- P2: visible_item_index slot-19 boundary (entity.rs) ---------------------------------------

#[test]
fn visible_item_index_boundary_stops_after_equipment_slot_18() {
    // Slot 18 (the last equipment slot) must render on the 3D model; slot 19 (the first equipped-bag
    // slot) must NOT — it still gets an inv-slot pointer, just no VISIBLE_ITEM.
    let (g18, g19) = ((0x4000u64 << 48) | 18, (0x4000u64 << 48) | 19);
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[(18, g18, 100), (19, g19, 200)],
        &[],
    )
    .unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            let vis18 = p
                .player_visible_item(VisibleItemIndex::Index18)
                .expect("slot 18 is equipment");
            assert_eq!(
                vis18.item, 100,
                "slot 18's own entry must be the one rendered, not slot 19's"
            );
            assert_eq!(
                p.player_field_inv(ItemSlot::Bag1),
                Some(Guid::new(g19)),
                "slot 19 still gets its inv-slot pointer, just no VISIBLE_ITEM"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

// ---- P1: movement.rs relayed_move_opcode 17-opcode table loop -----------------------------------

#[test]
fn relayed_move_opcode_maps_all_seventeen_client_variants() {
    use movement_opcodes as m;
    use wow_world_messages::vanilla::{
        MSG_MOVE_FALL_LAND_Client, MSG_MOVE_JUMP_Client, MSG_MOVE_SET_FACING_Client,
        MSG_MOVE_SET_RUN_MODE_Client, MSG_MOVE_SET_WALK_MODE_Client,
        MSG_MOVE_START_BACKWARD_Client, MSG_MOVE_START_FORWARD_Client,
        MSG_MOVE_START_STRAFE_LEFT_Client, MSG_MOVE_START_STRAFE_RIGHT_Client,
        MSG_MOVE_START_SWIM_Client, MSG_MOVE_START_TURN_LEFT_Client,
        MSG_MOVE_START_TURN_RIGHT_Client, MSG_MOVE_STOP_Client, MSG_MOVE_STOP_STRAFE_Client,
        MSG_MOVE_STOP_SWIM_Client, MSG_MOVE_STOP_TURN_Client, MovementInfo_MovementFlags,
    };
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 0,
        position: Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        orientation: 0.0,
        fall_time: 0.0,
    };
    let cases: Vec<(ClientOpcodeMessage, u32)> = vec![
        (
            ClientOpcodeMessage::MSG_MOVE_START_FORWARD(Box::new(MSG_MOVE_START_FORWARD_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_START_FORWARD,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_BACKWARD(Box::new(
                MSG_MOVE_START_BACKWARD_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_BACKWARD,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP(Box::new(MSG_MOVE_STOP_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_STRAFE_LEFT(Box::new(
                MSG_MOVE_START_STRAFE_LEFT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_STRAFE_LEFT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_STRAFE_RIGHT(Box::new(
                MSG_MOVE_START_STRAFE_RIGHT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_STRAFE_RIGHT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP_STRAFE(Box::new(MSG_MOVE_STOP_STRAFE_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP_STRAFE,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_JUMP(Box::new(MSG_MOVE_JUMP_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_JUMP,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_TURN_LEFT(Box::new(
                MSG_MOVE_START_TURN_LEFT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_TURN_LEFT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_TURN_RIGHT(Box::new(
                MSG_MOVE_START_TURN_RIGHT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_TURN_RIGHT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP_TURN(Box::new(MSG_MOVE_STOP_TURN_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP_TURN,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_SET_RUN_MODE(Box::new(MSG_MOVE_SET_RUN_MODE_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_SET_RUN_MODE,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_SET_WALK_MODE(Box::new(MSG_MOVE_SET_WALK_MODE_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_SET_WALK_MODE,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_FALL_LAND(Box::new(MSG_MOVE_FALL_LAND_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_FALL_LAND,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_SWIM(Box::new(MSG_MOVE_START_SWIM_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_START_SWIM,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP_SWIM(Box::new(MSG_MOVE_STOP_SWIM_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP_SWIM,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_SET_FACING(Box::new(MSG_MOVE_SET_FACING_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_SET_FACING,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_HEARTBEAT(Box::new(MSG_MOVE_HEARTBEAT_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_HEARTBEAT,
        ),
    ];
    assert_eq!(
        cases.len(),
        17,
        "the relayed opcode table has exactly 17 entries"
    );
    for (msg, expected) in &cases {
        assert_eq!(
            relayed_move_opcode(msg),
            Some(*expected),
            "opcode mismatch for {msg:?}"
        );
    }
}

// ---- P2: teleport_ack / force_run_speed / monster_move spline flags (movement.rs) ---------------

#[test]
fn teleport_ack_carries_target_position_with_no_movement_flags() {
    let msg = build_teleport_ack(7, 99, 1.0, 2.0, 3.0, 0.5);
    assert_eq!(msg.guid.guid(), 7);
    assert_eq!(msg.movement_counter, 99);
    assert_eq!(msg.info.position.x, 1.0);
    assert_eq!(msg.info.position.y, 2.0);
    assert_eq!(msg.info.position.z, 3.0);
    assert_eq!(msg.info.orientation, 0.5);
    assert!(
        msg.info.flags.is_empty(),
        "a teleport arrival is a clean stand-still, no movement flags"
    );
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

// ---- Cross-map teleport wire pins (SMSG_TRANSFER_PENDING / SMSG_NEW_WORLD) --------

#[test]
fn cross_map_teleport_sends_transfer_pending_then_new_world_carrying_target_map_and_position() {
    let [pending, new_world] = build_cross_map_teleport(36, 10.0, 20.0, 30.0, 1.5).unwrap();
    match pending {
        ServerOpcodeMessage::SMSG_TRANSFER_PENDING(p) => {
            assert_eq!(p.map.as_int(), 36);
            assert!(
                p.has_transport.is_none(),
                "no transport (ship/zeppelin) support yet"
            );
            let mut buf = Vec::new();
            p.write_unencrypted_server(&mut buf).unwrap();
            assert!(!buf.is_empty());
        }
        _ => panic!("expected SMSG_TRANSFER_PENDING"),
    }
    match new_world {
        ServerOpcodeMessage::SMSG_NEW_WORLD(w) => {
            assert_eq!(w.map.as_int(), 36);
            assert_eq!(w.position.x, 10.0);
            assert_eq!(w.position.y, 20.0);
            assert_eq!(w.position.z, 30.0);
            assert_eq!(w.orientation, 1.5);
            let mut buf = Vec::new();
            w.write_unencrypted_server(&mut buf).unwrap();
            assert!(!buf.is_empty());
        }
        _ => panic!("expected SMSG_NEW_WORLD"),
    }
}

#[test]
fn cross_map_teleport_rejects_an_unknown_map_id() {
    assert!(build_cross_map_teleport(u32::MAX, 0.0, 0.0, 0.0, 0.0).is_err());
}

#[test]
fn force_run_speed_carries_guid_speed_and_zero_move_event() {
    let msg = build_force_run_speed(7, 8.0);
    assert_eq!(msg.guid.guid(), 7);
    assert_eq!(msg.speed, 8.0);
    assert_eq!(msg.move_event, 0);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn monster_move_run_flag_selects_the_run_spline_bit() {
    let start = Vector3d {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    let dest = Vector3d {
        x: 10.0,
        y: 0.0,
        z: 0.0,
    };
    let walking = build_monster_move(1, start, dest, 1000, 1, false);
    assert_eq!(
        walking.spline_flags,
        SplineFlag::new(0),
        "a walking leg carries no RUN_MODE bit"
    );
    let running = build_monster_move(1, start, dest, 1000, 2, true);
    assert_eq!(
        running.spline_flags,
        SplineFlag::new(0x100),
        "a running leg sets RUN_MODE (0x100)"
    );
    assert_eq!(
        running.splines,
        vec![dest],
        "exactly one destination point (no multi-point spline)"
    );
    assert_eq!(running.spline_id, 2);
    assert_eq!(running.move_type, SMSG_MONSTER_MOVE_MonsterMoveType::Normal);
}

#[test]
fn monster_move_facing_carries_the_angle_and_a_degenerate_single_point_spline() {
    // #518: a stand-and-swing creature turns to face its target without moving. The wire shape must
    // be the `FacingAngle` variant (the client's ONLY source of a non-moving heading change), zero
    // duration, and a single-point spline at the mover's own position (there IS no destination).
    let pos = Vector3d {
        x: 5.0,
        y: -3.0,
        z: 12.0,
    };
    let msg = build_monster_move_facing(9, pos, 1.75, 42);
    assert_eq!(msg.guid.guid(), 9);
    assert_eq!(msg.spline_point, pos);
    assert_eq!(msg.spline_id, 42);
    assert_eq!(
        msg.move_type,
        SMSG_MONSTER_MOVE_MonsterMoveType::FacingAngle { angle: 1.75 }
    );
    assert_eq!(msg.duration, 0, "facing-only — nothing to interpolate");
    assert_eq!(
        msg.splines,
        vec![pos],
        "degenerate one-point spline at the mover's own position"
    );
}

#[test]
fn taxi_move_writes_absolute_catmull_rom_points_byte_for_byte() {
    let start = Vector3d { x: -9465.0, y: -1300.0, z: 50.0 };
    let points = vec![
        Vector3d { x: -9460.0, y: -1298.0, z: 52.0 },
        Vector3d { x: -9450.0, y: -1290.0, z: 54.0 },
        Vector3d { x: -9440.0, y: -1280.0, z: 56.0 },
    ];
    let (opcode, bytes) = build_taxi_move_raw(0xF00D, start, points.clone(), 12_345, 77).unwrap();
    assert_eq!(opcode, 0x00DD);

    let mut expected = vec![0x03, 0x0D, 0xF0]; // PackedGuid(0xF00D)
    for value in [start.x, start.y, start.z] {
        expected.extend_from_slice(&value.to_le_bytes());
    }
    expected.extend_from_slice(&77u32.to_le_bytes());
    expected.push(0); // MonsterMoveNormal
    expected.extend_from_slice(&0x300u32.to_le_bytes()); // RUN_MODE | FLYING/Catmull-Rom
    expected.extend_from_slice(&12_345u32.to_le_bytes());
    expected.extend_from_slice(&3u32.to_le_bytes());
    for point in points {
        for value in [point.x, point.y, point.z] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
    }
    assert_eq!(bytes, expected);
}

// ---- P1: item.rs build_buy_failed 5-way map + build_item_create_object CONTAINER branch --------

#[test]
fn buy_failed_maps_module_err_strings_to_the_closest_buy_result() {
    let cases: [(&str, BuyResult); 7] = [
        ("not enough money to buy this", BuyResult::NotEnoughMoney),
        ("target is out of range", BuyResult::DistanceTooFar),
        ("vendor is on another map", BuyResult::DistanceTooFar),
        ("inventory full", BuyResult::CantCarryMore),
        ("item sold out", BuyResult::ItemSoldOut),
        ("no stock remaining", BuyResult::ItemSoldOut),
        ("vendor does not sell that item", BuyResult::CantFindItem),
    ];
    for (err, expected) in cases {
        let msg = build_buy_failed(42, 25, err);
        assert_eq!(msg.result, expected, "err {err:?} must map to {expected:?}");
        assert_eq!(msg.guid.guid(), 42);
        assert_eq!(msg.item, 25);
    }
}

/// `lyracore_shared::bank::result`'s constants exist so the module can tag a refusal without a
/// dependency on the wire crate; they must never drift from gtker's own `BuyBankSlotResult::as_int`
/// numbering, or `parse_buy_bank_slot_result` silently relays the wrong client-visible result.
#[test]
fn bank_result_constants_match_gtker_buy_bank_slot_result() {
    use lyracore_shared::bank::result;
    assert_eq!(
        BuyBankSlotResult::FailedTooMany.as_int(),
        result::FAILED_TOO_MANY as u32
    );
    assert_eq!(
        BuyBankSlotResult::InsufficientFunds.as_int(),
        result::INSUFFICIENT_FUNDS as u32
    );
    assert_eq!(
        BuyBankSlotResult::NotBanker.as_int(),
        result::NOT_BANKER as u32
    );
    assert_eq!(BuyBankSlotResult::Ok.as_int(), result::OK as u32);
}

#[test]
fn buy_bank_slot_result_parses_the_leading_bracketed_code_not_the_prose() {
    let cases: [(&str, BuyBankSlotResult); 5] = [
        (
            "[0] no bank bag slots left to buy",
            BuyBankSlotResult::FailedTooMany,
        ),
        (
            "[1] not enough money (need 1000)",
            BuyBankSlotResult::InsufficientFunds,
        ),
        ("[2] target is not a banker", BuyBankSlotResult::NotBanker),
        // Prose alone (no bracket) must not accidentally match a keyword — the fallback is
        // the generic refusal, not a guess at intent.
        (
            "insufficient funds but no bracket tag",
            BuyBankSlotResult::NotBanker,
        ),
        ("[9] unknown future code", BuyBankSlotResult::NotBanker),
    ];
    for (err, expected) in cases {
        assert_eq!(
            parse_buy_bank_slot_result(err),
            expected,
            "err {err:?} must map to {expected:?}"
        );
    }
}

#[test]
fn item_create_object_bag_slots_build_a_container_with_num_slots() {
    let inst = ItemInstanceView {
        guid: (0x4000u64 << 48) | (1 << 8) | 19, // equipped-bag slot
        entry: 5000,
        owner_guid: 1,
        slot: 19,
        stack_count: 1,
        durability: 0,
        max_durability: 0,
        container_slots: 8, // an 8-slot bag
    };
    let msg = build_item_create_object(&inst);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            object_type,
            mask2: UpdateMask::Container(c),
            ..
        } => {
            assert_eq!(
                *object_type,
                ObjectType::Container,
                "a bag must build a CONTAINER object"
            );
            assert_eq!(c.object_entry(), Some(5000));
            assert_eq!(c.item_owner(), Some(Guid::new(1)));
            assert_eq!(
                c.container_num_slots(),
                Some(8),
                "the bag window needs its slot count"
            );
        }
        other => panic!("expected a Container CreateObject2, got {other:?}"),
    }
}

// ---- P1: corpse.rs display_id==0 -> 49 fallback -------------------------------------------------

#[test]
fn corpse_create_object_falls_back_to_display_49_when_zero() {
    // Defensive floor against a null-model crash: display_id 0 must never reach the wire —
    // fall back to 49 (Human Male) rather than let the client dereference a null model.
    let corpse = CorpseView {
        guid: 1,
        owner_guid: 1,
        display_id: 0,
        ..Default::default()
    };
    let msg = build_corpse_create_object(&corpse);
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(
                m.corpse_display_id(),
                Some(49),
                "display_id 0 must fall back to 49"
            );
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }
    // A real display id passes through unchanged.
    let real = CorpseView {
        display_id: 304,
        ..corpse
    };
    let msg2 = build_corpse_create_object(&real);
    match &msg2.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(m.corpse_display_id(), Some(304));
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }
}

// ---- P2: creature query response (npc.rs) + questgiver_status echo -----------------------------

#[test]
fn creature_query_response_maps_names_and_flags_and_serializes() {
    let c = CreatureView {
        entry: 6741,
        name: "Innkeeper Farley".to_string(),
        subname: "Innkeeper".to_string(),
        display_id: 2068,
        creature_type: 7, // Humanoid
        creature_family: 0,
        type_flags: 0x8,
        rank: 0,
    };
    let msg = build_creature_query_response(&c);
    assert_eq!(msg.creature_entry, 6741);
    {
        let found = msg.found.as_ref().expect("a real template must be found");
        assert_eq!(found.name1, "Innkeeper Farley");
        assert_eq!(found.sub_name, "Innkeeper");
        assert_eq!(found.display_id, 2068);
        assert_eq!(found.creature_type, 7);
        assert_eq!(found.type_flags, 0x8);
    }
    let mut buf = Vec::new();
    ServerOpcodeMessage::SMSG_CREATURE_QUERY_RESPONSE(Box::new(msg))
        .write_unencrypted_server(&mut buf)
        .unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn visible_item_values_render_and_clear_equipment_slots_only_work_item_087() {
    // Equip: the VALUES partial must carry PLAYER_VISIBLE_ITEM[15] = the item entry.
    let msg = build_visible_item_values(1, 15, 25).expect("slot 15 (mainhand) is equipment");
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            let vi = p
                .player_visible_item(VisibleItemIndex::Index15)
                .expect("visible item set");
            assert_eq!(
                vi.item, 25,
                "the mainhand VISIBLE_ITEM must carry the equipped entry"
            );
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    // Unequip: entry 0 is a real (zeroed) write, not a skipped update.
    let cleared = build_visible_item_values(1, 15, 0).unwrap();
    match &cleared.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            let vi = p
                .player_visible_item(VisibleItemIndex::Index15)
                .expect("cleared but present");
            assert_eq!(vi.item, 0);
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    // Non-equipment slots (backpack 23, bags 19, bag contents 120+) are None — no model residue.
    assert!(build_visible_item_values(1, 23, 25).is_none());
    assert!(build_visible_item_values(1, 19, 25).is_none());
    assert!(build_visible_item_values(1, 120, 25).is_none());
}

// ===========================================================================================
//  "SMSG_COMPRESSED_MOVES corrupts at crowd scale" — negative-finding regression pin against that
//  report.
//
//  The gateway does not construct `SMSG_COMPRESSED_MOVES` anywhere (nothing in `gateway/src` builds
//  it, and `git log -S "COMPRESSED_MOVES" --all` finds no commit that ever did) — the movement paths
//  actually in use are `SMSG_MONSTER_MOVE` (typed, one per creature leg) and the raw `MSG_MOVE_*`
//  peer relay (`codec::build_movement_relay_raw`), both pinned at crowd scale in
//  `world::tests::writer_thread_survives_movement_bursts_from_100_to_500_movers_without_desync`.
//
//  This test instead pins `wow_world_messages` 0.3's OWN codec for the opcode the issue names, since
//  the crash report's panic site (`smsg_compressed_moves.rs:28`, a flate2 `read_to_end().unwrap()`)
//  lives entirely inside that dependency. Sweeping 1..3000 movers finds NO corruption — the codec
//  round-trips cleanly the whole way; it only silently truncates its own u16 size field in the
//  thousands-of-movers range (see the ignored `_finds_the_actual_gtker_wrap_threshold` case below),
//  an order of magnitude past anything the report describes. So whatever produced the reported crash, it was
//  not this codec misbehaving at 150-300 movers — that theory is pinned FALSE here, permanently.
fn compressed_move_for(i: u64) -> wow_world_messages::vanilla::CompressedMove {
    use wow_world_messages::vanilla::{
        CompressedMove, CompressedMove_CompressedMoveOpcode, MonsterMove,
        MonsterMove_MonsterMoveType, SplineFlag, Vector3d,
    };
    CompressedMove {
        opcode: CompressedMove_CompressedMoveOpcode::SmsgMonsterMove {
            monster_move: MonsterMove {
                spline_point: Vector3d {
                    x: -8938.857 + i as f32,
                    y: -131.36594,
                    z: 83.57745,
                },
                spline_id: i as u32,
                move_type: MonsterMove_MonsterMoveType::Normal {
                    duration: 500,
                    spline_flags: SplineFlag::empty(),
                    splines: vec![Vector3d {
                        x: -8900.0 + i as f32,
                        y: -100.0,
                        z: 82.4,
                    }],
                },
            },
        },
        guid: wow_world_messages::Guid::new(0x1000_0000_0000_0000 + i),
    }
}

/// Encode `n` `CompressedMove` blocks via the crate's own `write_into_vec` (the deflate + size-prefix
/// path the panic trace lives inside), frame it as a real unencrypted packet, and decode it back
/// through `ServerOpcodeMessage::read_unencrypted` — the same dispatcher the gateway's own reader and
/// the headless wire client use. Returns `Ok(decoded_move_count)` or the decode error.
fn compressed_moves_roundtrip(n: usize) -> Result<usize, String> {
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::SMSG_COMPRESSED_MOVES;
    use wow_world_messages::Message;

    let msg = SMSG_COMPRESSED_MOVES {
        moves: (0..n as u64).map(compressed_move_for).collect(),
    };
    let mut body = Vec::new();
    msg.write_into_vec(&mut body).map_err(|e| e.to_string())?;

    let size = 2 + body.len(); // opcode(2) + body, matching gtker's own server framing
    let mut full = Vec::new();
    full.extend_from_slice(&(size as u16).to_be_bytes());
    full.extend_from_slice(&(SMSG_COMPRESSED_MOVES::OPCODE as u16).to_le_bytes());
    full.extend_from_slice(&body);

    match ServerOpcodeMessage::read_unencrypted(&mut std::io::Cursor::new(&full)) {
        Ok(ServerOpcodeMessage::SMSG_COMPRESSED_MOVES(decoded)) => Ok(decoded.moves.len()),
        Ok(other) => Err(format!("decoded as the wrong variant: {other}")),
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn compressed_moves_codec_roundtrips_cleanly_from_1_to_3000_movers() {
    // 100 = the report's own "clean" control; 150/200/300 = the reported-bad range; 500/1000/3000 =
    // margin well past it. Every one of these must decode back to exactly the mover count it encoded
    // — if any of them failed, that would BE the reported encode bug, in the one place it could live.
    for n in [1usize, 10, 50, 100, 150, 200, 300, 500, 1000, 3000] {
        match compressed_moves_roundtrip(n) {
            Ok(decoded) => assert_eq!(
                decoded, n,
                "n={n}: round-tripped to the wrong mover count ({decoded})"
            ),
            Err(e) => panic!(
                "n={n}: SMSG_COMPRESSED_MOVES failed to round-trip through wow_world_messages' own \
                 codec — this WOULD be the reported encode defect, found at n={n}: {e}"
            ),
        }
    }
}

#[test]
#[ignore = "slow (builds/compresses ~10k CompressedMove blocks); run explicitly to see the u16 wrap \
            threshold this crate's SMSG_COMPRESSED_MOVES::write_encrypted_server has — informational, \
            not a regression guard for that report (that opcode is never sent at this scale, or at all)"]
fn compressed_moves_codec_wraps_its_u16_size_field_only_far_past_reported_scale() {
    // Documents WHERE the real (latent, currently-unreachable) limit is: `write_encrypted_server`
    // computes `size = v.len().saturating_sub(2) as u16`, which silently truncates once the encoded
    // packet exceeds 65535 bytes. This finds the crossover empirically rather than asserting a
    // hand-derived number, so it stays correct if the wire shape of `CompressedMove` ever changes.
    let mut last_clean = 0usize;
    for n in (1000..=20000).step_by(1000) {
        match compressed_moves_roundtrip(n) {
            Ok(decoded) if decoded == n => last_clean = n,
            _ => {
                println!(
                    "wrap threshold: clean through n={last_clean}, corrupted by n={n} — both are \
                     roughly two orders of magnitude past the reported 150-300 mover range"
                );
                assert!(
                    n > 5000,
                    "the wrap threshold moved to n={n}, uncomfortably close to a scale this repo \
                     actually benchmarks (docs/bench 300+ mover raids) — worth re-checking against the original report"
                );
                return;
            }
        }
    }
    panic!("expected the encoder to wrap its u16 size field somewhere in 1000..20000 movers");
}

// ---- SMSG_SET_PROFICIENCY: what the client may wield and wear -----------------------------------

/// Extract the typed proficiency message, or fail loudly.
fn proficiency(msg: &ServerOpcodeMessage) -> &SMSG_SET_PROFICIENCY {
    match msg {
        ServerOpcodeMessage::SMSG_SET_PROFICIENCY(p) => p,
        other => panic!("expected SMSG_SET_PROFICIENCY, got {other}"),
    }
}

#[test]
fn set_proficiency_pins_both_masks_on_the_wire() {
    // A Warrior who has trained Plate Mail (750). Weapons: the class table admits every subclass up
    // to the fishing pole except the wand. Armor: misc+cloth+leather+mail+shield, plus the plate the
    // passive just unlocked.
    let msgs = build_set_proficiency_msgs(1, &[750]);
    let bytes = |msg: &ServerOpcodeMessage| {
        let mut buf = Vec::new();
        proficiency(msg).write_unencrypted_server(&mut buf).unwrap();
        buf
    };
    // size (u16 BE) = 5-byte body + 2-byte opcode; opcode 0x0127 (u16 LE); ItemClass; mask (u32 LE).
    assert_eq!(
        bytes(&msgs[0]),
        vec![0x00, 0x07, 0x27, 0x01, 2, 0xFF, 0xFF, 0x17, 0x00],
        "weapon mask = every subclass 0..=20 except the wand (19)"
    );
    assert_eq!(
        bytes(&msgs[1]),
        vec![0x00, 0x07, 0x27, 0x01, 4, 0x5F, 0x00, 0x00, 0x00],
        "armor mask = misc|cloth|leather|mail|plate|shield"
    );
    // Each bit is `1 << subclass`, so the mask reads back as the subclass set it stands for.
    let armor = proficiency(&msgs[1]).item_sub_class_mask;
    for subclass in [0u8, 1, 2, 3, 4, 6] {
        assert!(armor & (1 << subclass) != 0, "armor subclass {subclass}");
    }
    assert_eq!(armor & (1 << 5), 0, "subclass 5 is not an armor tier");
}

#[test]
fn the_armor_mask_follows_the_trained_passives_per_class() {
    let mask = |player_class, learned: &[u32]| {
        let msg = build_armor_proficiency_msg(player_class, learned);
        assert_eq!(proficiency(&msg).class.as_int(), 4);
        proficiency(&msg).item_sub_class_mask
    };
    const MAIL: u32 = 1 << 3;
    const PLATE: u32 = 1 << 4;

    // A Warrior wears mail from level 1 and plate only once 750 is trained.
    assert_eq!(mask(1, &[]) & MAIL, MAIL);
    assert_eq!(mask(1, &[]) & PLATE, 0);
    assert_eq!(mask(1, &[750]) & (MAIL | PLATE), MAIL | PLATE);
    // A Mage wears cloth and misc, whatever passives it somehow holds.
    assert_eq!(mask(8, &[]), 0b11);
    assert_eq!(mask(8, &[750, 8737]), 0b11);
    // The mail upgrade is the Hunter's, and it is the only tier it moves.
    assert_eq!(mask(3, &[]) & MAIL, 0);
    assert_eq!(mask(3, &[8737]) & MAIL, MAIL);
}

#[test]
fn login_sequence_carries_both_proficiency_masks_with_the_initial_state() {
    for (learned, plate_trained) in [(&[][..], false), (&[750u32][..], true)] {
        let msgs =
            login_sequence_messages(&warrior_entity(), learned, &[], &[], WorldEntry::FreshLogin)
                .unwrap();
        let at = |wanted: fn(&ServerOpcodeMessage) -> bool| {
            msgs.iter().position(wanted).expect("message in sequence")
        };
        let spells = at(|m| matches!(m, ServerOpcodeMessage::SMSG_INITIAL_SPELLS(_)));
        let buttons = at(|m| matches!(m, ServerOpcodeMessage::SMSG_ACTION_BUTTONS(_)));
        let first = at(|m| matches!(m, ServerOpcodeMessage::SMSG_SET_PROFICIENCY(_)));
        assert!(
            spells < first && first < buttons,
            "both masks ride the initial-state batch, after the spellbook they derive from"
        );

        let masks: Vec<(u8, u32)> = msgs
            .iter()
            .filter(|m| matches!(m, ServerOpcodeMessage::SMSG_SET_PROFICIENCY(_)))
            .map(|m| {
                (
                    proficiency(m).class.as_int(),
                    proficiency(m).item_sub_class_mask,
                )
            })
            .collect();
        assert_eq!(masks.len(), 2, "one message per item class");
        assert_eq!(masks[0].0, 2, "weapons first");
        assert_eq!(masks[1].0, 4, "then armor");
        assert!(
            masks[1].1 & (1 << 3) != 0,
            "a Warrior wears mail with nothing trained"
        );
        assert_eq!(
            masks[1].1 & (1 << 4) != 0,
            plate_trained,
            "plate follows the passive in the spellbook"
        );
    }
}
