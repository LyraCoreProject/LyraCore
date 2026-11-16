use super::{
    game_group, game_group_member, game_group_member_partition, game_group_roster_revision,
    group_kind_of, Group, GroupMember, GroupMemberPartition, GroupRosterRevision,
    PartyPartitionState,
};
use lyracore_shared::group::{GroupKind, RaidSlot, RAID_MAX_MEMBERS};
use spacetimedb::{reducer, ReducerContext, Table};

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn sync_group_mirror(
    ctx: &ReducerContext,
    group_id: u64,
    leader_guid: u64,
    loot_method_setting: u8,
    loot_threshold: u8,
    master_looter_guid: u64,
    members: Vec<u64>,
    request_actor: crate::account_ownership::SessionActor,
    partitions: Vec<GroupMemberPartition>,
    roster_revision: u64,
    group_kind: u8,
    raid_slots: Vec<u8>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::account_ownership::require_actor(ctx, request_actor)?;
    if group_id == 0 {
        return Err("group 0 is not a party".to_string());
    }
    if roster_revision == 0 {
        return Err("group mirror has no Realm roster revision".to_string());
    }
    if members.len() > RAID_MAX_MEMBERS || partitions.len() > RAID_MAX_MEMBERS * 2 {
        return Err(
            "group mirror exceeds the bounded current and departed group-member limit".to_string(),
        );
    }
    let (kind, slots) = mirror_kind_and_slots(&members, group_kind, &raid_slots)?;
    let mut partition_guids = std::collections::BTreeSet::new();
    for partition in &partitions {
        if partition.group_id != group_id
            || partition.membership_revision == 0
            || (partition.locator_revision == 0 && partition.state != PartyPartitionState::Unknown)
            || partition.member_active != members.contains(&partition.character_guid)
            || (!partition.member_active && partition.state != PartyPartitionState::Unknown)
            || !partition_guids.insert(partition.character_guid)
        {
            return Err("group mirror has an invalid member partition".to_string());
        }
    }
    if members.iter().any(|guid| !partition_guids.contains(guid)) {
        return Err("group mirror is missing a current member partition".to_string());
    }
    let groups = ctx.db.game_group();
    let member_tbl = ctx.db.game_group_member();
    let partition_tbl = ctx.db.game_group_member_partition();
    let current_revision = ctx
        .db
        .game_group_roster_revision()
        .group_id()
        .find(group_id);
    let incoming_active = !members.is_empty();
    match roster_update(
        current_revision
            .as_ref()
            .map(|current| (current.revision, current.active)),
        roster_revision,
        incoming_active,
    ) {
        RosterUpdate::Keep => return Ok(()),
        RosterUpdate::Conflict => {
            return Err("group mirror conflicts with the accepted roster state".to_string());
        }
        RosterUpdate::Apply => {}
    }
    if current_revision
        .as_ref()
        .is_some_and(|current| current.revision == roster_revision)
    {
        if let Some(group) = groups.group_id().find(group_id) {
            if incoming_active
                && (group.leader_guid != leader_guid
                    || group.loot_method != loot_method_setting
                    || group.loot_threshold != loot_threshold
                    || group.master_looter_guid != master_looter_guid
                    || group_kind_of(&group) != kind
                    || mirror_slots_differ(ctx, group_id, &slots))
            {
                return Err("group mirror conflicts with the accepted party rules".to_string());
            }
        }
    }
    let advances_roster = current_revision
        .as_ref()
        .is_none_or(|current| roster_revision > current.revision);
    if advances_roster {
        let stale_partitions: Vec<_> = partition_tbl
            .by_group()
            .filter(&group_id)
            .filter(|partition| {
                partition.member_active && !members.contains(&partition.character_guid)
            })
            .collect();
        for mut partition in stale_partitions {
            partition.member_active = false;
            partition.state = PartyPartitionState::Unknown;
            partition_tbl.character_guid().update(partition);
        }
    }
    for partition in partitions {
        match partition_tbl
            .character_guid()
            .find(partition.character_guid)
            .map(|current| partition_update(&current, &partition))
        {
            Some(PartitionUpdate::Keep) => {}
            Some(PartitionUpdate::Conflict) => {
                return Err(format!(
                    "group mirror has conflicting partition facts for member {}",
                    partition.character_guid
                ));
            }
            Some(PartitionUpdate::Replace) => {
                partition_tbl.character_guid().update(partition);
            }
            None => {
                partition_tbl.insert(partition);
            }
        }
    }
    let effective_members: Vec<_> = partition_tbl
        .by_group()
        .filter(&group_id)
        .filter(|row| row.member_active)
        .map(|row| (row.membership_revision, row.character_guid))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|(_, guid)| guid)
        .collect();
    if effective_members != members {
        return Err("group mirror roster conflicts with its accepted revision".to_string());
    }
    let current: Vec<(u64, u64)> = member_tbl
        .by_group()
        .filter(&group_id)
        .map(|m| (m.id, m.character_guid))
        .collect();
    let (stale_row_ids, arriving) = mirror_plan(&current, &effective_members);
    let mut membership_changed = arriving.clone();
    for id in stale_row_ids {
        if let Some((_, guid)) = current.iter().find(|(row_id, _)| *row_id == id) {
            crate::loot::tag::revoke_group_member(ctx, group_id, *guid);
            membership_changed.push(*guid);
        }
        member_tbl.id().delete(id);
    }
    if effective_members.is_empty() {
        groups.group_id().delete(group_id);
        store_group_revision(ctx, group_id, roster_revision, false);
        roster_changed(ctx, membership_changed, &effective_members);
        return Ok(());
    }
    let left_behind = detach_arrivals(ctx, &arriving);
    let effective_leader = if effective_members.contains(&leader_guid) {
        leader_guid
    } else {
        *effective_members
            .first()
            .ok_or_else(|| "group mirror has no effective leader".to_string())?
    };
    match groups.group_id().find(group_id) {
        Some(mut g) => {
            g.leader_guid = effective_leader;
            g.loot_method = loot_method_setting;
            g.loot_threshold = loot_threshold;
            g.master_looter_guid = master_looter_guid;
            g.group_type = kind.wire();
            groups.group_id().update(g);
        }
        None => {
            groups.insert(Group {
                group_id,
                leader_guid: effective_leader,
                loot_method: loot_method_setting,
                loot_threshold,
                // Round-robin loot follows this Shard's kills; keep its local cursor.
                rr_cursor: 0,
                master_looter_guid,
                group_type: kind.wire(),
            });
        }
    }
    write_mirror_slots(ctx, group_id, &slots);
    for guid in arriving {
        let owner_identity = crate::helpers::event_recipient_identity(
            crate::helpers::character_by_guid(ctx, guid).map(|c| c.owner_identity),
        );
        member_tbl.insert(GroupMember {
            id: 0,
            group_id,
            character_guid: guid,
            owner_identity,
            raid_slot: slots.get(&guid).copied().unwrap_or_default().wire(),
        });
    }
    store_group_revision(ctx, group_id, roster_revision, true);
    let members: Vec<u64> = effective_members.into_iter().chain(left_behind).collect();
    roster_changed(ctx, membership_changed, &members);
    Ok(())
}

