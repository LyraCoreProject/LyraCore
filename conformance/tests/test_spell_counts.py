import importlib.util
from pathlib import Path
import sqlite3
import re
import tempfile
import unittest


spec = importlib.util.spec_from_file_location('spell_counts', Path(__file__).parents[1] / 'spell_counts.py')
spells = importlib.util.module_from_spec(spec)
spec.loader.exec_module(spells)


class SpellCountsTests(unittest.TestCase):
    def setUp(self):
        self.connection = sqlite3.connect(':memory:')
        self.addCleanup(self.connection.close)
        self.connection.executescript('''
            CREATE TABLE spell_template(entry, build, name, effect1, effectApplyAuraName1, effectTriggerSpell1,
                effectImplicitTargetA1, effectImplicitTargetB1, effectMiscValue1,
                effect2, effectApplyAuraName2, effectTriggerSpell2,
                effectImplicitTargetA2, effectImplicitTargetB2, effectMiscValue2,
                effect3, effectApplyAuraName3, effectTriggerSpell3,
                effectImplicitTargetA3, effectImplicitTargetB3, effectMiscValue3);
            CREATE TABLE creature_template(entry, patch, trainer_type, trainer_class, trainer_id);
            CREATE TABLE npc_trainer(entry, spell, reqlevel, build_min, build_max);
            CREATE TABLE npc_trainer_template(entry, spell, reqlevel, build_min, build_max);
            CREATE TABLE skill_line_ability(id, build, skill_id, spell_id, class_mask, learn_on_get_skill);
            INSERT INTO creature_template VALUES (100,10,0,8,200), (101,10,0,1,0), (102,10,3,3,201);
        ''')

    def spell(self, entry, build=5875, effect=2, aura=0, trigger=0, second=0, name="Other"):
        self.connection.execute('INSERT INTO spell_template VALUES (?,?,?,?,?,?,0,0,0,?,0,0,0,0,0,0,0,0,0,0,0)',
                                (entry, build, name, effect, aura, trigger, second))

    def totals(self):
        return {row['class']: (row['supported'], row['trainable'])
                for row in spells.counts(self.connection, {2, 6, 10}, {3, 8})}

    def test_trainers_resolve_teaching_spells_and_deduplicate_class_skill_learning(self):
        self.spell(1)
        self.spell(2, effect=36, trigger=1)
        self.spell(3, effect=36, trigger=2)
        self.spell(4, effect=10)
        self.connection.executescript('''
            INSERT INTO npc_trainer VALUES (100,3,1,0,5875), (101,4,60,0,5875);
            INSERT INTO npc_trainer_template VALUES (200,2,1,0,5875), (201,4,1,0,5875);
            INSERT INTO skill_line_ability VALUES (1,5875,6,1,128,2);
        ''')
        self.assertEqual(self.totals()['Mage'], (1, 1))
        self.assertEqual(self.totals()['Warrior'], (1, 1))
        self.assertEqual(self.totals()['Hunter'], (0, 0))

    def test_latest_build_and_every_effect_and_aura_slot_determine_support(self):
        self.spell(1, build=4000)
        self.spell(1, build=5875, effect=6, aura=8, second=999)
        self.spell(1, build=6000)
        self.spell(2, effect=6, aura=999)
        self.spell(3, effect=0)
        self.spell(4, effect=6, aura=3)
        self.connection.executemany('INSERT INTO npc_trainer VALUES (100,?,1,0,5875)',
                                    [(entry,) for entry in range(1, 5)])
        self.assertEqual(self.totals()['Mage'], (1, 4))

    def test_build_ranges_class_masks_skill_lines_and_learning_flags_limit_trainability(self):
        for entry in range(1, 9):
            self.spell(entry)
        self.connection.executescript('''
            INSERT INTO npc_trainer VALUES (100,1,1,0,5874), (100,2,1,5876,9999), (100,3,61,0,5875);
            INSERT INTO skill_line_ability VALUES
                (1,5875,6,4,1,2), (2,5875,164,5,128,2), (3,5875,6,6,128,0),
                (4,4000,6,7,128,2), (4,5875,6,7,128,0), (4,6000,6,7,128,2),
                (5,5875,6,8,0,1);
        ''')
        self.assertEqual(self.totals()['Mage'], (1, 1))
        self.assertEqual(self.totals()['Warrior'], (0, 0))

    def test_latest_creature_patch_supplies_the_trainer_class(self):
        self.spell(1)
        self.connection.executescript('''
            INSERT INTO creature_template VALUES (100,1,0,1,200), (100,11,0,1,200);
            INSERT INTO npc_trainer_template VALUES (200,1,1,0,5875);
        ''')
        self.assertEqual(self.totals()['Mage'], (1, 1))
        self.assertEqual(self.totals()['Warrior'], (0, 0))

    def test_missing_and_cyclic_teaching_spells_fail(self):
        self.spell(1, effect=36, trigger=2)
        self.connection.execute('INSERT INTO npc_trainer VALUES (100,1,1,0,5875)')
        with self.assertRaisesRegex(ValueError, 'missing'):
            self.totals()
        self.spell(2, effect=36, trigger=1)
        with self.assertRaisesRegex(ValueError, 'cyclic'):
            self.totals()

    def test_discovery_missing_data_and_read_only_open(self):
        scratch = Path.home() / '.cache/lyracore-scratch/conformance'
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            self.assertEqual(spells.locate(None, root), (None, str(root / '**/mangos.sqlite')))
            missing = root / 'missing.sqlite'
            self.assertEqual(spells.locate(missing, root), (None, str(missing)))
            with self.assertRaises(sqlite3.OperationalError):
                spells.read(missing, set(), set())
            self.assertFalse(missing.exists())
            path = root / 'nested/mangos.sqlite'
            path.parent.mkdir()
            with sqlite3.connect(path) as target:
                self.connection.backup(target)
            self.assertEqual(spells.locate(None, root)[0], path)
            before = path.read_bytes()
            self.assertEqual(len(spells.read(path, {2}, {3})), 9)
            self.assertEqual(path.read_bytes(), before)
            (root / 'mangos.sqlite').touch()
            with self.assertRaisesRegex(ValueError, 'multiple'):
                spells.locate(None, root)


