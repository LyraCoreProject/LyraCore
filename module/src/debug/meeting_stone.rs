use spacetimedb::{reducer, ReducerContext, Table, TimeDuration};

use lyracore_shared::constants::go_type;
use lyracore_shared::group::{GroupKind, RaidSlot};

use crate::meeting_stone::{
    game_meeting_stone, game_meeting_stone_party, game_meeting_stone_seeker, MeetingStone,
};
use crate::{
    game_account_claim, game_character, game_gameobject_template, game_group, game_group_member,
    AccountClaim, Group, GroupMember,
};

const FIXTURE_CLAIM_MICROS: i64 = 60 * 60 * 1_000_000;

#[allow(clippy::too_many_arguments)]
#[reducer]
pub fn debug_stage_meeting_stone(
    ctx: &ReducerContext,
    entry: u32,
    go_guid: u64,
    min_level: u32,
    max_level: u32,
    area_id: u32,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let templates = ctx.db.game_gameobject_template();
    templates.entry().delete(entry);
    templates.insert(crate::GameObjectTemplate {
        entry,
        type_id: go_type::MEETINGSTONE,
        display_id: 0,
        name: format!("Meeting Stone {entry}"),
        data0: 0,
        data1: 0,
        gather_skill_line: 0,
        respawn_secs: 0,
        gather_gray: 0,
        lock_id: 0,
        size: 0.0,
    });
    let stones = ctx.db.game_meeting_stone();
    stones.entry().delete(entry);
    stones.insert(MeetingStone {
        entry,
        min_level,
        max_level,
        area_id,
    });
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(x, y);
    crate::GameObject::delete(ctx, go_guid);
    let go = crate::GameObject {
        guid: go_guid,
        template_entry: entry,
        map_id,
        x,
        y,
        z,
        orientation: 0.0,
        state: 0,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: 0,
        grid_x,
        grid_y,
        cell: lyracore_shared::spatial::cell_id_at(x, y),
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
    }
    .insert(ctx);
    crate::go_collider::register(ctx, &go);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[reducer]
pub fn debug_stage_meeting_stone_character(
    ctx: &ReducerContext,
    guid: u64,
    account_id: u64,
    race: u8,
    class: u8,
    level: u8,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::WorldEntity::delete(ctx, guid);
    let mut character = crate::helpers::character_by_guid(ctx, 1)
        .ok_or("the seeded Tester is the fixture template")?;
    character.guid = guid;
    character.name = format!("Seeker{guid}");
    character.race = race;
    character.class = class;
    character.level = level;
    character.online = true;
    character.first_login = false;
    (character.map_id, character.x, character.y, character.z) = (map_id, x, y, z);
    character.pending_instance_id = 0;
    if crate::helpers::character_by_guid(ctx, guid).is_some() {
        ctx.db.game_character().guid().update(character);
    } else {
        ctx.db.game_character().insert(character);
    }
    super::debug_spawn_player_entity(ctx, guid)?;
    stage_claim(ctx, guid, account_id)
}

#[reducer]
pub fn debug_stage_meeting_stone_claim(
    ctx: &ReducerContext,
    guid: u64,
    account_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    stage_claim(ctx, guid, account_id)
}

fn stage_claim(ctx: &ReducerContext, guid: u64, account_id: u64) -> Result<(), String> {
    let expires_micros = crate::time::now(ctx)
        .to_micros_since_unix_epoch()
        .checked_add(FIXTURE_CLAIM_MICROS)
        .ok_or("claim deadline overflow")?;
    let claims = ctx.db.game_account_claim();
    claims.account_id().delete(account_id);
    claims.insert(AccountClaim {
        account_id,
        generation: 1,
        request_nonce: u128::from(guid),
        character_guid: guid,
        expires_micros,
        closed: false,
    });
    Ok(())
}

#[reducer]
pub fn debug_stage_meeting_stone_group(
    ctx: &ReducerContext,
    leader_guid: u64,
    member_guids: Vec<u64>,
    raid: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let guids: Vec<u64> = std::iter::once(leader_guid).chain(member_guids).collect();
    if let Some(grouped) = guids
        .iter()
        .find(|guid| crate::group::group_of(ctx, **guid).is_some())
    {
        return Err(format!("{grouped} is already in a group"));
    }
    let kind = if raid {
        GroupKind::Raid
    } else {
        GroupKind::Party
    };
    if guids.len() > kind.member_cap() {
        return Err(format!("{} members do not fit a {kind:?}", guids.len()));
    }
    let group = ctx.db.game_group().insert(Group {
        group_id: 0,
        leader_guid,
        loot_method: crate::group::loot_method::GROUP,
        loot_threshold: 2,
        rr_cursor: 0,
        master_looter_guid: 0,
        group_type: kind.wire(),
    });
    let mut slots: Vec<RaidSlot> = Vec::new();
    for guid in guids {
        let slot = match kind {
            GroupKind::Party => RaidSlot::default(),
            GroupKind::Raid => RaidSlot::for_raid_joiner(slots.iter().copied())
                .ok_or("the raid has no free Subgroup")?,
        };
        slots.push(slot);
        let owner_identity = crate::helpers::character_by_guid(ctx, guid)
            .map_or(spacetimedb::Identity::ZERO, |character| {
                character.owner_identity
            });
        ctx.db.game_group_member().insert(GroupMember {
            id: 0,
            group_id: group.group_id,
            character_guid: guid,
            owner_identity,
            raid_slot: slot.wire(),
        });
    }
    Ok(())
}

#[reducer]
pub fn debug_backdate_meeting_stone_seeker(
    ctx: &ReducerContext,
    character_guid: u64,
    secs: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seekers = ctx.db.game_meeting_stone_seeker();
    let mut seeker = seekers
        .character_guid()
        .find(character_guid)
        .ok_or_else(|| format!("{character_guid} is not a Seeker"))?;
    seeker.queued_at = seeker
        .queued_at
        .checked_sub(seconds(secs))
        .ok_or("queued_at underflow")?;
    seekers.character_guid().update(seeker);
    Ok(())
}

#[reducer]
pub fn debug_backdate_meeting_stone_party(
    ctx: &ReducerContext,
    group_id: u64,
    secs: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let parties = ctx.db.game_meeting_stone_party();
    let mut party = parties
        .group_id()
        .find(group_id)
        .ok_or_else(|| format!("party {group_id} is not queued"))?;
    party.queued_at = party
        .queued_at
        .checked_sub(seconds(secs))
        .ok_or("queued_at underflow")?;
    party.next_reminder_at = party
        .next_reminder_at
        .checked_sub(seconds(secs))
        .ok_or("next_reminder_at underflow")?;
    parties.group_id().update(party);
    Ok(())
}

fn seconds(secs: u32) -> TimeDuration {
    TimeDuration::from_micros(i64::from(secs) * 1_000_000)
}
