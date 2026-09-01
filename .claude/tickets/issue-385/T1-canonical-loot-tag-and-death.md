# T1: Canonical Loot Tag and tag-owned death

Parent: issue #385. **Tracer bullet. Blocks T2 through T5. Run alone.**
Model: strongest. Estimated size: ~200k tokens.

## Problem

Creature death currently assigns XP, quest credit, kill hooks, and party loot from the killing
blow. The existing EventAI tap snapshot records first player damage but is private to quest code,
misses non-damage threat, and outlives combat. A guard kill also rolls ownerless player loot.

## Delivery

Turn the EventAI snapshot into the canonical Loot Tag deep module.

1. Add `Loot Tag` to `CONTEXT.md`. Explain that the existing `game_creature_quest_tap*` names are
   retained schema artifacts.
2. Add `UNIT_DYNFLAG_TAPPED` and `UNIT_DYNFLAG_TAPPED_BY_PLAYER` to the shared constants beside
   `UNIT_DYNFLAG_LOOTABLE`, with the vanilla numeric values from the shared brief.
3. Move the tag tables and their behavior from `quest.rs` to a focused loot-tag module. Preserve
   table accessors and row shapes. Expose narrow functions for first-threat recording, combat-end
   clearing, membership/recipient resolution, and corpse eligibility. EventAI must call the new
   API rather than carry a second ownership rule.
4. Record the first positive player-controlled threat in the common threat path. Resolve a pet to
   its controlling character. Keep the pre-lethal damage hook so a lethal first hit tags. Aggro
   without player-controlled threat, guards, and creatures do not tag.
5. Clear the tag in the canonical creature disengage/leave-combat path and the existing despawn
   path. Clearing also removes `TAPPED` and `TAPPED_BY_PLAYER` from stored dynamic flags. A dead
   creature must retain corpse flags but no live tag.
6. On first tag, set stored `TAPPED`. Do not store the viewer-relative
   `TAPPED_BY_PLAYER` bit globally.
7. Change creature death so reward recipients come from the Loot Tag, not the killing blow.
   Preserve the actual killing source for death attribution and death hooks. XP, reputation, quest
   kill credit, and per-recipient kill hooks use the entitled in-range tag recipients.
8. A death without a tag grants no player rewards and rolls no money or player loot. A tagged death
   rolls loot and inserts one `game_corpse_loot_eligible` row per eligible recipient, including a
   solo recipient. An empty recipient set means nobody can loot until decay.
9. Make group loot rules, roll audiences, and quest-only reservations consume the corpse eligible
   set. Remove the old behavior where eligibility rows exist only for multi-member group kills.
10. Enforce tag-time membership as a ceiling and current membership as a leave check. Later joiners
    gain nothing. A member who actually leaves loses rights. A logged-out member who remains in the
    party does not count as having left. If the current schema cannot distinguish a required leave
    transition, prefer a small additive table over a heuristic and explain it in the handoff.

Keep `CreatureDeathAttribution.source_guid` and `reward_guid` meaningful. Do not retarget generic
death callbacks that need the actual killer.

## Acceptance criteria

1. First direct damage, spell/DoT damage, player pet threat, healing threat, and taunt all converge
   on one idempotent first-tag function.
2. Creature aggression without player-controlled threat creates no tag.
3. A lethal first hit creates a tag before death processing.
4. A stranger's killing blow does not move XP, quest credit, reputation, or kill hooks away from
   the tag party.
5. A tagger's death does not clear the tag while the creature remains in combat.
6. Evade, leave combat, and despawn clear the tag with the threat lifecycle.
7. Later party joiners receive no reward or corpse eligibility. A leaver loses both.
8. The eligible set contains a solo tagger and every entitled in-range party member exactly once.
9. A guard, creature, or forced tag-less kill produces no reward, money, item, roll, or eligibility
   row.
10. A tagged death with all snapshot members dead, out of range, or off-map may roll the corpse but
    gives it no eligible viewers.
11. Existing EventAI quest-credit tests remain green through the canonical Loot Tag API.
12. Existing table row shapes are unchanged unless the handoff supplies the required proof for one
    additive table.

## Tests

Add focused Module tests beside the existing creature-death and EventAI attribution tests. Drive
public reducers or the established debug integration harness when it exists. Assert observable
rows and rewards, not private call order. Cover at least: first tag wins; foreign killing blow;
evade clear; solo eligibility; join and leave; tag-less death; pet attribution; and party reward
range.

## File ownership

- `CONTEXT.md`
- `crates/lyracore-shared/src/constants.rs`
- `module/src/loot/tag.rs` and the minimal `module/src/loot/mod.rs` export
- tag definitions and call sites in `module/src/quest.rs`
- `module/src/threat.rs`
- `module/src/combat/engage.rs`
- `module/src/combat/death.rs`
- `module/src/loot/rolls.rs`
- `module/src/creatures/tick/lifecycle.rs`
- `module/src/group.rs` only if exact leave semantics require a lifecycle hook
- focused Module test files and schema tripwires affected by moving existing definitions

Do not add actor-facing loot reducers or edit Gateway code. T2 through T4 own those paths.

## Definition of done

Touched Rust files are individually formatted. Focused tests and the Module wasm check are clean.
Commit and push to the shared feature branch, then report any API or schema decision T2 must copy.
