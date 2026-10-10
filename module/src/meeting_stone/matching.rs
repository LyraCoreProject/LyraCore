//! Pure Meeting Stone matching: class roles, a Party's Open Roles, the next Stone Add and Party
//! formation. The role table and fill order are cmangos's (cm:LFG/LFGMgr.cpp:91-158).

use spacetimedb::Timestamp;

use lyracore_shared::group::GROUP_MAX_MEMBERS;

use super::MeetingStoneSeeker;

/// 1.12 class ids (ChrClasses.dbc).
mod class {
    pub(crate) const WARRIOR: u8 = 1;
    pub(crate) const PALADIN: u8 = 2;
    pub(crate) const HUNTER: u8 = 3;
    pub(crate) const ROGUE: u8 = 4;
    pub(crate) const PRIEST: u8 = 5;
    pub(crate) const SHAMAN: u8 = 7;
    pub(crate) const MAGE: u8 = 8;
    pub(crate) const WARLOCK: u8 = 9;
    pub(crate) const DRUID: u8 = 11;
}

/// A Party's dungeon roles, in the order a Party fills them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Tank,
    Healer,
    Damage,
}

const ROLE_ORDER: [Role; 3] = [Role::Tank, Role::Healer, Role::Damage];

const DAMAGE_ROLES: u8 = 3;

/// How well `class` fills `role`: 0 not at all, then 1 low, 2 normal, 3 high. An unknown class
/// fills nothing.
fn role_priority(class: u8, role: Role) -> u8 {
    use class::*;
    const LOW: u8 = 1;
    const NORMAL: u8 = 2;
    const HIGH: u8 = 3;
    match (role, class) {
        (Role::Tank, WARRIOR) => HIGH,
        (Role::Tank, DRUID | PALADIN) => NORMAL,
        (Role::Healer, DRUID | PALADIN | PRIEST | SHAMAN) => HIGH,
        (Role::Damage, HUNTER | MAGE | ROGUE | WARLOCK) => HIGH,
        (Role::Damage, DRUID | PALADIN | SHAMAN | WARRIOR) => NORMAL,
        (Role::Damage, PRIEST) => LOW,
        _ => 0,
    }
}

fn fills(class: u8, role: Role) -> bool {
    role_priority(class, role) > 0
}

/// Which of a Party's one tank, one healer and three damage roles no member fills yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OpenRoles {
    tank: bool,
    healer: bool,
    damage: u8,
}

impl OpenRoles {
    const ALL: Self = Self {
        tank: true,
        healer: true,
        damage: DAMAGE_ROLES,
    };

    fn is_open(self, role: Role) -> bool {
        match role {
            Role::Tank => self.tank,
            Role::Healer => self.healer,
            Role::Damage => self.damage > 0,
        }
    }

    fn fill(&mut self, role: Role) {
        match role {
            Role::Tank => self.tank = false,
            Role::Healer => self.healer = false,
            Role::Damage => self.damage = self.damage.saturating_sub(1),
        }
    }
}

/// The Open Roles of a Party whose members have `classes`, in join order. Each member takes the
/// first open role its class fills, unless a member not yet placed has a higher priority for it
/// (cm:Groups/Group.cpp:1589-1685). A member left without a role holds a seat.
pub(super) fn open_roles(classes: &[u8]) -> OpenRoles {
    let mut open = OpenRoles::ALL;
    let mut placed = vec![false; classes.len()];
    for (seat, &class) in classes.iter().enumerate() {
        let outranked = |role: Role| {
            classes.iter().enumerate().any(|(other, &other_class)| {
                other != seat
                    && !placed[other]
                    && role_priority(other_class, role) > role_priority(class, role)
            })
        };
        let taken = ROLE_ORDER
            .into_iter()
            .find(|&role| fills(class, role) && open.is_open(role) && !outranked(role));
        if let Some(role) = taken {
            open.fill(role);
            placed[seat] = true;
        }
    }
    open
}

/// The longest wait first, ties by guid.
fn wait_order(seeker: &MeetingStoneSeeker) -> (Timestamp, u64) {
    (seeker.queued_at, seeker.character_guid)
}

/// The index in `solos` of the next Stone Add: for the first open role in fill order, the
/// longest-waiting Seeker whose class fills it. cmangos compares class priority as a `bool`
/// (cm:LFG/LFGQueue.cpp:300,316), so the wait decides.
pub(super) fn next_stone_add(open: OpenRoles, solos: &[MeetingStoneSeeker]) -> Option<usize> {
    ROLE_ORDER
        .into_iter()
        .filter(|&role| open.is_open(role))
        .find_map(|role| {
            solos
                .iter()
                .enumerate()
                .filter(|(_, seeker)| fills(seeker.class, role))
                .min_by_key(|(_, seeker)| wait_order(seeker))
                .map(|(index, _)| index)
        })
}

