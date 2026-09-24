//! Guild and officer chat on the Realm Chat path (`cm:Guild.cpp:554-594`,
//! `cm:ChatHandler.cpp:142-147,349-369`). Both Chat Kinds read the speaker's own Guild Rank and
//! every member's, from one indexed read of `game_guild_member` scoped to the speaker's Guild; no
//! call here reads a member of any other Guild.

use spacetimedb::ReducerContext;

use lyracore_shared::chat::{chat_kind, ChatRefusal};
use lyracore_shared::guild::{has_right, rights};

use super::{game_guild_member, member, ranks, GuildRank};
use crate::realm_chat::{ChatAudience, RealmChatRequest};

/// Who may speak and who hears one Guild or Officer line. Guild chat needs GCHATSPEAK to speak
/// and GCHATLISTEN to hear; officer chat needs OFFCHATSPEAK and OFFCHATLISTEN
/// (`cm:Guild.cpp:559-561,580-593`). Both are ignorable (`cm:Guild.cpp:570,591`): the Relay's
/// per-viewer ignore set is what filters a listener who ignores the speaker, not this rule.
pub(crate) fn guild_chat_audience(
    ctx: &ReducerContext,
    speaker_guid: u64,
    request: &RealmChatRequest,
) -> Result<ChatAudience, ChatRefusal> {
    let (speak_right, listen_right) = if request.kind == chat_kind::OFFICER {
        (rights::OFFCHATSPEAK, rights::OFFCHATLISTEN)
    } else {
        (rights::GCHATSPEAK, rights::GCHATLISTEN)
    };
    let speaker = member(ctx, speaker_guid).ok_or(ChatRefusal::NotInGuild)?;
    // One guild-scoped read of every Guild Rank, reused for every member below instead of a
    // repeated lookup per member.
    let guild_ranks = ranks(ctx, speaker.guild_id);
    if !has_right(rights_of(&guild_ranks, speaker.rank_id), speak_right) {
        return Err(ChatRefusal::NoGuildChatRight);
    }
    let recipients = ctx
        .db
        .game_guild_member()
        .by_guild()
        .filter(speaker.guild_id)
        .filter(|candidate| has_right(rights_of(&guild_ranks, candidate.rank_id), listen_right))
        .map(|candidate| candidate.character_guid)
        .collect();
    Ok(ChatAudience {
        recipients,
        ignorable: true,
        channel_name: String::new(),
    })
}

/// One Guild Rank's rights out of an already-read list, 0 for an unknown rank id
/// (`cm:Guild.cpp:660-666`'s rule).
fn rights_of(guild_ranks: &[GuildRank], rank_id: u32) -> u32 {
    guild_ranks
        .iter()
        .find(|rank| rank.rank_id == rank_id)
        .map_or(0, |rank| rank.rights)
}