/// Removes arriving members from their other local group and returns who stays behind there.
fn detach_arrivals(ctx: &ReducerContext, arriving: &[u64]) -> Vec<u64> {
    let member_tbl = ctx.db.game_group_member();
    let mut left_behind = Vec::new();
    for guid in arriving {
        for m in member_tbl.by_character().filter(guid).collect::<Vec<_>>() {
            crate::loot::tag::revoke_group_member(ctx, m.group_id, *guid);
            member_tbl.id().delete(m.id);
            left_behind.extend(
                member_tbl
                    .by_group()
                    .filter(&m.group_id)
                    .map(|r| r.character_guid),
            );
        }
    }
    left_behind
}

fn roster_changed(ctx: &ReducerContext, changed: Vec<u64>, members: &[u64]) {
    if changed.is_empty() {
        return;
    }
    let affected: std::collections::BTreeSet<u64> =
        changed.iter().chain(members).copied().collect();
    crate::native::health::group_changed(ctx, affected);
    for guid in changed {
        crate::instance::reconcile_instance_removal(ctx, guid);
    }
}

fn store_group_revision(ctx: &ReducerContext, group_id: u64, revision: u64, active: bool) {
    let revisions = ctx.db.game_group_roster_revision();
    let row = GroupRosterRevision {
        group_id,
        revision,
        active,
    };
    if revisions.group_id().find(group_id).is_some() {
        revisions.group_id().update(row);
    } else {
        revisions.insert(row);
    }
}

fn mirror_kind_and_slots(
    members: &[u64],
    group_kind: u8,
    raid_slots: &[u8],
) -> Result<(GroupKind, std::collections::BTreeMap<u64, RaidSlot>), String> {
    let kind = GroupKind::from_wire(group_kind)
        .ok_or_else(|| format!("group mirror has unknown group kind {group_kind}"))?;
    if members.len() > kind.member_cap() {
        return Err(format!(
            "group mirror has {} members for a group that holds at most {}",
            members.len(),
            kind.member_cap()
        ));
    }
    if raid_slots.len() != members.len() {
        return Err(format!(
            "group mirror has {} Raid Slots for {} members",
            raid_slots.len(),
            members.len()
        ));
    }
    let slots = members
        .iter()
        .zip(raid_slots)
        .map(|(&guid, &byte)| {
            RaidSlot::from_wire(byte)
                .map(|slot| (guid, slot))
                .ok_or_else(|| format!("group mirror has invalid Raid Slot {byte} for {guid}"))
        })
        .collect::<Result<_, _>>()?;
    Ok((kind, slots))
}