/// With five solo Seekers in one bucket, the longest wait leads a new Party and the next joins it.
/// Both cores count Seekers across every area and block on the first one's; one bucket does not.
pub(super) fn formation(
    solos: &[MeetingStoneSeeker],
) -> Option<(&MeetingStoneSeeker, &MeetingStoneSeeker)> {
    if solos.len() < GROUP_MAX_MEMBERS {
        return None;
    }
    let mut waiting: Vec<&MeetingStoneSeeker> = solos.iter().collect();
    waiting.sort_unstable_by_key(|seeker| wait_order(seeker));
    Some((waiting[0], waiting[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cmangos class table, row by row: the roles each class fills and its priority in each.
    #[test]
    fn the_role_table_is_the_cmangos_class_table() {
        use class::*;
        let row = |class: u8| ROLE_ORDER.map(|role| role_priority(class, role));
        assert_eq!(row(WARRIOR), [3, 0, 2]);
        assert_eq!(row(PALADIN), [2, 3, 2]);
        assert_eq!(row(HUNTER), [0, 0, 3]);
        assert_eq!(row(ROGUE), [0, 0, 3]);
        assert_eq!(row(PRIEST), [0, 3, 1]);
        assert_eq!(row(SHAMAN), [0, 3, 2]);
        assert_eq!(row(MAGE), [0, 0, 3]);
        assert_eq!(row(WARLOCK), [0, 0, 3]);
        assert_eq!(row(DRUID), [2, 3, 2]);
        assert_eq!(row(0), [0, 0, 0], "an unknown class fills nothing");
        assert_eq!(row(6), [0, 0, 0], "class 6 does not exist in 1.12");
    }

    fn open(tank: bool, healer: bool, damage: u8) -> OpenRoles {
        OpenRoles {
            tank,
            healer,
            damage,
        }
    }

    /// The paladin outranks nobody for tank while the warrior waits, so it heals.
    #[test]
    fn a_paladin_then_a_warrior_leave_three_damage_roles_open() {
        use class::*;
        assert_eq!(open_roles(&[PALADIN, WARRIOR]), open(false, false, 3));
    }

    /// Equal priority does not outrank, so the first warrior tanks and the second deals damage.
    #[test]
    fn two_warriors_leave_the_healer_and_two_damage_roles_open() {
        use class::*;
        assert_eq!(open_roles(&[WARRIOR, WARRIOR]), open(false, true, 2));
    }

    #[test]
    fn a_full_party_has_no_open_role() {
        use class::*;
        assert_eq!(
            open_roles(&[WARRIOR, PRIEST, MAGE, ROGUE, HUNTER]),
            open(false, false, 0)
        );
    }

    #[test]
    fn a_member_of_unknown_class_holds_a_seat_and_no_role() {
        use class::*;
        assert_eq!(open_roles(&[]), OpenRoles::ALL);
        assert_eq!(open_roles(&[0, MAGE]), open(true, true, 2));
    }

    /// A fourth damage dealer finds every damage role taken and fills nothing.
    #[test]
    fn a_member_whose_roles_are_taken_fills_nothing() {
        use class::*;
        assert_eq!(open_roles(&[MAGE, MAGE, MAGE, MAGE]), open(true, true, 0));
    }

    fn seeker(character_guid: u64, class: u8, waited_secs: i64) -> MeetingStoneSeeker {
        MeetingStoneSeeker {
            character_guid,
            area_id: 1581,
            team: 469,
            class,
            group_id: 0,
            queued_at: Timestamp::from_micros_since_unix_epoch(
                1_000_000_000_000 - waited_secs * 1_000_000,
            ),
        }
    }

    fn picked(open: OpenRoles, solos: &[MeetingStoneSeeker]) -> Option<u64> {
        next_stone_add(open, solos).map(|index| solos[index].character_guid)
    }

    #[test]
    fn the_next_stone_add_takes_tank_then_healer_then_damage() {
        use class::*;
        let solos = [
            seeker(1, MAGE, 90),
            seeker(2, PRIEST, 10),
            seeker(3, WARRIOR, 5),
        ];
        assert_eq!(picked(OpenRoles::ALL, &solos), Some(3));
        assert_eq!(picked(open(false, true, 3), &solos), Some(2));
        assert_eq!(picked(open(false, false, 3), &solos), Some(1));
        assert_eq!(picked(open(false, false, 0), &solos), None);
    }

    #[test]
    fn the_longest_wait_takes_a_role_and_the_lower_guid_breaks_a_tie() {
        use class::*;
        let solos = [
            seeker(7, PRIEST, 30),
            seeker(5, DRUID, 60),
            seeker(4, SHAMAN, 60),
        ];
        assert_eq!(picked(open(false, true, 0), &solos), Some(4));
        let solos = [
            seeker(7, PRIEST, 61),
            seeker(5, DRUID, 60),
            seeker(4, SHAMAN, 60),
        ];
        assert_eq!(picked(open(false, true, 0), &solos), Some(7));
    }

    #[test]
    fn a_seeker_of_unknown_class_is_never_a_stone_add() {
        assert_eq!(picked(OpenRoles::ALL, &[seeker(1, 0, 600)]), None);
    }

    #[test]
    fn five_solo_seekers_form_a_party_led_by_the_longest_wait() {
        use class::*;
        let waiting = || {
            vec![
                seeker(1, MAGE, 10),
                seeker(2, MAGE, 50),
                seeker(3, MAGE, 40),
                seeker(4, MAGE, 40),
            ]
        };
        assert!(formation(&waiting()).is_none());
        let mut five = waiting();
        five.push(seeker(5, MAGE, 5));
        let (leader, member) = formation(&five).expect("five Seekers form a party");
        assert_eq!((leader.character_guid, member.character_guid), (2, 3));
    }
}
