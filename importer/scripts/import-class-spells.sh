#!/usr/bin/env bash
# import-class-spells.sh — THE curated class-spell import: the hand-picked spell ids that make all
# NINE classes playable, read out of the client's own Spell.dbc into game_spell / game_spell_effect,
# plus the trainer offerings that let a character buy them.
#
# It has two halves, and they are curated to different depths:
#   1. The six classes with a Human option (Warrior, Paladin, Rogue, Priest, Mage, Warlock) get their
#      full level 1-20 RANK WINDOWS — every rank a character can train on the way to 20.
#   2. Hunter, Shaman and Druid get deliberately narrow STARTER KITS — the rank-1 ids their default
#      rotations and kits actually cast. Widening these to full 1-20 windows is a follow-up.
# Per-class comments below about Human racials or Human character-creation kits are exactly that:
# race-specific notes inside a nine-class import, not a scope limit on the file.
#
# WHEN YOU RUN IT: importer/scripts/import-world.sh already calls it at the right point of a full
# world import, and `lyracore import` drives that for you — running this by hand is the advanced path,
# for re-applying the class spells alone without redoing the whole ETL. NEEDS: a running node with the
# target database published, an extracted client Data directory (for Spell.dbc), and a built importer.
#
# Run from the checkout root; build the importer first: cargo build --bin lyracore-importer
# Usage: importer/scripts/import-class-spells.sh [path-to-client/Data]
#        DB=lyracore-world-1 importer/scripts/import-class-spells.sh   (a shard other than the default)
#
# [V] on an id below means: measured or guessed against ONE operator's client, not verified against
# yours. Confirm it against your own Spell.dbc before relying on it, and correct a wrong id rather
# than working around it. The safety net is loud, not silent: a typo'd id trips the importer's
# --only "requested N ids but matched M" WARNING instead of importing nothing or the wrong rank.
#
# WHY a script: the imported rows are DB DATA. An additive `spacetime publish` preserves
# them, but a DESTRUCTIVE `-c` reprovision wipes them — re-run this afterward. The module
# spine that makes them usable (trainer.rs resolves the LearnSpell wrapper -> learns the
# rank; cast.rs gates cast on caster.level >= spell.spell_level) ships in the module; this
# script only supplies the DATA.
#
# Each spell needs BOTH its trainer LearnSpell WRAPPER id AND its castable RANK id (the
# wrapper's game_spell_effect.trigger_spell) — the importer can't auto-follow the chain.
# Import is surgical (per-id delete+reload) and ADDITIVE (preserves the seed + 50xxx test
# fixtures). For a dry-run + coverage report, drop `--apply` from the command below.
set -euo pipefail

DBC="${1:-../wowclient/Data}"
# WHICH DATABASE. Set DB to target a shard other than the default. `import-world.sh` passes it
# through explicitly, so a multi-shard import puts each shard's curated spells on that shard — this
# script writing to the importer's default while the ETL wrote elsewhere was a real, silent bug.
DB="${DB:-lyracore}"

