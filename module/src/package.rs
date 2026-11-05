#![allow(
    unused_imports,
    reason = "Packages select the named API operations they need"
)]

#[cfg(feature = "debug_reducers")]
pub(crate) use crate::package_fixture as fixture;
#[cfg(test)]
pub(crate) use crate::package_test as test;

pub(crate) use crate::helpers::require_operator;
pub(crate) use crate::native::package_creation::{
    request_character, take_character_result, CharacterCreation,
};
pub(crate) use crate::native::package_relocation::{
    request_relocation, take_relocation_result, Relocation,
};
pub(crate) use crate::package_account::create_package_character as create_character;
pub(crate) use crate::package_config::ensure_package_config_default as ensure_config_default;

pub(crate) mod character {
    pub(crate) use crate::creatures::build_player_entity;
    pub(crate) use crate::stats::set_character_level;
    pub(crate) use crate::world::cascade_delete_character;
}

pub(crate) mod encounter {
    pub(crate) use crate::encounter::{
        encounter_reset, encounter_reset_full, equip_swap, get_encounter_data, get_encounter_state,
        move_to_point, open_door, reset_hp_fired, set_encounter_data, set_encounter_state,
        spawn_wave, watch_hp_threshold, wave_guid, EncounterSignal, DOOR_OPEN_STATE,
        ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS, ENCOUNTER_NOT_STARTED,
    };
}
