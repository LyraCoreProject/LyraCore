# Wave defense

This Spec records the agreed first release. Implementation and verification status belong in the
Package README. The mode is a dedicated realm for five-person cooperative defense, using existing
WoW 1.12 combat and an ordinary Benilla build with a required addon.

## Match rules

- Characters wait at level 1 in a shared lobby. Normal character selection changes class. The addon
  selects specialization and enters the queue. All nine launch specializations are available.
- A Party has one tank, one healer and three damage roles. Solo Characters, partial premades and full
  premades share one queue. Humans take priority; labeled bots fill missing roles after a timeout.
- Each Party enters a separate Instance. A successful run aims for 30 minutes across 15 waves, with
  bosses at waves 5, 10 and 15. The current wave must clear before the next starts.
- The team shares XP and levels from 1 to 60, reaching 60 before the final waves. Dead members retain
  their share. Each Character gets a draft choice at level 1 and every fifth level through 60: 13 in
  total. Essential role spells are guaranteed before combat begins.
- A draft offers spells and full-rank talents from the selected class and talent tree. Deeper talents
  require level milestones. Learned spells gain appropriate ranks automatically. Choices bank during
  combat and can be spent during preparation; unspent choices carry forward.
- Preparation has a fixed countdown. All-ready can shorten it; an idle member cannot extend it.
  Health and mana refill after a clear. Ordinary combat resource use remains during each wave.
- Wave rewards pay equal personal gold. NPC shops sell gear and optional combat consumables. No ammo,
  reagent or durability costs apply during a match. Gear, gold, spells, talents and levels reset in
  the lobby. Cross-match progress grants no combat stats.
- The base has shared health that persists across waves. Ordinary healing cannot repair it. Boss
  defeats repair a fixed amount. Base destruction loses the match; defeating the last boss wins.
- Death starts a respawn timer at the base. Resurrection spells work in combat with cooldowns.
- A departing human gets a separate bot replacement in the same role with level-appropriate gear.
  The human can reconnect and resume. No different human joins an active match in the first release.
- Completed waves award account progress even on a loss, with boss and win bonuses. Future rewards
  are specialization unlocks or cosmetics. Five attentive new players must be able to win without
  permanent power or voice chat.

## Launch roster

| Role | Specializations |
|---|---|
| Tank | Protection Warrior, Feral Druid (bear) |
| Healer | Holy Priest, Restoration Shaman |
| Damage | Retribution Paladin, Combat Rogue, Beast Mastery Hunter, Frost Mage, Affliction Warlock |

## First map and topology

The first map is the Scarlet Monastery Cathedral courtyard, map 189, with an undead assault on a
fortified base. Geometry and routes must pass client verification before release. Early waves teach
target priority and shared defense; later waves introduce changing fronts and overlapping threats.
Ordinary dungeon population does not enter match Instances.

The first deployment uses a single Shard that holds Realm-core and Character state. The shared lobby
has an empty Package-owned Instance on map 189. Each match has a separate Instance on the same Shard. Multiple Parties
can play concurrently. Split topology, a larger Instance Pool and ten-person matches are later work.

The Module owns matchmaking and round rules. Authenticated addon requests use the existing addon
chat transport, with private replies. The addon displays Module state and sends intent; it cannot
grant rewards or choose a result. No new Benilla opcode or custom binary is required.

## Release evidence

Required checks cover queue consent and roles, premade preservation, simultaneous isolated matches,
the full wave sequence, base defeat, draft replay and stale choices, reset, death and resurrection,
bot replacement and reconnect, and account progress without permanent stats. A real Benilla run must
check map routes, targeting, shops, addon messages and five-person combat. Isolated integration tests
must not publish into an existing realm.

The prototype is deployed on Argus. [Deployment notes](../deploy/wave-defense/README.md) record the
service layout and rollback procedure. Authenticated login, matchmaking, the initial draft and
lobby return pass through a Headless Client. Attended Benilla combat remains unverified.

## Form data

The local 1.12 Spell.dbc gives Bear Form 5487 and Dire Bear Form 9634 a cost of 55% of base mana.
Core preserves the mana pool while the form uses rage, and restores it on leaving form. The imported
boost spells 1178 and 9635 supply the form's health, attack power and equipment armor modifiers.
Bear adds 180% equipment armor; Dire Bear adds 360%. Agility armor does not receive that multiplier.
Spell 21178 supplies the 30% threat bonus. These relationships follow
[CMaNGOS HandleShapeshiftBoosts](https://github.com/cmangos/mangos-classic/blob/master/src/game/Spells/SpellAuras.cpp).
The same source maps Night Elf and Tauren bear displays to 2281 and 2289. Durable tests use the
Operator's actual imported spells and verify resources, defenses, attacks and return to caster form.
