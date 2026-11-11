use spacetimedb::ReducerContext;

use crate::{game_account_claim, game_world_entity};

// Callers name which meaning of "online" they need; the meanings differ on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Online {
    // Group invites and leader changes: a World Session here or a live world entity, so a
    // sessionless Character counts.
    SessionOrBody,
    // Meeting Stone Seekers: the Realm party facts on a Realm Shard, otherwise a World Session
    // here or a live Account Claim.
    SessionOrClaim,
    // Session-less Gates: the Character row's `online` flag.
    CharacterFlag,
}

pub(crate) fn is_online(ctx: &ReducerContext, character_guid: u64, meaning: Online) -> bool {
    match meaning {
        Online::SessionOrBody => {
            crate::native::has_world_session(ctx, character_guid)
                || ctx
                    .db
                    .game_world_entity()
                    .guid()
                    .find(character_guid)
                    .is_some()
        }
        Online::SessionOrClaim => {
            if crate::peer::world_shard(ctx).is_some() {
                return crate::native::realm_party::online(ctx, character_guid);
            }
            if crate::native::has_world_session(ctx, character_guid) {
                return true;
            }
            let now = crate::time::now(ctx).to_micros_since_unix_epoch();
            ctx.db
                .game_account_claim()
                .by_character()
                .filter(character_guid)
                .any(|claim| !claim.closed && claim.expires_micros > now)
        }
        Online::CharacterFlag => crate::helpers::character_by_guid(ctx, character_guid)
            .is_some_and(|character| character.online),
    }
}

pub(crate) fn world_session_ended(ctx: &ReducerContext, character_guid: u64) {
    crate::meeting_stone::remove_solo_seeker(ctx, character_guid);
    crate::channel::leave_all(ctx, character_guid);
}
