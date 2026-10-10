//! Runs `build.rs`'s own unit tests: marker parsing, file gates and registry wrappers.
//!
//! Cargo compiles a build script only as a build script, so its `#[cfg(test)]` module has no other
//! way to run. The generation half has no caller in this target.
#![allow(dead_code)]

#[path = "../build.rs"]
mod build_script;
