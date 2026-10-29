use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table};

use crate::breath_relay::game_breath_relay_event;
use crate::channel::game_chat_channel_notice_event;
use crate::guild::game_guild_event;
use crate::guild::membership::game_guild_invite;
use crate::mail_timer::game_mail_arrival;
use crate::realm_chat::game_realm_chat_event;
use crate::rest::game_rest_state_event;
use crate::{
    game_addon_message, game_auction_notice, game_bot_invite_intent, game_chat_event,
    game_combat_event, game_duel_event, game_emote_event, game_group_event, game_group_invite,
    game_levelup_event, game_movement_violation, game_roll_event, game_spell_cast_event,
    game_spell_impact_event, game_system_message_event, game_teleport_event, game_trade_event,
    game_trade_session, game_whisper_event, game_xp_event, EVENT_TTL_MICROS, INVITE_TTL_MICROS,
};
use crate::{game_party_command_intent, game_party_command_receipt};

#[table(accessor = game_event_reaper_schedule, scheduled(reap_movement_events))]
pub struct EventReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

#[reducer]
pub fn reap_movement_events(ctx: &ReducerContext, _schedule: EventReaperSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    let cutoff = crate::time::now(ctx).to_micros_since_unix_epoch() - EVENT_TTL_MICROS;

    macro_rules! reap {
        ($accessor:ident) => {{
            let t = ctx.db.$accessor();
            let stale: Vec<u64> = t
                .iter()
                .filter(|e| e.created_at.to_micros_since_unix_epoch() < cutoff)
                .map(|e| e.id)
                .collect();
            for id in stale {
                t.id().delete(&id);
            }
        }};
    }
    // Some of these tables are retired and only drain old rows; see docs/danger-zones.md.
    reap!(game_combat_event);
    reap!(game_xp_event);
    reap!(game_levelup_event);
    reap!(game_spell_cast_event);
    reap!(game_spell_impact_event);
    reap!(game_chat_event);
    reap!(game_emote_event);
    reap!(game_whisper_event);
    reap!(game_realm_chat_event);
    reap!(game_chat_channel_notice_event);
    reap!(game_system_message_event);
    reap!(game_addon_message);
    reap!(game_roll_event);
    reap!(game_group_event);
    reap!(game_trade_event);
    reap!(game_duel_event);
    reap!(game_bot_invite_intent);
    reap!(game_auction_notice);
    reap!(game_movement_violation);
    reap!(game_rest_state_event);
    reap!(game_breath_relay_event);
    reap!(game_guild_event);
    reap!(game_mail_arrival);

    {
        let t = ctx.db.game_group_invite();
        let invite_cutoff = crate::time::now(ctx).to_micros_since_unix_epoch() - INVITE_TTL_MICROS;
        let stale: Vec<u64> = t
            .iter()
            .filter(|e| e.created_at.to_micros_since_unix_epoch() < invite_cutoff)
            .map(|e| e.id)
            .collect();
        for id in stale {
            t.id().delete(id);
        }
    }

    {
        let t = ctx.db.game_guild_invite();
        let invite_cutoff = crate::time::now(ctx).to_micros_since_unix_epoch() - INVITE_TTL_MICROS;
        let stale: Vec<u64> = t
            .iter()
            .filter(|e| e.created_at.to_micros_since_unix_epoch() < invite_cutoff)
            .map(|e| e.target_guid)
            .collect();
        for target_guid in stale {
            t.target_guid().delete(target_guid);
        }
    }

    {
        let sessions = ctx.db.game_trade_session();
        let now = crate::time::now(ctx).to_micros_since_unix_epoch();
        let stale: Vec<u64> = sessions
            .iter()
            .filter(|s| {
                crate::trade::session_is_stale(s.created_at.to_micros_since_unix_epoch(), now)
            })
            .map(|s| s.initiator_guid)
            .collect();
        for guid in stale {
            crate::trade::cancel_trade_for(ctx, guid);
        }
    }

    crate::spell::stacking::sweep_dr_state(ctx);

    crate::loot::sweep_loot_rolls(ctx);

    {
        let t = ctx.db.game_teleport_event();
        let stale: Vec<u64> = t
            .iter()
            .filter(|e| (e.created_micros as i64) < cutoff)
            .map(|e| e.id)
            .collect();
        for id in stale {
            t.id().delete(id);
        }
    }

    // Pending commands retain i64::MAX; only terminal results enter this bounded expiry prefix.
    const COMMAND_REAP_LIMIT: usize = 64;
    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
    let intents = ctx.db.game_party_command_intent();
    for intent in intents
        .by_result_reap()
        .filter(..=now)
        .take(COMMAND_REAP_LIMIT)
        .collect::<Vec<_>>()
    {
        if matches!(intent.state, crate::bridge::CommandIntentState::Finished(_)) {
            intents.id().delete(intent.id);
        }
    }
    let receipts = ctx.db.game_party_command_receipt();
    for receipt in receipts
        .by_retention()
        .filter(..=now)
        .take(COMMAND_REAP_LIMIT)
        .collect::<Vec<_>>()
    {
        receipts.id().delete(receipt.id);
    }

    crate::corpse::sweep_corpse_decay(ctx);
}
