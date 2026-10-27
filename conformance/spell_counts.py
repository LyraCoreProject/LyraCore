import argparse
from contextlib import closing
import json
from pathlib import Path
import sqlite3
import sys


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
AURA_EFFECTS = {6, 27, 35, 119}
SCRIPT_NAMES = {
    'Holy Light', 'Flash of Light', 'Life Tap', 'Charge', 'Blink', 'Seal of Righteousness',
    'Retaliation', 'Battle Stance', 'Defensive Stance', 'Berserker Stance', 'Bear Form',
    'Dire Bear Form', 'Cat Form', 'Judgement', 'Sinister Strike', 'Backstab', 'Gouge', 'Garrote',
    'Resurrection', 'Redemption', 'Rebirth', 'Ancestral Spirit', 'Pick Pocket',
    'Create Healthstone', 'Create Healthstone (Minor)', 'Create Healthstone (Lesser)',
    'Create Healthstone (Greater)', 'Create Healthstone (Major)', 'Soulstone Resurrection',
}


def effect_supported(name, effect, aura, target_a, target_b, misc, effects, auras):
    if effect == 108 and misc == 21:
        return True
    if effect in AURA_EFFECTS or aura != 0:
        mapped = aura in auras
    else:
        mapped = effect in effects or (effect == 5 and 9 in (target_a, target_b))
    return mapped or name in SCRIPT_NAMES or (name == 'Stealth' and aura == 16)


def latest(connection, table, key, columns):
    return connection.execute(f'''
        SELECT {','.join('s.' + column for column in columns)} FROM {table} s
        JOIN (SELECT {key}, MAX(build) AS build FROM {table}
              WHERE build <= ? GROUP BY {key}) chosen
        ON s.{key} = chosen.{key} AND s.build = chosen.build
    ''', (BUILD,))


def counts(connection, supported_effects, supported_auras):
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

    def supported(spell):
        name, slots = spells[spell]
        slots = [slot for slot in slots if slot[0] != 0]
        return bool(slots) and all(effect_supported(name, effect, aura, a, b, misc,
                                                   supported_effects, supported_auras)
                                   for effect, aura, _, a, b, misc in slots)

    return [{'class': name, 'trainable': len(trainable[class_id]),
             'supported': sum(supported(spell) for spell in trainable[class_id])}
            for class_id, (name, _) in CLASSES.items()]


def locate(explicit, root):
    if explicit is not None:
        return explicit if explicit.is_file() else None, str(explicit)
    candidates = sorted(root.rglob('mangos.sqlite'))
    if len(candidates) > 1:
        raise ValueError('multiple mangos.sqlite files; select one with --vmangos')
    return (candidates[0] if candidates else None), str(root / '**' / 'mangos.sqlite')


def read(path, supported_effects, supported_auras):
    with closing(sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True)) as connection:
        connection.execute('PRAGMA query_only = ON')
        return counts(connection, supported_effects, supported_auras)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--vmangos', type=Path)
    args = parser.parse_args()
    capabilities = json.load(sys.stdin)
    path, expected = locate(args.vmangos, Path.home() / '.cache/lyracore-scratch/vmangos-db')
    if path is None:
        print(f'\nTrainable spells: no data; expected {expected}')
        return
    rows = read(path, set(capabilities['effects']), set(capabilities['auras']))
    print(f'\nTrainable spells: build {BUILD}; {path}')
    print('Active class-trainer offers plus automatic class-skill learning; teaching spells resolved, ranks counted separately.')
    print('Supported means every nonempty effect and aura kind is supported; this does not establish spell conformance.')
    print('\n| Class | Trainable | Supported kinds | Unsupported kinds |')
    print('| --- | ---: | ---: | ---: |')
    for row in rows:
        print(f"| {row['class']} | {row['trainable']} | {row['supported']} | {row['trainable'] - row['supported']} |")


if __name__ == '__main__':
    main()
