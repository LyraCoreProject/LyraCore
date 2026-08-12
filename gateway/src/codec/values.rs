//! Partial-VALUES update builders — the crash-critical `build_*_values` family plus the raw
//! VALUES path and the packed-guid helper. MOVED VERBATIM out of `mod.rs`: the
//! finalize → `dirty_reset()` → re-set-only-changed-fields discipline (which keeps
//! `OBJECT_FIELD_TYPE` off a partial update) is DELIBERATE and PROVEN against the 5875 client.
//! Do NOT change any logic and do NOT migrate the typed builders onto the raw encoder.

use super::*;

/// Own the partial-VALUES `dirty_reset` ritual for a UNIT mask, ONCE (review finding D1). Every
/// `build_*_values` unit builder repeats the same crash-critical discipline: build the mask (whose
/// `new()` force-seeds `OBJECT_FIELD_TYPE` dirty), `finalize()`, `dirty_reset()` to STRIP that seeded
/// TYPE bit, then re-apply ONLY the changed field(s) so the wire carries them alone and NEVER
/// re-sends `OBJECT_FIELD_TYPE` (re-sending it crashes the 5875 client at null+0x110 — see
/// `build_health_values` for the full root-cause). `apply` sets just the caller's field(s) on the
/// reset mask; the output bytes are identical to the pre-extraction inline form (pinned by the codec
/// golden/crash-trap tests). Do NOT migrate this onto the raw encoder.
fn unit_values(guid: u64, apply: impl FnOnce(&mut UpdateUnit)) -> SMSG_UPDATE_OBJECT {
    let mut unit = UpdateUnit::builder().finalize();
    unit.dirty_reset();
    apply(&mut unit);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Unit(unit),
        }],
    }
}

/// Own the partial-VALUES `dirty_reset` ritual for a PLAYER mask, ONCE (review finding D1) — the
/// player-shaped twin of [`unit_values`]. Same discipline: finalize → `dirty_reset()` (strips the
/// `new()`-seeded `OBJECT_FIELD_TYPE` dirty bit) → re-apply ONLY the changed field(s), so the wire
/// never re-sends `OBJECT_FIELD_TYPE` (which strips the PLAYER bit on the player's OWN object and
/// crashes the 5875 client at null+0x110). Byte-identical to the pre-extraction inline form (pinned
/// by the codec golden/crash-trap tests). Do NOT migrate this onto the raw encoder.
fn player_values(guid: u64, apply: impl FnOnce(&mut UpdatePlayer)) -> SMSG_UPDATE_OBJECT {
    let mut player = UpdatePlayer::builder().finalize();
    player.dirty_reset();
    apply(&mut player);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Player(player),
        }],
    }
}

/// Build a VALUES partial-update (`Object::Values`) carrying only `UNIT_FIELD_HEALTH` (Tier 3
/// combat). Unlike `build_create_object`, this pushes a *single changed field* to observers via a
/// sparse update mask — no re-create, no flicker. The foundation for combat damage (and any future
/// live field change).
///
/// CRITICAL: `wow_world_messages`' update-mask builder force-seeds `OBJECT_FIELD_TYPE` (bit 2)
/// dirty in `new()`, so a naive `builder().set_unit_health().finalize()` re-transmits the object
/// TYPE as a value word *inside this partial update*. A real 1.12 server NEVER re-sends TYPE on a
/// VALUES update — and doing so crashes the 5875 client: it overwrites the client's cached
/// `OBJECT_FIELD_TYPE` with the Unit value `0x09` (OBJECT|UNIT), stripping the PLAYER bit (`0x10`)
/// from the player's OWN object (spawned as `0x19` = OBJECT|UNIT|PLAYER). Mis-typed, the next
/// player-frame dispatch dereferences a null sub-object → ACCESS_VIOLATION at null+0x110 (the combat
/// client crash root-caused 2026-06-16; a creature's VALUES was harmless only because its type is
/// already `0x09`). `dirty_reset()` clears the dirty mask (incl. the seeded TYPE bit); re-setting
/// ONLY health then re-dirties bit 22 alone, so the wire carries just mask word `0x00400000` + the
/// health value — exactly what cmangos/vmangos send. (gtker's own strict reader rejects a TYPE-less
/// mask, but the real client — the only consumer — requires its absence.)
pub fn build_health_values(guid: u64, health: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_health(health as i32);
    })
}

