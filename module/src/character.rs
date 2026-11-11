use lyracore_package_delta::packed_race_class_id;
use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

use crate::auth::{game_account, next_character_guid, require_guid_range};
use crate::guild::game_guild;
use crate::world::{build_player_entity, teleport, WorldEntity};
use crate::{
    game_char_base_info, game_instance, game_start_position, game_world_entity,
    helpers::entity_by_owner,
};

// The named constant avoids unary-expression precedence in the table macro's default serializer.
const NO_WATCHED_FACTION: i32 = -1;

#[table(accessor = game_character, public, index(accessor = by_account, btree(columns = [account_id])))]
pub struct Character {
    #[primary_key]
    pub guid: u64,
    pub account_id: u64,
    pub owner_identity: Identity,
    #[unique]
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub level: u8,
    pub xp: u32,
    pub next_level_xp: u32,
    pub map_id: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub first_login: bool,
    pub online: bool,
    #[default(0)]
    pub money: u32,
    #[default(0)]
    pub rested_xp: u32,
    #[default(0u64)]
    pub last_logout_micros: u64,
    #[default(0u32)]
    pub home_map: u32,
    #[default(0u32)]
    pub home_zone: u32,
    #[default(0.0f32)]
    pub home_x: f32,
    #[default(0.0f32)]
    pub home_y: f32,
    #[default(0.0f32)]
    pub home_z: f32,
    #[default(0)]
    pub played_total_secs: u32,
    #[default(0u64)]
    pub session_start_micros: u64,
    #[default(0u32)]
    pub health: u32,
    // Health zero, rather than power zero, identifies an uninitialized vitals snapshot.
    #[default(0u32)]
    pub power: u32,
    #[default(0u32)]
    pub respec_count: u32,
    #[default(0i64)]
    pub death_expire_micros: i64,

    #[default(0u64)]
    pub pending_instance_id: u64,
    // Legacy: debug_repair_after_publish moves a nonzero value onto the Account and zeroes it.
    #[default(0)]
    pub gm_level: u8,

    #[default(false)]
    pub pending_ghost: bool,
    #[default(false)]
    pub resting: bool,
    #[default(0u64)]
    pub rested_since_micros: u64,
    #[default(false)]
    pub pending_godmode: bool,

    #[default(10000)]
    pub pending_run_speed_mult_bp: u32,
    #[default(0)]
    pub bank_bag_slots: u8,
    #[default(NO_WATCHED_FACTION)]
    pub watched_faction_index: i32,
    #[default(0u8)]
    pub rest_state: u8,
}

// The 5875 client refuses a Character list longer than this limit.
pub(crate) const CHARACTERS_PER_ACCOUNT: usize = 10;

#[derive(Clone, Copy, Default)]
pub(crate) struct Appearance {
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn create_character(
    ctx: &ReducerContext,
    account_id: u64,
    name: String,
    race: u8,
    class: u8,
    gender: u8,
    skin: u8,
    face: u8,
    hair_style: u8,
    hair_color: u8,
    facial_hair: u8,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let appearance = Appearance {
        gender,
        skin,
        face,
        hair_style,
        hair_color,
        facial_hair,
    };
    insert_new_character(ctx, account_id, name, race, class, appearance)
        .map(|_| ())
        .map_err(String::from)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CreateCharacterRefusal {
    NameInUse,
    InvalidRaceClass,
    AccountFull,
    Refused(crate::Refusal),
}

impl From<String> for CreateCharacterRefusal {
    fn from(cause: String) -> Self {
        Self::Refused(cause.into())
    }
}

// The create_character reducer and the Package API return this text.
impl std::fmt::Display for CreateCharacterRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameInUse => "NAME_IN_USE".fmt(f),
            Self::InvalidRaceClass => "INVALID_RACE_CLASS".fmt(f),
            Self::AccountFull => "SERVER_LIMIT".fmt(f),
            Self::Refused(refusal) => refusal.fmt(f),
        }
    }
}

impl From<CreateCharacterRefusal> for String {
    fn from(refusal: CreateCharacterRefusal) -> Self {
        refusal.to_string()
    }
}