# wrappers + ranks, per class (Parry 3128/3127 is shared Warrior<->Paladin, deduped to Warrior).
# Tier 2b combat-state-machine kit added (re-imported through the updated importer so the reclassification +
# flags land): Heroic Strike (78) + Cleave (845) → E_NEXT_SWING (on-next-swing QUEUE); Overpower (7384,
# spell_level 12) → SPELL_ATTR_REQ_OVERPOWER react-window flag; Revenge (6572) → SPELL_ATTR_REQ_REVENGE flag.
# 78/6572 were already imported (this just re-runs them); 845/7384 are NEW self-contained ranks (no
# LearnSpell wrapper → the rank id IS the trainer learn target, like Kick 1766). All four are in
# the Warrior createinfo kit (game_createinfo_spell) so they're castable; Overpower's L12 is enforced by the cast level-gate.
# Tier 3a STANCE system added: Battle Stance (2457, L1 — the baseline switch-back), Defensive Stance (71,
# L10 — −10% taken/dealt, +threat) and Berserker Stance (2458, L30 — imported for the full switch set). All
# three reclassify the inert ModShapeshift marker to E_SET_STANCE (p0 = 0-based stance id) and are
# self-contained (no LearnSpell wrapper → the rank id IS the learn target). Each carries Stances=0 (usable in
# any stance — they're the switches themselves); the per-ability Stances usability mask (Charge/Thunder
# Clap/Overpower Battle-only, Rend Battle/Defensive, Hamstring Battle/Berserker) is imported from the DBC and
# gates in resolve_cast_at. All three are in the Warrior createinfo kit → castable at login.
# Sunder Armor (7386, spell_level 10 — the iconic stacking armor debuff: A_MOD_RESISTANCE −armor, max_stacks 5)
# added: it is in the Warrior createinfo kit (seed::CREATEINFO_KIT) but had been surviving only on a stale manual import,
# so a -c reprovision + this canonical ETL dropped it (cast → graceful 'unknown spell' Err at L10). Self-contained
# rank (no LearnSpell wrapper → the rank id IS the trainer learn target, like Cleave 845), so it rides IDS_WARRIOR
# for BOTH --only (header import) and --trainer (the L10 learn/re-learn path).
# 1-20 rank-window extension — [V] every id, confirm each against your own Spell.dbc: Battle Shout
# R2 5242 [V] / R3 6192 [V], Sunder Armor R2 7405 [V], Demoralizing Shout R2 6190 [V], Thunder Clap
# R2 8198 [V].
IDS_WARRIOR="1343,1423,1606,1716,1738,2688,3128,6549,6674,6343,772,284,1715,100,2687,3127,6546,6673,78,6572,845,7384,7386,2457,71,2458,5242,6192,7405,6190,8198"
# 635 Holy Light R1 + 20154 Seal of Righteousness R1 added: these are the Human Paladin CREATEINFO
# baseline (in the Paladin createinfo kit). They ride the --only set so their game_spell headers import (the
# baseline rows must exist for the spellbook/cast-gate).
# 20154 + 21084 are HEADER-ONLY (TRAIN_PALADIN excludes them): the cmangos npc_trainer data never offers
# either directly — the SoR upgrade path is wrapper 21083 (→21084, req L6), already in this list. Offering
# them directly duplicated "Seal of Righteousness (Rank 1)" in the trainer window (BOTH DBC rows carry
# rank text "Rank 1") and let a paladin buy a second seal identical to the createinfo one.
# 7328 Redemption R1 added: the paladin-healer bots' combat rez — E_RESURRECT maps clean.
# 1-20 rank-window extension — [V] every id, confirm each against your own Spell.dbc: Holy Light
# R3 647 [V]. Seal of Righteousness's next rank-up (20287) and Judgement are ALREADY present below
# (no change needed there) — Judgement's own rank line is deliberately left as-is.
# Consecration 26573 (L20): self-contained rank (no LearnSpell wrapper → the id IS the
# learn target). eff1 is the E_PERSISTENT_AREA ground-AoE (importer name-rescue of the ground
# A_PERIODIC_DAMAGE); its own tick_ground_areas damages hostiles in the 8yd Holy zone for 8s.
IDS_PALADIN="1873,1875,1878,1911,1937,5572,5584,10294,10321,19741,20437,21083,639,465,633,1022,1152,498,853,10290,20271,21084,19740,20287,21082,635,20154,7328,647,26573"
# shellcheck disable=SC2034 # reference list only (documents which ids TRAIN_PALADIN excludes from
# IDS_PALADIN above — see the header-only note on 20154/21084 just above); not consumed by this script.
TRAIN_PALADIN="1873,1875,1878,1911,1937,5572,5584,10294,10321,19741,20437,21083,639,465,633,1022,1152,498,853,10290,20271,19740,20287,21082,635,7328,647,26573"
# Kick R1 (1766, L12) added: a self-contained ability (no LearnSpell wrapper → trainer to_learn falls
# back to 1766). The interrupt is E_INTERRUPT (importer reclassifies raw effect 68); eff1 E_DAMAGE works
# regardless. Its learn path is the OVR_ROGUE override binding further down, not a dump offering.
# Garrote R1 (703, L14) + Feint R1 (1966) added: both self-contained (no LearnSpell
# wrapper → the rank id IS the learn target). Garrote = REQ_STEALTH|REQ_BEHIND opener with an
# A_PERIODIC_DAMAGE bleed (period now imports correctly via the EffectAmplitude .to_bits() fix); Feint =
# E_REDUCE_THREAT (one-time current-threat drop). Both are in the Rogue createinfo kit → castable at login.
# 1-20 rank-window extension — [V] every id, confirm each against your own Spell.dbc: Gouge R2 1998?
# [V] (rank uncertain — CONFIRM this one in particular before relying on it). Slice and Dice R1 (5171)
# is ALREADY present below (no change needed there).
IDS_ROGUE="1789,2592,5167,1762,1780,5278,6763,652,1424,2984,5175,1784,53,921,1757,1776,5277,6760,6770,674,2983,5171,1766,703,1966,1998"
# 2139 Counterspell added: the mage-dps bots' interrupt (it stays latent against creatures until
# creature casting is interruptible).
# 1-20 rank-window extension — [V] every id, confirm each against your own Spell.dbc: Fireball
# R3 145 [V] / R4 3140 [V], Frostbolt R3 7322 [V] / R4 8406 [V], Frost Nova R2 6131 [V].
# Blink 1953 (L20): self-contained rank (no LearnSpell wrapper → the id IS the learn
# target, like Kick 1766). Its eff1 is the E_BLINK forward-teleport (importer name-rescue); eff2 the
# root/snare A_IMMUNITY. Rides --only (header) AND --trainer (the L20 learn offering) via IDS_MAGE.
# Flamestrike 2120 (L16): the first client-castable CLICKED-GROUND spell — eff0 the
# initial dest-anchored AoE nuke (importer forces its E_DAMAGE to T_AREA_ENEMY), eff1 the burning
# patch (name-rescued to E_PERSISTENT_AREA, anchored at the cast's DEST — the same ground-anchoring
# the Paladin's Consecration uses).
IDS_MAGE="1472,1142,5507,1173,1249,2141,2136,1168,1191,5146,1174,1194,5565,1459,116,5504,143,587,118,205,5143,7300,122,5505,2139,145,3140,7322,8406,6131,1953,2120"
# 588 Inner Fire R1 + 527 Dispel Magic R1 added: NOT createinfo (trainer-taught); they
# are NOT Human Priest createinfo, so they were demoted to the trainer. Their game_spell headers already
# import via --only here; adding them to the --trainer list gives the (now non-baseline) spells a learn
# path so a fresh Priest can buy them. 1243 PW:Fortitude (also demoted) was already in this list.
# 1-20 rank-window extension — [V] every id, confirm each against your own Spell.dbc: Heal R1 2054
# [V], Smite R3 6060 [V]. Lesser Heal R3 (2053) is ALREADY present below (no change needed there).
IDS_PRIEST="1255,1258,2056,1275,2851,1265,6073,1259,2013,2057,8093,1243,589,2052,591,17,586,139,594,2006,2053,8092,588,527,2054,6060"
# 1-20 rank-window extension — [V] every id, confirm each against your own Spell.dbc: Shadow Bolt
# R3 705 [V] / R4 1088 [V], Corruption R2 6222 [V]. Immolate R2 (707) is ALREADY present below
# (no change needed there).
IDS_WARLOCK="1374,1393,6221,1381,1476,1296,5783,1375,1383,6203,7662,348,702,172,695,1454,980,5782,707,696,6201,1120,688,697,705,1088,6222"