/// Build a VALUES partial-update carrying the ghost fields — `PLAYER_FLAGS` (GHOST bit) and
/// `UNIT_FIELD_BYTES_1` (the vis-ghost render byte) — so a player who Releases Spirit turns into a
/// ghost for observers already holding the object (slice 5). Player mask; same `dirty_reset`
/// discipline as `build_health_values` so it never re-sends OBJECT_FIELD_TYPE (health relays
/// separately via `build_health_values`). On reclaim the same fields (cleared) relay the un-ghost.
pub fn build_ghost_values(guid: u64, player_flags: u32, unit_bytes_1: u32) -> SMSG_UPDATE_OBJECT {
    let (b1a, b1b, b1c, b1d) = unpack4(unit_bytes_1);
    player_values(guid, |player| {
        player.set_player_flags(player_flags as i32);
        player.set_unit_bytes_1(b1a, b1b, b1c, b1d);
    })
}

/// Build a VALUES partial-update carrying `PLAYER_BYTES_2` — same `dirty_reset` discipline as
/// `build_health_values`. `player_bytes_2` is the FULL descriptor u32 (byte0 facial hair, byte2 owned
/// bank bag slot count, byte3 rest state); a partial VALUES overwrites the whole field, so all three
/// ride along even though only the slot count changed.
pub fn build_bank_bag_slots_values(guid: u64, player_bytes_2: u32) -> SMSG_UPDATE_OBJECT {
    let (a, b, c, d) = unpack4(player_bytes_2);
    player_values(guid, |player| {
        player.set_player_bytes_2(a, b, c, d);
    })
}

/// `SMSG_UPDATE_OBJECT` opcode (vanilla build 5875). Cross-checked against the gtker header in
/// `raw_values_body_matches_gtker_envelope` below.
const SMSG_UPDATE_OBJECT_OPCODE: u16 = 0x00A9;

/// Build a RAW `SMSG_UPDATE_OBJECT` (one `Object::Values` block) for `guid` carrying `mask` — the
/// escape past gtker's slot-0 update-mask wall. The body envelope (block count, `has_transport`,
/// update-type byte, packed guid) is byte-identical to what gtker's
/// `SMSG_UPDATE_OBJECT(Object::Values{..})` serializes (pinned by the test below); only the trailing
/// update-mask is ours, so it can carry fields gtker can't express (auras 1..47, multi-field items).
/// Returns `(opcode, body)` for [`Outbound::Raw`](crate::world::Outbound::Raw). The caller owns the
/// 5875 crash-trap discipline: `mask` must NOT set `OBJECT_FIELD_TYPE` on a partial update.
pub fn build_values_update_raw(guid: u64, mask: &update_mask::UpdateMaskValues) -> (u16, Vec<u8>) {
    // 5875 crash trap: a partial VALUES update must never carry OBJECT_FIELD_TYPE (index 2) — the
    // client strips the PLAYER bit and faults at null+0x110. UpdateMaskValues never inserts it
    // structurally; this guards a caller that wrongly does.
    debug_assert!(
        mask.get(update_mask::idx::OBJECT_TYPE).is_none(),
        "raw VALUES update must NOT set OBJECT_FIELD_TYPE (5875 null+0x110 crash trap)"
    );
    let mut body = Vec::new();
    body.extend_from_slice(&1u32.to_le_bytes()); // amount_of_objects = 1
    body.push(0u8); // has_transport = 0
    body.push(0u8); // update_type = VALUES (0)
    write_packed_guid_u64(&mut body, guid); // guid1, packed
    mask.write_to(&mut body); // mask1
    (SMSG_UPDATE_OBJECT_OPCODE, body)
}

/// Build a partial VALUES update that sets ONE `PLAYER_EXPLORED_ZONES` word — the map fog-clear.
/// `word_idx` is 0..64 (= `area_bit / 32`); `word_value` is the FULL u32 for that word,
/// i.e. the OR of EVERY explored area_bit in that 32-bucket (a partial VALUES overwrites the whole
/// word, so passing only the new bit would clobber the other explored areas sharing it). Routes through
/// the raw path, which never carries `OBJECT_FIELD_TYPE` (the 5875 null+0x110 crash trap) by design.
pub fn build_explored_zones_values(guid: u64, word_idx: u16, word_value: u32) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_u32(
        update_mask::idx::PLAYER_EXPLORED_ZONES_1 + word_idx,
        word_value,
    );
    build_values_update_raw(guid, &mask)
}