pub(crate) fn check_new_character(
    ctx: &ReducerContext,
    name: &str,
    race: u8,
    class: u8,
) -> Result<(), CreateCharacterRefusal> {
    require_guid_range(ctx)?;
    if crate::peer::role(ctx).is_some()
        && crate::peer::require_sender(ctx, crate::peer::realm_shard(ctx)).is_err()
    {
        return Err("Character creation requires a Realm name reservation"
            .to_string()
            .into());
    }
    let folded = name.to_lowercase();
    if ctx
        .db
        .game_character()
        .iter()
        .any(|character| character.name.to_lowercase() == folded)
    {
        return Err(CreateCharacterRefusal::NameInUse);
    }
    let cbi = ctx.db.game_char_base_info();
    if cbi.count() > 0
        && cbi
            .race_class()
            .find(packed_race_class_id(race, class) as u16)
            .is_none()
    {
        return Err(CreateCharacterRefusal::InvalidRaceClass);
    }
    Ok(())
}

pub(crate) fn account_has_room(ctx: &ReducerContext, account_id: u64) -> bool {
    ctx.db
        .game_character()
        .by_account()
        .filter(&account_id)
        .count()
        < CHARACTERS_PER_ACCOUNT
}

pub(crate) fn insert_new_character(
    ctx: &ReducerContext,
    account_id: u64,
    name: String,
    race: u8,
    class: u8,
    appearance: Appearance,
) -> Result<u64, CreateCharacterRefusal> {
    check_new_character(ctx, &name, race, class)?;
    if !account_has_room(ctx, account_id) {
        return Err(CreateCharacterRefusal::AccountFull);
    }
    let chars = ctx.db.game_character();
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or_else(|| format!("no such account id: {account_id}"))?;

    let positions = ctx.db.game_start_position();
    let race_class = packed_race_class_id(race, class) as u16;
    let human_warrior = packed_race_class_id(1, 1) as u16;
    let sp = positions
        .race_class()
        .find(race_class)
        .or_else(|| positions.race_class().find(human_warrior))
        .ok_or_else(|| "no start position seeded".to_string())?;

    if !crate::native::directory::owns_map(ctx, sp.map_id) {
        return Err("World Shard does not own the start map".to_string().into());
    }
    let next_guid = next_character_guid(ctx);

    let row = Character {
        guid: next_guid,
        account_id,
        owner_identity: account.identity.unwrap_or(Identity::ZERO),
        name: name.clone(),
        race,
        class,
        gender: appearance.gender,
        skin: appearance.skin,
        face: appearance.face,
        hair_style: appearance.hair_style,
        hair_color: appearance.hair_color,
        facial_hair: appearance.facial_hair,
        level: 1,
        xp: 0,
        next_level_xp: crate::xp::xp_to_next_level(1),
        map_id: sp.map_id,
        zone_id: sp.zone_id,
        x: sp.x,
        y: sp.y,
        z: sp.z,
        orientation: sp.orientation,
        first_login: true,
        online: false,
        money: 0,
        rested_xp: 0,
        last_logout_micros: 0,
        home_map: sp.map_id,
        home_zone: sp.zone_id,
        home_x: sp.x,
        home_y: sp.y,
        home_z: sp.z,
        played_total_secs: 0,
        session_start_micros: 0,
        health: 0,
        power: 0,
        respec_count: 0,
        death_expire_micros: 0,
        pending_instance_id: 0,
        gm_level: 0,
        pending_ghost: false,
        resting: false,
        rested_since_micros: 0,
        pending_godmode: false,
        pending_run_speed_mult_bp: RUN_SPEED_BP_1X,
        bank_bag_slots: 0,
        watched_faction_index: -1,
        rest_state: lyracore_shared::packing::REST_STATE_NORMAL,
    };

    if chars.try_insert(row).is_err() {
        return if chars.name().find(&name).is_some() {
            Err(CreateCharacterRefusal::NameInUse)
        } else {
            Err("character create failed".to_string().into())
        };
    }
    let owner = account.identity.unwrap_or(Identity::ZERO);
    crate::spell::spellbook::grant_createinfo_spells(ctx, next_guid, owner, race, class);
    crate::action_bar::grant_createinfo_actions(ctx, next_guid, owner, race, class);

    crate::items::grant_starter_item(ctx, next_guid, owner)?;
    crate::native::directory::changed(ctx, account_id, next_guid, true);
    Ok(next_guid)
}

#[reducer]
pub fn delete_character(
    ctx: &ReducerContext,
    account_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_delete_character(ctx, account_id, character_guid)
}

