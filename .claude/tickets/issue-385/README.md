# Issue #385: Loot Tag ownership, split into tickets

Source: `gh issue view 385 --comments`, "Loot tagging: the first attacker's party owns a
creature's kill and loot".

## State of the world

The Module already records the first player-caused damage to a creature for EventAI quest credit:

- `game_creature_quest_tap` stores the creature and controlling character.
- `game_creature_quest_tap_member` stores the party roster at that instant.
- `record_creature_tap` attributes pets to their controlling character and is called before the
  lethal-hit branch.
- `clear_creature_tap` currently runs only on despawn.

This is the Loot Tag state in all but name. Deepen it into the canonical rule instead of creating
a second ownership system. Keep the existing table and generated binding names because they are
pinned schema artifacts. Move the implementation to the loot module, document the domain term,
and update EventAI to consume the shared rule.

The first positive player-controlled threat wins. Direct damage, spells, DoTs, pets, healing
threat, and taunts must converge on that rule. Creature or guard threat never creates a tag. A
lethal first hit still needs the existing pre-death recording call because surviving-hit threat
bookkeeping cannot see it.

The tag-time roster is a ceiling. Later joiners gain no rights. A member who leaves the tagged
party loses rights. A disconnected member does not leave the party. At death, only snapshot
members who remain entitled, are alive, are on the same map and instance, and are within the
existing 74-yard reward range receive rewards or corpse eligibility. If nobody qualifies, the
corpse remains unavailable until decay.

## Pattern to establish

The Module owns one Gate and one recipient calculation:

```text
first player-controlled threat
    -> durable live-creature Loot Tag and party snapshot
    -> tag-relative kill recipients
    -> one corpse-eligibility row per eligible recipient
    -> every corpse action checks the same eligibility
```

The Gateway does not infer ownership. It subscribes to the existing private tag and eligibility
tables, renders the Module result per viewer, and maps a stable Loot Tag Refusal to the vanilla
`DIDNT_KILL` loot error. Transport failures remain fatal; gameplay Refusals do not end the
session.

Dynamic flags use the vanilla values in shared constants:

```text
UNIT_DYNFLAG_LOOTABLE          = 0x0001
UNIT_DYNFLAG_TAPPED            = 0x0004
UNIT_DYNFLAG_TAPPED_BY_PLAYER  = 0x0008
```

The stored live entity has `TAPPED`. The Gateway adds `TAPPED_BY_PLAYER` only for an entitled
viewer. On a corpse it removes `LOOTABLE` for every viewer without a corpse-eligibility row.

## Playerbots boundary

The playerbots Package named by stories 20 through 22 is not present in this checkout. Do not
invent a second bot policy in LyraCore. T5 files one linked follow-up against the Package owner with
the stable tag and eligibility contract after the Module API is final. The LyraCore change still
covers session-less bot characters wherever they use Module combat, reward, and loot reducers.

## Execution order

```text
T1  canonical Loot Tag and tag-owned death (serial tracer)
 |
T2  Module Loot Gates and reducer seam
 |\
 | +-- T3  viewer-relative dynamic flags ----+
 +---- T4  loot Refusal protocol mapping ----+  parallel worktrees
                                            |
                                            T5  integration, follow-up, PR
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|---|---|---:|---|
| T1 | Canonical Loot Tag and tag-owned death | strongest | ~200k | `CONTEXT.md`, shared dynamic-flag constants, Module tag/threat/combat/death/quest/group/loot-roll paths and focused Module tests |
| T2 | Module Loot Gates and reducer seam | mid | ~180k | Module item-taking, money, skinning, Gateway reducer entry points, actor wrappers, focused Module tests |
| T3 | Viewer-relative dynamic flags | mid | ~160k | Gateway subscriptions, connection base queries, entity-create/update projection, focused Gateway tests |
| T4 | Loot Refusal protocol mapping | mid | ~170k | Gateway loot codec, loot handler/store seam, new reducer binding and focused Gateway tests |
| T5 | Integrate, verify, file Package follow-up, and open PR | strongest | ~180k | union reconciliation, cross-layer tests, ticket notes, GitHub follow-up and PR |

## Shared rules

- Read `CODING_STANDARDS.md`, `CONTEXT.md`, and this file before editing.
- Apply `.claude/skills/unslop/SKILL.md` to prose. Do not add issue numbers to code comments.
- Do not touch a production or development realm.
- Do not change an existing table's fields. An additive table is allowed only if the tracer proves
  the existing tag snapshot cannot express a required invariant and records why.
- GameObject and chest loot are unchanged.
- Durable ownership and every authorization decision stay in the Module.
- A Loot Tag denial is a gameplay Refusal that names the actor and corpse and is logged in the
  Module. It is not a transport error.
- Format touched Rust files individually. Repo-wide formatting rewrites unrelated legacy drift.
- For Module changes, run the wasm check and focused tests. Before excepting any failure, prove it
  fails on the unmodified base commit.
- Stay within the file ownership in the ticket. Report a cross-ticket gap instead of editing a
  sibling's files.
- No PR, GitHub comment, or merge unless the ticket assigns it.
- Return the commit, exact test commands and results, and handoff notes.