/// Build a partial VALUES update that sets `PLAYER_BYTES_2` — the rest-state flip. `player_bytes_2`
/// is the FULL descriptor u32 (byte 0 facial hair + byte 3 rest state); a partial VALUES overwrites the
/// whole field, so the module ships the complete value. Byte 3 = RESTED (0x01) → the client draws the zzz
/// icon + blue XP bar; NORMAL (0x02) → normal. Routes through the OBJECT_FIELD_TYPE-free raw path.
pub fn build_rest_state_values(guid: u64, player_bytes_2: u32) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_u32(update_mask::idx::PLAYER_BYTES_2, player_bytes_2);
    build_values_update_raw(guid, &mask)
}

/// Vanilla packed-guid encoding (mirrors gtker's `write_packed_guid`): a leading bit-pattern byte
/// where bit `i` marks a NON-zero byte `i` of the little-endian guid, then those non-zero bytes.
pub(crate) fn write_packed_guid_u64(out: &mut Vec<u8>, guid: u64) {
    let bytes = guid.to_le_bytes();
    let mut bit_pattern: u8 = 0;
    let mut data: Vec<u8> = Vec::with_capacity(8);
    for (i, &b) in bytes.iter().enumerate() {
        if b != 0 {
            bit_pattern |= 1 << i;
            data.push(b);
        }
    }
    out.push(bit_pattern);
    out.extend_from_slice(&data);
}

/// Build a VALUES partial-update carrying `UNIT_DYNAMIC_FLAGS` (idx 143) so a corpse's DEAD bit
/// reaches clients that already have the object (slice 2 killing blow). New observers get it at
/// CREATE instead (`build_create_object` sets it). Same `dirty_reset` discipline as
/// `build_health_values` so the wire carries ONLY this field and never re-sends OBJECT_FIELD_TYPE.
pub fn build_dynamic_flags_values(guid: u64, dynamic_flags: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_dynamic_flags(dynamic_flags as i32);
    })
}

/// VALUES partial-update carrying `UNIT_FIELD_FLAGS` so observers see a unit's flag changes live — the
/// `UNIT_FLAG_IN_COMBAT` bit toggling as it enters/leaves combat (the combat indicator, incl. a pure
/// caster the auto-attack `SMSG_ATTACKSTART` relay can't cover). Same `dirty_reset` discipline as
/// `build_dynamic_flags_values` so the wire carries ONLY this field and never re-sends OBJECT_FIELD_TYPE.
pub fn build_unit_flags_values(guid: u64, unit_flags: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_flags(unit_flags as i32);
    })
}

/// VALUES partial-update carrying `UNIT_FIELD_BYTES_2` so observers see a weapon DRAWN or STOWED the
/// moment it happens — the `CMSG_SETSHEATHED` a client sends on `Z`. Unit mask, not player-gated: a
/// creature draws its weapon on engage too. Same `dirty_reset` discipline as its siblings (the wire
/// carries only this field, never OBJECT_FIELD_TYPE). Where a stowed weapon HANGS is a different
/// field entirely — the per-item `sheathe_type` in the item query. [#101]
pub fn build_sheath_values(guid: u64, unit_bytes_2: u32) -> SMSG_UPDATE_OBJECT {
    let (b2a, b2b, b2c, b2d) = unpack4(unit_bytes_2);
    unit_values(guid, |unit| {
        unit.set_unit_bytes_2(b2a, b2b, b2c, b2d);
    })
}

/// VALUES partial-update carrying the unit's current power so the resource bar moves LIVE (e.g. a
/// warrior gaining rage in combat — without this the bar stays at 0 and the client greys every
/// rage-costed spell). `power_b` is the power-type byte from `unit_bytes_0` and selects the descriptor
/// index, matching `build_create_object`: rage=1→POWER2, focus=2→POWER3, energy=3→POWER4,
/// happiness=4→POWER5, mana=0→POWER1. Player mask + the `dirty_reset` discipline (no OBJECT_FIELD_TYPE).
pub fn build_power_values(guid: u64, power_b: u8, power_cur: u32) -> SMSG_UPDATE_OBJECT {
    let cur = power_cur as i32;
    player_values(guid, |player| match power_b {
        1 => {
            player.set_unit_power2(cur);
        }
        2 => {
            player.set_unit_power3(cur);
        }
        3 => {
            player.set_unit_power4(cur);
        }
        4 => {
            player.set_unit_power5(cur);
        }
        _ => {
            player.set_unit_power1(cur);
        }
    })
}

