import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('ledger', Path(__file__).parents[1] / 'creature-behavior-ledger.py')
ledger = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ledger)


class CreatureBehaviorLedger(unittest.TestCase):
    def test_reachable_relay_behavior_belongs_to_its_creature(self):
        batches = [
            ('import_creature_ai_definitions', 'entry:7@1@1,health:20:0,100,0,once,all,ordinary,any-posture,start-relay:8:self'),
            ('import_creature_ai_relay_definitions', '8@1@parallel@map-or-instance@0,0,0,source>source,talk:10~1,0,1,source>source,start-relay:8:self'),
        ]
        self.assertEqual(ledger.behaviors(batches), {7: {'speech', 'phase or HP rules'}})

    def test_guid_rules_join_the_creature_and_script_credit_keeps_its_category(self):
        guid = (0xf130 << 48) | (7 << 24) | 123
        batches = [('import_creature_ai_definitions', f'guid:{guid}@1@1,spawn,100,0,once,all,ordinary,any-posture,script-quest-event:20:self:0:0+spawn-at-actor:8:100:0')]
        self.assertEqual(ledger.behaviors(batches), {7: {'quest credit', 'summons'}})


if __name__ == '__main__':
    unittest.main()
