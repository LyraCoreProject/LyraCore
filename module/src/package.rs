#![cfg_attr(not(has_packages), allow(unused_imports))]

//! The Package API root for Package-level operations: Operator authorization, Package-owned
//! Characters, Package Config and encounter choreography. `fixture` exists only with
//! `debug_reducers` and `test` only in a test build.

#[cfg(feature = "debug_reducers")]
pub(crate) mod fixture;
#[cfg(test)]
pub(crate) mod test;

pub(crate) use crate::helpers::require_operator;
pub(crate) use crate::package_account::create_package_character as create_character;
pub(crate) use crate::package_config::ensure_package_config_default as ensure_config_default;

/// The core steps a Package runs to build, level and remove its own Characters.
pub(crate) mod character {
    pub(crate) use crate::creatures::build_player_entity;
    pub(crate) use crate::stats::set_character_level;
    pub(crate) use crate::world::{cascade_delete_character, ghost_restored_fields};
}

/// The encounter kernel a Package drives through `encounter_package!`.
pub(crate) mod encounter {
    pub(crate) use crate::encounter::{
        encounter_reset, encounter_reset_full, equip_swap, get_encounter_data, get_encounter_state,
        move_to_point, open_door, reset_hp_fired, set_encounter_data, set_encounter_state,
        spawn_wave, watch_hp_threshold, wave_guid, EncounterSignal, DOOR_OPEN_STATE,
        ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS, ENCOUNTER_NOT_STARTED,
    };
}
