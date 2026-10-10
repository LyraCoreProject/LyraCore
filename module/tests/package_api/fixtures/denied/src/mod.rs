mod goals;

use crate::helpers::require_operator;
use crate::package::fixture::apply_damage;
use crate::package as api;

fn run(ctx: &spacetimedb::ReducerContext, _: crate::UnusedUpperCamel) {
    crate::game_operation(ctx);
    crate::auth::create_character(ctx); // package-api: exempt the Package owns its Characters
}
