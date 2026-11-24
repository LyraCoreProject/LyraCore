use lyracore_shared::quest::{share_event_kind, share_result, ShareCandidate};
use spacetimedb::{table, ReducerContext, Table};

use super::{
    accept_gates, character_quest_row, game_quest_flags, game_quest_template, quest_slots_used,
    QuestTemplate, MAX_QUEST_LOG_SIZE, PARTY_SHARE_RANGE_SQ,
};
use crate::game_world_entity;

/// One Quest Share per receiving Character.
#[table(
    accessor = game_quest_share,
    index(accessor = by_sharer, btree(columns = [sharer_guid]))
)]
pub struct QuestShare {
    #[primary_key]
    pub receiver_guid: u64,
    pub sharer_guid: u64,
    pub quest_entry: u32,
}

crate::outside_table_catalog!(game_quest_share);

crate::character_owned!(delete, fn sweep_delete_game_quest_share(ctx, character_guid) {
    forget_quest_shares(ctx, character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_quest_share());

/// vmangos `WorldSession::HandlePushQuestToParty`; unlike it, refuses an unshareable quest.
pub(crate) fn apply_push_quest_to_party(
    ctx: &ReducerContext,
    sender_guid: u64,
    quest_entry: u32,
) -> Result<(), crate::Refusal> {
    if !character_quest_row(ctx, sender_guid, quest_entry).is_some_and(|q| q.is_active()) {
        return Err("you are not on that quest".into());
    }
    if !quest_shareable(ctx, quest_entry) {
        return Err("that quest cannot be shared".into());
    }
    let sender_entity = crate::helpers::live_entity(ctx, sender_guid)
        .map_err(|_| "player not in world".to_string())?;
    let m = crate::group::group_of(ctx, sender_guid).ok_or_else(|| "not in a group".to_string())?;
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;
    for member in crate::group::members_of(ctx, m.group_id) {
        let receiver_guid = member.character_guid;
        if receiver_guid == sender_guid {
            continue;
        }
        let Some(receiver) = ctx.db.game_world_entity().guid().find(receiver_guid) else {
            continue;
        };
        push_result(ctx, sender_guid, receiver_guid, share_result::SHARING_QUEST);
        let result = evaluate_share(ctx, &sender_entity, &tmpl, &receiver);
        if result != share_result::SHARING_QUEST {
            push_result(ctx, sender_guid, receiver_guid, result);
            continue;
        }
        let shares = ctx.db.game_quest_share();
        shares.receiver_guid().delete(receiver_guid);
        shares.insert(QuestShare {
            receiver_guid,
            sharer_guid: sender_guid,
            quest_entry,
        });
        crate::group::push_event(
            ctx,
            receiver_guid,
            share_event_kind::QUEST_SHARE,
            sender_guid,
            quest_entry.to_string(),
        );
    }
    Ok(())
}

fn evaluate_share(
    ctx: &ReducerContext,
    sender: &crate::WorldEntity,
    tmpl: &QuestTemplate,
    member: &crate::WorldEntity,
) -> u8 {
    let existing = character_quest_row(ctx, member.guid, tmpl.entry);
    lyracore_shared::quest::share_result(ShareCandidate {
        same_partition: member.map_id == sender.map_id && member.instance_id == sender.instance_id,
        in_range: within_share_range(member, sender),
        on_quest: existing.as_ref().is_some_and(|q| q.is_active()),
        finished: existing.as_ref().is_some_and(|q| q.rewarded) && !tmpl.repeatable,
        log_full: quest_slots_used(ctx, member.guid) >= MAX_QUEST_LOG_SIZE,
        gates_ok: accept_gates(ctx, member, tmpl).is_ok(),
        holds_quest_share: held_share(ctx, member.guid).is_some(),
    })
}

fn within_share_range(a: &crate::WorldEntity, b: &crate::WorldEntity) -> bool {
    let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
    dx * dx + dy * dy + dz * dz <= PARTY_SHARE_RANGE_SQ
}

/// vmangos `Player::CanShareQuest`: a shareable quest the sharer is on.
pub(super) fn can_share(ctx: &ReducerContext, sharer_guid: u64, quest_entry: u32) -> bool {
    quest_shareable(ctx, quest_entry)
        && character_quest_row(ctx, sharer_guid, quest_entry).is_some_and(|q| q.is_active())
}

fn quest_shareable(ctx: &ReducerContext, quest_entry: u32) -> bool {
    ctx.db
        .game_quest_flags()
        .quest_entry()
        .find(quest_entry)
        .is_some_and(|quest| lyracore_shared::quest::shareable(quest.flags))
}

pub(super) fn require_quest_share(
    ctx: &ReducerContext,
    receiver_guid: u64,
    sharer_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    held_share(ctx, receiver_guid)
        .filter(|share| share.names(sharer_guid, quest_entry))
        .map(|_| ())
        .ok_or_else(|| "no party member shared that quest".to_string())
}

/// Ends the receiver's Quest Share after an accept; its sharer hears when it was for this quest.
pub(super) fn settle_on_accept(ctx: &ReducerContext, receiver_guid: u64, quest_entry: u32) {
    let Some(share) = take(ctx, receiver_guid) else {
        return;
    };
    if share.quest_entry == quest_entry {
        push_result(
            ctx,
            share.sharer_guid,
            receiver_guid,
            share_result::ACCEPT_QUEST,
        );
    }
}

/// Ends a Quest Share after a refused accept left quest log and bags unchanged; true: commit.
pub(crate) fn end_after_refused_accept(
    ctx: &ReducerContext,
    receiver_guid: u64,
    quest_entry: u32,
) -> bool {
    let Some(share) = take(ctx, receiver_guid) else {
        return false;
    };
    let entity = |guid: u64| ctx.db.game_world_entity().guid().find(guid);
    let apart = entity(receiver_guid)
        .zip(entity(share.sharer_guid))
        .is_some_and(|(receiver, sharer)| {
            receiver.map_id != sharer.map_id
                || receiver.instance_id != sharer.instance_id
                || !within_share_range(&receiver, &sharer)
        });
    if share.quest_entry == quest_entry && apart {
        push_result(ctx, share.sharer_guid, receiver_guid, share_result::TOO_FAR);
    }
    true
}

/// vmangos `WorldSession::HandleQuestPushResult`: relays the receiver's answer to its sharer.
pub(crate) fn answer_quest_share(ctx: &ReducerContext, receiver_guid: u64, answer: u8) {
    if let Some(share) = take(ctx, receiver_guid) {
        push_result(ctx, share.sharer_guid, receiver_guid, answer);
    }
}

/// Ends every Quest Share the Character holds or gave.
pub(crate) fn forget_quest_shares(ctx: &ReducerContext, character_guid: u64) {
    let shares = ctx.db.game_quest_share();
    shares.receiver_guid().delete(character_guid);
    for share in shares
        .by_sharer()
        .filter(character_guid)
        .collect::<Vec<_>>()
    {
        shares.receiver_guid().delete(share.receiver_guid);
    }
}

impl QuestShare {
    fn names(&self, sharer_guid: u64, quest_entry: u32) -> bool {
        self.sharer_guid == sharer_guid && self.quest_entry == quest_entry
    }
}

/// The receiver's Quest Share while its sharer is still in the receiver's group.
fn held_share(ctx: &ReducerContext, receiver_guid: u64) -> Option<QuestShare> {
    let share = ctx
        .db
        .game_quest_share()
        .receiver_guid()
        .find(receiver_guid)?;
    let group_of = |guid| crate::group::group_of(ctx, guid).map(|member| member.group_id);
    let grouped = group_of(receiver_guid).is_some_and(|id| group_of(share.sharer_guid) == Some(id));
    grouped.then_some(share)
}

fn take(ctx: &ReducerContext, receiver_guid: u64) -> Option<QuestShare> {
    let shares = ctx.db.game_quest_share();
    let share = shares.receiver_guid().find(receiver_guid)?;
    shares.receiver_guid().delete(receiver_guid);
    Some(share)
}

fn push_result(ctx: &ReducerContext, sharer_guid: u64, receiver_guid: u64, result: u8) {
    crate::group::push_event(
        ctx,
        sharer_guid,
        share_event_kind::QUEST_PUSH_RESULT,
        receiver_guid,
        result.to_string(),
    );
}

#[cfg(test)]
mod tests {
    use super::QuestShare;

    #[test]
    fn a_quest_share_admits_only_its_sharer_and_quest() {
        let share = QuestShare {
            receiver_guid: 2,
            sharer_guid: 1,
            quest_entry: 100,
        };
        assert!(share.names(1, 100));
        assert!(!share.names(1, 101));
        assert!(!share.names(3, 100));
    }
}