/// VALUES partial-update carrying `UNIT_FIELD_TARGET` so observers see who a unit has selected (the
/// target ring / target-of-target / assist). Relayed for any unit (a creature's aggro target too).
/// UNIT mask + the `dirty_reset` discipline (never re-sends OBJECT_FIELD_TYPE).
pub fn build_target_values(guid: u64, target: u64) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_target(Guid::new(target));
    })
}

/// VALUES partial-update carrying `UNIT_FIELD_MAXHEALTH` + the unit's max power, so a non-level-up
/// max-vitals change (e.g. +Stamina/+Intellect gear or a Fortitude/Mark aura) moves the bar
/// DENOMINATOR live for observers. `power_b` selects the max-power descriptor exactly like
/// `build_power_values`. UNIT mask + the `dirty_reset` discipline (never re-sends OBJECT_FIELD_TYPE).
pub fn build_max_vitals_values(
    guid: u64,
    max_health: u32,
    power_b: u8,
    max_power: u32,
) -> SMSG_UPDATE_OBJECT {
    let max_p = max_power as i32;
    unit_values(guid, |unit| {
        unit.set_unit_maxhealth(max_health as i32);
        match power_b {
            1 => {
                unit.set_unit_maxpower2(max_p);
            }
            2 => {
                unit.set_unit_maxpower3(max_p);
            }
            3 => {
                unit.set_unit_maxpower4(max_p);
            }
            4 => {
                unit.set_unit_maxpower5(max_p);
            }
            _ => {
                unit.set_unit_maxpower1(max_p);
            }
        }
    })
}

/// Build a VALUES partial-update carrying `UNIT_FIELD_RESISTANCES[0]` — the "Armor" slot (descriptor
/// index 155, read by the paperdoll via `UnitArmor`) — so the character sheet's Armor readout moves LIVE
/// when an `A_MOD_RESISTANCE(armor)` aura applies/expires (e.g. Demon Skin) or armor gear is
/// equipped/unequipped. `armor` is the gateway-computed EFFECTIVE armor (base + armor auras + gear; see
/// `stdb::armor::effective_armor`), which equals the module's combat `effective_armor` — so the sheet
/// shows exactly what physical mitigation uses. UNIT mask (armor is a UNIT field, like max-vitals) + the
/// same `dirty_reset` discipline as `build_health_values` so the wire carries ONLY index 155 and never
/// re-sends OBJECT_FIELD_TYPE (the 5875 null+0x110 crash trap).
/// Build a VALUES partial-update carrying armor WITH its green "(+N)" split:
/// `UNIT_FIELD_RESISTANCES[0]` = the effective TOTAL and `PLAYER_FIELD_RESISTANCEBUFFMODSPOSITIVE[0]`
/// (index 1187, PLAYER block) = the positive AURA portion the paperdoll colors green.
/// TWO wrong guesses preceded this (live-found white armor both rounds): the buff-mods fields are
/// NOT unit fields in 1.12 — unit-space 162 is UNIT_FIELD_BASE_MANA — they live in the PLAYER
/// block, where gtker HAS a typed int setter. Player mask (self-only push; only your own sheet
/// reads it), same dirty_reset discipline as every partial VALUES.
pub fn build_armor_values(guid: u64, total: u32, pos_buff: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |p| {
        p.set_unit_normal_resistance(total as i32);
        p.set_player_field_resistancebuffmodspositive(pos_buff as i32);
    })
}

/// The character-sheet paperdoll numbers — a plain READ of `module::spell::recompute_sheet`'s output
/// (#517; `stdb::armor::sheet_stats` fetches the row), rendered by [`build_sheet_stats_values`].
/// `strength`/`agility`/`stamina`/`intellect`/`spirit` are the EFFECTIVE attribute (base + bonus, the
/// white total `UNIT_FIELD_STAT0..4` wants); `*_bonus` is the SIGNED aura+gear(+enchant) delta the
/// module already folded — split into the green/red paperdoll halves below via plain sign arithmetic,
/// not a second aura read. `attack_power` is the stat-derived base AP; `ap_mods` is the `A_MOD_COMBAT(ATTACK_POWER)`
/// aura portion alone (Battle Shout) — vanilla renders those through two different wire fields.
/// `crit_pct` (#532) is `module::combat::effective_crit_bp`/100.0 — the SAME basis-point value the
/// swing table rolls against, converted to the float percent `PLAYER_CRIT_PERCENTAGE` wants; no
/// second crit formula lives on the gateway.
pub struct SheetStatsValues {
    pub strength: u32,
    pub agility: u32,
    pub stamina: u32,
    pub intellect: u32,
    pub spirit: u32,
    pub str_bonus: i32,
    pub agi_bonus: i32,
    pub sta_bonus: i32,
    pub int_bonus: i32,
    pub spi_bonus: i32,
    pub attack_power: u32,
    pub ap_mods: i32,
    pub dmg_min: u32,
    pub dmg_max: u32,
    pub crit_pct: f32,
}

