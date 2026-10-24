use lyracore_shared::guild::{event_kind, LEADER_RANK};
use spacetimedb::ReducerContext;

use super::membership::{disband_guild, game_guild_invite};
use super::{game_guild, game_guild_member, member, petition, push_event, GuildMember};

pub(crate) fn forget_deleted_character(ctx: &ReducerContext, character_guid: u64) {
    ctx.db
        .game_guild_invite()
        .target_guid()
        .delete(character_guid);
    if let Some(departed) = member(ctx, character_guid) {
        leave_guild(ctx, departed);
    }
    petition::withdraw(ctx, character_guid);
}

fn leave_guild(ctx: &ReducerContext, departed: GuildMember) {
    let guild_id = departed.guild_id;
    let Some(mut guild) = ctx.db.game_guild().guild_id().find(guild_id) else {
        GuildMember::delete(ctx, departed.character_guid);
        return;
    };
    if guild.leader_guid == departed.character_guid {
        let others: Vec<GuildMember> = ctx
            .db
            .game_guild_member()
            .by_guild()
            .filter(guild_id)
            .filter(|row| row.character_guid != departed.character_guid)
            .collect();
        let Some(mut heir) = successor(others) else {
            disband_guild(ctx, guild_id);
            return;
        };
        heir.rank_id = LEADER_RANK;
        let heir = heir.update(ctx);
        guild.leader_guid = heir.character_guid;
        ctx.db.game_guild().guild_id().update(guild);
        push_event(
            ctx,
            guild_id,
            0,
            event_kind::LEADER_CHANGED,
            0,
            0,
            vec![departed.name.clone(), heir.name],
        );
    }
    GuildMember::delete(ctx, departed.character_guid);
    push_event(
        ctx,
        guild_id,
        0,
        event_kind::LEFT,
        departed.character_guid,
        0,
        vec![departed.name],
    );
}

fn successor(members: Vec<GuildMember>) -> Option<GuildMember> {
    members
        .into_iter()
        .min_by_key(|row| (row.rank_id, row.joined_micros, row.character_guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(character_guid: u64, rank_id: u32, joined_micros: i64) -> GuildMember {
        GuildMember {
            character_guid,
            guild_id: 7,
            rank_id,
            name: format!("Member{character_guid}"),
            public_note: String::new(),
            officer_note: String::new(),
            realm_account_id: 0,
            joined_micros,
        }
    }

    fn heir(members: Vec<GuildMember>) -> Option<u64> {
        successor(members).map(|row| row.character_guid)
    }

    #[test]
    fn the_highest_rank_leads_after_the_guild_leader() {
        assert_eq!(
            heir(vec![row(11, 4, 100), row(12, 1, 900), row(13, 2, 50)]),
            Some(12)
        );
    }

    #[test]
    fn the_earliest_join_breaks_a_rank_tie_then_the_lowest_guid() {
        assert_eq!(
            heir(vec![row(21, 1, 500), row(22, 1, 300), row(23, 3, 100)]),
            Some(22)
        );
        assert_eq!(heir(vec![row(32, 2, 400), row(31, 2, 400)]), Some(31));
    }

    #[test]
    fn nobody_left_means_no_successor() {
        assert_eq!(heir(Vec::new()), None);
    }
}
