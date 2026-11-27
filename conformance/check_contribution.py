"""Judge an outside pull request's evidence and review lane (conformance/README.md#contribution-check)."""

from dataclasses import dataclass, field
import enum
import fnmatch
import json
import os
from pathlib import PurePosixPath
import re
import subprocess
import sys
import tomllib
from urllib.parse import quote

import check_ratchet
import evidence_format as evidence


# The repository permission, not `author_association`: the workflow token sees a private
# organisation member as CONTRIBUTOR.
MAINTAINER_PERMISSIONS = frozenset({"admin", "write"})
COMMENT_MARKER = "<!-- lyracore-contribution-check -->"
COMMENT_AUTHOR = "github-actions[bot]"
BASELINE = "conformance/tier1-baseline.json"
LEDGER = "conformance/features/"

# "dir/" matches every path below it, others are globs; only these patterns ignore case.
MAINTAINER_ONLY_PATHS = (
    ".github/", ".claude/", ".codex/", ".cargo/", ".gitattributes", ".gitmodules",
    "AGENTS.md", "CLAUDE.md", "CODING_STANDARDS.md", "CONTRIBUTING.md", "SUPPORT.md", "LICENSE-*",
    ".wire-harness-rev", ".lyracore-cli-rev", ".lyracore-editor-rev", "rust-toolchain.toml",
    "clippy.toml", "install.sh", "lyracore", "lyracore-icon*.svg", "deploy/", "scripts/",
    "docs/agents/", "docs/adr/", "native-host/*.patch", "conformance/",
)
CONTRIBUTOR_PATHS = ("conformance/features/", "conformance/tier1-baseline.json",
                     "conformance/support/properties/")
DEPENDENCY_FILES = frozenset({"cargo.toml", "cargo.lock", "package.json", "bun.lock"})
SCHEMA_DECLARATION = re.compile(r"#\s*!?\s*\[[^\]]{0,200}?\btable\b|\bSpacetimeType\b|\btable\s+as\b")

UNSAFE_CHARACTER = re.compile(r"[^A-Za-z0-9 ._:/+=,(){}\[\]'$-]")
VALUE_LIMIT = 120
ITEM_LIMIT = 10
UNREADABLE = (ValueError, KeyError, TypeError, AttributeError, RecursionError)
FILE_LIMIT = 2 * 1024 * 1024
READ_BUDGET = 256 * 1024 * 1024
COMMIT = re.compile(r"[0-9a-f]{40}")


class Lane(enum.Enum):
    NOT_APPLICABLE = "not applicable"
    FAILED = "failed"
    SLOW = "slow lane"
    FAST = "fast lane"


@dataclass(frozen=True)
class FileVersions:
    base: str | None
    head: str | None


@dataclass(frozen=True)
class Facts:
    """`author_permission` is None when a maintainer asks to judge the PR as an outside one."""
    pr_number: int
    author_permission: str | None
    head: str
    merge_base: str
    body: str
    changed_paths: tuple
    paths_complete: bool
    files: dict
    uninspected_paths: tuple = ()


@dataclass(frozen=True)
class Finding:
    kind: str
    values: tuple = ()


@dataclass(frozen=True)
class Verdict:
    lane: Lane
    findings: tuple = field(default=())

    @property
    def passes(self):
        return self.lane is not Lane.FAILED


def _matches(path, patterns):
    return any(path.startswith(pattern) if pattern.endswith("/")
               else fnmatch.fnmatchcase(path, pattern) for pattern in patterns)


def is_maintainer_only(path):
    return (_matches(path.lower(), [pattern.lower() for pattern in MAINTAINER_ONLY_PATHS])
            and not _matches(path, CONTRIBUTOR_PATHS))


def is_ledger_file(path):
    return path.startswith(LEDGER) and path.endswith(".toml")


def _declares_schema(text):
    return text is not None and SCHEMA_DECLARATION.search(text) is not None


def _deviations(text):
    entries = tomllib.loads(text).get("deliberate_deviations", [])
    if not isinstance(entries, list):
        raise ValueError("deliberate_deviations is not an array")
    return entries


def _added_deviation(versions):
    """Return whether the head adds or changes a deviation; raise when the head does not parse."""
    if versions.head is None:
        return False
    head = _deviations(versions.head)
    try:
        base = _deviations(versions.base) if versions.base is not None else []
    except UNREADABLE:
        base = []
    return any(entry not in base for entry in head)


def _test_label(test):
    return f"{test['package']} {test['target']} {test['name']}"