pub(crate) fn character_deletion_gate(
    ctx: &ReducerContext,
    account_id: u64,
    character_guid: u64,
) -> Result<(), String> {
    let c = ctx
        .db
        .game_character()
        .guid()
        .find(character_guid)
        .ok_or_else(|| "NO_SUCH_CHAR".to_string())?;
    if c.account_id != account_id {
        return Err("NOT_OWNER".to_string());
    }
    if crate::helpers::character_by_guid(ctx, character_guid).is_none() {
        return Err("CHAR_IN_TRANSIT".to_string());
    }
    if crate::native::realm_auction::pending(ctx, character_guid) {
        return Err("CHAR_AUCTION_TRANSACTION_PENDING".into());
    }
    if crate::native::realm_petition::pending(ctx, character_guid) {
        return Err("CHAR_GUILD_TRANSACTION_PENDING".into());
    }
    if crate::auction::character_has_auction_value(ctx, character_guid) {
        return Err("CHAR_HAS_AUCTION_VALUE".to_string());
    }
    if ctx
        .db
        .game_guild()
        .by_leader()
        .filter(character_guid)
        .next()
        .is_some()
    {
        return Err("Guild Leader cannot delete their Character".into());
    }
    Ok(())
}

pub(crate) fn apply_delete_character(
    ctx: &ReducerContext,
    account_id: u64,
    character_guid: u64,
) -> Result<(), String> {
    character_deletion_gate(ctx, account_id, character_guid)?;
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_some()
    {
        return Err("CHAR_IN_WORLD".into());
    }
    crate::mail_text::drop_character_letters(ctx, character_guid);
    cascade_delete_character(ctx, character_guid);
    crate::native::directory::changed(ctx, account_id, character_guid, false);
    Ok(())
}
pub(crate) fn set_home(ctx: &ReducerContext, guid: u64) {
    let Some(e) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(guid) {
        c.home_map = e.map_id;
        c.home_x = e.x;
        c.home_y = e.y;
        c.home_z = e.z;
        c.home_zone = c.zone_id;
        chars.guid().update(c);
    }
}

/// Returns a Character to its home, in world or offline.
pub(crate) fn recall_to_home(
    ctx: &ReducerContext,
    guid: u64,
) -> Result<(), crate::actor::ActionRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};

    let mut c = crate::helpers::character_by_guid(ctx, guid).ok_or_else(|| {
        ActionRefusal::new(ActionRefusalKind::MissingActor, "Character is unavailable")
    })?;
    if ctx.db.game_world_entity().guid().find(guid).is_some() {
        return teleport(
            ctx,
            guid,
            c.home_map,
            0,
            [c.home_x, c.home_y, c.home_z, c.orientation],
        );
    }
    if !crate::native::directory::owns_map(ctx, c.home_map) {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "home is on a map this World Shard does not own",
        ));
    }
    if crate::taxi::is_in_flight(ctx, guid) {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "Character is in flight",
        ));
    }
    c.map_id = c.home_map;
    c.x = c.home_x;
    c.y = c.home_y;
    c.z = c.home_z;
    c.zone_id = c.home_zone;
    c.pending_instance_id = 0;
    ctx.db.game_character().guid().update(c);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorldEntry {
    Login,
    WorldPort,
}

pub(crate) fn restore_reaped_instance(ctx: &ReducerContext, character: &mut Character) {
    if character.pending_instance_id != 0
        && ctx
            .db
            .game_instance()
            .instance_id()
            .find(character.pending_instance_id)
            .is_none_or(|instance| instance.map_id != character.map_id)
    {
        let reaped_id = character.pending_instance_id;
        match crate::instance::stranding_fallback(
            crate::instance::imported_entrance_fallback(ctx, character.map_id),
            crate::instance::is_dungeon_map(character.map_id),
        ) {
            crate::instance::StrandingFallback::Entrance(m, x, y, z, o) => {
                character.map_id = m;
                character.x = x;
                character.y = y;
                character.z = z;
                character.orientation = o;
            }
            crate::instance::StrandingFallback::InPlaceOpenWorld => {}
            crate::instance::StrandingFallback::HearthstoneHome => {
                spacetimedb::log::warn!(
                    "player_login: dungeon map {} has no imported entrance; moving {} to hearthstone",
                    character.map_id, character.guid
                );
                character.map_id = character.home_map;
                character.x = character.home_x;
                character.y = character.home_y;
                character.z = character.home_z;
                character.orientation = 0.0;
            }
        }
        character.pending_instance_id = 0;
        spacetimedb::log::info!(
            "player_login: instance {reaped_id} was reaped — {} falls back to map {} at instance 0",
            character.guid,
            character.map_id
        );
    }
}

