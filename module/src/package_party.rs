use std::collections::BTreeSet;

use lyracore_shared::group::GroupKind;
use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::group::{self, game_group, Departure, JoinTarget};
use crate::meeting_stone::{game_meeting_stone_party, game_meeting_stone_seeker};

const AUTHORITY_MICROS: i64 = 45_000_000;

#[table(accessor = game_package_party_authority)]
pub struct PackagePartyAuthority {
    #[primary_key]
    pub gateway_id: String,
    pub single_shard: bool,
    pub expires_micros: i64,
}

#[reducer]
pub fn gw_certify_package_party_authority(
    ctx: &ReducerContext,
    gateway_id: String,
    single_shard: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if gateway_id.is_empty() || gateway_id.len() > 128 {
        return Err("Invalid Gateway identity".into());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let authorities = ctx.db.game_package_party_authority();
    let expired: Vec<_> = authorities
        .iter()
        .filter(|row| row.expires_micros <= now)
        .collect();
    for expired in expired {
        authorities.gateway_id().delete(expired.gateway_id);
    }
    let row = PackagePartyAuthority {
        gateway_id: gateway_id.clone(),
        single_shard,
        expires_micros: now.saturating_add(AUTHORITY_MICROS),
    };
    if authorities.gateway_id().find(gateway_id).is_some() {
        authorities.gateway_id().update(row);
    } else {
        authorities.insert(row);
    }
    Ok(())
}

pub fn require_authority(ctx: &ReducerContext) -> Result<(), String> {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let mut live = ctx
        .db
        .game_package_party_authority()
        .iter()
        .filter(|row| row.expires_micros > now)
        .peekable();
    if live.peek().is_none() || live.any(|row| !row.single_shard) {
        return Err("Package parties require a certified single-Shard Realm".into());
    }
    Ok(())
}

pub(crate) fn protects_group(ctx: &ReducerContext, group_id: u64) -> bool {
    use crate::instance::{game_instance, game_package_instance};
    group_id != 0
        && ctx
            .db
            .game_instance()
            .by_party()
            .filter(group_id)
            .any(|instance| {
                ctx.db
                    .game_package_instance()
                    .instance_id()
                    .find(instance.instance_id)
                    .is_some()
            })
}

// Callers establish every member's consent and propagate Refusals; see docs/package-api.md.
pub fn form_party(
    ctx: &ReducerContext,
    package_name: &str,
    members: &[u64],
) -> Result<u64, String> {
    require_authority(ctx)?;
    if !crate::package_teardown::runs(ctx, &package_name.replace('-', "_")) {
        return Err("PackageDisabled".into());
    }
    let selected: BTreeSet<_> = members.iter().copied().collect();
    if !(2..=5).contains(&members.len()) || selected.len() != members.len() || selected.contains(&0)
    {
        return Err("Package Party needs two to five distinct Characters".into());
    }
    let mut partition = None;
    let mut groups = BTreeSet::new();
    let mut retained_group = None;
    for &guid in members {
        crate::helpers::character_by_guid(ctx, guid).ok_or("Missing Character")?;
        let entity = crate::helpers::live_entity(ctx, guid)?;
        if entity.dead || entity.health == 0 || crate::taxi::is_in_flight(ctx, guid) {
            return Err("Package Party member is unavailable".into());
        }
        let here = (entity.map_id, entity.instance_id);
        if partition.is_some_and(|partition| partition != here) {
            return Err("Package Party members must share one partition".into());
        }
        partition = Some(here);
        if ctx
            .db
            .game_meeting_stone_seeker()
            .character_guid()
            .find(guid)
            .is_some()
        {
            return Err("Package Party member is in the Meeting Stone Queue".into());
        }
        let membership =
            group::checked_group_membership(ctx, guid).map_err(|error| format!("{error:?}"))?;
        if let Some((_, group)) = membership {
            if group::group_kind_of(&group) != GroupKind::Party {
                return Err("Package Party cannot include a Raid".into());
            }
            let roster = group::bounded_roster(ctx, group.group_id, GroupKind::Party)
                .ok_or("Invalid Party roster")?;
            if roster.len() < 2
                || !roster
                    .iter()
                    .any(|member| member.character_guid == group.leader_guid)
            {
                return Err("Invalid Party roster".into());
            }
            if ctx
                .db
                .game_meeting_stone_party()
                .group_id()
                .find(group.group_id)
                .is_some()
            {
                return Err("Package Party member is in the Meeting Stone Queue".into());
            }
            if roster
                .iter()
                .any(|member| !selected.contains(&member.character_guid))
            {
                return Err("Package Party must include every premade member".into());
            }
            groups.insert(group.group_id);
            retained_group.get_or_insert(group.group_id);
        }
    }
    if let Some(group_id) = retained_group {
        if group::members_of(ctx, group_id).len() == selected.len() {
            return Ok(group_id);
        }
    }
    for &guid in members {
        let membership =
            group::checked_group_membership(ctx, guid).map_err(|error| format!("{error:?}"))?;
        if membership.is_some_and(|(member, _)| Some(member.group_id) != retained_group) {
            group::remove_member(ctx, guid, Departure::Left);
        }
    }
    let (group_id, formed) = if let Some(group_id) = retained_group {
        (group_id, false)
    } else {
        let joined = group::plan_join(
            ctx,
            members[1],
            JoinTarget::NewParty {
                leader_guid: members[0],
            },
        )
        .map_err(|error| format!("{error:?}"))?
        .apply(ctx);
        (joined.group_id, true)
    };
    for &guid in members {
        if group::checked_group_membership(ctx, guid)
            .map_err(|error| format!("{error:?}"))?
            .is_none()
        {
            group::plan_join(ctx, guid, JoinTarget::Group(group_id))
                .map_err(|error| format!("{error:?}"))?
                .apply(ctx);
        }
    }
    for departed in groups.into_iter().filter(|previous| *previous != group_id) {
        group::advance_group_revision(ctx, departed, true, false);
    }
    group::advance_group_revision(ctx, group_id, !formed, true);
    Ok(group_id)
}

// Callers obtain the incoming Character's consent and propagate Refusals for atomic replacement.
pub fn replace(
    ctx: &ReducerContext,
    package_name: &str,
    group_id: u64,
    old_guid: u64,
    new_guid: u64,
) -> Result<(), String> {
    use crate::instance::{game_instance, game_package_instance};
    require_authority(ctx)?;
    if !crate::package_teardown::runs(ctx, &package_name.replace('-', "_")) {
        return Err("PackageDisabled".into());
    }
    if old_guid == 0 || new_guid == 0 || old_guid == new_guid {
        return Err("Package Party replacement needs two distinct Characters".into());
    }
    let incoming = crate::helpers::character_by_guid(ctx, new_guid)
        .ok_or("Incoming Character is unavailable")?;
    let body = crate::helpers::live_entity(ctx, new_guid)?;
    if crate::taxi::is_in_flight(ctx, new_guid) {
        return Err("Incoming Character is in flight".into());
    }
    if !incoming.online {
        crate::sessionless::group_action_gate(ctx, new_guid)
            .map_err(|refusal| format!("{refusal:?}"))?;
    }
    let group = ctx
        .db
        .game_group()
        .group_id()
        .find(group_id)
        .ok_or("Package Party is unavailable")?;
    if group::group_kind_of(&group) != GroupKind::Party {
        return Err("Package Party replacement cannot change a Raid".into());
    }
    let roster = group::bounded_roster(ctx, group_id, GroupKind::Party)
        .ok_or("Invalid Package Party roster")?;
    if roster.len() != 5
        || !roster
            .iter()
            .any(|member| member.character_guid == group.leader_guid)
    {
        return Err("Package Party replacement requires a full five-Character Party".into());
    }
    let incoming_lobby = ctx
        .db
        .game_instance()
        .instance_id()
        .find(body.instance_id)
        .is_some_and(|lobby| {
            lobby.map_id == body.map_id
                && lobby.party_id == 0
                && !lobby.reset_requested
                && ctx
                    .db
                    .game_package_instance()
                    .instance_id()
                    .find(lobby.instance_id)
                    .is_some_and(|owner| owner.package_name == package_name)
        });
    let owned = ctx
        .db
        .game_instance()
        .by_party()
        .filter(group_id)
        .any(|instance| {
            !instance.reset_requested
                && instance.map_id == body.map_id
                && (body.instance_id == 0
                    || body.instance_id == instance.instance_id
                    || incoming_lobby)
                && ctx
                    .db
                    .game_package_instance()
                    .instance_id()
                    .find(instance.instance_id)
                    .is_some_and(|owner| owner.package_name == package_name)
        });
    if !owned {
        return Err("Package Party replacement requires its owned Instance".into());
    }
    let old =
        group::checked_group_membership(ctx, old_guid).map_err(|error| format!("{error:?}"))?;
    let new =
        group::checked_group_membership(ctx, new_guid).map_err(|error| format!("{error:?}"))?;
    if old.is_none()
        && new
            .as_ref()
            .is_some_and(|(member, _)| member.group_id == group_id)
    {
        return Ok(());
    }
    if !old.is_some_and(|(member, _)| member.group_id == group_id) {
        return Err("Departing Character is not in this Package Party".into());
    }
    if new.is_some() {
        return Err("Incoming Character already belongs to a Party".into());
    }
    group::remove_member(ctx, old_guid, Departure::Left);
    group::plan_join(ctx, new_guid, JoinTarget::Group(group_id))
        .map_err(|error| format!("{error:?}"))?
        .apply(ctx);
    group::advance_group_revision(ctx, group_id, true, true);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_package_form_party(ctx: &ReducerContext, members: Vec<u64>) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    form_party(ctx, "fixture", &members).map(|_| ())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_package_replace_party_member(
    ctx: &ReducerContext,
    package_name: String,
    group_id: u64,
    old_guid: u64,
    new_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    replace(ctx, &package_name, group_id, old_guid, new_guid)
}
