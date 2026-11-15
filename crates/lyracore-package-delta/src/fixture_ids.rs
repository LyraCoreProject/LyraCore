use crate::ids::PackageRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdSpace {
    Creature,
    Item,
    Spell,
    GameObject,
    Quest,
}

impl IdSpace {
    #[must_use]
    pub const fn range(self) -> PackageRange {
        match self {
            IdSpace::Creature => PackageRange::Creature,
            IdSpace::Item => PackageRange::Item,
            IdSpace::Spell => PackageRange::Spell,
            IdSpace::GameObject => PackageRange::Gameobject,
            IdSpace::Quest => PackageRange::Quest,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FixtureId {
    pub name: &'static str,
    pub space: IdSpace,
    pub id: u32,
    /// False for the demo content `init` wrote before the Fixture Reserved IDs existed.
    pub reserved: bool,
}

macro_rules! fixture_ids {
    (
        reserved { $($space:ident $name:ident = $id:expr;)* }
        unreserved { $($u_space:ident $u_name:ident = $u_id:expr;)* }
    ) => {
        $(pub const $name: u32 = $id;)*
        $(pub const $u_name: u32 = $u_id;)*

        pub const ALL: &[FixtureId] = &[
            $(FixtureId { name: stringify!($name), space: IdSpace::$space, id: $name, reserved: true },)*
            $(FixtureId { name: stringify!($u_name), space: IdSpace::$u_space, id: $u_name, reserved: false },)*
        ];
    };
}

fixture_ids! {
    reserved {
        Creature TEST_WOLF_ENTRY = 51_000;
        Creature PROFESSION_TRAINER_ENTRY = 51_001;
        Creature TEST_WOLF_ELDER_ENTRY = 51_002;
        Creature SCENARIO_QUESTGIVER_ENTRY = 51_003;
        Creature SCENARIO_VENDOR_ENTRY = 51_004;
        Creature WEAPON_MASTER_ENTRY = 51_005;
        Creature FLIGHT_MASTER_ENTRY = lyracore_shared::constants::taxi_fixture::FLIGHT_MASTER_ENTRY;
        Creature RIDING_TRAINER_ENTRY = 51_007;
        Creature TEST_TAME_BOAR_ENTRY = 51_008;

        Item FIXTURE_BLADE = 5_090_050;
        Item FIXTURE_JERKY = 5_090_052;
        Item FIXTURE_REINS = 5_090_054;

        Spell COMBAT_INSIGHT_SPELL = 50_000;
        Spell QUICKENING_SPELL = 50_010;
        Spell TEST_SNARE_SPELL = 50_011;
        Spell TEST_STUN_SPELL = 50_020;
        Spell TEST_ROOT_SPELL = 50_021;
        Spell TEST_FEAR_SPELL = 50_022;
        Spell TEST_POLY_SPELL = 50_023;
        Spell TEST_FIRE_WARD_SPELL = 50_030;
        Spell TEST_STUN_IMMUNITY_SPELL = 50_040;
        Spell TEST_CONJURE_SPELL = 50_050;
        Spell TEST_POWER_WORD_SHIELD_SPELL = 50_072;
        Spell ROUGH_COPPER_BOMB_SPELL = 50_096;
        Spell MINOR_HEALING_SPELL = 50_110;
        Spell LINEN_BANDAGE_SPELL = 50_111;
        Spell MINOR_MANA_POTION_SPELL = 50_113;
        Spell REFRESHING_WATER_SPELL = 50_114;
        Spell EATING_SPELL = 50_115;
        Spell WELL_FED_SPELL = 50_116;
        Spell LESSER_HEALING_SPELL = 50_117;
        Spell HEAVY_LINEN_BANDAGE_SPELL = 50_118;
        Spell LEARN_COOKING_SPELL_ID = 50_080;
        Spell LEARN_SKINNING_SPELL_ID = 50_081;
        Spell LEARN_LEATHERWORKING_SPELL_ID = 50_082;
        Spell LEARN_MINING_SPELL_ID = 50_083;
        Spell LEARN_HERBALISM_SPELL_ID = 50_084;
        Spell LEARN_ALCHEMY_SPELL_ID = 50_085;
        Spell LEARN_FIRST_AID_SPELL_ID = 50_086;
        Spell LEARN_TAILORING_SPELL_ID = 50_087;
        Spell LEARN_BLACKSMITHING_SPELL_ID = 50_088;
        Spell LEARN_AXE_1H_SPELL_ID = 50_130;
        Spell LEARN_POLEARM_SPELL_ID = 50_131;
        Spell LEARN_APPRENTICE_RIDING_SPELL_ID = 50_132;
        Spell LEARN_JOURNEYMAN_RIDING_SPELL_ID = 50_133;
        Spell TEST_REGENERATION_SPELL = 50_137;
        Spell TEST_PROC_MARK = 50_140;
        Spell TEST_PROC_COIN = 50_141;
        Spell TEST_PROC_CHARGES = 50_142;
        Spell TEST_PROC_COOLDOWN = 50_143;
        Spell TEST_PROC_PPM = 50_144;
        Spell TEST_PROC_ZAP = 50_145;
        Spell ENCHANT_MINOR_STRENGTH_SPELL = 50_201;
        Spell ENCHANT_MINOR_STAMINA_SPELL = 50_202;
        Spell TEST_TAME_BEAST_SPELL = 50_300;
        Spell FIXTURE_MOUNT_SPELL = 50_310;
        Spell FIXTURE_DAZED_SPELL = 50_311;
    }
    unreserved {
        Item DEMO_TEMPERED_BLADE = 50;
        Item RECRUITS_TUNIC = 51;
        Item DEMO_TOUGH_JERKY = 52;
        Item BATTERED_BUCKLER = 50_053;

        GameObject BATTERED_CHEST = 50_100;
        GameObject SUSPICIOUS_LEVER = 50_101;
        GameObject DEMO_COPPER_VEIN = 50_102;
        GameObject DEMO_PEACEBLOOM = 50_103;

        Quest SCENARIO_QUEST = 50_900;
        Quest REWARD_LETTER_CARD_QUEST = 509_091;
        Quest REWARD_LETTER_ELDER_QUEST = 509_092;
        Quest REWARD_LETTER_PLAIN_QUEST = 509_093;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_id_is_unique_in_its_space_and_inside_its_reserved_range() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert!(
                    a.space != b.space || a.id != b.id,
                    "{} and {} are both {:?} {}",
                    a.name,
                    b.name,
                    a.space,
                    a.id
                );
            }
            assert_eq!(
                a.space.range().is_fixture_reserved(u64::from(a.id)),
                a.reserved,
                "{} ({:?} {}) is listed as reserved = {}",
                a.name,
                a.space,
                a.id,
                a.reserved
            );
        }
    }
}
