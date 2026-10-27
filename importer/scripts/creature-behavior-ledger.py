#!/usr/bin/env python3
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import re
import sqlite3


CATEGORIES = {
    'speech': {'speak', 'talk', 'broadcast'},
    'casts': {'cast', 'cast-options', 'cast-spell'},
    'summons': {'summon', 'spawn-at-actor', 'spawn-creature', 'remove-guardians'},
    'phase or HP rules': {'phase', 'phase-range', 'phase-random', 'phase-inc-checked'},
    'flee or help': {'flee', 'help'},
    'despawn or emote': {'force-despawn', 'despawn-source', 'emote', 'random-emote', 'set-stand-state', 'force-death'},
    'quest credit': {'quest-event', 'script-quest-event', 'kill-credit'},
    'movement or combat controls': {'ranged-mode', 'combat-movement', 'auto-attack', 'attack-start', 'evade', 'follow-movement', 'immobilized', 'walking', 'idle', 'patrol-paused', 'lethal-floor', 'zone-combat', 'move-point', 'move-dynamic', 'pause-waypoints', 'set-run', 'set-movement'},
    'presentation or state': {'faction', 'display-template', 'unit-flags', 'npc-flags', 'sheath', 'facing', 'creature-mount', 'virtual-main-hand', 'posture', 'mana', 'remove-aura', 'set-active', 'set-facing', 'modify-npc-flags', 'activate-object', 'face-orientation', 'set-faction', 'modify-unit-flags', 'set-gossip-menu', 'despawn-gameobject', 'set-equipment', 'update-creature-template', 'set-world-state'},
    'encounter or AI notification': {'notify-encounter', 'throw-ai-event', 'send-ai-event'},
    'threat': {'threat-selected', 'threat-all'},
}


def programs(batches):
    subjects, relays = defaultdict(list), {}
    for reducer, batch in batches:
        for line in batch.splitlines():
            if reducer.startswith('import_creature_ai_definitions'):
                subject, _, rules = line.split('@', 2)
                subjects[subject].extend(rules.split('~'))
            elif reducer == 'import_creature_ai_relay_definitions':
                key, _, _, _, steps = line.split('@', 4)
                relays[int(key)] = [step.rsplit(',', 1)[-1] for step in steps.split('~') if step]
    return subjects, relays


def instruction_categories(instruction, relays, visited):
    kind, _, parameters = instruction.partition(':')
    if kind == 'creature-control':
        return instruction_categories(parameters, relays, visited)
    result = {label for label, kinds in CATEGORIES.items() if kind in kinds}
    if kind in {'start-relay', 'start-script', 'start-relay-chance', 'start-script-choice'}:
        choices = parameters.split(':')[0]
        for key in re.findall(r'\d+', choices):
            key = int(key)
            if key and key not in visited:
                for step in relays.get(key, []):
                    result |= instruction_categories(step, relays, visited | {key})
    return result


def behaviors(batches):
    subjects, relays = programs(batches)
    result = defaultdict(set)
    for subject, rules in subjects.items():
        if subject.startswith('entry:'):
            entry = int(subject.split(':')[1])
        elif subject.startswith('guid:'):
            entry = (int(subject.split(':')[1]) >> 24) & 0xffffff
        else:
            raise ValueError(f'unknown subject {subject}')
        for rule in rules:
            fields = rule.split(',')
            if fields[1].split(':')[0] in {'health', 'target-health', 'friendly-health'}:
                result[entry].add('phase or HP rules')
            for instruction in fields[-1].split('+'):
                result[entry] |= instruction_categories(instruction, relays, set())
    return result


def selected(connection, table):
    if table not in {'creature_template', 'creature', 'creature_spells', 'creature_ai_events'}:
        raise ValueError(table)
    rows = connection.execute(f'SELECT * FROM {table}').fetchall()
    if table == 'creature_template':
        chosen = {}
        for row in rows:
            if row['patch'] <= 10 and (row['entry'] not in chosen or chosen[row['entry']]['patch'] < row['patch']):
                chosen[row['entry']] = row
        return chosen.values()
    if table == 'creature':
        return [row for row in rows if row['patch_min'] <= 10 <= row['patch_max'] and not row['spawn_flags'] & 2]
    return rows