/// Build a VALUES partial-update carrying the paperdoll numbers: the five EFFECTIVE attributes (white
/// `UNIT_FIELD_STAT0..4`) PLUS the PLAYER_FIELD_POSSTAT/NEGSTAT split derived from each `*_bonus` sign
/// — the client renders the stat number GREEN with a "(+N)" tooltip when POSSTAT is non-zero (same
/// mechanism as the armor green), the stat-derived base attack power PLUS `UNIT_FIELD_ATTACK_POWER_MODS`
/// (the Battle-Shout-style aura portion, rendered as its own green "(+N)"), and the melee damage range
/// (min/max are FLOAT fields — the client renders "N - M" and derives DPS with BASEATTACKTIME). Player
/// mask (POSSTAT/NEGSTAT/ATTACK_POWER_MODS are PLAYER-block fields, like the armor buff-mods); typed
/// setters throughout.
pub fn build_sheet_stats_values(guid: u64, s: &SheetStatsValues) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |p| {
        p.set_unit_strength(s.strength as i32);
        p.set_unit_agility(s.agility as i32);
        p.set_unit_stamina(s.stamina as i32);
        p.set_unit_intellect(s.intellect as i32);
        p.set_unit_spirit(s.spirit as i32);
        p.set_player_field_posstat0(s.str_bonus.max(0));
        p.set_player_field_posstat1(s.agi_bonus.max(0));
        p.set_player_field_posstat2(s.sta_bonus.max(0));
        p.set_player_field_posstat3(s.int_bonus.max(0));
        p.set_player_field_posstat4(s.spi_bonus.max(0));
        p.set_player_field_negstat0(s.str_bonus.min(0));
        p.set_player_field_negstat1(s.agi_bonus.min(0));
        p.set_player_field_negstat2(s.sta_bonus.min(0));
        p.set_player_field_negstat3(s.int_bonus.min(0));
        p.set_player_field_negstat4(s.spi_bonus.min(0));
        p.set_unit_attack_power(s.attack_power as i32);
        // UNIT_FIELD_ATTACK_POWER_MODS packs two UNSIGNED shorts (pos, neg-as-magnitude), mirroring
        // mangos's `SetInt16Value(field, 0/1, ..)` — never a signed short (a negative AP debuff isn't
        // wired yet; `ap_mods` is currently always ≥0 from Battle Shout, so `neg` is 0 in practice).
        p.set_unit_attack_power_mods(s.ap_mods.max(0) as u16, (-s.ap_mods).max(0) as u16);
        p.set_unit_mindamage(s.dmg_min as f32);
        p.set_unit_maxdamage(s.dmg_max as f32);
        p.set_player_crit_percentage(s.crit_pct);
    })
}

pub fn build_resistance_values(guid: u64, armor: u32) -> SMSG_UPDATE_OBJECT {
    unit_values(guid, |unit| {
        unit.set_unit_normal_resistance(armor as i32);
    })
}

/// Build a VALUES partial-update carrying `PLAYER_FIELD_COINAGE` so the player's money updates LIVE
/// after looting (slice 3). Player mask. Same `dirty_reset` discipline as `build_health_values` so
/// the wire carries ONLY the coinage field and never re-sends OBJECT_FIELD_TYPE (the crash field).
pub fn build_coinage_values(guid: u64, money: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_field_coinage(money as i32);
    })
}

