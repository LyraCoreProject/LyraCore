//! Entity (player/creature) wire mapping: `EntityView`, the CREATE_OBJECT2 self/peer spawn, the
//! post-login SMSG sequence, and destroy. Pure code-motion out of `mod.rs`.

use super::*;

/// A minimal mirror of the entity-row fields the encoder needs. The gateway fills this from
/// the `game_world_entity` subscription cache; it is decoupled from the module's table type.
///
/// `zone_id` is the one field not on the `game_world_entity` row — it rides along from the
/// `game_character` row at login so the gateway can address `SMSG_BINDPOINTUPDATE` (the world
/// entity carries only `map_id`/position; deriving a zone from position would need map/area
/// geometry the entity row deliberately doesn't carry, so the gateway rides the value in from
/// login instead).
#[derive(Clone, Debug, Default)]
pub struct EntityView {
    pub guid: u64,
    pub map_id: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub last_move_ms: u32,
    /// Live MovementFlags (u32) — stamped into the CREATE so a peer spawns in its current move state.
    pub movement_flags: u32,
    /// GM playtest run-speed multiplier in basis points (10_000 = 1.0×). The CREATE movement block
    /// carries the derived speed so an entity rebuild (for example, a cross-map teleport) preserves
    /// the client's movement prediction without waiting for an UPDATE-only speed relay.
    pub run_speed_mult_bp: u32,
    pub type_mask: u32,
    pub entry: u32,
    pub scale_x: f32,
    pub health: u32,
    pub max_health: u32,
    pub power: u32,
    pub max_power: u32,
    pub level: u32,
    pub faction_template: u32,
    /// UNIT_FIELD_TARGET — who this unit has selected (relayed so peers see the target ring / assist).
    pub target_guid: u64,
    pub unit_bytes_0: u32,
    pub display_id: u32,
    pub native_display_id: u32,
    pub unit_flags: u32,
    pub base_attack_time_ms: u32,
    pub dynamic_flags: u32,
    pub player_bytes: u32,
    pub player_bytes_2: u32,
    pub player_bytes_3: u32,
    pub player_flags: u32,
    pub xp: u32,
    pub next_level_xp: u32,
    pub money: u32, // PLAYER_FIELD_COINAGE for a player; corpse loot for a creature (slice 3)
    pub unit_bytes_1: u32, // UNIT_FIELD_BYTES_1; byte 3 carries UNIT_VIS_FLAG_GHOST while a ghost (slice 5)
    // The five base attributes (UNIT_FIELD_STAT0..4) for the character sheet; 0 for a creature.
    pub strength: u32,
    pub agility: u32,
    pub stamina: u32,
    pub intellect: u32,
    pub spirit: u32,
    pub npc_flags: u32, // UNIT_NPC_FLAGS — gossip/vendor/etc. for a creature; 0 for a player
    /// Owning player for a SUMMONED unit (warlock pet) — drives UNIT_FIELD_SUMMONEDBY/CREATEDBY on
    /// the pet CREATE so the 5875 client can bind it to the owner's pet frame (023). 0 = wild.
    pub owner_guid: u64,
    /// UNIT_FIELD_RESISTANCES[0] — the EFFECTIVE armor shown on the character sheet (base + armor auras +
    /// gear; see `stdb::armor::effective_armor`). Defaults to the server-authoritative BASE armor (the
    /// module's agility*2, also what combat mitigates with) for peers/creatures, where the extra fold isn't
    /// worth it (your sheet shows only your own armor); the self-login CREATE overrides it with the
    /// gear-folded value so the operator sees real worn armor on relog (auras self-correct via the on_aura
    /// relay). Matches the module's combat `effective_armor`, so the sheet equals the mitigation value.
    pub effective_armor: u32,
    /// Persisted hearthstone / bind-point from `game_character.home_*`.  Zero-initialised for
    /// creature rows (which never build a `SMSG_BINDPOINTUPDATE`).
    pub home_map: u32,
    pub home_zone: u32,
    pub home_x: f32,
    pub home_y: f32,
    pub home_z: f32,
}

/// Whether a create block is for the player's own character (SELF flag set) or a peer the
/// observer is seeing. Both use `CREATE_OBJECT2`; SELF-ness lives in the movement flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateKind {
    SelfPlayer,
    Peer,
}

/// Map an equipment-slot ordinal (0..=18) to its `VisibleItemIndex` — the `PLAYER_VISIBLE_ITEM[slot]`
/// descriptor that renders gear on the 3D character model (slice-2). `None` for a non-equipment slot
/// (equipped bags 19..22, backpack 23..38, bank, …): those carry an inventory-slot guid but nothing
/// model-visible. The index is 0-based and matches the equipment-slot ordinal 1:1 (15 = MAINHAND).
pub(crate) fn visible_item_index(slot: u8) -> Option<VisibleItemIndex> {
    use VisibleItemIndex::*;
    Some(match slot {
        0 => Index0,
        1 => Index1,
        2 => Index2,
        3 => Index3,
        4 => Index4,
        5 => Index5,
        6 => Index6,
        7 => Index7,
        8 => Index8,
        9 => Index9,
        10 => Index10,
        11 => Index11,
        12 => Index12,
        13 => Index13,
        14 => Index14,
        15 => Index15,
        16 => Index16,
        17 => Index17,
        18 => Index18,
        _ => return None,
    })
}

