//! Row→view converters and the thin row mirrors the gateway reads from subscription caches.
//! Pure projections from the generated `bindings` row types into the codec's view structs.

use super::bindings::*;

pub(crate) fn hunter_pet_protocol_view(
    pet: HunterPetProtocol,
) -> crate::codec::HunterPetProtocolView {
    crate::codec::HunterPetProtocolView {
        pet_id: pet.pet_id,
        owner_guid: pet.owner_guid,
        live_pet_guid: pet.live_pet_guid,
        creature_entry: pet.creature_entry,
        name: pet.name,
        name_timestamp: pet.name_timestamp,
        level: pet.level,
        pet_xp: pet.pet_xp,
        next_level_xp: pet.next_level_xp,
        happiness: pet.happiness,
        loyalty_level: pet.loyalty_level,
    }
}

/// Flatten a `game_character` row into the codec's `CharacterView` (the char-select projection).
/// One mapper so the 17-field projection isn't copy-pasted per query (mirrors `entity_view`).
pub(crate) fn character_view(c: Character) -> crate::codec::CharacterView {
    crate::codec::CharacterView {
        guid: c.guid,
        name: c.name,
        race: c.race,
        class: c.class,
        gender: c.gender,
        skin: c.skin,
        face: c.face,
        hair_style: c.hair_style,
        hair_color: c.hair_color,
        facial_hair: c.facial_hair,
        level: c.level,
        map_id: c.map_id,
        zone_id: c.zone_id,
        x: c.x,
        y: c.y,
        z: c.z,
        first_login: c.first_login,
        equipment: Default::default(), // filled by Coordinator::characters from item tables
        played_total_secs: c.played_total_secs,
        session_start_micros: c.session_start_micros,
    }
}

/// Flatten a `game_item_template` row into the codec's `ItemTemplateView` (items slice-1).
pub(crate) fn item_template_view(t: ItemTemplate) -> crate::codec::ItemTemplateView {
    crate::codec::ItemTemplateView {
        entry: t.entry,
        class: t.class,
        subclass: t.subclass,
        name: t.name,
        display_id: t.display_id,
        quality: t.quality,
        inventory_type: t.inventory_type,
        item_level: t.item_level,
        required_level: t.required_level,
        max_durability: t.max_durability,
        buy_price: t.buy_price,
        sell_price: t.sell_price,
        max_stack: t.max_stack,
        damage_min: t.damage_min,
        damage_max: t.damage_max,
        delay_ms: t.delay_ms,
        stat_strength: t.stat_strength,
        stat_agility: t.stat_agility,
        stat_stamina: t.stat_stamina,
        stat_intellect: t.stat_intellect,
        stat_spirit: t.stat_spirit,
        stat_armor: t.stat_armor,
        block_value: t.block_value,
        spellid_1: t.spellid_1,
        spelltrigger_1: t.spelltrigger_1,
        spellid_2: t.spellid_2,
        spelltrigger_2: t.spelltrigger_2,
        bonding: t.bonding,
        sheath: t.sheath,
        holy_res: t.holy_res,
        fire_res: t.fire_res,
        nature_res: t.nature_res,
        frost_res: t.frost_res,
        shadow_res: t.shadow_res,
        arcane_res: t.arcane_res,
        spellid_3: t.spellid_3,
        spelltrigger_3: t.spelltrigger_3,
        spellid_4: t.spellid_4,
        spelltrigger_4: t.spelltrigger_4,
        spellid_5: t.spellid_5,
        spelltrigger_5: t.spelltrigger_5,
        required_skill: t.required_skill,
        required_skill_rank: t.required_skill_rank,
        required_reputation_faction: t.required_reputation_faction,
        required_reputation_rank: t.required_reputation_rank,
        max_count: t.max_count,
        item_flags: t.item_flags,
        page_text: t.page_text,
        start_quest: t.start_quest,
        bag_family: t.bag_family,
        allowed_class: t.allowed_class,
        allowed_race: t.allowed_race,
    }
}

