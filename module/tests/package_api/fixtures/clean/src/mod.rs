//! A Package that names only Package API version 2. `crate::spell::pending_cast` here is inert.

mod fixture;
mod runner;
#[cfg(test)]
mod tests;

use crate::actor::{self, movement::route_step, ActionRefusalKind};
use crate::package::character::build_player_entity;
use crate::package::encounter::{self, ENCOUNTER_DONE};
use crate::tables::{game_package_config, PartyPartitionState};
use crate::{game_character, Character, WorldEntity};

crate::game_hook!(on_login, fn welcome(ctx, payload) {
    crate::actor::system_message(ctx, payload.character_guid, "crate::helpers::live_entity");
    let _ = crate::script_binding::ask(ctx, "welcome", payload.character_guid, 0);
    let _ = crate::hooks::LevelupPayload::default;
    let _ = crate::pkg_clean::runner::step;
});

fn setup(ctx: &spacetimedb::ReducerContext) -> Result<(), String> {
    crate::package::require_operator(ctx)?;
    let _ = super::game_world_entity;
    Ok(())
}