/// Build the `SMSG_UPDATE_OBJECT` CREATE_OBJECT2 block for an entity (Phase 4 self-spawn /
/// Phase 6 first-sighting). The `wow_world_messages` update-mask builder owns the descriptor
/// bit indices, packed-guid encoding, and the movement-block layout — so the gateway only maps
/// the row's values to typed setters. The row's packed bytes (`unit_bytes_0`, `player_bytes*`)
/// are unpacked here into the typed values the builder wants.
///
/// `inventory` (items slices 1–2) is `(slot ordinal, item guid, item entry)` triples. Each seeds the
/// player's `PLAYER_FIELD_INV_SLOT_*` descriptor (the slot→guid pointer, ANY slot — gtker's typed
/// `set_player_field_inv(ItemSlot, Guid)` is not walled like the aura array); and for an EQUIPMENT
/// slot (0..=18) it ALSO sets `PLAYER_VISIBLE_ITEM[slot]` to the item ENTRY so the 3D character model
/// renders the gear (slice-2). Only applied for a player CREATE; pass `&[]` for creatures/peers.
/// (Peers SHOULD eventually see equipped gear via VISIBLE_ITEM too, but slice-2 drives only self.)
/// The per-class STATIC skill base (spec schools + weapons + language) — the login CREATE's slot
/// layout starts from this. Shared with the live skill relay (234) so a line's PLAYER_SKILL_INFO
/// slot NEVER moves mid-session (the client keys the pane rows on the slot index).
pub fn class_static_skills(class_b: u8) -> &'static [(Skill, u16, u16)] {
    match class_b {
        1 => &[
            (Skill::Arms, 1, 1),
            (Skill::Fury, 1, 1),
            (Skill::Protection, 1, 1),
            (Skill::Defense, 1, 5),
            (Skill::Unarmed, 1, 5),
            (Skill::Swords, 1, 5),
            (Skill::Maces, 1, 5),
            (Skill::Axes, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
        8 => &[
            // Mage
            (Skill::Fire, 1, 1),
            (Skill::Frost, 1, 1),
            (Skill::Arcane, 1, 1),
            (Skill::Cloth, 1, 1),
            (Skill::Staves, 1, 5),
            (Skill::Daggers, 1, 5),
            (Skill::Wands, 1, 5),
            (Skill::Defense, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
        5 => &[
            // Priest
            (Skill::Holy, 1, 1),
            (Skill::Discipline, 1, 1),
            (Skill::Shadow, 1, 1),
            (Skill::Cloth, 1, 1),
            (Skill::Maces, 1, 5),
            (Skill::Staves, 1, 5),
            (Skill::Wands, 1, 5),
            (Skill::Defense, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
        9 => &[
            // Warlock
            (Skill::Affliction, 1, 1),
            (Skill::Demonology, 1, 1),
            (Skill::Destruction, 1, 1),
            (Skill::Cloth, 1, 1),
            (Skill::Daggers, 1, 5),
            (Skill::Swords, 1, 5),
            (Skill::Wands, 1, 5),
            (Skill::Defense, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
        11 => &[
            // Druid
            (Skill::Balance, 1, 1),
            (Skill::FeralCombat, 1, 1),
            (Skill::Restoration, 1, 1),
            (Skill::Leather, 1, 1),
            (Skill::Staves, 1, 5),
            (Skill::Maces, 1, 5),
            (Skill::Unarmed, 1, 5),
            (Skill::Defense, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
        2 => &[
            // Paladin (all 3 spec schools so an aura/spell finds its tab)
            (Skill::Holy, 1, 1),
            (Skill::Protection, 1, 1),
            (Skill::Retribution, 1, 1),
            (Skill::Defense, 1, 5),
            (Skill::Maces, 1, 5),
            (Skill::Swords, 1, 5),
            (Skill::Mail, 1, 1),
            (Skill::LanguageCommon, 300, 300),
        ],
        3 => &[
            // Hunter
            (Skill::BeastMastery, 1, 1),
            (Skill::Marksmanship, 1, 1),
            (Skill::Survival, 1, 1),
            (Skill::Defense, 1, 5),
            (Skill::Bows, 1, 5),
            (Skill::Guns, 1, 5),
            (Skill::Daggers, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
        4 => &[
            // Rogue
            (Skill::Assassination, 1, 1),
            (Skill::Combat, 1, 1),
            (Skill::Subtlety, 1, 1),
            (Skill::Defense, 1, 5),
            (Skill::Daggers, 1, 5),
            (Skill::Swords, 1, 5),
            (Skill::Maces, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
        7 => &[
            // Shaman
            (Skill::ElementalCombat, 1, 1),
            (Skill::Enhancement, 1, 1),
            (Skill::Restoration, 1, 1),
            (Skill::Defense, 1, 5),
            (Skill::Maces, 1, 5),
            (Skill::Staves, 1, 5),
            (Skill::Mail, 1, 1),
            (Skill::LanguageCommon, 300, 300),
        ],
        _ => &[
            // unknown class: martial fallback (Attack + weapons; no spell tab)
            (Skill::Unarmed, 1, 5),
            (Skill::Swords, 1, 5),
            (Skill::Maces, 1, 5),
            (Skill::Axes, 1, 5),
            (Skill::Daggers, 1, 5),
            (Skill::Staves, 1, 5),
            (Skill::Defense, 1, 5),
            (Skill::LanguageCommon, 300, 300),
        ],
    }
}

/// The deterministic PLAYER_SKILL_INFO slot layout (234): the class static base, overridden by any
/// matching learned row, then every remaining learned line APPENDED — index i IS the descriptor
/// slot. Used by the login CREATE and the live relay's slot map; both MUST derive from this one fn.
pub fn skill_slot_layout(class_b: u8, learned: &[(u32, u16, u16)]) -> Vec<(Skill, u16, u16)> {
    let mut lines: Vec<(Skill, u16, u16)> = class_static_skills(class_b).to_vec();
    for &(line, cur, max) in learned {
        let Ok(sk) = Skill::try_from(line) else {
            continue;
        };
        match lines.iter_mut().find(|(s, _, _)| *s == sk) {
            Some(entry) => *entry = (sk, cur, max),
            None => lines.push((sk, cur, max)),
        }
    }
    lines
}

pub fn build_create_object(
    entity: &EntityView,
    kind: CreateKind,
    inventory: &[(u8, u64, u32)],
    learned_skills: &[(u32, u16, u16)],
) -> Result<SMSG_UPDATE_OBJECT> {
    // race/class/gender/power are packed in unit_bytes_0 for both players and creatures (a creature
    // seeds dummy-but-valid Human/Warrior/Male/Mana — the bytes are non-rendering for a Unit).
    let (race_b, class_b, gender_b, power_b) = unpack4(entity.unit_bytes_0);
    let race = Race::try_from(race_b).map_err(|_| anyhow!("invalid race {race_b}"))?;
    let class = Class::try_from(class_b).map_err(|_| anyhow!("invalid class {class_b}"))?;
    let gender = Gender::try_from(gender_b).map_err(|_| anyhow!("invalid gender {gender_b}"))?;
    let power = Power::try_from(power_b).map_err(|_| anyhow!("invalid power {power_b}"))?;

    // Branch on the PLAYER bit: a creature (type Unit) gets a Unit update mask + ObjectType::Unit;
    // a player gets the full player descriptor block. The game_world_entity row carries both shapes.
    let is_player = entity.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0;
    let (object_type, mask2) = if is_player {
        let (skin, face, hair_style, hair_color) = unpack4(entity.player_bytes);
        let (facial_hair, pb2_b, pb2_c, pb2_d) = unpack4(entity.player_bytes_2);
        let (pb3_a, pb3_b, pb3_c, pb3_d) = unpack4(entity.player_bytes_3);
        let (ub1_a, ub1_b, ub1_c, ub1_d) = unpack4(entity.unit_bytes_1); // ghost vis bit in byte 3 (slice 5)

        // Character-sheet weapon damage (PARITY with module `combat::tables`): attack power =
        // (level*3 + str*2) - 20 (saturating); the UNARMED swing range = base[1..=3] + the
        // AP*attack_time/(14*1000) bonus. The module stays authoritative for the actual swing — this
        // only feeds the paperdoll's UNIT_FIELD_MINDAMAGE/MAXDAMAGE/ATTACK_POWER (below). A future
        // pass folds the equipped main-hand weapon's range in (today these are unarmed-only).
        let attack_power = (entity.level * 3 + entity.strength * 2).saturating_sub(20);
        let dmg_bonus = attack_power * entity.base_attack_time_ms / 14_000;
        let (min_damage, max_damage) = (1 + dmg_bonus, 3 + dmg_bonus);

        let mut builder = UpdatePlayer::builder()
            .set_object_guid(Guid::new(entity.guid))
            .set_object_scale_x(entity.scale_x)
            .set_unit_health(entity.health as i32)
            .set_unit_maxhealth(entity.max_health as i32)
            .set_unit_level(entity.level as i32)
            .set_unit_factiontemplate(entity.faction_template as i32)
            .set_unit_target(Guid::new(entity.target_guid))
            .set_unit_bytes_0(race, class, gender, power)
            .set_unit_displayid(entity.display_id as i32)
            .set_unit_nativedisplayid(entity.native_display_id as i32)
            .set_unit_flags(entity.unit_flags as i32)
            .set_unit_baseattacktime(entity.base_attack_time_ms as i32)
            .set_unit_bytes_1(ub1_a, ub1_b, ub1_c, ub1_d) // ghost render bit (slice 5)
            .set_player_features(skin, face, hair_style, hair_color)
            .set_player_bytes_2(facial_hair, pb2_b, pb2_c, pb2_d)
            .set_player_bytes_3(pb3_a, pb3_b, pb3_c, pb3_d)
            .set_player_flags(entity.player_flags as i32)
            .set_player_xp(entity.xp as i32)
            .set_player_next_level_xp(entity.next_level_xp as i32)
            .set_player_field_coinage(entity.money as i32) // purse in copper (slice 3)
            // Free talent points (PLAYER_CHARACTER_POINTS1) so the talent pane shows a non-zero count
            // (rank 27). Mirrors the module's `talent::talent_points_available`: 1/level from level 10
            // (= level - 9), clamped at 0 below 10. NOTE: this does NOT yet subtract points already spent
            // (the gateway doesn't read game_character_talent here) — it's a display of points EARNED, exact
            // for a fresh character and approximate after spending. Learning is reducer-gated server-side;
            // the in-client *clickable* pane additionally needs talent data aligned to the client's
            // Talent.dbc (real ids + rank-spells) — a separate follow-up, not wired here.
            .set_player_character_points1((entity.level as i32 - 9).max(0))
            // The five base attributes (UNIT_FIELD_STAT0..4) so the character sheet shows real
            // numbers (set at login from the cmangos curve; 0 until that data is loaded). Part of the
            // full CREATE mask — not a partial VALUES update — so no dirty_reset concern.
            .set_unit_strength(entity.strength as i32)
            .set_unit_agility(entity.agility as i32)
            .set_unit_stamina(entity.stamina as i32)
            .set_unit_intellect(entity.intellect as i32)
            .set_unit_spirit(entity.spirit as i32)
            // EFFECTIVE armor for the character sheet (UNIT_FIELD_RESISTANCES[0], the "normal"/physical
            // resistance slot, read by the paperdoll via `UnitArmor`). `effective_armor` = base (the
            // module's agility*2, used for combat mitigation) + worn gear armor (folded by
            // `stdb::armor::effective_armor` at the self-login call site); it equals the module's combat
            // `effective_armor`, so the displayed Armor matches what mitigation uses. Login-present armor
            // AURAS aren't folded here (the coordinator cache has no game_aura) — the on_aura relay pushes
            // them the instant they insert. Part of the full CREATE mask (OBJECT_FIELD_TYPE belongs here),
            // so no dirty_reset concern.
            .set_unit_normal_resistance(entity.effective_armor as i32)
            // Weapon damage for the character sheet's Main-Hand panel. gtker's UpdatePlayer has no
            // default for these float/int fields, so leaving them unset makes the 5875 client read
            // uninitialized floats → "-1.#IND" (NaN) in the damage/DPS tooltip. Send the derived values.
            .set_unit_attack_power(attack_power as i32)
            // NOTE: do NOT set UNIT_FIELD_ATTACK_POWER_MULTIPLIER — the client treats it as `1 + value`,
            // so sending 1.0 DOUBLES the displayed attack power (134 → 268) and does NOT fix the paperdoll
            // tooltip's "x0%"/1.#INF (that `percent` comes from a different field — still TODO). Leaving it
            // unset (default 0.0 ⇒ ×1.0) keeps the attack-power readout correct. The melee-damage STAT line
            // (UNIT_FIELD_MINDAMAGE/MAXDAMAGE) already displays correctly (20–22); the slot hover tooltip's
            // INF is a separate cosmetic issue and combat is server-authoritative regardless.
            .set_unit_mindamage(min_damage as f32)
            .set_unit_maxdamage(max_damage as f32)
            // PLAYER_FIELD_MOD_DAMAGE_DONE_PCT[physical] = 1.0 — THE fix for the Main-Hand tooltip's
            // "1.#INF - 1.#INF x0%" / "-1.$" DPS. It's the `percent` the 5875 paperdoll DIVIDES weapon
            // damage by (UnitDamage's 7th return: `displayDamage = MINDAMAGE / percent`); unset it arrives
            // as 0.0 → divide-by-zero → +Inf. It's a FLOAT descriptor (a ×multiplier, default 1.0) but
            // gtker's setter is typed i32, so we pass the bit pattern of 1.0f32 (0x3F800000). The typed
            // setter reaches school index 0 = physical, which is exactly the melee school. (The melee
            // *stat line* already read correctly from MINDAMAGE/MAXDAMAGE; this fixes the hover tooltip.)
            .set_player_field_mod_damage_done_pct(1.0_f32.to_bits() as i32);

        // Current/max power live at the descriptor index matching the unit's power type
        // (Mana=1, Rage=2, Focus=3, Energy=4, Happiness=5).
        let (power_cur, power_max) = (entity.power as i32, entity.max_power as i32);
        builder = match power_b {
            1 => builder
                .set_unit_power2(power_cur)
                .set_unit_maxpower2(power_max),
            2 => builder
                .set_unit_power3(power_cur)
                .set_unit_maxpower3(power_max),
            3 => builder
                .set_unit_power4(power_cur)
                .set_unit_maxpower4(power_max),
            4 => builder
                .set_unit_power5(power_cur)
                .set_unit_maxpower5(power_max),
            _ => builder
                .set_unit_power1(power_cur)
                .set_unit_maxpower1(power_max),
        };

        // Skill lines — without them the spellbook has no tab to hold a class ability and the client
        // throws SpellBookFrame.lua:342 "Invalid spell slot in GetSpellTexture". Vanilla routes each
        // known spell to a tab via SkillLineAbility.skillLine, so the player must POSSESS that line.
        // Battle Shout (6673) lives in Fury (256) — NOT Arms — so Fury is the load-bearing one; we
        // send all three Warrior class lines so any class ability resolves, plus the weapon/defense
        // skills (1/5 at L1) and Common (300/300). Warrior fixture only (class 1); other classes get
        // none until per-character skill data exists (importer follow-up). Creatures get none.
        // Additive to the full CREATE mask (where OBJECT_FIELD_TYPE correctly belongs) — not a partial
        // VALUES update, so the dirty_reset discipline does not apply here.
        // Per-class skill lines (parity #1): a class's spell-school tabs (so its spells render a spellbook
        // tab instead of the SpellBookFrame Lua crash) + its weapon/armor/defense skills + Common. We send
        // ALL of a class's spec schools so any ability resolves a tab. A class with no ported spells gets a
        // martial set (it can melee; no spells means no spell tab is needed). Warrior is byte-identical to
        // the prior hardcoded set. Per-RACE language (Orcish for Horde) is a follow-up — Common for now.
        // LIVE skills (work-item 061, replacing the old static Cooking-1/75 fixture): the block
        // starts from the per-class STATIC base above (the spec-school/weapon/language rows the
        // client needs pre-import), then the character's `game_player_skill` rows OVERRIDE any
        // matching line (a trained weapon skill shows its real climb at relog) and every remaining
        // learned line (Mining, Herbalism, First Aid, Cooking, …) is APPENDED at its real
        // current/max — so a profession appears iff learned, and never as a phantom fixture. An
        // unrecognized line id is skipped (Skill::try_from guards the enum), and the for-loop's
        // `SkillInfoIndex::try_from` still guards the slot count (128 descriptor slots). This is the
        // login-create snapshot; the LIVE mid-session climb relay is still deferred (the skill
        // block is descriptor-wall territory — the pane refreshes on relog).
        let skill_lines = skill_slot_layout(class_b, learned_skills);
        for (i, &(skill, min, max)) in skill_lines.iter().enumerate() {
            if let Ok(idx) = SkillInfoIndex::try_from(i as u8) {
                builder =
                    builder.set_player_skill_info(SkillInfo::new(skill, 0, min, max, 0, 0), idx);
            }
        }
        // Items slices 1–2: point the player's inventory-slot descriptors at the items it carries, so
        // the client renders them in those slots. `set_player_field_inv` writes the 2-word guid at
        // `PLAYER_FIELD_INV_SLOT_HEAD + slot*2`. An unknown slot ordinal is skipped (never panics).
        // For an EQUIPMENT slot (0..=18) also set PLAYER_VISIBLE_ITEM[slot] to the item ENTRY — that
        // descriptor is what makes the weapon/armor appear ON the 3D character model (slice-2). Both
        // are full-CREATE-mask fields (OBJECT_FIELD_TYPE belongs here), so no dirty_reset concern.
        for &(slot, item_guid, entry) in inventory {
            if let Ok(s) = ItemSlot::try_from(slot) {
                builder = builder.set_player_field_inv(s, Guid::new(item_guid));
            }
            if let Some(vi_index) = visible_item_index(slot) {
                builder = builder.set_player_visible_item(
                    VisibleItem {
                        item: entry,
                        ..Default::default()
                    },
                    vi_index,
                );
            }
        }
        (ObjectType::Player, UpdateMask::Player(builder.finalize()))
    } else {
        // Creature (Unit): no player descriptor fields. `entry` is REQUIRED so the client can issue
        // CMSG_CREATURE_QUERY; without it the creature renders nameless.
        let mut unit_builder = UpdateUnit::builder()
            .set_object_guid(Guid::new(entity.guid))
            .set_object_entry(entity.entry as i32)
            .set_object_scale_x(entity.scale_x)
            .set_unit_health(entity.health as i32)
            .set_unit_maxhealth(entity.max_health as i32)
            .set_unit_level(entity.level as i32)
            .set_unit_factiontemplate(entity.faction_template as i32)
            .set_unit_target(Guid::new(entity.target_guid))
            .set_unit_bytes_0(race, class, gender, power)
            .set_unit_displayid(entity.display_id as i32)
            .set_unit_nativedisplayid(entity.native_display_id as i32)
            .set_unit_flags(entity.unit_flags as i32)
            .set_unit_baseattacktime(entity.base_attack_time_ms as i32)
            .set_unit_dynamic_flags(entity.dynamic_flags as i32) // slice 2: corpse/lootable bits
            .set_unit_npc_flags(entity.npc_flags as i32); // gossip/vendor/questgiver/trainer icons (item 6)
                                                          // 023: a SUMMONED unit (warlock pet) carries its owner in SUMMONEDBY/CREATEDBY — what the
                                                          // client needs to treat the unit as this player's pet (frame binding). Wild creatures skip
                                                          // both fields (byte-identical CREATE).
        if entity.owner_guid != 0 {
            unit_builder = unit_builder
                .set_unit_summonedby(Guid::new(entity.owner_guid))
                .set_unit_createdby(Guid::new(entity.owner_guid));
        }
        (ObjectType::Unit, UpdateMask::Unit(unit_builder.finalize()))
    };

    // A Living movement block carrying the entity's CURRENT movement flags, so a peer entering AOI
    // range mid-move spawns in its real state (running/strafing) rather than idle-floating (it missed
    // the MSG_MOVE_START sent while out of range). Only the directional + walk bits are stamped:
    // FORWARD|BACKWARD|STRAFE_LEFT|STRAFE_RIGHT|TURN_LEFT|TURN_RIGHT|WALK_MODE. The conditional-data
    // bits (ON_TRANSPORT 0x200, JUMPING/FALLING 0x2000, SWIMMING 0x200000, SPLINE_* 0x4/0x8000000) are
    // MASKED OUT — `MovementBlock_MovementFlags::new` takes their sub-struct payloads as args, and a
    // set bit with `None` payload would mis-encode; those states just spawn idle (corrected by the
    // peer's next packet). SELF is added only for the player's own body (peers omit it).
    const SIMPLE_MOVE_MASK: u32 = 0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x100; // = 0x13F
    let running_speed = speeds::RUN * (entity.run_speed_mult_bp as f32 / 10_000.0);
    let mut update_flag = MovementBlock_UpdateFlag::empty()
        .set_all(MovementBlock_UpdateFlag_All { unknown1: 1 })
        .set_living(MovementBlock_UpdateFlag_Living::Living {
            flags: MovementBlock_MovementFlags::new(
                entity.movement_flags & SIMPLE_MOVE_MASK,
                None,
                None,
                None,
                None,
                None,
            ),
            timestamp: entity.last_move_ms,
            living_position: Vector3d {
                x: entity.x,
                y: entity.y,
                z: entity.z,
            },
            living_orientation: entity.orientation,
            fall_time: 0.0,
            walking_speed: speeds::WALK,
            running_speed,
            backwards_running_speed: speeds::RUN_BACK,
            swimming_speed: speeds::SWIM,
            backwards_swimming_speed: speeds::SWIM_BACK,
            turn_rate: speeds::TURN_RATE,
        });
    if kind == CreateKind::SelfPlayer {
        update_flag = update_flag.set_self();
    }

    Ok(SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::CreateObject2 {
            guid3: Guid::new(entity.guid),
            object_type,
            movement2: MovementBlock { update_flag },
            mask2,
        }],
    })
}

/// Emit the post-login SMSG sequence (Phase 4, gateway translation §5) to the owner, in the
/// order the client expects — `SMSG_LOGIN_VERIFY_WORLD` must precede `SMSG_TUTORIAL_FLAGS`.
/// Slice values: zeroed account-data/factions/action-bar, real-time clock scale, no starting
/// spells, bind = the start position. All headers encrypted.
pub fn login_sequence_messages(
    entity: &EntityView,
    learned: &[u32],
    reputations: &[(i32, i32, bool)],
    player_actions: &[(u8, u32, u8)],
) -> Result<Vec<ServerOpcodeMessage>> {
    let map = Map::try_from(entity.map_id).map_err(|_| anyhow!("invalid map {}", entity.map_id))?;
    let area = Area::try_from(entity.zone_id).unwrap_or_default();
    let position = Vector3d {
        x: entity.x,
        y: entity.y,
        z: entity.z,
    };

    // Known spells + a default action bar. CRITICAL for combat: the baseline melee toggle is the
    // "Attack" ability (spell 6603). Without it known AND on the action bar, the 5875 client has no way
    // to start auto-attack — there's no default keybind, and right-click does NOT engage a NEUTRAL
    // (yellow) target — so it never emits CMSG_ATTACKSWING (confirmed via the inbound opcode trace:
    // CMSG_SET_SELECTION fires on target, CMSG_ATTACKSWING never does). So we grant Attack + lay out a
    // default bar (Attack in slot 0, then each known castable ability). A vanilla action button for a
    // spell is just the spell id (button type 0 = spell); the bar has 120 slots.
    const ATTACK_SPELL: u32 = 6603; // "Attack" — toggles melee auto-attack on the current target
                                    // Spellbook + action bar come from the character's `game_player_spell` rows — the same rows the
                                    // module's cast gate reads, so the client book can never show a spell the server would reject.
                                    // Creation granted the createinfo kit (class abilities + racials) as rows; trainers/talents/quests
                                    // append. Deduped so a spell isn't listed twice.
    let mut known: Vec<u32> = vec![ATTACK_SPELL];
    for &id in learned {
        if !known.contains(&id) {
            known.push(id);
        }
    }
    let initial_spells: Vec<InitialSpell> = known
        .iter()
        .map(|&id| InitialSpell {
            spell_id: id as u16,
            unknown1: 0,
        })
        .collect();
    // Action bar (work-item 212): if the character has IMPORTED `game_player_action` rows (creation
    // copied `game_createinfo_action` — the real vanilla default bar for this race/class), build the
    // bar from THOSE rows and trust them completely — no merge with the synth below, so a
    // real-but-Attack-less imported bar is never silently patched (the dump is expected to carry its
    // own auto-attack button if vanilla's real layout has one; double-injecting our own would put a
    // second, possibly redundant Attack somewhere the imported layout didn't intend). Each button
    // packs as `action | (action_type << 24)` — the vanilla client's own action-button encoding
    // (`action` in the low 24 bits, `action_type` the high byte; type 0 = spell, matching the synth
    // below's implicit type-0 raw-spell-id packing). Absent (pre-import, the common case today) →
    // fall back to the byte-identical synth that shipped before this work item.
    let mut action_buttons = [0u32; 120];
    if !player_actions.is_empty() {
        for &(button, action, action_type) in player_actions {
            if (button as usize) < action_buttons.len() {
                action_buttons[button as usize] = action | ((action_type as u32) << 24);
            }
        }
    } else {
        for (slot, &id) in known.iter().take(120).enumerate() {
            action_buttons[slot] = id;
        }
    }

    Ok(vec![
        ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(Box::new(SMSG_LOGIN_VERIFY_WORLD {
            map,
            position,
            orientation: entity.orientation,
        })),
        ServerOpcodeMessage::SMSG_ACCOUNT_DATA_TIMES(Box::new(SMSG_ACCOUNT_DATA_TIMES {
            data: [0u32; 32],
        })),
        ServerOpcodeMessage::SMSG_LOGIN_SETTIMESPEED(SMSG_LOGIN_SETTIMESPEED {
            // The weekday must agree with the date by the crate's calendar (2026-06-15 -> Tuesday);
            // the value is cosmetic (sets the in-game clock).
            datetime: DateTime::new(26, Month::June, 15, Weekday::Tuesday, 12, 0),
            timescale: 0.016_666_668,
        }),
        ServerOpcodeMessage::SMSG_TUTORIAL_FLAGS(Box::new(SMSG_TUTORIAL_FLAGS {
            tutorial_data: [0xFFFF_FFFFu32; 8],
        })),
        ServerOpcodeMessage::SMSG_INITIAL_SPELLS(Box::new(SMSG_INITIAL_SPELLS {
            unknown1: 0,
            // Known spells = "Attack" (6603) + the SHARED player-castable set (Battle Shout, Rend, …),
            // so the spellbook and the module's cast gate never drift. The Warrior abilities live in skill
            // lines sent at CREATE (Fury/Arms), so the client resolves their tabs. (Slice: the warrior set
            // is sent to the lone test character; a per-class known-spell list is an importer follow-up.)
            initial_spells,
            cooldowns: vec![],
        })),
        ServerOpcodeMessage::SMSG_ACTION_BUTTONS(Box::new(SMSG_ACTION_BUTTONS {
            data: action_buttons,
        })),
        ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(Box::new(SMSG_INITIALIZE_FACTIONS {
            // #13 slice 2: fold each persisted `game_player_reputation` row's standing into its stored
            // `reputation_index` slot (0..63) — the SAME Faction.dbc ReputationListID the live
            // SET_FACTION_STANDING relay addresses (see build_set_faction_standing_raw's crash note).
            // HARD GUARDRAIL: index by reputation_index, NEVER faction_id — faction_id indexed past the
            // client's 64-slot array caused the McBride ERROR #132 crash. Slots with no persisted row stay
            // Neutral/0 (the prior stub value), matching a faction the player has never gained rep with.
            factions: {
                let mut slots: Vec<FactionInitializer> = (0..64)
                    .map(|_| FactionInitializer {
                        flag: FactionFlag::empty(),
                        standing: 0,
                    })
                    .collect();
                for &(index, standing, at_war) in reputations {
                    if let Some(slot) = usize::try_from(index).ok().and_then(|i| slots.get_mut(i)) {
                        slot.standing = standing as u32; // signed standing, bit-cast (client reads it i32) — same as build_set_faction_standing_raw
                                                         // 195 slice B: the persisted At-War checkbox survives relog via this flag
                                                         // byte (AT_WAR = 0x02). VISIBLE (0x01) rides along — a slot with a stored
                                                         // row is a faction the player has met, and the client needs VISIBLE to
                                                         // render the pane row the checkbox lives on.
                        if at_war {
                            slot.flag = FactionFlag::new_visible().set_at_war();
                        }
                    }
                }
                slots
            },
        })),
        ServerOpcodeMessage::SMSG_SET_REST_START(SMSG_SET_REST_START { unknown1: 0 }),
        ServerOpcodeMessage::SMSG_BINDPOINTUPDATE(Box::new(SMSG_BINDPOINTUPDATE {
            // Always send the persisted hearthstone bind point (home_*). `create_character` seeds
            // home_* to the race start-position and `bind_home` updates them to the innkeeper;
            // both paths produce real coordinates.  A `home_map != 0` sentinel is wrong because
            // Eastern Kingdoms is map_id 0 — every human/dwarf/gnome character would fail that
            // check and receive the login position instead of their actual bind point.
            position: Vector3d {
                x: entity.home_x,
                y: entity.home_y,
                z: entity.home_z,
            },
            map: Map::try_from(entity.home_map).unwrap_or(map),
            area: Area::try_from(entity.home_zone).unwrap_or(area),
        })),
    ])
}

/// Build `SMSG_DESTROY_OBJECT` for a guid leaving view (Phase 7). Vanilla carries just the
/// guid (no `is_on_death`).
pub fn build_destroy_object(guid: u64) -> SMSG_DESTROY_OBJECT {
    SMSG_DESTROY_OBJECT {
        guid: Guid::new(guid),
    }
}

/// Build `SMSG_PET_SPELLS` for a freshly summoned pet (023): the default vanilla pet action bar —
/// Attack/Follow/Stay commands in slots 0–2, the pet's castable spells (if any) in slots 3–6, and
/// the Aggressive/Defensive/Passive react states in slots 7–9, each slot packed as
/// `action | (active_state << 24)` — the packing the 5875 client decodes. The pet starts Enabled,
/// Follow, Defensive — the 1.12 summon defaults.
pub fn build_pet_spells(pet_guid: u64, spells: &[u32]) -> SMSG_PET_SPELLS {
    // mangos ActiveStates: the high byte tagging what kind of button a slot holds.
    const ACT_COMMAND: u32 = 0x07; // attack/follow/stay
    const ACT_REACT: u32 = 0x06; // aggressive/defensive/passive
    const ACT_DISABLED: u32 = 0x81; // an autocastable spell, autocast OFF (the summon default)
    let pack = |action: u32, state: u32| (action & 0x00FF_FFFF) | (state << 24);
    let mut bars = [0u32; 10];
    bars[0] = pack(PetCommandState::Attack.as_int() as u32, ACT_COMMAND);
    bars[1] = pack(PetCommandState::Follow.as_int() as u32, ACT_COMMAND);
    bars[2] = pack(PetCommandState::Stay.as_int() as u32, ACT_COMMAND);
    for (i, &spell) in spells.iter().take(4).enumerate() {
        bars[3 + i] = pack(spell, ACT_DISABLED);
    }
    bars[7] = pack(PetReactState::Aggressive.as_int() as u32, ACT_REACT);
    bars[8] = pack(PetReactState::Defensive.as_int() as u32, ACT_REACT);
    bars[9] = pack(PetReactState::Passive.as_int() as u32, ACT_REACT);
    SMSG_PET_SPELLS {
        pet: Guid::new(pet_guid),
        action_bars: Some(SMSG_PET_SPELLS_action_bars {
            duration: 0, // permanent (a warlock pet has no despawn timer)
            react: PetReactState::Defensive,
            command: PetCommandState::Follow,
            unknown: 0,
            pet_enabled: PetEnabled::Enabled,
            action_bars: bars,
            spells: spells.iter().map(|&s| pack(s, ACT_DISABLED)).collect(),
            cooldowns: Vec::new(),
        }),
    }
}

/// The empty `SMSG_PET_SPELLS` (guid 0, no bar block) — the vanilla "remove the pet bar" form,
/// sent when the pet despawns/dies (023).
pub fn build_pet_spells_clear() -> SMSG_PET_SPELLS {
    SMSG_PET_SPELLS {
        pet: Guid::new(0),
        action_bars: None,
    }
}

/// `SMSG_SET_FACTION_STANDING` opcode (0x0124) — RAW-encoded (a vanilla wire gap; gtker writes the entry
/// faction as a u16, the 5875 client reads u32, hence the hand-rolled 12-byte body).
const SMSG_SET_FACTION_STANDING_OPCODE: u16 = 0x0124;

/// Build the live reputation-bar update (#13) relayed when a `game_player_reputation` row is inserted/updated,
/// in the exact 5875 layout: `count(u32)` then per faction `reputation_index(u32)` + `standing(u32)`.
///
/// CRASH ROOT-CAUSE (3rd McBride attempt — the real one): the entry field is the Faction.dbc **ReputationListID**
/// (a SMALL 0..63 index into the client's fixed rep array), NOT the faction id. SMSG_INITIALIZE_FACTIONS sets up
/// 64 slots; SET_FACTION_STANDING addresses ONE by that index. We were sending `faction.as_int()` = the faction
/// id (Stormwind 72), so the client indexed slot 72 — past the 64-slot array → null faction record → `mov
/// esi,[ebx]` with ebx=0 → ERROR #132 ACCESS_VIOLATION (the client crash dump's args carried opcode 0x124 + the
/// standing 3175, confirming this packet). The earlier u16→u32 width fix was a real-but-insufficient symptom fix;
/// the VALUE was always wrong. `reputation_index` now comes straight off the row (the module stamps it from
/// game_faction at grant time); callers pass `< 0` rows nowhere (the relay skips them — no rep bar to address).
pub fn build_set_faction_standing_raw(
    reputation_index: u32,
    standing: i32,
) -> Option<(u16, Vec<u8>)> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&1u32.to_le_bytes()); // count = 1
    body.extend_from_slice(&reputation_index.to_le_bytes()); // the Faction.dbc ReputationListID (0..63), as u32
    body.extend_from_slice(&(standing as u32).to_le_bytes()); // signed standing, bit-cast (client reads it i32)
    Some((SMSG_SET_FACTION_STANDING_OPCODE, body))
}
