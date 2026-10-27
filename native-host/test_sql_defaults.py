import unittest

from sql_defaults import insert_defaults


class SpellDefaultsTest(unittest.TestCase):
    def test_older_broadcasts_are_silent_without_overwriting_explicit_sound(self):
        query = "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'Greeting')"
        self.assertEqual(insert_defaults(query), "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'Greeting',0)")
        explicit = "INSERT INTO game_creature_ai_broadcast_text (id,sound_id) VALUES (1,42)"
        self.assertEqual(insert_defaults(explicit), explicit)

    def test_old_spell_inserts_keep_names_and_explicit_behavior(self):
        query = "INSERT INTO game_spell (spell_id,name,custom_flags) VALUES (1,'a); ''b',8),(2,'c',16);\nINSERT INTO game_spell_effect (id,kind) VALUES (4,2);SELECT * FROM game_spell"
        expected = "INSERT INTO game_spell (spell_id,name,custom_flags,min_target_level,single_target_key) VALUES (1,'a); ''b',8,0,0),(2,'c',16,0,0);\nINSERT INTO game_spell_effect (id,kind,bonus_coefficient) VALUES (4,2,-1.0);SELECT * FROM game_spell"
        self.assertEqual(insert_defaults(query), expected)
        self.assertEqual(insert_defaults(expected), expected)

    def test_unrelated_statements_are_unchanged(self):
        for query in ["DELETE FROM game_spell", "INSERT INTO game_spell_chain (spell_id) VALUES (1)", "SELECT ';' FROM game_spell"]:
            self.assertEqual(insert_defaults(query), query)

    def test_legacy_creature_catalogues_get_silent_sound_and_male_gender_defaults(self):
        query = "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'Greeting');INSERT INTO game_creature_display (id,display_id) VALUES (2,100)"
        expected = "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'Greeting',0);INSERT INTO game_creature_display (id,display_id,gender,bounding_radius) VALUES (2,100,0,0.389)"
        self.assertEqual(insert_defaults(query), expected)
