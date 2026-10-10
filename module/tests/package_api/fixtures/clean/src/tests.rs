#![cfg(test)]

use crate::package::test::{ask_offline, RuntimeScript};

#[test]
fn the_manifest_names_tables() {
    assert!(!crate::CHARACTER_OWNED_TABLES.is_empty());
}