# NON-HUMAN class STARTER lists — [V] every id, confirm each against your own Spell.dbc (a typo'd id
# fails LOUD via the --only "requested N but matched M" WARNING, it does not import the wrong rank
# quietly). Deliberately narrow: exactly the rank-1 ids the bot default rotations/kits cast — the
# 1-20 rank-window widening for these three classes is a follow-up.
# HUNTER: Arcane Shot R1 3044 [V], Serpent Sting R1 1978 [V], Raptor Strike R1 2973 [V] — NOTE:
# 2973 is imported/trainer-offered but wired to NO rotation/kit row (the bot's melee filler is the
# frame's melee arming, not this spell); it would import as instant E_DAMAGE (the on-next-swing
# rescue is name-scoped to Heroic Strike/Cleave). Kept for player training; wire it to bots only
# after an on-next-swing rescue for it exists.
IDS_HUNTER="3044,1978,2973"
# SHAMAN: Healing Wave R1 331 [V], Ancestral Spirit R1 2008 [V] (E_RESURRECT via the importer name
# rescue), Lightning Bolt R1 403 [V], Flame Shock R1 8050 [V], Lightning Shield R1 324 [V].
IDS_SHAMAN="331,2008,403,8050,324"
# DRUID: the form switches Bear Form 5487 [V] + Cat Form 768 [V] (both name-rescued to E_SET_STANCE
# with p0 = our stance id 3/4; Dire Bear 9634 is L40 — outside the curated window, rides a later
# widening), Growl 6795 [V] (the bear taunt), Maul R1 6807 [V] (its Stances mask must import as our
# Bear|DireBear bits 0x28 — see the importer's translate_stance_mask), Healing Touch R1 5185 [V],
# Rejuvenation R1 774 [V], Rebirth R1 20484 [V] (E_RESURRECT by name), Mark of the Wild R1 1126 [V],
# Wrath R1 5176 [V], Moonfire R1 8921 [V].
IDS_DRUID="5487,768,6795,6807,5185,774,20484,1126,5176,8921"