class ImporterMappingTests(unittest.TestCase):
    def test_name_corrections_match_the_importer_table(self):
        source = (Path(__file__).parents[2] / 'importer/src/spell.rs').read_text()
        function = source.split('fn correct_script_effect_kind(', 1)[1].split('\nfn ', 1)[0]
        table = function.split('match name {', 1)[1]
        names = set(re.findall(r'"([^"\n]+)"', table))
        self.assertEqual(names, spells.SCRIPT_NAMES | {'Stealth'})
        self.assertIn('if name == "Stealth" && aura != AuraMod::ModStealth', source)

    def test_name_dependent_effects_and_importer_special_cases(self):
        def supported(name, effect, aura=0, a=0, b=0, misc=0):
            return spells.effect_supported(name, effect, aura, a, b, misc,
                                            {2, 59, 63, 91}, {3, 16})
        for effect, name in [(5, 'Blink'), (29, 'Blink'), (96, 'Charge')]:
            self.assertFalse(supported('Other', effect))
            self.assertTrue(supported(name, effect))
        for effect in [59, 63, 91]:
            self.assertTrue(supported('Other', effect))
        self.assertTrue(supported('Home', 5, b=9))
        self.assertFalse(supported('Stealth', 3))
        self.assertTrue(supported('Stealth', 6, aura=16))
        self.assertTrue(supported('Other', 108, misc=21))
        self.assertFalse(supported('Other', 108, misc=1))
        self.assertFalse(supported('Other', 2, aura=999))
        self.assertTrue(supported('Life Tap', 3))
