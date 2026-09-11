import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-playerbots-action-lifecycle.py"
SPEC = importlib.util.spec_from_file_location("check_playerbots_action_lifecycle", SCRIPT)
CHECKER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(CHECKER)


class SourceCaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "module/tests").mkdir(parents=True)

    def tearDown(self):
        self.temporary.cleanup()

    def write(self, relative: str, source: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)

    def case(self, target: str, name: str, source: str) -> dict[str, str]:
        return {"target": target, "name": name, "source": source}

    def test_exact_top_level_test_is_found(self):
        self.write("module/tests/exact.rs", '#[test]\n#[ignore = "fixture"]\nfn works() {}\n')

        self.assertTrue(
            CHECKER.source_case_exists(
                self.root, self.case("exact", "works", "module/tests/exact.rs")
            )
        )

    def test_helper_comment_and_strings_do_not_name_a_test(self):
        self.write(
            "module/tests/exact.rs",
            """
fn helper() {}
// #[test] fn commented() {}
/* outer /* nested */ #[test] fn block_commented() {} */
const TEXT: &str = "#[test] fn quoted() {}";
const RAW: &str = r#"#[test] fn raw_quoted() {}"#;
const RAW_C: &CStr = cr##"a quote " #[test] fn raw_c_quoted() {} "##;
""",
        )

        for name in [
            "helper",
            "commented",
            "block_commented",
            "quoted",
            "raw_quoted",
            "raw_c_quoted",
        ]:
            with self.subTest(name=name):
                self.assertFalse(
                    CHECKER.source_case_exists(
                        self.root, self.case("exact", name, "module/tests/exact.rs")
                    )
                )

    def test_unprefixed_name_does_not_find_a_test_inside_an_inline_module(self):
        self.write(
            "module/tests/exact.rs",
            "mod nested { #[test] fn works() {} }\n",
        )

        self.assertFalse(
            CHECKER.source_case_exists(
                self.root, self.case("exact", "works", "module/tests/exact.rs")
            )
        )

    def test_target_must_match_the_test_crate(self):
        self.write("module/tests/exact.rs", "#[test]\nfn works() {}\n")
        self.write("module/tests/other.rs", "")

        self.assertFalse(
            CHECKER.source_case_exists(
                self.root, self.case("other", "works", "module/tests/exact.rs")
            )
        )

    def test_external_module_requires_the_declared_path_and_prefix(self):
        self.write(
            "module/tests/exact.rs",
            '#[path = "exact/runner.rs"]\nmod runner;\n',
        )
        self.write("module/tests/exact/runner.rs", "#[test]\nfn works() {}\n")
        exact = self.case(
            "exact", "runner::works", "module/tests/exact/runner.rs"
        )

        self.assertTrue(CHECKER.source_case_exists(self.root, exact))
        self.assertFalse(
            CHECKER.source_case_exists(
                self.root,
                self.case("exact", "helper::works", "module/tests/exact/runner.rs"),
            )
        )

    def test_commented_or_quoted_external_module_is_not_found(self):
        self.write(
            "module/tests/exact.rs",
            """
// #[path = "exact/runner.rs"] mod runner;
const TEXT: &str = r#"#[path = "exact/runner.rs"] mod runner;"#;
""",
        )
        self.write("module/tests/exact/runner.rs", "#[test]\nfn works() {}\n")

        self.assertFalse(
            CHECKER.source_case_exists(
                self.root,
                self.case("exact", "runner::works", "module/tests/exact/runner.rs"),
            )
        )

    def test_external_module_path_ignores_a_forged_comment_literal(self):
        self.write(
            "module/tests/exact.rs",
            '#[path /* path = "exact/runner.rs" */ = "other/runner.rs"]\nmod runner;\n',
        )
        self.write("module/tests/exact/runner.rs", "#[test]\nfn works() {}\n")

        self.assertFalse(
            CHECKER.source_case_exists(
                self.root,
                self.case("exact", "runner::works", "module/tests/exact/runner.rs"),
            )
        )


if __name__ == "__main__":
    unittest.main()
