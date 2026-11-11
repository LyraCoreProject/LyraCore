#[macro_export]
macro_rules! character_owned {
    (delete, fn $name:ident($ctx:ident, $guid:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext, $guid: u64) $body
    };
    (restamp, fn $name:ident($ctx:ident, $guid:ident, $identity:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $identity: spacetimedb::Identity,
        ) $body
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        by = $index:ident,
        remint = $pk:ident $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$index().filter(&$guid).collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, mut row| {
                    // Surrogate keys are Shard-local; mint new keys at the destination.
                    row.$pk = 0;
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        by = $index:ident,
        keep_key $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$index().filter(&$guid).collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, row| {
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        primary_key = $key:ident $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$key().find($guid).into_iter().collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, row| {
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) $body
    };
    (not_transported, fn $name:ident()) => {
        pub(crate) fn $name(
            _ctx: &spacetimedb::ReducerContext,
            _character_guid: u64,
            io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::not_transported(io);
        }
    };
}

#[macro_export]
macro_rules! game_tick_pass {
    (fn $name:ident($ctx:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext) $body
    };
}

#[macro_export]
macro_rules! game_hook {
    ($event:ident, fn $name:ident($ctx:ident, $payload:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $payload: &$crate::hooks::payload_for::$event,
        ) $body
    };
}

#[macro_export]
macro_rules! game_package_characters {
    (fn $name:ident($ctx:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext) -> Vec<u64> $body
    };
}

#[macro_export]
macro_rules! game_client_command {
    ($parse:ident, $apply:ident, $reply:ident) => {};
}

#[macro_export]
macro_rules! encounter_package {
    ($binding:ident, fn $name:ident($ctx:ident, $instance_id:ident, $signal:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $instance_id: u64,
            $signal: $crate::encounter::EncounterSignal,
        ) -> Result<(), String> $body
    };
}

include!(concat!(env!("OUT_DIR"), "/character_sweeps.rs"));

include!(concat!(env!("OUT_DIR"), "/package_mods.rs"));

include!(concat!(env!("OUT_DIR"), "/package_registries.rs"));

mod account_ownership;
mod action_bar;
mod actor;
mod areatrigger;
mod auction;
mod auth;
mod away;
mod breath;
mod breath_relay;
mod bridge;
mod fatigue;

mod channel;
mod character;
mod chat;
mod combat;
mod config;
mod corpse;
mod creatures;
mod exploration;

#[cfg(feature = "debug_reducers")]
mod debug;

mod duel;
mod encounter;
mod faction;
mod gameobject;
mod gc;
mod geometry;
mod gm;

mod go_collider;
#[cfg(feature = "debug_reducers")]
mod go_collider_fixture;
mod go_model;
mod gossip;
mod graveyard;
mod group;
pub mod guild;
mod gw;
mod helpers;
pub mod hooks;
mod import_meta;

mod combo;
mod ghost;
mod inspect;
mod instance;
mod items;
mod loot;

mod mail;

mod mail_escrow;

mod mail_reward;

mod mail_text;

mod mail_timer;

mod meeting_stone;

mod motion;

mod modifiers;
mod mount;
mod nav;
mod npc_interaction;
mod online;

mod operations;
mod operator_content;

mod package;

mod package_account;

mod package_config;
#[cfg(feature = "debug_reducers")]
pub(crate) mod package_fixture;

mod package_import;

mod package_teardown;
mod peer;

#[cfg(test)]
pub(crate) mod package_test;
mod professions;

#[cfg(test)]
mod publish_safety;
mod quest;

mod realm_chat;
mod realm_core;
mod refusal;
mod remote_bindings;
pub(crate) use refusal::Refusal;
mod reputation;
mod rest;
mod retired;

mod runtime_script;

mod conformance_rolls;
mod script_binding;
mod seed;
mod sessionless;
mod skill;
mod skilldata;
mod spell;
mod stats;
mod tables;
mod talent;
mod taxi;
mod terrain;
#[cfg(test)]
mod test_scan;
mod threat;
mod time;
mod trade;
mod trainer;
mod transfer;
mod transport;

mod vmap;

#[cfg(test)]
mod tripwires;

mod aoi;
mod native;
mod weather;
mod world;
mod xp;

pub use tables::*;

pub(crate) const EVENT_TTL_MICROS: i64 = 1_000_000;

pub(crate) const INVITE_TTL_MICROS: i64 = 120 * 1_000_000;
