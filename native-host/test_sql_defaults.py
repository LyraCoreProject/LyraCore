import unittest

from sql_defaults import in_table_order, insert_defaults


class SpellDefaultsTest(unittest.TestCase):
    def test_older_broadcasts_are_silent_without_overwriting_explicit_sound(self):
        query = "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'Greeting')"
        self.assertEqual(insert_defaults(query), "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'Greeting',0)")
        explicit = "INSERT INTO game_creature_ai_broadcast_text (id,sound_id) VALUES (1,42)"
        self.assertEqual(insert_defaults(explicit), explicit)

    def test_every_row_of_a_batch_gets_the_defaults_and_quoted_delimiters_survive(self):
        query = "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'a); ''b'),(2,'c');\nSELECT * FROM game_spell"
        expected = "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'a); ''b',0),(2,'c',0);\nSELECT * FROM game_spell"
        self.assertEqual(insert_defaults(query), expected)
        self.assertEqual(insert_defaults(expected), expected)

    def test_unrelated_statements_are_unchanged(self):
        for query in ["DELETE FROM game_spell", "INSERT INTO game_spell_chain (spell_id) VALUES (1)", "SELECT ';' FROM game_spell"]:
            self.assertEqual(insert_defaults(query), query)

    def test_legacy_creature_catalogues_get_silent_sound_and_male_gender_defaults(self):
        query = "INSERT INTO game_creature_ai_broadcast_text (id,male_text) VALUES (1,'Greeting');INSERT INTO game_creature_display (id,display_id) VALUES (2,100)"
        expected = "INSERT INTO game_creature_ai_broadcast_text (id,male_text,sound_id) VALUES (1,'Greeting',0);INSERT INTO game_creature_display (id,display_id,gender,bounding_radius) VALUES (2,100,0,0.389)"
        self.assertEqual(insert_defaults(query), expected)


class TableOrderTest(unittest.TestCase):
    ORDER = {'game_faction': ['faction_id', 'base_standing', 'base_standing_2', 'base_race_mask_1']}

    def order(self, query):
        return in_table_order(query, self.ORDER.__getitem__)

    def test_named_columns_and_every_row_follow_the_table_order(self):
        query = "INSERT INTO game_faction (faction_id,base_race_mask_1,base_standing_2,base_standing) VALUES (1,2,1000,-42000),(3,'a,(b)',0,7)"
        self.assertEqual(self.order(query), "INSERT INTO game_faction (faction_id,base_standing,base_standing_2,base_race_mask_1) VALUES (1,-42000,1000,2),(3,7,0,'a,(b)')")

    def test_ordered_unknown_and_unrelated_statements_are_unchanged(self):
        for query in [
            "INSERT INTO game_faction (faction_id,base_standing) VALUES (1,2)",
            "INSERT INTO game_faction (faction_id,other) VALUES (1,2)",
            "SELECT * FROM game_faction",
        ]:
            self.assertEqual(self.order(query), query)
