// Before changing how the catalog is generated, read docs/module-build.md.
include!(concat!(env!("OUT_DIR"), "/table_catalog.rs"));

pub use crate::group::PartyPartitionState;

/// Keeps the named tables, declared in the same file, out of the catalog the Package API promises.
#[macro_export]
macro_rules! outside_table_catalog {
    ($($table:ident),+ $(,)?) => {};
}