/// Map a `game_world_entity` row into the codec's `EntityView`.
///
/// The entity carries its own live zone, resolved from terrain by the Module wherever a position is
/// established. `durable_zone` is the `game_character` copy, used only when the live row says `0` —
/// the Module's "no terrain covers this position", which a character parked off the imported slice
/// really can have.
pub(crate) fn entity_view(e: WorldEntity, durable_zone: u32) -> crate::codec::EntityView {
    crate::codec::EntityView {
        guid: e.guid,
        map_id: e.map_id,
        instance_id: e.instance_id,
        zone_id: if e.zone_id != 0 {
            e.zone_id
        } else {
            durable_zone
        },
        x: e.x,
        y: e.y,
        z: e.z,
        orientation: e.orientation,
        last_move_ms: e.last_move_ms,
        movement_flags: e.movement_flags,
        run_speed_mult_bp: e.run_speed_mult_bp,
        type_mask: e.type_mask,
        entry: e.entry,
        scale_x: e.scale_x,
        health: e.health,
        max_health: e.max_health,
        power: e.power,
        max_power: e.max_power,
        level: e.level,
        faction_template: e.faction_template,
        target_guid: e.target_guid,
        unit_bytes_0: e.unit_bytes_0,
        display_id: e.display_id,
        native_display_id: e.native_display_id,
        unit_flags: e.unit_flags,
        mount_display_id: e.mount_display_id,
        base_attack_time_ms: e.base_attack_time_ms,
        dynamic_flags: e.dynamic_flags,
        player_bytes: e.player_bytes,
        player_bytes_2: e.player_bytes_2,
        player_bytes_3: e.player_bytes_3,
        player_flags: e.player_flags,
        xp: e.xp,
        next_level_xp: e.next_level_xp,
        money: e.money,
        unit_bytes_1: e.unit_bytes_1,
        unit_bytes_2: e.unit_bytes_2,
        strength: e.strength,
        agility: e.agility,
        stamina: e.stamina,
        intellect: e.intellect,
        spirit: e.spirit,
        npc_flags: e.npc_flags,
        // Default the sheet's EFFECTIVE armor to the row's BASE armor (peers/creatures don't fold gear —
        // your sheet only shows your own armor). The self-login CREATE overrides this with the gear-folded
        // value (`Coordinator::effective_armor`); the relays then keep it live.
        effective_armor: e.armor,
        owner_guid: e.owner_guid,
        // Home fields are not on the world-entity row; caller fills them in from game_character when
        // building the self-login sequence.  Peer/creature views never need these, so 0 is fine.
        home_map: 0,
        home_zone: 0,
        home_x: 0.0,
        home_y: 0.0,
        home_z: 0.0,
    }
}

/// Map a `game_corpse` row into the codec's `CorpseView` for the CORPSE CREATE_OBJECT (slice 5).
pub(crate) fn corpse_view(c: Corpse) -> crate::codec::CorpseView {
    crate::codec::CorpseView {
        guid: c.guid,
        owner_guid: c.owner_guid,
        x: c.x,
        y: c.y,
        z: c.z,
        orientation: c.orientation,
        display_id: c.display_id,
        bytes_1: c.bytes_1,
        bytes_2: c.bytes_2,
        is_bones: c.is_bones,
    }
}

/// Flatten a `game_gameobject` row joined with its `game_gameobject_template` into the codec's
/// `GameObjectView` for the GameObject CREATE_OBJECT (display + type come from the template).
pub(crate) fn go_view(go: GameObject, tmpl: &GameObjectTemplate) -> crate::codec::GameObjectView {
    crate::codec::GameObjectView {
        guid: go.guid,
        template_entry: go.template_entry,
        x: go.x,
        y: go.y,
        z: go.z,
        orientation: go.orientation,
        state: go.state,
        type_id: tmpl.type_id,
        display_id: tmpl.display_id,
        rotation_0: go.rotation_0,
        rotation_1: go.rotation_1,
        rotation_2: go.rotation_2,
        rotation_3: go.rotation_3,
        size: tmpl.size,
    }
}

// --- thin row mirrors the gateway reads from subscription caches ---

#[derive(Clone, Debug)]
pub struct RealmRow {
    pub id: u8,
    pub name: String,
    pub address: String,
    pub realm_type: u32,
    /// `game_realm.flags` / `.timezone`, mirrored for the realm-list reply. Nothing reads them yet —
    /// `logon::realms` builds `RealmInfo` from the four fields above plus a live character count, and
    /// the 1.12 realm-list packet's flags/timezone are still written from gtker defaults. Carried
    /// (rather than dropped) so filling those two wire fields in is a codec edit, not a re-thread of
    /// the whole read path.
    #[allow(dead_code)]
    pub flags: u8,
    pub population: f32,
    #[allow(dead_code)]
    pub timezone: u8,
}

#[derive(Clone, Debug)]
pub struct AccountRow {
    pub id: u64,
    /// The account name this row was looked UP by (`account_by_username`), echoed back for callers
    /// that log or re-key on it. No current caller does — the SRP path already has the name in hand.
    #[allow(dead_code)]
    pub username: String,
    pub salt: Vec<u8>,
    pub verifier: Vec<u8>,
    pub banned: bool,
    pub alpha_test_tools: bool,
}
