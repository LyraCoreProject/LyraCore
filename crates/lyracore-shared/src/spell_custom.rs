//! Source spell flags shared by catalogue import and gameplay.
//! Definitions: vmangos SpellDefines.h at 464179081673cfd240f7ffe7f0daf96bca0b5a70.

pub const ALLOW_STACK_BETWEEN_CASTER: u32 = 0x0001;
pub const NEGATIVE: u32 = 0x0002;
pub const POSITIVE: u32 = 0x0004;
pub const CHAN_NO_DIST_LIMIT: u32 = 0x0008;
pub const FIXED_DAMAGE: u32 = 0x0010;
pub const IGNORE_ARMOR: u32 = 0x0020;
pub const BEHIND_TARGET: u32 = 0x0040;
pub const FACE_TARGET: u32 = 0x0080;
pub const SINGLE_TARGET_AURA: u32 = 0x0100;
pub const AURA_APPLY_BREAKS_STEALTH: u32 = 0x0200;
pub const NOT_REMOVED_ON_EVADE: u32 = 0x0400;
pub const SEND_CHANNEL_VISUAL: u32 = 0x0800;
pub const SEPARATE_AURA_PER_CASTER: u32 = 0x1000;
pub const TRIGGER_WEAPON_PROCS: u32 = 0x2000;
pub const KNOWN: u32 = 0x3fff;
pub const UNSUPPORTED: u32 = SEND_CHANNEL_VISUAL;

#[must_use]
pub fn is_negative(flags: u32, derived: bool) -> bool {
    flags & POSITIVE == 0 && (flags & NEGATIVE != 0 || derived)
}

/// Low 40 bits identify family and icon; the next byte identifies Judgement or Polymorph.
#[must_use]
pub fn single_target_key(family: u8, icon: u32, category: u8) -> u64 {
    u64::from(icon) | (u64::from(family) << 32) | (u64::from(category) << 40)
}

#[must_use]
pub fn single_target_conflicts(a: u64, b: u64) -> bool {
    let category = a >> 40;
    a & 0xff_ffff_ffff == b & 0xff_ffff_ffff || (category != 0 && category == b >> 40)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_overrides_negative_and_derived_polarity() {
        assert!(is_negative(NEGATIVE, false));
        assert!(!is_negative(POSITIVE, true));
        assert!(!is_negative(POSITIVE | NEGATIVE, true));
        assert!(is_negative(0, true));
    }

    #[test]
    fn single_target_families_match_icons_or_special_categories() {
        let key = single_target_key(3, 1, 2);
        assert!(single_target_conflicts(key, single_target_key(3, 1, 0)));
        assert!(single_target_conflicts(key, single_target_key(3, 2, 2)));
        assert!(!single_target_conflicts(key, single_target_key(4, 1, 0)));
        assert!(!single_target_conflicts(key, single_target_key(3, 2, 0)));
    }
}
