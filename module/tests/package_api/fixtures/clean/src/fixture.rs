#![cfg(feature = "debug_reducers")]

use crate::package::fixture::{apply_damage, SessionActor};

fn grant() {
    super::super::package::fixture::grant_xp();
}