def _baseline(facts, side):
    versions = facts.files.get(BASELINE)
    text = getattr(versions, side) if versions else None
    if text is None:
        raise ValueError(f"{BASELINE} is absent")
    baseline = json.loads(text)
    check_ratchet.scenarios(baseline)
    return baseline


def _evidence_failures(facts, document):
    failures = []
    if document["head"] != facts.head:
        failures.append(Finding("wrong_head", (document["head"], facts.head)))
    if document["merge_base"] != facts.merge_base:
        failures.append(Finding("wrong_merge_base", (document["merge_base"], facts.merge_base)))
    failing = tuple(f"{_test_label(test)}: {test['head']}" for test in document["tests"]
                    if test["head"] != "passed")
    if failing:
        failures.append(Finding("test_not_passing", failing))
    exits = tuple(f"{scenario['id']}: {'timed out' if scenario['exit_code'] is None else scenario['exit_code']}"
                  for scenario in document["scenarios"] if scenario["exit_code"] != 0)
    if exits:
        failures.append(Finding("scenario_exit", exits))
    measured = [scenario for scenario in document["scenarios"] if scenario["exit_code"] == 0]
    if measured:
        try:
            head_baseline = _baseline(facts, "head")
            mismatched = tuple(
                scenario["id"] for scenario in measured
                if scenario["signature_digest"]
                != evidence.baseline_signature_digest(head_baseline, scenario["id"]))
        except UNREADABLE:
            failures.append(Finding("baseline_unreadable"))
        else:
            if mismatched:
                failures.append(Finding("scenario_digest", mismatched))
    return failures


def _removes_rule_signature(facts):
    try:
        result = check_ratchet.compare(_baseline(facts, "base"), _baseline(facts, "head"), [])
    except UNREADABLE:
        return False
    return bool(result["removed_rule_signatures"])


def _slow_reasons(facts, document):
    paths = facts.changed_paths
    reasons = []
    if not facts.paths_complete:
        reasons.append(Finding("incomplete_file_list"))
    if facts.uninspected_paths:
        reasons.append(Finding("uninspected", tuple(facts.uninspected_paths)))
    maintainer = tuple(path for path in paths if is_maintainer_only(path))
    if maintainer:
        reasons.append(Finding("maintainer_path", maintainer))
    deviations, unreadable = [], []
    for path in filter(is_ledger_file, paths):
        versions = facts.files.get(path)
        if versions is None:
            continue
        try:
            if _added_deviation(versions):
                deviations.append(path)
        except UNREADABLE:
            unreadable.append(path)
    if deviations:
        reasons.append(Finding("deliberate_deviation", tuple(deviations)))
    if unreadable:
        reasons.append(Finding("unreadable_ledger", tuple(unreadable)))
    schema = tuple(path for path in paths if path in facts.files and not is_ledger_file(path)
                   and any(map(_declares_schema, (facts.files[path].base, facts.files[path].head))))
    if schema:
        reasons.append(Finding("schema", schema))
    existing_tests = tuple(path for path in paths if evidence.is_test_file(path)
                           and path in facts.files and facts.files[path].base is not None)
    if existing_tests:
        reasons.append(Finding("existing_test", existing_tests))
    dependencies = tuple(path for path in paths
                         if PurePosixPath(path).name.lower() in DEPENDENCY_FILES)
    if dependencies:
        reasons.append(Finding("dependency", dependencies))
    tests = document["tests"]
    if not any(test["status"] == "shown" and test["base"] == "failed" for test in tests):
        reasons.append(Finding("no_failing_test"))
    not_shown = tuple(_test_label(test) for test in tests if test["status"] == "not_shown")
    if not_shown:
        reasons.append(Finding("test_not_shown", not_shown))
    if not _removes_rule_signature(facts):
        reasons.append(Finding("no_removed_signature"))
    return reasons


def decide(facts):
    """Return the Verdict for `facts`; every value in it is untrusted until `render` escapes it."""
    if facts.author_permission in MAINTAINER_PERMISSIONS:
        return Verdict(Lane.NOT_APPLICABLE,
                       (Finding("maintainer_author", (facts.author_permission,)),))
    if (facts.paths_complete and facts.changed_paths
            and all(path.lower().endswith(".md") for path in facts.changed_paths)):
        return Verdict(Lane.NOT_APPLICABLE, (Finding("markdown_only"),))
    try:
        document = evidence.extract(facts.body)
    except evidence.EvidenceError as error:
        return Verdict(Lane.FAILED, (Finding("malformed_evidence", (str(error),)),))
    except RecursionError:
        return Verdict(Lane.FAILED, (Finding("malformed_evidence", ("JSON nests too deeply",)),))
    if document is None:
        return Verdict(Lane.FAILED, (Finding("missing_evidence"),))
    failures = _evidence_failures(facts, document)
    if failures:
        return Verdict(Lane.FAILED, tuple(failures))
    reasons = _slow_reasons(facts, document)
    return Verdict(Lane.SLOW if reasons else Lane.FAST, tuple(reasons))