/// Build a VALUES partial-update carrying `PLAYER_AMMO_ID` so the client treats ammo as loaded —
/// without it Auto Shot greys out / refuses ("Ammo needs to be in the paper-doll ammo slot"). The value
/// is the ammo item ENTRY (mangos convention: `SetUInt32Value(PLAYER_AMMO_ID, item->GetEntry())`; the
/// client resolves the count + projectile from it). Uses gtker's typed `set_player_ammo_id` (it maps to
/// the vanilla field index internally — safer than a hand-rolled constant). Same `dirty_reset` discipline
/// as `build_coinage_values` so the wire carries ONLY this field and never re-sends OBJECT_FIELD_TYPE.
pub fn build_player_ammo_id_values(guid: u64, ammo_entry: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_ammo_id(ammo_entry as i32);
    })
}

/// `PLAYER_CHARACTER_POINTS1` partial VALUES — the talent pane's unspent-points counter. Pushed
/// LIVE after a talent pick (fires the client's CHARACTER_POINTS_CHANGED → TalentFrame refresh)
/// and once after the login CREATE for a character with spent points (the CREATE's formula counts
/// EARNED only). Same `dirty_reset` discipline as the other partial builders (never re-sends
/// OBJECT_FIELD_TYPE, the 5875 crash field).
/// `PLAYER_SKILL_INFO[slot]` partial VALUES — a LIVE skill-up/train moves the open skill
/// pane without a relog, and the 5875 client generates its own yellow "Your skill in X has
/// increased to N." chat line from this field change (mangos sends no chat packet — verified
/// against cm_Player::UpdateSkillPro). Slot comes from the session's skill-slot map (the login
/// CREATE layout, `skill_slot_layout`); same dirty_reset discipline as every partial builder.
/// Build a VALUES partial carrying only the owner's `UNIT_FIELD_SUMMON`: points the player at
/// their summoned pet's guid (what the client's pet frame keys on). Pass 0 to clear on despawn.
/// Same dirty_reset discipline as every VALUES builder here (see `build_health_values`).
pub fn build_owner_summon_values(owner_guid: u64, pet_guid: u64) -> SMSG_UPDATE_OBJECT {
    player_values(owner_guid, |player| {
        player.set_unit_summon(Guid::new(pet_guid));
    })
}

pub fn build_skill_values(
    guid: u64,
    slot: u8,
    skill: Skill,
    cur: u16,
    max: u16,
) -> Option<SMSG_UPDATE_OBJECT> {
    let idx = SkillInfoIndex::try_from(slot).ok()?;
    Some(player_values(guid, |player| {
        player.set_player_skill_info(SkillInfo::new(skill, 0, cur, max, 0, 0), idx);
    }))
}

pub fn build_talent_points_values(guid: u64, points: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_character_points1(points as i32);
    })
}

/// Build a VALUES partial-update carrying `PLAYER_XP` + `PLAYER_NEXT_LEVEL_XP` so the client's XP
/// bar advances LIVE (Tier 5 / XP). `SMSG_LOG_XPGAIN` only shows the floating "+N" text — the bar
/// itself is driven by these descriptor fields, which otherwise only sync at create-object (login).
/// Player-only (these are player fields). Same `dirty_reset` discipline as `build_health_values` so
/// the wire carries ONLY the two xp fields and never re-sends OBJECT_FIELD_TYPE (the crash field).
pub fn build_player_xp_values(guid: u64, xp: u32, next_level_xp: u32) -> SMSG_UPDATE_OBJECT {
    player_values(guid, |player| {
        player.set_player_xp(xp as i32);
        player.set_player_next_level_xp(next_level_xp as i32);
    })
}

/// Build `SMSG_UPDATE_AURA_DURATION` (opcode 0x0137) — the buff/debuff TIMER for the player's OWN
/// auras. The 1.12 UpdateMask aura array carries NO duration, so without this packet the client shows
/// "0 seconds" and flashes the icon (the buff looks about to expire). `duration_ms` is the remaining
/// window for the aura in `slot`. Sent after the aura VALUES sync when an aura with a finite duration
/// is applied/refreshed on this player.
pub fn build_aura_duration(slot: u8, duration_ms: u32) -> SMSG_UPDATE_AURA_DURATION {
    SMSG_UPDATE_AURA_DURATION {
        aura_slot: slot,
        aura_duration: duration_ms,
    }
}

