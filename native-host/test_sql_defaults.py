import unittest

from sql_defaults import spell_defaults


class SpellDefaultsTest(unittest.TestCase):
    def test_old_spell_inserts_keep_names_and_explicit_behavior(self):
        query = "INSERT INTO game_spell (spell_id,name,custom_flags) VALUES (1,'a); ''b',8),(2,'c',16);\nINSERT INTO game_spell_effect (id,kind) VALUES (4,2);SELECT * FROM game_spell"
        expected = "INSERT INTO game_spell (spell_id,name,custom_flags,min_target_level,single_target_key) VALUES (1,'a); ''b',8,0,0),(2,'c',16,0,0);\nINSERT INTO game_spell_effect (id,kind,bonus_coefficient) VALUES (4,2,-1.0);SELECT * FROM game_spell"
        self.assertEqual(spell_defaults(query), expected)
        self.assertEqual(spell_defaults(expected), expected)

    def test_unrelated_statements_are_unchanged(self):
        for query in ["DELETE FROM game_spell", "INSERT INTO game_spell_chain (spell_id) VALUES (1)", "SELECT ';' FROM game_spell"]:
            self.assertEqual(spell_defaults(query), query)
