use super::*;

#[derive(Clone, Debug, Default)]
pub struct EntityView {
    pub guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub zone_id: u32,
    pub watched_faction_index: Option<i32>,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub last_move_ms: u32,
    pub movement_flags: u32,
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
    pub target_guid: u64,
    pub unit_bytes_0: u32,
    pub display_id: u32,
    pub native_display_id: u32,
    pub unit_flags: u32,
    pub mount_display_id: u32,
    pub base_attack_time_ms: u32,
    pub offhand_attack_time_ms: Option<u32>,
    pub dynamic_flags: u32,
    pub player_bytes: u32,
    pub player_bytes_2: u32,
    pub player_bytes_3: u32,
    pub player_flags: u32,
    pub xp: u32,
    pub next_level_xp: u32,
    pub money: u32,
    pub unit_bytes_1: u32, // UNIT_FIELD_BYTES_1 byte 3 carries UNIT_VIS_FLAG_GHOST.
    /// UNIT_FIELD_BYTES_2 byte 0 is sheath state: 0 stowed, 1 melee, 2 ranged.
    pub unit_bytes_2: u32,
    pub strength: u32,
    pub agility: u32,
    pub stamina: u32,
    pub intellect: u32,
    pub spirit: u32,
    pub sheet_ap_base: u32,
    pub sheet_ap_mods: i32,
    pub sheet_dmg_min: u32,
    pub sheet_dmg_max: u32,
    pub sheet_ranged_ap: u32,
    pub sheet_ranged_dmg_min: u32,
    pub sheet_ranged_dmg_max: u32,
    pub npc_flags: u32,
    pub owner_guid: u64,
    pub effective_armor: u32,
    pub magic_resistances: [u32; 6],
    pub home_map: u32,
    pub home_zone: u32,
    pub home_x: f32,
    pub home_y: f32,
    pub home_z: f32,
    pub guild_id: u32,
    pub guild_rank: u32,
    pub spline: Option<CreateSpline>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreateSpline {
    pub nodes: Vec<Vector3d>,
    pub time_passed_ms: u32,
    pub duration_ms: u32,
    pub id: u32,
}

fn create_movement_flags(entity: &EntityView) -> Result<MovementBlock_MovementFlags> {
    let mut flags = MovementBlock_MovementFlags::new(
        entity.movement_flags & super::movement_info::SIMPLE_MOVE_MASK,
        None,
        None,
        None,
        None,
        None,
    );
    if let Some(spline) = &entity.spline {
        anyhow::ensure!(
            spline.nodes.len() >= 4
                && spline.duration_ms > 0
                && spline.time_passed_ms < spline.duration_ms
                && spline
                    .nodes
                    .iter()
                    .all(|node| node.x.is_finite() && node.y.is_finite() && node.z.is_finite()),
            "invalid CREATE spline"
        );
        let mut nodes = spline.nodes.clone();
        for (index, node) in nodes.iter_mut().enumerate().skip(1) {
            if spline.nodes[index] == spline.nodes[index - 1] {
                node.z += if index & 1 == 0 { 0.02 } else { 0.01 };
            }
        }
        flags = flags.set_spline_enabled(MovementBlock_MovementFlags_SplineEnabled {
            spline_flags: MovementBlock_SplineFlag::new(0x0000_0300, None),
            time_passed: spline.time_passed_ms,
            duration: spline.duration_ms,
            id: spline.id,
            nodes,
            final_node: *spline.nodes.last().expect("validated spline has nodes"),
        });
    }
    Ok(flags)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateKind {
    SelfPlayer,
    Peer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateFreshness {
    New,
    Known,
}

pub fn set_create_freshness(message: &mut SMSG_UPDATE_OBJECT, freshness: CreateFreshness) {
    for object in &mut message.objects {
        let (guid3, object_type, movement2, mask2) = match object.clone() {
            Object::CreateObject {
                guid3,
                object_type,
                movement2,
                mask2,
            }
            | Object::CreateObject2 {
                guid3,
                object_type,
                movement2,
                mask2,
            } => (guid3, object_type, movement2, mask2),
            _ => continue,
        };
        *object = match freshness {
            CreateFreshness::New => Object::CreateObject2 {
                guid3,
                object_type,
                movement2,
                mask2,
            },
            CreateFreshness::Known => Object::CreateObject {
                guid3,
                object_type,
                movement2,
                mask2,
            },
        };
    }
}

pub fn visible_item_index(slot: u8) -> Option<VisibleItemIndex> {
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

pub fn visible_item_of(item: &ItemInstanceView) -> VisibleItem {
    VisibleItem {
        item: item.entry,
        enchants: [item.enchantment, 0],
        random_property_id: item.random_property_id,
        ..Default::default()
    }
}

/// Skill descriptor slots must stay stable throughout the World Session.
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

fn create_packed_bytes(entity: &EntityView, power: u8) -> (u32, u32, u32) {
    let player_bytes_2 = if entity.player_bytes_2 & 0x0000_ff00 == 0 {
        entity.player_bytes_2 | 0x0000_ee00
    } else {
        entity.player_bytes_2
    };
    let unit_bytes_1 = if matches!(power, 0 | 1) && entity.unit_bytes_1 & 0x0000_ff00 == 0 {
        entity.unit_bytes_1 | 0x0000_ee00
    } else {
        entity.unit_bytes_1
    };
    let unit_bytes_2 = if entity.unit_bytes_2 & 0x0000_ff00 == 0 {
        entity.unit_bytes_2 | 0x0000_2800
    } else {
        entity.unit_bytes_2
    };
    (player_bytes_2, unit_bytes_1, unit_bytes_2)
}

pub fn build_create_object(
    entity: &EntityView,
    kind: CreateKind,
    inventory: &[ItemInstanceView],
    learned_skills: &[(u32, u16, u16)],
) -> Result<SMSG_UPDATE_OBJECT> {
    let (race_b, class_b, gender_b, power_b) = unpack4(entity.unit_bytes_0);
    let race = Race::try_from(race_b).map_err(|_| anyhow!("invalid race {race_b}"))?;
    let class = Class::try_from(class_b).map_err(|_| anyhow!("invalid class {class_b}"))?;
    let gender = Gender::try_from(gender_b).map_err(|_| anyhow!("invalid gender {gender_b}"))?;
    let power = Power::try_from(power_b).map_err(|_| anyhow!("invalid power {power_b}"))?;

    let is_player = entity.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0;
    let (object_type, mask2) = if is_player {
        let (skin, face, hair_style, hair_color) = unpack4(entity.player_bytes);
        let (player_bytes_2, unit_bytes_1, unit_bytes_2) = create_packed_bytes(entity, power_b);
        let (facial_hair, pb2_b, pb2_c, pb2_d) = unpack4(player_bytes_2);
        let (pb3_a, pb3_b, pb3_c, pb3_d) = unpack4(entity.player_bytes_3);
        let (ub1_a, ub1_b, ub1_c, ub1_d) = unpack4(unit_bytes_1);
        let (ub2_a, ub2_b, ub2_c, ub2_d) = unpack4(unit_bytes_2);

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
            .set_unit_mod_cast_speed(1.0)
            .set_unit_bytes_1(ub1_a, ub1_b, ub1_c, ub1_d)
            // gtker misnames these parameters; field 164 is UNIT_FIELD_BYTES_2, sheath in byte 0.
            .set_unit_bytes_2(ub2_a, ub2_b, ub2_c, ub2_d)
            .set_player_features(skin, face, hair_style, hair_color)
            .set_player_bytes_2(facial_hair, pb2_b, pb2_c, pb2_d)
            .set_player_bytes_3(pb3_a, pb3_b, pb3_c, pb3_d)
            .set_player_flags(entity.player_flags as i32);
        builder = builder.set_unit_power4(100).set_unit_maxpower4(100);
        if matches!(kind, CreateKind::SelfPlayer) {
            builder = builder
                .set_player_xp(entity.xp as i32)
                .set_player_next_level_xp(entity.next_level_xp as i32)
                .set_player_field_coinage(entity.money as i32)
                .set_player_character_points1(lyracore_shared::talent::points_available(
                    entity.level,
                    0,
                ) as i32)
                .set_unit_strength(entity.strength as i32)
                .set_unit_agility(entity.agility as i32)
                .set_unit_stamina(entity.stamina as i32)
                .set_unit_intellect(entity.intellect as i32)
                .set_unit_spirit(entity.spirit as i32)
                .set_unit_normal_resistance(entity.effective_armor as i32)
                .set_unit_holy_resistance(entity.magic_resistances[0] as i32)
                .set_unit_fire_resistance(entity.magic_resistances[1] as i32)
                .set_unit_nature_resistance(entity.magic_resistances[2] as i32)
                .set_unit_frost_resistance(entity.magic_resistances[3] as i32)
                .set_unit_shadow_resistance(entity.magic_resistances[4] as i32)
                .set_unit_arcane_resistance(entity.magic_resistances[5] as i32)
                // Unset damage fields become uninitialized floats in the build 5875 paperdoll.
                .set_unit_attack_power(entity.sheet_ap_base as i32)
                .set_unit_attack_power_mods(
                    entity.sheet_ap_mods.max(0) as u16,
                    (-entity.sheet_ap_mods).max(0) as u16,
                )
                // Attack power uses 1 + multiplier; setting 1.0 doubles displayed AP.
                .set_unit_mindamage(entity.sheet_dmg_min as f32)
                .set_unit_maxdamage(entity.sheet_dmg_max as f32)
                .set_unit_ranged_attack_power(entity.sheet_ranged_ap as i32)
                .set_unit_ranged_attack_power_mods(0, 0)
                .set_unit_minrangeddamage(entity.sheet_ranged_dmg_min as f32)
                .set_unit_maxrangeddamage(entity.sheet_ranged_dmg_max as f32)
                // The paperdoll divides damage by this multiplier; zero produces infinities.
                .set_player_field_mod_damage_done_pct(1.0_f32.to_bits() as i32);
        }

        builder = builder
            .set_player_guildid(entity.guild_id as i32)
            .set_player_guildrank(entity.guild_rank as i32);
        if matches!(kind, CreateKind::SelfPlayer) {
            builder = builder
                .set_player_field_watched_faction_index(entity.watched_faction_index.unwrap_or(-1));
        }
        if entity.mount_display_id != 0 {
            builder = builder.set_unit_mountdisplayid(entity.mount_display_id as i32);
        }

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

        if matches!(kind, CreateKind::SelfPlayer) {
            let skill_lines = skill_slot_layout(class_b, learned_skills);
            for (i, &(skill, min, max)) in skill_lines.iter().enumerate() {
                if let Ok(idx) = SkillInfoIndex::try_from(i as u8) {
                    builder = builder
                        .set_player_skill_info(SkillInfo::new(skill, 0, min, max, 0, 0), idx);
                }
            }
        }
        for item in inventory {
            if matches!(kind, CreateKind::SelfPlayer) {
                if let Ok(s) = ItemSlot::try_from(item.slot) {
                    builder = builder.set_player_field_inv(s, Guid::new(item.guid));
                }
            }
            if let Some(vi_index) = visible_item_index(item.slot) {
                builder = builder.set_player_visible_item(visible_item_of(item), vi_index);
            }
        }
        (ObjectType::Player, UpdateMask::Player(builder.finalize()))
    } else {
        let (cb2_a, cb2_b, cb2_c, cb2_d) = unpack4(entity.unit_bytes_2);
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
            .set_unit_mod_cast_speed(1.0)
            .set_unit_bytes_2(cb2_a, cb2_b, cb2_c, cb2_d)
            .set_unit_dynamic_flags(entity.dynamic_flags as i32)
            .set_unit_npc_flags(entity.npc_flags as i32);
        if entity.owner_guid != 0 {
            unit_builder = unit_builder
                .set_unit_summonedby(Guid::new(entity.owner_guid))
                .set_unit_createdby(Guid::new(entity.owner_guid));
        }
        if entity.mount_display_id != 0 {
            unit_builder = unit_builder.set_unit_mountdisplayid(entity.mount_display_id as i32);
        }
        (ObjectType::Unit, UpdateMask::Unit(unit_builder.finalize()))
    };

    let running_speed = speeds::RUN * (entity.run_speed_mult_bp as f32 / 10_000.0);
    let mut update_flag = MovementBlock_UpdateFlag::new(0x40, None, None, None, None, None)
        .set_all(MovementBlock_UpdateFlag_All { unknown1: 1 })
        .set_living(MovementBlock_UpdateFlag_Living::Living {
            flags: create_movement_flags(entity)?,
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

    let create = match kind {
        CreateKind::SelfPlayer => Object::CreateObject2 {
            guid3: Guid::new(entity.guid),
            object_type,
            movement2: MovementBlock { update_flag },
            mask2,
        },
        CreateKind::Peer => Object::CreateObject {
            guid3: Guid::new(entity.guid),
            object_type,
            movement2: MovementBlock { update_flag },
            mask2,
        },
    };
    let mut message = SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![create],
    };
    let offhand_attack_time_ms = if is_player {
        entity.offhand_attack_time_ms
    } else {
        Some(entity.base_attack_time_ms)
    };
    if let Some(delay) = offhand_attack_time_ms {
        add_create_fields(&mut message, |mask| {
            mask.set_u32(127, delay);
        })?;
    }
    Ok(message)
}

pub fn build_self_create_object(
    entity: &EntityView,
    inventory: &[ItemInstanceView],
    learned_skills: &[(u32, u16, u16)],
    quests: &[update_mask::QuestLogSlot],
) -> Result<SMSG_UPDATE_OBJECT> {
    let mut create =
        build_create_object(entity, CreateKind::SelfPlayer, inventory, learned_skills)?;
    if quests.is_empty() {
        return Ok(create);
    }
    anyhow::ensure!(
        quests
            .iter()
            .all(|q| u16::from(q.slot) < update_mask::idx::QUEST_LOG_SLOTS),
        "quest log slot outside the Character descriptor"
    );
    add_create_fields(&mut create, |mask| {
        let quest_mask = update_mask::full_quest_log_mask(quests);
        for index in update_mask::idx::PLAYER_QUEST_LOG_1_1
            ..update_mask::quest_log_field(update_mask::idx::QUEST_LOG_SLOTS, 0)
        {
            mask.set_u32(index, quest_mask.get(index).unwrap_or(0));
        }
    })?;
    Ok(create)
}

fn add_create_fields(
    create: &mut SMSG_UPDATE_OBJECT,
    add: impl FnOnce(&mut update_mask::UpdateMaskValues),
) -> Result<()> {
    use wow_world_messages::vanilla::ServerMessage;

    let mask2 = match &mut create.objects[0] {
        Object::CreateObject { mask2, .. } | Object::CreateObject2 { mask2, .. } => mask2,
        _ => unreachable!("build_create_object returns one CREATE"),
    };
    // This local envelope carries OBJECT_FIELD_TYPE and must never reach the client.
    let envelope = SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(0),
            mask1: mask2.clone(),
        }],
    };
    let mut bytes = Vec::new();
    envelope.write_unencrypted_server(&mut bytes)?;
    let decoded = lyracore_shared::values_mask::parse_values_updates(&bytes[4..]);
    let fields = &decoded
        .first()
        .ok_or_else(|| anyhow!("could not read CREATE descriptor"))?
        .fields;
    let mut mask = update_mask::UpdateMaskValues::new();
    for &(index, value) in fields {
        mask.set_u32(index, value);
    }
    add(&mut mask);
    bytes.truncate(4 + 4 + 1 + 1 + 1);
    mask.write_to(&mut bytes);
    let size = u16::try_from(bytes.len() - 2)?;
    bytes[..2].copy_from_slice(&size.to_be_bytes());
    let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(mut envelope) =
        ServerOpcodeMessage::read_unencrypted(&mut bytes.as_slice())?
    else {
        unreachable!("the local envelope is SMSG_UPDATE_OBJECT");
    };
    let Object::Values { mask1, .. } = envelope.objects.remove(0) else {
        unreachable!("the local envelope contains one VALUES descriptor");
    };
    *mask2 = mask1;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldEntry {
    FreshLogin,
    WorldPort,
}

pub fn realm_datetime(secs_since_unix_epoch: u64) -> DateTime {
    let days = (secs_since_unix_epoch / 86_400) as i64;
    let secs_of_day = (secs_since_unix_epoch % 86_400) as u32;
    let hours = (secs_of_day / 3_600) as u8;
    let minutes = ((secs_of_day % 3_600) / 60) as u8;

    let (year, month, month_day) = lyracore_shared::calendar::civil_from_days(days);
    // Unix day zero was Thursday; vanilla numbers weekdays from Sunday, zero.
    let weekday = Weekday::try_from(((days + 4) % 7) as u32)
        .expect("(days + 4) % 7 is always 0..=6, a valid Weekday");

    DateTime::new(
        (year - 2000) as u8,
        Month::try_from(month - 1).expect("civil_from_days month is always 1..=12"),
        (month_day - 1) as u8,
        weekday,
        hours,
        minutes,
    )
}

/// Only fresh login sends LOGIN_VERIFY_WORLD, and it must precede TUTORIAL_FLAGS.
pub fn login_sequence_messages(
    entity: &EntityView,
    learned: &[u32],
    reputations: &[(i32, i32, bool)],
    player_actions: &[(u8, u32, u8)],
    entry: WorldEntry,
    unix_seconds: u64,
) -> Result<Vec<ServerOpcodeMessage>> {
    let map = Map::try_from(entity.map_id).map_err(|_| anyhow!("invalid map {}", entity.map_id))?;
    let area = Area::try_from(entity.zone_id).unwrap_or_default();
    let position = Vector3d {
        x: entity.x,
        y: entity.y,
        z: entity.z,
    };

    const ATTACK_SPELL: u32 = 6603;
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

    let [weapon_proficiency, armor_proficiency] =
        build_set_proficiency_msgs(((entity.unit_bytes_0 >> 8) & 0xFF) as u8, learned);

    let mut msgs = Vec::new();
    if entry == WorldEntry::FreshLogin {
        msgs.push(ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(Box::new(
            SMSG_LOGIN_VERIFY_WORLD {
                map,
                position,
                orientation: entity.orientation,
            },
        )));
    }
    msgs.extend([
        ServerOpcodeMessage::SMSG_ACCOUNT_DATA_TIMES(Box::new(SMSG_ACCOUNT_DATA_TIMES {
            data: [0u32; 32],
        })),
        ServerOpcodeMessage::SMSG_LOGIN_SETTIMESPEED(SMSG_LOGIN_SETTIMESPEED {
            datetime: realm_datetime(unix_seconds),
            timescale: 0.016_666_668,
        }),
        ServerOpcodeMessage::SMSG_TUTORIAL_FLAGS(Box::new(SMSG_TUTORIAL_FLAGS {
            tutorial_data: [0xFFFF_FFFFu32; 8],
        })),
        ServerOpcodeMessage::SMSG_INITIAL_SPELLS(Box::new(SMSG_INITIAL_SPELLS {
            unknown1: 0,
            initial_spells,
            cooldowns: vec![],
        })),
        weapon_proficiency,
        armor_proficiency,
        ServerOpcodeMessage::SMSG_ACTION_BUTTONS(Box::new(SMSG_ACTION_BUTTONS {
            data: action_buttons,
        })),
        ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(Box::new(SMSG_INITIALIZE_FACTIONS {
            // Reputation slots use Faction.dbc ReputationListID, never the faction id.
            factions: {
                let mut slots: Vec<FactionInitializer> = (0..64)
                    .map(|_| FactionInitializer {
                        flag: FactionFlag::empty(),
                        standing: 0,
                    })
                    .collect();
                for &(index, standing, at_war) in reputations {
                    if let Some(slot) = usize::try_from(index).ok().and_then(|i| slots.get_mut(i)) {
                        slot.standing = standing as u32;
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
            // Map 0 is Eastern Kingdoms, so home_map cannot use zero as an absent sentinel.
            position: Vector3d {
                x: entity.home_x,
                y: entity.home_y,
                z: entity.home_z,
            },
            map: Map::try_from(entity.home_map).unwrap_or(map),
            area: Area::try_from(entity.home_zone).unwrap_or(area),
        })),
    ]);
    Ok(msgs)
}

pub fn build_destroy_object(guid: u64) -> SMSG_DESTROY_OBJECT {
    SMSG_DESTROY_OBJECT {
        guid: Guid::new(guid),
    }
}

pub fn build_pet_spells(pet_guid: u64, spells: &[u32]) -> SMSG_PET_SPELLS {
    const ACT_COMMAND: u32 = 0x07;
    const ACT_REACT: u32 = 0x06;
    const ACT_DISABLED: u32 = 0x81;
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
            duration: 0,
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

pub fn build_pet_spells_clear() -> SMSG_PET_SPELLS {
    SMSG_PET_SPELLS {
        pet: Guid::new(0),
        action_bars: None,
    }
}

// gtker writes a u16 faction slot, but build 5875 requires u32.
const SMSG_SET_FACTION_STANDING_OPCODE: u16 = 0x0124;

/// The client indexes a 64-slot array by reputation_index, not faction id.
pub fn build_set_faction_standing_raw(
    reputation_index: u32,
    standing: i32,
) -> Option<(u16, Vec<u8>)> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&reputation_index.to_le_bytes());
    body.extend_from_slice(&(standing as u32).to_le_bytes());
    Some((SMSG_SET_FACTION_STANDING_OPCODE, body))
}
