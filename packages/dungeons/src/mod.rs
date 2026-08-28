//! The `dungeons` Package: encounter choreography for every scripted dungeon, one submodule per
//! dungeon. The registry scan resolves every file through `crate::pkg_dungeons`, so each
//! submodule's markers must stay visible through the glob re-exports below. Adding a dungeon is
//! one new submodule file plus its `mod`/`pub use` pair. See README.md for the Package boundary.

mod blackfathom_deeps;
mod blackrock_depths;
mod dire_maul;
mod eventai_instance_test;
mod razorfen_kraul;
mod shadowfang_keep;
mod sunken_temple;
mod wailing_caverns;
mod zulgurub;

pub(crate) use blackfathom_deeps::*;
pub(crate) use blackrock_depths::*;
pub(crate) use dire_maul::*;
pub(crate) use razorfen_kraul::*;
pub(crate) use shadowfang_keep::*;
pub(crate) use sunken_temple::*;
pub(crate) use wailing_caverns::*;
pub(crate) use zulgurub::*;