/// Build a VALUES partial-update setting a single `PLAYER_FIELD_INV_SLOT[slot]` to `item_guid` (pass
/// `0` to CLEAR the slot) — the slot→item pointer the client reads to place an item in a bag cell. A
/// bought/looted item only appears (and a sold/consumed item only vanishes) LIVE if this rides
/// alongside the item CREATE/DESTROY; without it the change shows only after a relog. `None` for a
/// non-inventory slot ordinal. Player mask + the same `dirty_reset` discipline as the rest of this
/// family (so it never re-sends OBJECT_FIELD_TYPE — the 5875 crash field).
pub fn build_inv_slot_values(
    player_guid: u64,
    slot: u8,
    item_guid: u64,
) -> Option<SMSG_UPDATE_OBJECT> {
    let s = ItemSlot::try_from(slot).ok()?;
    Some(player_values(player_guid, |player| {
        player.set_player_field_inv(s, Guid::new(item_guid));
    }))
}

/// Build a VALUES partial-update for `PLAYER_VISIBLE_ITEM[slot]` — the descriptor the client
/// renders the 3D gear model (and paperdoll slot) from. The LOGIN create sets
/// these (entity.rs), but a MID-SESSION equip only relayed the INV_SLOT guid pointer, so gear
/// never appeared on the model until relog. `entry` 0 clears (unequip). `None` for a
/// non-equipment slot (bags/backpack are not model-visible).
pub fn build_visible_item_values(
    player_guid: u64,
    slot: u8,
    entry: u32,
) -> Option<SMSG_UPDATE_OBJECT> {
    let vi_index = super::entity::visible_item_index(slot)?;
    Some(player_values(player_guid, |player| {
        player.set_player_visible_item(
            VisibleItem {
                item: entry,
                ..Default::default()
            },
            vi_index,
        );
    }))
}

/// Build a VALUES partial-update for an ITEM object carrying its stack count + durability — so a stack
/// merge/split or a repair/wear shows LIVE without a relog (the item's OWN fields, distinct from the
/// player's bag-slot pointer in [`build_inv_slot_values`]). Item mask + the same `dirty_reset`
/// discipline as the player/unit families (never re-sends OBJECT_FIELD_TYPE).
pub fn build_item_values(guid: u64, stack_count: u32, durability: u32) -> SMSG_UPDATE_OBJECT {
    let mut item = UpdateItem::builder().finalize();
    item.dirty_reset();
    item.set_item_stack_count(stack_count.max(1) as i32);
    item.set_item_durability(durability as i32);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Item(item),
        }],
    }
}

/// Build a raw VALUES partial-update on a CONTAINER (bag) object setting `CONTAINER_FIELD_SLOT_N`
/// to `item_guid` (pass `0` to CLEAR the slot pointer). `slot_in_bag` is 0-indexed (slot 0 is the
/// first bag slot, matching `CONTAINER_FIELD_SLOT_1`). Uses the raw path because the gtker vanilla
/// `UpdateContainerBuilder` only exposes `set_container_slot_1`; slots 1+ are unreachable through
/// its typed API (the same 'gtker descriptor-setter wall' as multi-aura). Returns `(opcode, body)`
/// for `Outbound::Raw`. Does NOT set `OBJECT_FIELD_TYPE` — safe on a partial VALUES update.
pub fn build_container_slot_values(
    bag_guid: u64,
    slot_in_bag: u8,
    item_guid: u64,
) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    // CONTAINER_FIELD_SLOT_1 = 50; each slot is a u64 spanning two u32 field indices.
    let field = update_mask::idx::CONTAINER_FIELD_SLOT_1 + u16::from(slot_in_bag) * 2;
    mask.set_u64(field, item_guid);
    build_values_update_raw(bag_guid, &mask)
}