# Createinfo rank-1s that carry SCRIPT/special effects and so need the importer's
# reclassification — most baselines are plain damage/heal and map fine, so ONLY these are re-imported:
# Rogue Sinister Strike R1 (1752, combo gen) + Eviscerate R1 (2098, finisher). A level-1 rogue starts with
# these, so without this their combo loop is inert. Add others here if a later phase finds an inert baseline.
# Createinfo R1 abilities for EVERY human class (the spells a fresh Human knows at creation, per the cmangos
# playercreateinfo_spell) MUST import here so their game_spell headers exist for the spellbook + cast-gate —
# a baseline id with no header casts as a graceful unknown-spell Err on a clean -c rebuild. Mage Fireball/
# Frost Armor (133,168 — Frost Armor's eff2 mis-firing chill is neutered E_TRIGGER->A_FLAG by name), Rogue
# Sinister Strike/Eviscerate (1752,2098 — the combo loop), Priest Smite/Lesser Heal (585,2050), Warlock
# Shadow Bolt/Demon Skin (686,687). NOT trainer offerings (createinfo-known), so they ride --only here, not a
# --trainer list. (Summon Imp 688 + Immolate 348 were DEMOTED off the Warlock baseline to trainer-learnable —
# they now live in IDS_WARLOCK, not here; the Imp creature_template 416 is force-included by the creature ETL.)
IDS_BASELINE="1752,2098,133,168,585,2050,686,687"

# Hidden TRIGGERED spells that are NOT learnable / NOT a trainer offering, but MUST be imported so a
# learnable spell's effect can resolve them at runtime. Arcane Missiles (5143, in IDS_MAGE) is a CHANNEL
# whose per-tick A_PERIODIC_TRIGGER effect casts 7268 ('Arcane Missile' — the per-bolt E_DAMAGE) each tick;
# without 7268 in game_spell the channel ticks would have nothing to resolve. Bloodrage (2687, in
# IDS_WARRIOR) has effect_index=2 as E_TRIGGER with trigger_spell=29131 (the periodic 10-rage energize
# trickle); without 29131 in game_spell the trickle no-ops and Bloodrage delivers only its instant 10 rage.
# These ride ONLY the global --only set below (NOT any --trainer list), so they import without ever
# appearing as a trainer offering.
IDS_TRIGGERED="7268,29131"

