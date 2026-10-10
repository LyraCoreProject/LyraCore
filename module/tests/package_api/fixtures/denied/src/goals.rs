#![cfg(test)]

use super::super::spell::{pending_cast, CastStart};
use crate as core;

fn hit() {
    crate::package::fixture::apply_damage();
}
