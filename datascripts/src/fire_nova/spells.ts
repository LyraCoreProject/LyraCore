// Fire Nova: the worked example of authoring spells as a Package Delta.
//
// It clones a real spell, changes what it needs, loops a rank ladder, and emits one artifact. Every
// column below is named the way the Module declares it, so a schema move fails the typecheck here
// rather than at import.
//
// Run it with `lyracore packages build`, or directly:
//
//   bun run datascripts/src/fire_nova/spells.ts
//
// The artifact lands at `packages/fire_nova/data/.generated/spell.json`. Only THIS file is
// committed: the artifact is derived, and regenerating it is one command.
//
// # Why a clone
//
// A Package may not invent a spell out of nothing — an insert has to carry every column, and half a
// row is not a spell. Cloning a real one gives a complete, coherent starting point, and every column
// the author does not name keeps the source's value.

import { run } from "../../lib/index.ts";

/// The Package this Datascript writes for: the folder under `packages/` and the Package identity the
/// artifact carries.
const PACKAGE = "fire_nova";

/// Fireball (Rank 1) — a direct fire nuke, which is the shape Fire Nova starts from.
const FIREBALL = 133;

/// The first identifier in the Package Spell Range this Package claims. Five consecutive ranks
/// follow it.
const FIRST_RANK_ID = 6_000_100;

const RANKS = 5;

/// Our own TargetKind taxonomy (`module/src/spell/taxonomy.rs`): every enemy inside the radius.
const T_AREA_ENEMY = 4;

/// The nova's burst radius, in yards, at every rank.
const RADIUS_YD = 8;

/// A rank's level requirement, mana cost and damage. Written as one table so the ladder reads as
/// data rather than as arithmetic scattered through a loop.
const LADDER = [
  { level: 12, cost: 40, damage: 33 },
  { level: 22, cost: 80, damage: 71 },
  { level: 32, cost: 145, damage: 129 },
  { level: 42, cost: 240, damage: 224 },
  { level: 52, cost: 355, damage: 340 },
];

await run(PACKAGE, (data) => {
  const fireball = data.spell(FIREBALL);

  for (let rank = 0; rank < RANKS; rank += 1) {
    const tier = LADDER[rank]!;
    const nova = fireball.clone(FIRST_RANK_ID + rank);

    nova.set("name", `Fire Nova (Rank ${rank + 1})`);
    nova.set("castTimeMs", 0); // instant, unlike the Fireball it was cloned from
    nova.set("cooldownMs", 8_000);
    nova.set("rangeYd", 0); // centred on the caster, so there is no cast range
    nova.set("cost", tier.cost);
    nova.set("spellLevel", tier.level);
    nova.set("maxStacks", 0);

    // The clone carries Fireball's own damage effect. Retarget it at everything nearby and set this
    // rank's magnitude; the columns not named here — school, kind, mechanic — stay Fireball's.
    const burst = nova.effect(0);
    burst.set("target", T_AREA_ENEMY);
    burst.set("radiusYd", RADIUS_YD);
    burst.set("basePoints", tier.damage);
    burst.set("dieSides", 0); // a flat burst, not Fireball's damage range
    burst.set("chainTargets", 0);
  }
});