// One argument per UPDATE_FIELD the ding writes — the wire's field list, not an accidental parameter pile.
#[allow(clippy::too_many_arguments)]
/// Build a VALUES partial-update for a level-up "ding" (Tier 5 / XP). Pushes the descriptor fields
/// `SMSG_LEVELUP_INFO` does NOT carry — `UNIT_FIELD_LEVEL` (idx 34; the character panel reads this,
/// the popup is cosmetic only), `UNIT_FIELD_MAXHEALTH` + `UNIT_FIELD_HEALTH` (the new-level full
/// heal), and `PLAYER_XP`/`PLAYER_NEXT_LEVEL_XP`. UpdatePlayer mask (player descriptor). Same
/// `dirty_reset` discipline as `build_health_values`: finalize, reset, then re-set ONLY these fields,
/// so OBJECT_FIELD_TYPE is never re-sent (re-sending it strips the PLAYER bit → 5875 client crash).
pub fn build_levelup_values(
    guid: u64,
    level: u32,
    health: u32,
    max_health: u32,
    xp: u32,
    next_level_xp: u32,
    power_b: u8,
    max_power: u32,
) -> SMSG_UPDATE_OBJECT {
    let max_p = max_power as i32;
    player_values(guid, |player| {
        player.set_unit_level(level as i32);
        player.set_unit_maxhealth(max_health as i32);
        player.set_unit_health(health as i32);
        // The ding also moves the mana/rage DENOMINATOR (new-level max power), matching build_max_vitals_values.
        match power_b {
            1 => {
                player.set_unit_maxpower2(max_p);
            }
            2 => {
                player.set_unit_maxpower3(max_p);
            }
            3 => {
                player.set_unit_maxpower4(max_p);
            }
            4 => {
                player.set_unit_maxpower5(max_p);
            }
            _ => {
                player.set_unit_maxpower1(max_p);
            }
        }
        player.set_player_xp(xp as i32);
        player.set_player_next_level_xp(next_level_xp as i32);
        // PLAYER_CHARACTER_POINTS1 = free talent points earned so far (level − 9, floor 0).
        // Only non-zero from L10 onward (1 point per level starting at 10). The CREATE packet sets
        // this too, but without a mid-session VALUES update the talent pane stays at 0 until relog.
        // NOTE: does NOT subtract already-spent points (no game_character_talent read here) — that
        // refinement is a separate work item.
        player.set_player_character_points1((level as i32 - 9).max(0));
    })
}

#[cfg(test)]
mod lint_tests {
    use super::*;
    use wow_world_messages::vanilla::ServerMessage;

    /// The TYPED-path half of the packet-lint wall (pairs with
    /// `world::packet_lint`'s raw-frame half): serialize a representative from every
    /// `build_*_values` shape in this file and assert the wire mask NEVER carries
    /// `OBJECT_FIELD_TYPE` (bit 2) — the 5875 null+0x110 crash class a missing `dirty_reset`
    /// reintroduces. `lyracore_shared::values_mask` decodes the frames exactly as the client would.
    #[test]
    fn every_values_builder_serializes_without_the_object_type_bit() {
        let g = 0x9u64;
        let msgs: Vec<(&str, SMSG_UPDATE_OBJECT)> = vec![
            ("health", build_health_values(g, 41)),
            ("ghost", build_ghost_values(g, 0x10, 0)),
            ("dynamic_flags", build_dynamic_flags_values(g, 1)),
            ("unit_flags", build_unit_flags_values(g, 0x80000)),
            ("power", build_power_values(g, 0, 55)),
            ("target", build_target_values(g, 0xF130_0000_0000_0001)),
            ("max_vitals", build_max_vitals_values(g, 100, 0, 200)),
            ("armor", build_armor_values(g, 60, 20)),
            ("resistance", build_resistance_values(g, 40)),
            ("coinage", build_coinage_values(g, 900)),
            ("ammo", build_player_ammo_id_values(g, 2512)),
            (
                "owner_summon",
                build_owner_summon_values(g, 0xF130_0000_0000_0009),
            ),
            (
                "skill",
                build_skill_values(g, 3, Skill::Fishing, 100, 150).unwrap(),
            ),
            ("talent_points", build_talent_points_values(g, 2)),
            ("xp", build_player_xp_values(g, 400, 1000)),
            (
                "inv_slot",
                build_inv_slot_values(g, 15, 0x4000_0000_0000_0001).unwrap(),
            ),
            (
                "visible_item",
                build_visible_item_values(g, 15, 25).unwrap(),
            ),
            ("item", build_item_values(0x4000_0000_0000_0001, 5, 70)),
        ];
        for (name, m) in msgs {
            let mut buf = Vec::new();
            m.write_unencrypted_server(&mut buf).unwrap();
            // Unencrypted server framing: size u16 BE + opcode u16 LE, then the body.
            let body = &buf[4..];
            let updates = lyracore_shared::values_mask::parse_values_updates(body);
            assert!(
                !updates.is_empty(),
                "{name}: no VALUES block decoded — framing drift?"
            );
            for u in &updates {
                assert!(
                    !u.fields.iter().any(|&(idx, _)| idx == 2),
                    "{name}: OBJECT_FIELD_TYPE (bit 2) leaked into a VALUES partial — dirty_reset missing (5875 crash class)"
                );
                assert!(!u.fields.is_empty(), "{name}: empty VALUES mask");
            }
        }
    }
}