def ledger(connection, before, after, scoped, manifest):
    old, new = behaviors(before), behaviors(after)
    templates = {row['entry']: row for row in selected(connection, 'creature_template')}
    lists = {row['entry'] for row in selected(connection, 'creature_spells') if any(row[f'spellId_{n}'] for n in range(1, 9))}
    events = {row['creature_id'] for row in selected(connection, 'creature_ai_events')}
    for entry, row in templates.items():
        if row['spell_list_id'] in lists:
            new[entry].add('casts')
    maps = defaultdict(set)
    for row in selected(connection, 'creature'):
        for key in ['id', 'id2', 'id3', 'id4', 'id5']:
            if not row[key]:
                break
            maps[row[key]].add(row['map'])
    refusals = defaultdict(set)
    for finding in manifest['findings']:
        if finding['classification'] != 'unsupported':
            continue
        for key in finding.get('source_ids', []):
            if match := re.search(r':creature:(\d+)$', key):
                refusals[int(match[1])].add(finding['code'])
    scoped_entries = behaviors(scoped)
    for entry in sorted(old):
        missing = old[entry] - new[entry]
        if not missing:
            continue
        row = templates.get(entry)
        if row is None:
            cause = 'No vmangos creature template'
            name = f'Entry {entry}'
        else:
            name = row['name']
            if row['script_name']:
                cause = f"vmangos C++ AI `{row['script_name']}`"
            elif row['ai_name'] not in ('', 'EventAI'):
                cause = f"vmangos C++ AI `{row['ai_name']}`"
            elif entry not in events:
                cause = 'No vmangos EventAI'
            elif refusals[entry]:
                cause = 'Refused translation: ' + ', '.join(f'`{code}`' for code in sorted(refusals[entry]))
            else:
                cause = 'No vmangos EventAI for the missing behavior categories'
        content = []
        if 36 in maps[entry]:
            content.append('Deadmines, playable with dungeons Package')
        dungeon_maps = maps[entry] - {0, 1, 30, 36, 369, 449, 450, 489, 529}
        if dungeon_maps:
            content.append('Instance population ' + ', '.join(map(str, sorted(dungeon_maps))) + ', encounter support unverified')
        if entry in scoped_entries:
            content.append('Alliance starter profile')
        yield entry, name.replace('|', '\\|'), ', '.join(sorted(missing)), cause, '; '.join(content) or 'Outside current starter and Deadmines checks'


def main():
    parser = argparse.ArgumentParser(description='Compare behavior categories from exported production EventAI plans.')
    parser.add_argument('sqlite', type=Path)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('scoped', type=Path)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('--source-sha', required=True)
    args = parser.parse_args()
    with sqlite3.connect(args.sqlite.resolve().as_uri() + '?mode=ro', uri=True) as connection:
        connection.row_factory = sqlite3.Row
        connection.execute('PRAGMA trusted_schema=OFF')
        connection.execute('PRAGMA query_only=ON')
        rows = list(ledger(connection, *(json.loads(path.read_text()) for path in [args.before, args.after, args.scoped, args.manifest])))
    print('## Creature behavior lost from the imported program\n')
    print(f'Generated by `importer/scripts/creature-behavior-ledger.py` from the M4 production plans and vmangos `{args.source_sha}`, patch 10. The inputs stay local.\n')
    print('This comparison reports behavior categories present in the previous imported EventAI and its reachable relays but absent from the vmangos program. Source spell lists count as casts. Equal categories do not prove equivalent spells, speech, timing or targets; partial losses within a retained category need later conformance checks. A source script name identifies C++ behavior that this importer does not load. Refusal codes identify dropped rules, without asserting that they implement every missing classic action.\n')
    print('Deadmines marks content with a playable Package. Other Instance populations do not imply playable encounters. The Alliance marker means the previous scoped EventAI plan includes the entry. These are static comparisons, not new gameplay verification.\n')
    counts = Counter('C++ AI' if 'C++ AI' in row[3] else 'Refused translation' if row[3].startswith('Refused') else 'Absent source behavior' for row in rows)
    print(f'{len(rows)} creatures. ' + '; '.join(f'{key}: {count}' for key, count in sorted(counts.items())) + '.\n')
    print('| Entry | Name | Missing behavior | Cause | Content |\n| ---: | --- | --- | --- | --- |')
    for row in rows:
        print('| ' + ' | '.join(map(str, row)) + ' |')


if __name__ == '__main__':
    main()
