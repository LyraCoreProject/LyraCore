import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3


BUILD = 5875
CLASSES = {
    1: ('Warrior', (26, 256, 257)),
    2: ('Paladin', (184, 267, 594)),
    3: ('Hunter', (50, 51, 163)),
    4: ('Rogue', (38, 39, 40, 253, 633)),
    5: ('Priest', (56, 78, 613)),
    7: ('Shaman', (373, 374, 375)),
    8: ('Mage', (6, 8, 237)),
    9: ('Warlock', (354, 355, 593)),
    11: ('Druid', (134, 573, 574)),
}


def latest(connection, table, key, columns):
    return connection.execute(f'''
        SELECT {','.join('s.' + column for column in columns)} FROM {table} s
        JOIN (SELECT {key}, MAX(build) AS build FROM {table}
              WHERE build <= ? GROUP BY {key}) chosen
        ON s.{key} = chosen.{key} AND s.build = chosen.build
    ''', (BUILD,))


def counts(connection):
    columns = ['entry', 'name'] + [f'{field}{slot}' for slot in (1, 2, 3)
                            for field in ('effect', 'effectApplyAuraName', 'effectTriggerSpell',
                                          'effectImplicitTargetA', 'effectImplicitTargetB', 'effectMiscValue')]
    spells = {}
    for entry, name, *slots in latest(connection, 'spell_template', 'entry', columns):
        if entry in spells:
            raise ValueError(f'duplicate current spell {entry}')
        spells[entry] = (name, [tuple(slots[i:i + 6]) for i in range(0, 18, 6)])

    def learned(spell, visiting=frozenset()):
        if spell in visiting:
            raise ValueError(f'cyclic teaching spell {spell}')
        if spell not in spells:
            raise ValueError(f'missing build-{BUILD} spell {spell}')
        targets = [trigger for effect, _, trigger, *_ in spells[spell][1] if effect == 36]
        if targets:
            return set().union(*(learned(target, visiting | {spell}) for target in targets))
        return {spell}

    trainable = {class_id: set() for class_id in CLASSES}
    trainers = connection.execute('''
        SELECT c.entry, c.trainer_id, c.trainer_class FROM creature_template c
        JOIN (SELECT entry, MAX(patch) AS patch FROM creature_template
              WHERE patch <= 10 GROUP BY entry) chosen
        ON c.entry = chosen.entry AND c.patch = chosen.patch
        WHERE c.trainer_type = 0 AND c.trainer_class != 0
    ''')
    direct = {}
    templates = {}
    for table, offers in [('npc_trainer', direct), ('npc_trainer_template', templates)]:
        for entry, spell in connection.execute(f'''
            SELECT entry, spell FROM {table}
            WHERE build_min <= ? AND build_max >= ? AND reqlevel <= 60
        ''', (BUILD, BUILD)):
            offers.setdefault(entry, set()).add(spell)
    for entry, template, class_id in trainers:
        if class_id in trainable:
            for spell in direct.get(entry, set()) | templates.get(template, set()):
                trainable[class_id].update(learned(spell))
    for skill, spell, mask, learn in latest(
            connection, 'skill_line_ability', 'id',
            ['skill_id', 'spell_id', 'class_mask', 'learn_on_get_skill']):
        if learn not in (1, 2):
            continue
        for class_id, (_, lines) in CLASSES.items():
            if skill in lines and (mask == 0 or mask & (1 << (class_id - 1))):
                trainable[class_id].update(learned(spell))

    return [{'class': name, 'spells': [
        {'id': spell, 'name': spells[spell][0], 'slots': [
            {'effect': effect, 'aura': aura, 'target_a': a, 'target_b': b, 'misc': misc}
            for effect, aura, _, a, b, misc in spells[spell][1] if effect != 0]}
        for spell in sorted(trainable[class_id])]}
            for class_id, (name, _) in CLASSES.items()]


def locate(explicit, root):
    if explicit is not None:
        return explicit if explicit.is_file() else None, str(explicit)
    current = root / 'current.json'
    if not current.is_file():
        return None, str(current)
    path = root.parent.parent / json.loads(current.read_text())['sqlite_path']
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError(f'vmangos snapshot path is outside {root}')
    return path if path.is_file() else None, str(path)


def read(path):
    with closing(sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True)) as connection:
        connection.execute('PRAGMA query_only = ON')
        return counts(connection)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--vmangos', type=Path)
    args = parser.parse_args()
    path, expected = locate(args.vmangos, Path(__file__).resolve().parents[1] / '.import/vmangos')
    if path is None:
        print(json.dumps({'missing': expected}))
        return
    print(json.dumps({'path': str(path), 'classes': read(path)}))


if __name__ == '__main__':
    main()
