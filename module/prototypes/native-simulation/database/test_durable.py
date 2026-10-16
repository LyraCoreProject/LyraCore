import unittest
import tempfile
from pathlib import Path

from run_durable import native_evidence, standalone_logs, test_counts


class EvidenceTests(unittest.TestCase):
    def test_startup_gateway_output_is_not_module_execution_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name in ("startup-relays-42.log", "startup-relays-42-gateway.log"):
                (directory / name).touch()
            self.assertEqual(standalone_logs(directory), [directory / "startup-relays-42.log"])

    def test_later_wasm_success_cannot_hide_native_fallback(self):
        with self.assertRaisesRegex(ValueError, "fell back"):
            native_evidence("LYRACORE_NATIVE_DATABASE_V1 reducer=tick_creatures\n"
                            "LYRACORE_NATIVE_DATABASE_FALLBACK: replacement uses Wasm")

    def test_a_wasm_fixture_cannot_count_as_native_execution(self):
        with self.assertRaisesRegex(ValueError, "no native"):
            native_evidence("Module published; all assertions passed")

    def test_an_unmatched_program_cannot_hide_behind_earlier_native_calls(self):
        with self.assertRaisesRegex(ValueError, "without a matching"):
            native_evidence("LYRACORE_NATIVE_DATABASE_V1 reducer=init\n"
                            "LYRACORE_NATIVE_DATABASE_WASM: no native artifact for published Wasm")

    def test_evidence_names_every_observed_reducer_once(self):
        log = "\n".join(f"LYRACORE_NATIVE_DATABASE_V1 reducer={name}"
                        for name in ("tick_creatures", "init", "tick_creatures"))
        self.assertEqual(native_evidence(log), ["init", "tick_creatures"])

    def test_filtered_out_tests_do_not_establish_parity(self):
        with self.assertRaisesRegex(ValueError, "no tests"):
            test_counts("test result: ok. 0 passed; 0 failed; 12 ignored;")

    def test_an_earlier_failure_is_not_hidden_by_a_later_pass(self):
        with self.assertRaisesRegex(ValueError, "passing"):
            test_counts("test result: FAILED. 2 passed; 1 failed;\n"
                        "test result: ok. 4 passed; 0 failed;")

    def test_multiple_test_targets_contribute_to_the_total(self):
        self.assertEqual(test_counts("test result: ok. 2 passed; 0 failed;\n"
                                     "test result: ok. 4 passed; 0 failed;"), 6)


if __name__ == "__main__":
    unittest.main()
