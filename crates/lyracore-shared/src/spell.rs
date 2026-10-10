//! Spell channel classification shared by gameplay and its protocol projection.

pub const SPELL_ATTR_CHANNELED: u32 = 0x0080;

/// Periodic triggers retain their legacy channel behavior when the header has no flag.
pub fn is_channel_aura(effect_kind: u8, cast_flags: u32) -> bool {
    effect_kind == 0x93 || cast_flags & SPELL_ATTR_CHANNELED != 0
}