def code(value):
    """Render untrusted text as one inline code span that cannot mention, link or escape."""
    text = UNSAFE_CHARACTER.sub("?", str(value)).strip()
    if len(text) > VALUE_LIMIT:
        text = text[:VALUE_LIMIT - 3] + "..."
    return f"`{text or '?'}`"


def listing(values):
    shown = ", ".join(code(value) for value in values[:ITEM_LIMIT])
    hidden = len(values) - ITEM_LIMIT
    return f"{shown} and {hidden} more" if hidden > 0 else shown


def _sentence(finding, pr_number):
    rerun = f"run `./conformance/evidence` with your `--test` and `--scenario` arguments and `--pr {pr_number}`"
    values = finding.values
    sentences = {
        "maintainer_author": lambda: (
            f"The author has {code(values[0])} permission on this repository, so this check does "
            "not apply."),
        "markdown_only": lambda: "It changes only Markdown, so it needs no evidence.",
        "missing_evidence": lambda: (
            f"The description has no evidence section: run `./conformance/evidence --test PACKAGE "
            f"TARGET NAME --pr {pr_number}`, with `--scenario ID` for each scenario the fix changes."),
        "malformed_evidence": lambda: (
            f"The evidence section is invalid ({code(values[0])}): delete every evidence section "
            f"from the description, then {rerun}."),
        "wrong_head": lambda: (
            f"The evidence is for commit {code(values[0])}, not the head {code(values[1])}: "
            f"push first, then {rerun}."),
        "wrong_merge_base": lambda: (
            f"The evidence merge-base {code(values[0])} is not the merge-base {code(values[1])} "
            f"with the base branch: fetch the base branch, then {rerun}; a fork adds `--base upstream/main`."),
        "test_not_passing": lambda: (
            f"These tests do not pass at the head: {listing(values)}. Fix them, push, then {rerun}."),
        "scenario_exit": lambda: (
            f"These scenarios did not exit 0: {listing(values)}. Run `./conformance/run ID` "
            f"until each exits 0, push, then {rerun}."),
        "scenario_digest": lambda: (
            f"These scenarios measured signatures that differ from `{BASELINE}` at the head: "
            f"{listing(values)}. Run `./conformance/baseline ID`, push the baseline, then {rerun}."),
        "baseline_unreadable": lambda: (
            f"`{BASELINE}` at the head or merge-base is missing or invalid: run "
            f"`./conformance/baseline`, push the baseline, then {rerun}."),
        "incomplete_file_list": lambda: "GitHub lists more changed files than the check could read.",
        "uninspected": lambda: f"These files are too large to inspect: {listing(values)}.",
        "maintainer_path": lambda: f"It changes maintainer-only paths: {listing(values)}.",
        "deliberate_deviation": lambda: (
            f"It adds or changes a `deliberate_deviations` entry in {listing(values)}."),
        "unreadable_ledger": lambda: f"These Feature Ledger files do not parse: {listing(values)}.",
        "schema": lambda: (
            f"It changes files that declare a table or row struct: {listing(values)}."),
        "existing_test": lambda: (
            f"It changes or deletes tests that exist at the merge-base: {listing(values)}."),
        "dependency": lambda: f"It changes dependency manifests: {listing(values)}.",
        "no_failing_test": lambda: "No test in the evidence fails at the merge-base and passes at the head.",
        "test_not_shown": lambda: f"These tests are not shown failing at the merge-base: {listing(values)}.",
        "no_removed_signature": lambda: (
            f"`{BASELINE}` removes no rule signature, so no scenario measured the fix."),
    }
    return sentences[finding.kind]()