pub(crate) fn apply_player_login(
    ctx: &ReducerContext,
    account: &crate::Account,
    character_guid: u64,
    owner: spacetimedb::Identity,
    entry: WorldEntry,
) -> Result<(), String> {
    let chars = ctx.db.game_character();
    let mut character = chars
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no such character: {character_guid}"))?;
    if character.account_id != account.id {
        return Err("character does not belong to caller".to_string());
    }

    if crate::transfer::is_in_transit(ctx, character_guid) {
        return Err(format!(
            "character {character_guid} is in transit between shards"
        ));
    }

    if entry == WorldEntry::Login {
        crate::items::begin_buyback_session(ctx, character_guid);
    }
    restamp_owned_data(ctx, character_guid, owner);

    let entities = ctx.db.game_world_entity();
    // Persist the previous live entity before replacement so ghost relog cannot lose progression.
    if let Some(stale) = entities.guid().find(character_guid) {
        persist_entity(ctx, &stale, false);
        crate::WorldEntity::delete(ctx, character_guid);
        character = chars.guid().find(character_guid).unwrap_or(character);
    }

    restore_reaped_instance(ctx, &mut character);
    if !crate::native::directory::owns_map(ctx, character.map_id) {
        return Err("World Shard does not own the Character map".into());
    }
    if !character.pending_ghost {
        crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(character_guid));
    }

    let mut entity = build_player_entity(ctx, &character, owner);
    if entry == WorldEntry::Login {
        crate::taxi::prepare_world_entry(ctx, &mut entity);
    }
    if character.pending_ghost {
        let (dead, health, player_flags, unit_bytes_1) =
            crate::ghost::ghost_restored_fields(entity.player_flags, entity.unit_bytes_1);
        entity.dead = dead;
        entity.health = health;
        entity.player_flags = player_flags;
        entity.unit_bytes_1 = unit_bytes_1;
    }
    crate::away::at_world_entry(ctx, &mut entity, entry);
    entity.insert(ctx);
    crate::ghost::restore(ctx, character_guid);
    crate::spell::recompute_display(ctx, character_guid);
    if entry == WorldEntry::WorldPort {
        crate::taxi::arm_taxi_flight(ctx, character_guid);
    }
    crate::mount::restore_mount_on_rebuild(ctx, character_guid);

    crate::items::grant_starter_item(ctx, character.guid, owner)?;
    crate::spell::spellbook::grant_createinfo_spells(
        ctx,
        character.guid,
        owner,
        character.race,
        character.class,
    );

    crate::skill::ensure_player_skills(
        ctx,
        character.guid,
        owner,
        character.level as u32,
        character.class,
    );

    crate::talent::apply_learned_talents(ctx, character.guid, owner, character.level as u32);

    crate::spell::apply_starting_passives(
        ctx,
        character.guid,
        character.race,
        character.class,
        character.level,
    );

    crate::spell::recompute_vitals(ctx, character.guid);
    crate::spell::recompute_sheet(ctx, character.guid);

    let (rested, consumed) = crate::xp::accrue_rested_on_login(
        crate::time::now(ctx).to_micros_since_unix_epoch(),
        character.last_logout_micros,
        character.rested_xp,
        character.level as u32,
        character.resting,
    );
    character.rested_xp = rested;
    let previous_rest_state = match entry {
        WorldEntry::Login => lyracore_shared::packing::REST_STATE_NORMAL,
        WorldEntry::WorldPort => character.rest_state,
    };
    character.rest_state = crate::rest::rest_state_after_bonus(
        previous_rest_state,
        character.rested_xp,
        character.level,
    );
    if consumed {
        character.last_logout_micros = 0;
    }
    if character.resting {
        character.rested_since_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;
    }

    character.session_start_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;

    character.owner_identity = owner;
    character.online = true;
    character.first_login = false;
    let rest_state = character.rest_state;
    chars.guid().update(character);
    if let Some(mut entity) = ctx.db.game_world_entity().guid().find(character_guid) {
        let previous_bytes = entity.player_bytes_2;
        crate::rest::set_rest_state_byte(&mut entity, rest_state);
        let resolved_area = crate::terrain::area_at(ctx, entity.map_id, entity.x, entity.y);
        let resolved_zone = resolved_area.as_ref().map(crate::terrain::zone_of);
        let previous_zone = entity.zone_id;
        let was_resting = entity.resting;
        let previous_flags = entity.player_flags;
        if resolved_zone.is_some() {
            crate::world::apply_zone_transition(ctx, &mut entity, resolved_zone);
            crate::rest::check_rest_state(ctx, &mut entity, resolved_area.as_ref());
        }
        if previous_zone != entity.zone_id
            || was_resting != entity.resting
            || previous_flags != entity.player_flags
            || previous_bytes != entity.player_bytes_2
        {
            entity.update(ctx);
        }
    }
    crate::instance::reconcile_instance_removal(ctx, character_guid);
    crate::hooks::fire_on_login(ctx, &crate::hooks::LoginPayload { character_guid });
    Ok(())
}

