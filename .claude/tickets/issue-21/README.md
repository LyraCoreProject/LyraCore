# Issue #21 — Hunter pet system, broken into tracer-bullet tickets

Source: https://github.com/LyraCoreProject/LyraCore/issues/21, specified 2026-08-16.

## State of the world

The original issue is stale in useful ways. These pieces have already shipped on `main`:

- a live pet is an ordinary creature entity with `owner_guid`, without a wild spawn row;
- Warlock summon, replace, despawn, follow, chase, melee, data-driven casting and kill credit;
- the creature behavior cycle's pet phase and its in-memory harness;
- Follow, Stay, Attack, Dismiss and Passive, Defensive, Aggressive through `CMSG_PET_ACTION`;
- owner summon-field updates plus `SMSG_PET_SPELLS` publication and clearing;
- the full `CreatureFamily.dbc` import, including `pet_food_mask` and `pet_talent_type`.

Do not rebuild those. Deepen the substrate with a durable Hunter-pet identity. CMaNGOS Classic is
the reference for uncertain Vanilla mechanics. Its current `Pet.cpp` applies happiness, loyalty,
pet XP and the 75/100/125 physical-damage multiplier only to `HUNTER_PET`; `SUMMON_PET` returns
before those systems. Warlock demons therefore share control and behavior but have no Hunter care
row.

## Shared code shape

Keep the live/durable split explicit:

```text
durable HunterPet (one current identity per owner)
             │ materializes / updates
             ▼
live WorldEntity (owner_guid != 0) ──► existing creature cycle, movement, casts and swings
```

- Gameplay state and timers live in module reducers/tables. The gateway translates packets only.
- Taming and feeding are generic imported spell-effect kinds, not spell-id branches.
- Sender/owner authorization is checked inside the module.
- Failed gates do not consume food, delete the wild creature, or leave half a pet.
- Test at the highest existing seams: spell completion, inventory use, kill-credit, creature-cycle
  harness, shared combat math, and focused gateway codec/dispatch. Do not assert helper calls or
  table layout.
- Use concise current-style comments. Do not add issue numbers to code comments.

## Execution DAG

```text
T1 durable identity + completed tame (serial tracer)
 ├── T2 feeding + happiness + damage + care timer ─┐
 ├── T3 pet XP + levels + loyalty progression ─────┤ parallel worktrees
 └── T4 Hunter protocol + name query ───────────────┘
                                                   │
                                                   ▼
                                      T5 integration and acceptance
```

| Ticket | Model | Estimate | Primary ownership |
|---|---|---:|---|
| T1 | strongest | ~190k | Hunter identity/tame module, tame importer mapping, tame fixtures/tests |
| T2 | strongest | ~180k | pet-care module, item food metadata, feed mapping, care schedule, damage seam |
| T3 | mid | ~170k | pet-progression module, kill-XP hook, loyalty transitions |
| T4 | mid | ~160k | gateway pet protocol/codec/dispatch and necessary read model |
| T5 | strongest | ~170k | integrated reconciliation, bindings, end-to-end acceptance and docs |

## Integration conventions

- T1 should create narrow `hunter_pet` APIs that T2–T4 call. Later tickets should prefer new
  modules (`pet_care`, `pet_progression`, a focused gateway pet module) over growing the legacy
  summoned-pet file.
- Parallel tickets may append exports to creature/spell module roots. Keep those edits minimal so
  integration is mechanical. T5 owns final naming and duplicate removal.
- Generated gateway bindings are integration-owned unless a ticket needs them to compile. Never
  hand-edit generated files when the repository's generator can produce them.
- No production realm, database or deployed gateway may be touched.
- Each ticket must format and run the focused crate tests it changes. T5 runs the complete relevant
  workspace checks.