# PET abilities — cast by a SUMMONED creature (not the player), so importer-only (--only) and NOT a
# trainer/baseline offering (a player never learns them). The Warlock Imp (creature_template 416, summoned
# by Summon Imp 688 in IDS_WARLOCK) casts Firebolt 3110 via a game_creature_spell rotation row keyed to
# 416 — pass_cast runs it with ZERO new engine code. CRITICAL: 3110 is a 0-mana pet spell, and the Imp is
# built with power=0/max_power=0, so the cost must import as 0 (the cost gate now exempts creatures anyway,
# but 3110 is genuinely free). Like IDS_BASELINE/IDS_TRIGGERED, this rides ONLY the global --only set.
IDS_PET="3110"

# Warrior ids ABOVE the 1-10 window: Slam (1464, spell_level 30) + Shield Wall (871, L28).
# Both are trainer-taught into game_player_spell (the module cast-gate reads those rows), but a fresh warrior can't reach
# their level in the alpha. Imported here ONLY so their game_spell headers exist — book/script consistency:
# an id in CASTABLE with no row would cast as a graceful 'unknown spell' Err. NOT for play → they ride ONLY the
# global --only set below, never a --trainer list (like IDS_BASELINE/IDS_TRIGGERED/IDS_PET).
IDS_WARRIOR_ABOVE_WINDOW="1464,871"

# Human RACIAL spells (per-race createinfo, race_spells(1) in constants.rs): Sword Spec(20597),
# The Human Spirit(20598), Diplomacy(20599), Perception(20600), Mace Spec(20598→20864). Header import so
# their game_spell/effect rows exist — the passive ones (SPELL_ATTR_PASSIVE) are applied at login by
# apply_racial_passives, Perception is the active racial. Createinfo-known, not trained → --only only.
IDS_RACIAL_HUMAN="20597,20598,20599,20600,20864"

IDS="${IDS_WARRIOR},${IDS_PALADIN},${IDS_ROGUE},${IDS_MAGE},${IDS_PRIEST},${IDS_WARLOCK},${IDS_HUNTER},${IDS_SHAMAN},${IDS_DRUID},${IDS_BASELINE},${IDS_TRIGGERED},${IDS_PET},${IDS_WARRIOR_ABOVE_WINDOW},${IDS_RACIAL_HUMAN}"