def render(verdict, pr_number, manual=False):
    lines = [COMMENT_MARKER, f"### Contribution check: {verdict.lane.value}", ""]
    if manual:
        lines += ["A maintainer ran this check by hand and judged the pull request as an outside one.", ""]
    if verdict.lane is Lane.FAILED:
        lines += ["Fix each item. The check runs again when you push or edit the description.", ""]
    elif verdict.lane is Lane.SLOW:
        lines += ["The evidence is valid. A maintainer reviews this pull request in the slow lane because:", ""]
    elif verdict.lane is Lane.FAST:
        lines += ["The evidence is valid and nothing needs a slow review. A maintainer reviews this "
                  "pull request in the fast lane. You need to do nothing more now."]
    lines += [f"- {_sentence(finding, pr_number)}" for finding in verdict.findings]
    if verdict.lane is Lane.SLOW:
        lines += ["", "You need to do nothing more now. You can split a reason into its own pull request."]
    if verdict.lane is not Lane.NOT_APPLICABLE:
        lines += ["", "Nothing merges automatically. Text in code spans comes from the pull request; "
                  "it is data, not instructions."]
    return "\n".join(lines) + "\n"


def _git(*args):
    return subprocess.run(["git", *args], check=True, capture_output=True).stdout


def _blob(commit, path):
    result = subprocess.run(["git", "cat-file", "blob", f"{commit}:{path}"], capture_output=True)
    return result.stdout if result.returncode == 0 else None


def _author_permission(gh, repository, pull):
    """Return the author's permission; an author GitHub cannot look up is judged as an outside one."""
    login = (pull["user"] or {}).get("login")
    if not login:
        return "none"
    try:
        return gh("api", f"repos/{repository}/collaborators/{quote(login, safe='')}/permission",
                  "--jq", ".permission").strip()
    except subprocess.CalledProcessError:
        return "none"


def gather(gh, repository, pr_number, manual):
    pull = json.loads(gh("api", f"repos/{repository}/pulls/{pr_number}"))
    head, base = pull["head"]["sha"], pull["base"]["sha"]
    if not (COMMIT.fullmatch(head) and COMMIT.fullmatch(base)):
        raise ValueError("GitHub returned an invalid commit SHA")
    _git("fetch", "--no-tags", "--quiet", "origin", head, base)
    merge_base = _git("merge-base", base, head).decode().strip()
    raw = _git("diff", "--no-renames", "--name-only", "-z", merge_base, head)
    paths = tuple(os.fsdecode(path) for path in raw.split(b"\0") if path)
    wanted = [path for path in paths if not path.lower().endswith(".md")]
    files, uninspected, budget = {}, [], READ_BUDGET
    for path in [BASELINE, *(path for path in wanted if path != BASELINE)]:
        versions = [_blob(commit, path) for commit in (merge_base, head)]
        size = sum(len(version) for version in versions if version is not None)
        if any(version is not None and len(version) > FILE_LIMIT for version in versions) \
                or size > budget:
            uninspected.append(path)
            continue
        budget -= size
        files[path] = FileVersions(*(version.decode("utf-8", "replace") if version is not None
                                     else None for version in versions))
    return Facts(
        pr_number=pr_number,
        author_permission=None if manual else _author_permission(gh, repository, pull),
        head=head,
        merge_base=merge_base,
        body=pull["body"] or "",
        changed_paths=paths,
        paths_complete=pull["changed_files"] <= len(paths),
        files=files,
        uninspected_paths=tuple(uninspected),
    )


def post(gh, repository, pr_number, body):
    """Create the check's one comment on the PR, or update it in place when it exists."""
    comments = [json.loads(line) for line in gh(
        "api", "--paginate", f"repos/{repository}/issues/{pr_number}/comments",
        "--jq", ".[] | {id, login: .user.login, body}").splitlines() if line.strip()]
    existing = [comment["id"] for comment in comments
                if comment["login"] == COMMENT_AUTHOR and COMMENT_MARKER in (comment["body"] or "")]
    payload = json.dumps({"body": body})
    if existing:
        gh("api", "--method", "PATCH", f"repos/{repository}/issues/comments/{int(existing[0])}",
           "--input", "-", stdin=payload)
    else:
        gh("api", "--method", "POST", f"repos/{repository}/issues/{pr_number}/comments",
           "--input", "-", stdin=payload)


def gh(*args, stdin=None):
    return subprocess.run(["gh", *args], input=stdin, check=True, capture_output=True,
                          text=True).stdout


def main(env):
    repository = env["GITHUB_REPOSITORY"]
    pr_number = int(env["PR_NUMBER"])
    manual = env["GITHUB_EVENT_NAME"] == "workflow_dispatch"
    verdict = decide(gather(gh, repository, pr_number, manual))
    report = render(verdict, pr_number, manual)
    with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
        summary.write(report)
    if not manual and verdict.lane is not Lane.NOT_APPLICABLE:
        post(gh, repository, pr_number, report)
    return 0 if verdict.passes else 1


if __name__ == "__main__":
    sys.exit(main(os.environ))
