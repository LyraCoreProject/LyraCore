#!/usr/bin/env python3

import argparse
import collections
import json
import pathlib
import re
import sqlite3

parser = argparse.ArgumentParser(description="Audit event objectives against emitted credit and pinned source references.")
parser.add_argument("sqlite", type=pathlib.Path)
parser.add_argument("quest_sql", type=pathlib.Path)
parser.add_argument("reducers", type=pathlib.Path)
parser.add_argument("manifest", type=pathlib.Path)
parser.add_argument("--cpp-root", type=pathlib.Path, required=True)
args = parser.parse_args()

text = args.quest_sql.read_text()
statements = json.loads(text) if text.lstrip().startswith("[") else text.split(";")
requirements, imported = set(), set()
for statement in statements:
    match = re.search(r"INSERT INTO (game_quest_template|game_quest_event_requirement) .*? VALUES (.*)", statement, re.S)
    if not match:
        continue
    values = re.sub(r"'(?:[^']|'')*'", "''", match[2])
    if match[1] == "game_quest_template":
        imported.update(map(int, re.findall(r"\((\d+),", values)))
    else:
        requirements.update(int(quest) for _, quest in re.findall(r"\((\d+),(\d+)\)", values))
if not imported:
    raise SystemExit("No imported quests found")
batches = json.loads(args.reducers.read_text())
credits = {int(quest) for _, batch in batches for quest in re.findall(r"quest-event:(\d+)", batch)}
manifest = json.loads(args.manifest.read_text())
reasons = {
    table: {script["id"]: script["reason"] for script in census["unsupported"]}
    for table, census in manifest["scripts"].items()
}
references = collections.defaultdict(list)
categories = collections.defaultdict(set)
with sqlite3.connect(args.sqlite.resolve().as_uri() + "?mode=ro", uri=True) as source:
    source.row_factory = sqlite3.Row
    explored = {row[0] for row in source.execute("SELECT quest FROM areatrigger_involvedrelation")}
    quests = {row["entry"]: row for row in source.execute("SELECT * FROM quest_template WHERE patch <= 10 ORDER BY patch")}
    candidates = {quest for quest in imported - explored if quests[quest]["SpecialFlags"] & 2}
    events = list(source.execute("SELECT id, creature_id, action1_script, action2_script, action3_script FROM creature_ai_events"))
    for table in ["creature_ai_scripts", *sorted(reasons)]:
        for row in source.execute(f'SELECT id, datalong FROM "{table}" WHERE command = 7 ORDER BY id'):
            quest, key = row["datalong"], row["id"]
            if quest not in candidates:
                continue
            if table != "creature_ai_scripts":
                reason = reasons[table].get(key, "emitted credit" if quest in credits else "no emitted credit instruction")
                categories[quest].add("Script without dispatcher" if "no-translated-event-starts-script" in reason or "dispatcher" in reason else "Refused SQL")
                references[quest].append(f"`{table}:{key}`: `{reason}`")
                continue
            categories[quest].add("Refused SQL")
            roots = [event for event in events if key in [event[f"action{n}_script"] for n in range(1, 4)]]
            for root in roots:
                tag = f'event:{root["id"]}:creature:{root["creature_id"]}'
                findings = [finding["code"] for finding in manifest["findings"] if tag in finding.get("source_ids", [])]
                reason = ", ".join(findings) or ("emitted credit" if quest in credits else "event is not emitted from an EventAI template")
                references[quest].append(f'`creature_ai_scripts:{key}`, event `{root["id"]}`: `{reason}`')
            if not roots:
                references[quest].append(f"`creature_ai_scripts:{key}`: no event references this command list")
    spells = {row["entry"]: row for row in source.execute("SELECT * FROM spell_template WHERE build <= 5875 ORDER BY build")}
    for spell in spells.values():
        for effect in range(1, 4):
            quest = spell[f"effectMiscValue{effect}"]
            if spell[f"effect{effect}"] == 16 and quest in candidates:
                categories[quest].add("C++ spell effect")
                references[quest].append(f'`spell_template:{spell["entry"]}`, effect {effect}: quest-complete effect 16; `SpellEffects.cpp:EffectQuestComplete`')

for path in sorted(args.cpp_root.rglob("*")):
    if path.suffix not in (".cpp", ".h"):
        continue
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if not re.search(r"QUEST_|AreaExploredOrEventHappens|GroupEventHappens", line):
            continue
        for quest in candidates & set(map(int, re.findall(r"\b\d+\b", line))):
            categories[quest].add("C++ script")
            references[quest].append(f'`{path.relative_to(args.cpp_root)}:{number}` (quest constant or credit call)')
