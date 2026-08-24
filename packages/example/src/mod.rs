//! Reference Package: the maintained template `lyracore packages new <name>` copies and renames.
//! Read it, don't just run it — it is documentation as much as it is code.
//!
//! A Package is a folder under `packages/<name>/` that `module/build.rs` discovers with zero
//! core-file edits. `src/mod.rs` is its Rust root: the build generates `pub mod pkg_<name>` for it
//! and compiles the module in. Nested submodule files also work; see `module/build.rs`'s scan doc
//! for the facade re-export they must follow (this reference Package is one file, so it doesn't
//! need one).
//!
//! The commented hook below shows the shape without registering dormant gameplay behavior. Pick an
//! event from the catalog at the top of `module/src/hooks.rs`, uncomment the pattern, and implement
//! the behavior the Package needs.
//!
//! A Package may also ship a `client/` directory (addons and client overrides, installed by
//! `lyracore client sync`). This reference Package deliberately has none — `lyracore packages new`
//! prints how to add one.
//!
//! ```ignore
//! crate::game_hook!(on_group_invite, fn example_on_group_invite(ctx, payload) {
//!     // Read `payload.target_guid` and `payload.inviter_guid`, then call the same core operation
//!     // that handles the equivalent built-in behavior.
//! });
//! ```