fn restamp_owned_data(ctx: &ReducerContext, character_guid: u64, identity: Identity) {
    for sweep in crate::CHARACTER_OWNED_RESTAMP_SWEEPS {
        sweep(ctx, character_guid, identity);
    }
}

pub(crate) fn cascade_delete_character(ctx: &ReducerContext, character_guid: u64) {
    // Preserve the issued GUID floor before deleting the last row that refers to it.
    crate::auth::bump_guid_high_water(ctx, character_guid);
    crate::guild::forget_deleted_character(ctx, character_guid);
    for sweep in crate::CHARACTER_OWNED_DELETE_SWEEPS {
        sweep(ctx, character_guid);
    }

    crate::combat::disengage(ctx, character_guid);
    crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(character_guid));
    crate::WorldEntity::delete(ctx, character_guid);
    ctx.db.game_character().guid().delete(character_guid);
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_delete_character(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    if crate::native::realm_auction::pending(ctx, character_guid) {
        return Err("CHAR_AUCTION_TRANSACTION_PENDING".into());
    }
    if crate::native::realm_petition::pending(ctx, character_guid) {
        return Err("CHAR_GUILD_TRANSACTION_PENDING".into());
    }
    if crate::helpers::character_by_guid(ctx, character_guid).is_none()
        && ctx
            .db
            .game_character()
            .guid()
            .find(character_guid)
            .is_some()
    {
        return Err("CHAR_IN_TRANSIT".to_string());
    }
    crate::mail_text::drop_character_letters(ctx, character_guid);
    cascade_delete_character(ctx, character_guid);
    Ok(())
}

pub(crate) fn persisted_pending_ghost(dead: bool, player_flags: u32) -> bool {
    dead && player_flags & lyracore_shared::constants::player_flags::GHOST != 0
}

pub(crate) const RUN_SPEED_BP_1X: u32 = 10_000;

pub(crate) fn persisted_gm_playtest(
    godmode: bool,
    run_speed_mult_bp: u32,
    set_offline: bool,
) -> (bool, u32) {
    if set_offline {
        (false, RUN_SPEED_BP_1X)
    } else {
        (godmode, run_speed_mult_bp)
    }
}

fn accrue_played_on_persist(now_micros: i64, session_start_micros: u64, pool: u32) -> u32 {
    if session_start_micros == 0 {
        return pool;
    }
    let elapsed_secs = (now_micros as u64).saturating_sub(session_start_micros) / 1_000_000;
    pool.saturating_add(elapsed_secs as u32)
}

pub(crate) fn persist_entity(ctx: &ReducerContext, entity: &WorldEntity, set_offline: bool) {
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(entity.guid) {
        c.x = entity.x;
        c.y = entity.y;
        c.z = entity.z;
        c.orientation = entity.orientation;
        if let Some(z) = crate::terrain::zone_id_at(ctx, entity.map_id, entity.x, entity.y) {
            c.zone_id = z;
        }
        c.level = entity.level as u8;
        c.xp = entity.xp;
        c.next_level_xp = entity.next_level_xp;
        c.money = entity.money;
        c.bank_bag_slots = entity.bank_bag_slots;
        // Zero means uninitialized health; pending_ghost restores a released ghost separately.
        c.health = entity.health.max(1);
        c.power = entity.power;

        c.death_expire_micros = entity.death_expire_micros;

        c.pending_instance_id = entity.instance_id;
        c.pending_ghost = persisted_pending_ghost(entity.dead, entity.player_flags);
        let (pending_godmode, pending_speed_bp) =
            persisted_gm_playtest(entity.godmode, entity.run_speed_mult_bp, set_offline);
        c.pending_godmode = pending_godmode;
        c.pending_run_speed_mult_bp = pending_speed_bp;
        // Close the played-time span on every persist so relog cannot count it twice.
        c.played_total_secs = accrue_played_on_persist(
            crate::time::now(ctx).to_micros_since_unix_epoch(),
            c.session_start_micros,
            c.played_total_secs,
        );
        c.session_start_micros = 0;
        c.resting = entity.resting;
        if set_offline {
            c.online = false;
            c.last_logout_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;
            crate::rest::materialize_on_logout(
                &mut c,
                crate::time::now(ctx).to_micros_since_unix_epoch(),
            );
        }
        chars.guid().update(c);
    }
}

