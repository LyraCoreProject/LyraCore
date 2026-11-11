pub mod addon;
pub mod auth_session;
pub mod channel;
pub mod social;
pub mod update_mask;

pub mod auction;
pub mod breath;
pub mod char;
pub mod combat;
pub mod corpse;
pub mod duel;
pub mod entity;
pub mod gameobject;
pub mod guild;
pub mod item;
pub mod leveling;
pub mod loot;
pub mod mail;
pub mod meeting_stone;
pub mod member_stats;
pub mod movement;
pub mod movement_info;
pub mod npc;
pub mod pet;
pub mod quest;
pub mod taxi;
pub mod trade;
pub mod trainer;
pub mod values;
pub mod weather;

pub use social::{
    build_chat_message_to, build_emote_anim, build_friend_list_response, build_friend_status_raw,
    build_gm_system_message, build_group_decline, build_group_invite, build_group_list,
    build_group_list_raw, build_group_set_leader, build_ignore_list_response, build_minimap_ping,
    build_party_command_result, build_raid_group_only, build_random_roll, build_ready_check,
    build_ready_check_answer, build_realm_chat_line, build_target_icon_list_raw,
    build_target_icon_update, build_text_emote, build_who_response_raw, FriendOnline, FriendView,
    WhoPlayerView,
};

pub use auction::*;
pub use breath::*;
pub use char::*;
pub use combat::*;
pub use corpse::*;
pub use duel::*;
pub use entity::*;
pub use gameobject::*;
pub use guild::*;
pub use item::*;
pub use leveling::*;
pub use loot::*;
pub use mail::*;
pub use meeting_stone::*;
pub use member_stats::*;
pub use movement::*;
pub use movement_info::*;
pub use npc::*;
pub use pet::*;
pub use quest::*;
pub use taxi::*;
pub use trade::*;
pub use trainer::*;
pub use values::*;
pub use weather::*;

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
pub mod property_tests;