# The orphan table supplies the quest id to the shared credit call below it.
orphans = args.cpp_root / "src/scripts/world/areatrigger_scripts.cpp"
for number, line in enumerate(orphans.read_text().splitlines(), 1):
    if "{" in line and "}" in line:
        values = list(map(int, re.findall(r"\b\d+\b", line.split("//")[0])))
        if len(values) == 3 and values[2] in candidates:
            quest = values[2]
            categories[quest].add("C++ script")
            references[quest].append(f'`src/scripts/world/areatrigger_scripts.cpp:{number}`, orphan trigger table and `AreaExploredOrEventHappens` at line 51')

held = requirements - credits
assert held <= {2460, 2608}, held
assert requirements == (candidates & credits) | (candidates & {2460, 2608})
priority = ["Refused SQL", "Script without dispatcher", "C++ spell effect", "C++ script"]
category = {quest: next((kind for kind in priority if kind in categories[quest]), "No grant source located") for quest in candidates}
counts = collections.Counter(category[quest] for quest in candidates - requirements)
print("# Event-credit quest audit\n")
print("Generated by `audit-event-credit.py <sqlite> <quest-statements.json> <eventai-reducers.json> <eventai-manifest.json> --cpp-root <pinned-source-directory>`. Inputs are local and uncommitted.\n")
print("Source pin: `464179081673cfd240f7ffe7f0daf96bca0b5a70`, patch 10, client build 5875. The production quest plan determines scope; area-trigger objectives are excluded from this event-credit rule. This is a static source and import audit, not a gameplay test of every quest.\n")
print(f"Of {len(candidates)} selected quests marked for event credit, {len(requirements & credits)} with emitted credit and {len(held)} without a working grant retain an event requirement. The other {len(candidates - requirements)} complete through their normal imported objectives. This is an eligibility audit; it does not establish the missing scripted scenes.\n")
print("| Fallback source category | Quests |\n| --- | ---: |")
for kind in [*priority, "No grant source located"]:
    print(f"| {kind} | {counts[kind]} |")
print("\nCategories are exclusive in the table above, taking SQL before C++ when both exist. References below retain all sources found. A quest constant locates its script but does not by itself prove a reachable credit call.\n")
print("The twelve Taming the Beast quests are 6061–6064, 6082–6085, 6087–6088 and 6101–6102. Quest 6086 is Training the Beast. Their source credit spells are dispatched by the taming aura callback in `SpellAuras.cpp:1817–1866`; that C++ callback is not imported. A Native check of the imported quest 6085 accepted the quest, received and returned the rod, then accepted Training the Beast 6086. Casting the rod instead deletes the required item on both pre-fix Core 928e3489 and the corrected Module; that existing item-use defect remains. The source callback and a successful taming sequence are unverified. Reward spell 1579 teaches three spells in vmangos; the existing single-spell reward importer refuses that wrapper, so this change does not establish Tame Beast unlock parity.\n")
print("Quest 2460 requires Shenthul's salute event 340101. Condition 806 tests the active incomplete quest, and command 7 grants credit within 20 yards, including both units' bounding radii. Quest 2608 retains its classic event gate and cannot complete on accept. Condition 3694 and speech overrides translate, but generic script 2608 remains refused. Its persistent emote state 69, clearing state 0, source-relative move at 4 seconds and point-movement controller options at 14, 20 and 30 seconds are missing. Command 7 at 33 seconds is therefore not emitted. This is a known completion regression against classic, held rather than granting free credit.\n")

print("The prior ledger had 15 quests refused at command 7, rather than 14: 219, 452, 590, 648, 836, 1173, 2767, 3367, 3382, 3982, 4265, 4901, 5713, 5944 and 6523. Distance/group fields now translate for all 15. Nine scripts have no translated event dispatcher. The remaining scripts still refuse command 34 for 648, 836 and 2767, command flags for 3382, command 20 for 5713, and command 71 for 5944. Quest 4265 has a separate start script without a classic dispatcher. Thus none of those quests gains a complete emitted grant sequence from this command fix alone. Shenthul 2460 is the new working credit source.\n")
print("| Quest | Outcome | Source category | Source reference and reason |\n| ---: | --- | --- | --- |")
for quest in sorted(candidates):
    outcome = ("Event requirement; emitted credit" if quest in credits else "Event requirement; grant unsupported") if quest in requirements else "Normal objectives; no event gate"
    reason = "; ".join(dict.fromkeys(references[quest])) or "No SQL command 7, quest-complete spell effect or quest reference in the pinned script sources was located. Source grant remains unresolved."
    print(f"| {quest} | {outcome} | {category[quest]} | {reason} |")