pub(crate) fn remove_live_character(
    ctx: &ReducerContext,
    entity: WorldEntity,
    departure: CharacterDeparture,
) {
    // Fire logout hooks before deletion while handlers can still read the live entity.
    crate::hooks::fire_on_logout(
        ctx,
        &crate::hooks::LogoutPayload {
            character_guid: entity.guid,
        },
    );
    crate::duel::interrupt_duel_for(ctx, entity.guid);
    persist_entity(ctx, &entity, true);

    finish_logout(
        ctx,
        entity.guid,
        persisted_pending_ghost(entity.dead, entity.player_flags),
        departure,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CharacterDeparture {
    Logout,
    Transfer,
}

pub(crate) fn remove_loading_character(
    ctx: &ReducerContext,
    guid: u64,
    departure: CharacterDeparture,
) {
    crate::hooks::fire_on_logout(
        ctx,
        &crate::hooks::LogoutPayload {
            character_guid: guid,
        },
    );
    let Some(mut character) = crate::helpers::character_by_guid(ctx, guid) else {
        return;
    };
    character.online = false;
    character.last_logout_micros = crate::time::now(ctx).to_micros_since_unix_epoch() as u64;
    if departure == CharacterDeparture::Logout {
        character.pending_godmode = false;
        character.pending_run_speed_mult_bp = RUN_SPEED_BP_1X;
    }
    crate::rest::materialize_on_logout(
        &mut character,
        crate::time::now(ctx).to_micros_since_unix_epoch(),
    );
    let keep_corpse = character.pending_ghost;
    ctx.db.game_character().guid().update(character);
    finish_logout(ctx, guid, keep_corpse, departure);
}

fn finish_logout(
    ctx: &ReducerContext,
    guid: u64,
    keep_corpse: bool,
    departure: CharacterDeparture,
) {
    crate::combat::disengage(ctx, guid);
    crate::trade::cancel_trade_for(ctx, guid);
    if !keep_corpse {
        crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(guid));
    }
    crate::spell::clear_resurrect_request_and_option(ctx, guid);
    crate::spell::break_stealth(ctx, guid);
    crate::creatures::despawn_pets(ctx, guid);
    if departure == CharacterDeparture::Logout {
        crate::items::clear_buyback(ctx, guid);
    }
    crate::WorldEntity::delete(ctx, guid);
}

#[reducer(client_disconnected)]
pub fn on_disconnect(ctx: &ReducerContext) {
    // Every call from a Peer Shard connects and disconnects as that Shard; it holds no World Session.
    if crate::peer::is_peer(ctx, ctx.sender()) {
        return;
    }
    crate::native::close(ctx);
    if let Some(entity) = entity_by_owner(ctx, ctx.sender()) {
        if !crate::native::has_world_session(ctx, entity.guid) {
            remove_live_character(ctx, entity, CharacterDeparture::Logout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{accrue_played_on_persist, persisted_gm_playtest, RUN_SPEED_BP_1X};

    #[test]
    fn gm_playtest_state_carries_across_a_map_change_and_resets_on_a_real_logout() {
        assert_eq!(
            persisted_gm_playtest(true, 30_000, false),
            (true, 30_000),
            "a cross-map hop carries godmode and the speed multiplier"
        );
        assert_eq!(
            persisted_gm_playtest(true, 30_000, true),
            (false, RUN_SPEED_BP_1X),
            "a real logout resets GM playtest state to off / 1x"
        );
        assert_eq!(
            persisted_gm_playtest(false, RUN_SPEED_BP_1X, false),
            (false, RUN_SPEED_BP_1X)
        );
    }

    #[test]
    fn accrue_played_folds_elapsed_whole_seconds_into_the_pool() {
        assert_eq!(accrue_played_on_persist(5_000_000, 0, 42), 42);
        let now = 1_000_090_500_000_i64;
        let session_start = 1_000_000_000_000_u64;
        assert_eq!(accrue_played_on_persist(now, session_start, 10), 100);
        assert_eq!(accrue_played_on_persist(1_000_000_500, 1_000_000_000, 0), 0);
    }
}
