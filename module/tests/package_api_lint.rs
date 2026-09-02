//! Runs `build.rs`'s own unit tests — the Package API lint's path scanner — under `cargo test`.
//!
//! Cargo compiles a build script only as a build script, so its `#[cfg(test)]` module has no other
//! way to run. Declaring the file as a module here needs no seam and cannot drift from what the
//! build itself executes; the codegen half of the file simply has no caller in this target.
#![allow(dead_code)]

#[path = "../build.rs"]
mod build_script;