# Class-trainer CREATURE-TEMPLATE entries (Northshire Valley starting-area human class trainers, all
# already spawned on map 0 with the TRAINER npc_flag). The offerings key to these entries; every spawned
# trainer of the same entry serves the same list (like vendors). Each class's IDS_* list (the SAME data
# already passed via --only) IS the offering source — no second data list. required_level + cost are
# DBC-DERIVED in the importer (the spell's spell_level + a level-keyed formula), so NO cmangos value is
# shipped. To also serve the Stormwind/Goldshire trainers of a class, add a second `--trainer <entry>=…`
# with the same IDS_* (cheap; the per-entry surgical delete keeps re-imports idempotent).
# Every same-class trainer entry serves the same list — the Goldshire anchor + the Northshire
# starters + the Stormwind roster. This is not redundancy: a service-coverage audit found all of them
# spawned with the TRAINER flag and teaching NOTHING, so a fresh character could not train anywhere
# before walking to Goldshire.
TRAINERS_WARRIOR="913 911 914 5479 5480"   # Lyria(GS) Llane Beshere(NS) Ander Germaine/Wu Shen/Ilsa Corbin(SW)
TRAINERS_PALADIN="927 925 928 5491 5492"   # Wilhelm(GS) Sammuel(NS) Grayson/Arthur/Katherine(SW)
TRAINERS_ROGUE="917 915 918 13283"         # Keryn(GS) Jorik Kerridan(NS) Osborne/Tony Romano(SW)
TRAINERS_PRIEST="377 375 376 5484 5489 11397" # Josetta(GS) Anetta(NS) Laurena/Benjamin/Joshua/Nara(SW)
TRAINERS_MAGE="328 198 331 5497 5498"      # Zaldimar(GS) Khelden Bremen(NS) Dumas/Jennea/Elsharin(SW)
# shellcheck disable=SC2034 # kept for the trainer roster's documentation shape; warlock has no
# `bind` override call below — see "Warlock's full tree is in the dump — no overrides" further down.
TRAINERS_WARLOCK="906 459 461 5495 5496"   # Maximillian(GS) Drusilla(NS) Demisette/Ursula/Sandahl(SW)
# Non-Human class trainers — [V] all three creature entries, confirm them against your own dump. A
# wrong entry means the bot trainer pass finds zero offerings for that class: a graceful no-op, never
# a wrong-class grant. These three entries are DUPLICATED in the bot package's class-trainer table —
# change one, change both, or bots will train against a creature the world does not offer.
# These trainers stand in their own starting areas (not Northshire); the offerings still import fine
# (a game_trainer_spell row is keyed by creature-template entry, wherever that template is spawned).
TRAINERS_HUNTER="3596 5515 5516 5517" # Ayanna(Dolanaar)[V] + SW dwarven-district trio (Einris/Ulfir/Thorfin)
TRAINERS_SHAMAN="3062"                # Canaga Earthcaller (Valley of Trials) [V]
TRAINERS_DRUID="3033 5504 5505 5506"  # Mardant(Shadowglen)[V] + SW Park trio (Sheldras/Theridran/Maldryn)
# INTENTIONALLY UNBOUND (and annotated as known-empty in import-world.sh's coverage audit, so they
# don't read as gaps): 2485 Larimaine Purdue (portal trainer — teleport spells aren't in the curated
# lists yet), 4732 Randal Hunter (riding — no mount system yet), 2879 Karrina Mekenda (pet trainer —
# no hunter pet system yet), 11867 Woo Ping (weapon master — his offerings import from the dump's own
# npc_trainer rows, so he needs no curated binding here).

TRAINER_ARGS=()
bind() { local ids="$1"; shift; for e in "$@"; do TRAINER_ARGS+=(--trainer "${e}=${ids}"); done; }
# The --dump ETL imports the dump's FULL class trees for in-box (human) trainers, with the
# dump's real costs/reqlevels — these curated bindings are the OVERRIDE layer, carrying ONLY the
# "specials" verified ABSENT from cmangos npc_trainer (cmangos delivers them via spell_learn /
# level-up instead): stances/Cleave/Overpower/Sunder/Shield Wall/Slam (warrior), SoR/Consecration
# (paladin), Kick/Garrote/Feint (rogue), Inner Fire/Dispel (priest), Flamestrike/Blink (mage).
# Warlock's full tree is in the dump — no overrides. The full IDS_* lists still ride --only above
# (headers + effects must import for the ranks to be castable). NON-HUMAN trainers are map-1
# (outside the --dump box → the ETL adds nothing for them) and keep their full curated bindings.
OVR_WARRIOR="845,7384,7386,71,2457,2458,871,1464"
OVR_PALADIN="20154,21084,26573"
OVR_ROGUE="1766,703,1966"
OVR_PRIEST="588,527"
OVR_MAGE="2120,1953"
bind "${OVR_WARRIOR}" ${TRAINERS_WARRIOR}
bind "${OVR_PALADIN}" ${TRAINERS_PALADIN}
bind "${OVR_ROGUE}"   ${TRAINERS_ROGUE}
bind "${OVR_PRIEST}"  ${TRAINERS_PRIEST}
bind "${OVR_MAGE}"    ${TRAINERS_MAGE}
bind "${IDS_HUNTER}"  ${TRAINERS_HUNTER}
bind "${IDS_SHAMAN}"  ${TRAINERS_SHAMAN}
bind "${IDS_DRUID}"   ${TRAINERS_DRUID}
exec ./target/debug/lyracore-importer --db "$DB" --dbc "${DBC}" --spells --only "${IDS}" "${TRAINER_ARGS[@]}" --apply