fn write_mirror_slots(
    ctx: &ReducerContext,
    group_id: u64,
    slots: &std::collections::BTreeMap<u64, RaidSlot>,
) {
    let member_tbl = ctx.db.game_group_member();
    for mut row in member_tbl.by_group().filter(&group_id).collect::<Vec<_>>() {
        let slot = slots.get(&row.character_guid).copied().unwrap_or_default();
        if row.raid_slot != slot.wire() {
            row.raid_slot = slot.wire();
            member_tbl.id().update(row);
        }
    }
}

fn mirror_slots_differ(
    ctx: &ReducerContext,
    group_id: u64,
    slots: &std::collections::BTreeMap<u64, RaidSlot>,
) -> bool {
    ctx.db
        .game_group_member()
        .by_group()
        .filter(&group_id)
        .any(|row| {
            slots
                .get(&row.character_guid)
                .is_some_and(|slot| slot.wire() != row.raid_slot)
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RosterUpdate {
    Keep,
    Apply,
    Conflict,
}

fn roster_update(
    current: Option<(u64, bool)>,
    incoming_revision: u64,
    incoming_active: bool,
) -> RosterUpdate {
    let Some((current_revision, current_active)) = current else {
        return RosterUpdate::Apply;
    };
    match incoming_revision.cmp(&current_revision) {
        std::cmp::Ordering::Less => RosterUpdate::Keep,
        std::cmp::Ordering::Greater => RosterUpdate::Apply,
        std::cmp::Ordering::Equal if current_active == incoming_active => RosterUpdate::Apply,
        std::cmp::Ordering::Equal => RosterUpdate::Conflict,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PartitionUpdate {
    Keep,
    Replace,
    Conflict,
}

fn partition_state_order(state: PartyPartitionState) -> u8 {
    match state {
        PartyPartitionState::Unknown => 0,
        PartyPartitionState::Known => 1,
        PartyPartitionState::PendingTransfer => 2,
    }
}

fn partition_update(
    current: &GroupMemberPartition,
    incoming: &GroupMemberPartition,
) -> PartitionUpdate {
    match incoming
        .membership_revision
        .cmp(&current.membership_revision)
    {
        std::cmp::Ordering::Less => return PartitionUpdate::Keep,
        std::cmp::Ordering::Greater => return PartitionUpdate::Replace,
        std::cmp::Ordering::Equal => {}
    }
    if current.member_active != incoming.member_active {
        return if current.member_active {
            PartitionUpdate::Replace
        } else {
            PartitionUpdate::Keep
        };
    }
    if !current.member_active {
        return PartitionUpdate::Keep;
    }
    if current.group_id != incoming.group_id {
        return PartitionUpdate::Conflict;
    }
    match incoming.locator_revision.cmp(&current.locator_revision) {
        std::cmp::Ordering::Less => PartitionUpdate::Keep,
        std::cmp::Ordering::Greater => PartitionUpdate::Replace,
        std::cmp::Ordering::Equal => {
            let incoming_order = partition_state_order(incoming.state);
            let current_order = partition_state_order(current.state);
            match incoming_order.cmp(&current_order) {
                std::cmp::Ordering::Less => PartitionUpdate::Keep,
                std::cmp::Ordering::Greater => PartitionUpdate::Replace,
                std::cmp::Ordering::Equal
                    if incoming.map_id == current.map_id
                        && incoming.instance_id == current.instance_id =>
                {
                    PartitionUpdate::Keep
                }
                std::cmp::Ordering::Equal => PartitionUpdate::Conflict,
            }
        }
    }
}

fn mirror_plan(current: &[(u64, u64)], incoming: &[u64]) -> (Vec<u64>, Vec<u64>) {
    let stale = current
        .iter()
        .filter(|(_, guid)| !incoming.contains(guid))
        .map(|(id, _)| *id)
        .collect();
    let arriving = incoming
        .iter()
        .filter(|guid| !current.iter().any(|(_, have)| have == *guid))
        .copied()
        .collect();
    (stale, arriving)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_push_needs_a_known_kind_and_one_valid_slot_per_member() {
        let (kind, slots) = mirror_kind_and_slots(&[10, 11], 1, &[0, 0x81]).unwrap();
        assert_eq!(kind, GroupKind::Raid);
        assert_eq!(slots[&11], RaidSlot::new(1, true).unwrap());
        assert!(mirror_kind_and_slots(&[10, 11], 2, &[0, 0]).is_err());
        assert!(mirror_kind_and_slots(&[10, 11], 1, &[0]).is_err());
        assert!(mirror_kind_and_slots(&[10], 1, &[0, 0]).is_err());
        assert!(mirror_kind_and_slots(&[10, 11], 1, &[0, 8]).is_err());
        assert_eq!(
            mirror_kind_and_slots(&[], 0, &[]).map(|(kind, slots)| (kind, slots.len())),
            Ok((GroupKind::Party, 0)),
            "the disband tombstone carries no members and no slots"
        );
    }

    #[test]
    fn the_mirror_diff_deletes_departed_rows_and_inserts_arriving_members() {
        let current = [(10u64, 100u64), (11, 200)];
        assert_eq!(mirror_plan(&current, &[100, 200]), (vec![], vec![]));
        assert_eq!(mirror_plan(&current, &[100, 200, 300]), (vec![], vec![300]));
        assert_eq!(mirror_plan(&current, &[100]), (vec![11], vec![]));
        assert_eq!(mirror_plan(&current, &[200, 300]), (vec![10], vec![300]));
        assert_eq!(mirror_plan(&current, &[]), (vec![10, 11], vec![]));
        assert_eq!(mirror_plan(&[], &[100, 200]), (vec![], vec![100, 200]));
    }

    fn partition(
        membership_revision: u64,
        member_active: bool,
        locator_revision: u64,
        state: PartyPartitionState,
        map_id: u32,
    ) -> GroupMemberPartition {
        GroupMemberPartition {
            character_guid: 100,
            group_id: 7,
            membership_revision,
            member_active,
            map_id,
            instance_id: 0,
            locator_revision,
            state,
        }
    }

    #[test]
    fn partition_updates_follow_realm_membership_and_locator_order() {
        let known = partition(10, true, 4, PartyPartitionState::Known, 0);
        let transient_unknown = partition(10, true, 4, PartyPartitionState::Unknown, 0);
        assert_eq!(
            partition_update(&known, &transient_unknown),
            PartitionUpdate::Keep,
            "a transient read miss at the same Realm locator cannot erase a confirmed partition"
        );
        assert_eq!(
            partition_update(&transient_unknown, &known),
            PartitionUpdate::Replace,
            "the same settled Realm locator recovers after a transient read miss"
        );

        let pending = partition(10, true, 4, PartyPartitionState::PendingTransfer, 0);
        assert_eq!(partition_update(&known, &pending), PartitionUpdate::Replace);
        assert_eq!(partition_update(&pending, &known), PartitionUpdate::Keep);
        let destination = partition(10, true, 5, PartyPartitionState::Known, 36);
        assert_eq!(
            partition_update(&pending, &destination),
            PartitionUpdate::Replace,
            "Realm-core's next locator revision settles the crossing before arrival release"
        );

        let removed = partition(10, false, 5, PartyPartitionState::Unknown, 36);
        assert_eq!(
            partition_update(&destination, &removed),
            PartitionUpdate::Replace
        );
        assert_eq!(
            partition_update(&removed, &destination),
            PartitionUpdate::Keep
        );
        let rejoined = partition(11, true, 5, PartyPartitionState::Known, 36);
        assert_eq!(
            partition_update(&removed, &rejoined),
            PartitionUpdate::Replace
        );
    }

    #[test]
    fn complete_roster_revision_orders_members_rules_and_disband() {
        assert_eq!(roster_update(None, 1, true), RosterUpdate::Apply);
        assert_eq!(
            roster_update(Some((2, true)), 1, true),
            RosterUpdate::Keep,
            "a delayed pre-join roster cannot remove the newer member or restore old rules"
        );
        assert_eq!(
            roster_update(Some((2, true)), 3, true),
            RosterUpdate::Apply,
            "a revisioned leave or leadership/loot change replaces the complete roster"
        );
        assert_eq!(
            roster_update(Some((4, false)), 3, true),
            RosterUpdate::Keep,
            "the durable disband tombstone survives restart and rejects an older live roster"
        );
        assert_eq!(
            roster_update(Some((4, false)), 4, true),
            RosterUpdate::Conflict,
            "equal-revision state cannot revive a disbanded party"
        );
    }

    #[test]
    fn equal_revisions_cannot_disagree_about_a_known_partition() {
        let current = partition(10, true, 4, PartyPartitionState::Known, 0);
        let conflicting = partition(10, true, 4, PartyPartitionState::Known, 36);
        assert_eq!(
            partition_update(&current, &conflicting),
            PartitionUpdate::Conflict
        );
    }
}
