pub mod codec;
pub mod packet_lint;
pub mod srp;
pub mod who;
pub mod world;
pub use wow_login_messages as login_messages;
pub use wow_world_messages as messages;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AwayStatus {
    None,
    Afk,
    Dnd,
}
