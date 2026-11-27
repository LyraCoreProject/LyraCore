import hashlib
import json
from pathlib import PurePosixPath
import re


# conformance/README.md#pull-request-evidence defines every field and the digest.
SCHEMA_VERSION = 1
SECTION_START = "<!-- lyracore-evidence -->"
SECTION_END = "<!-- /lyracore-evidence -->"
SECTION_SUMMARY = "Conformance evidence"

HEAD_OUTCOMES = ("passed", "failed", "did_not_compile", "not_found", "timed_out")
BASE_OUTCOMES = HEAD_OUTCOMES + ("not_run",)
SHOWN_BASE_OUTCOMES = ("failed", "did_not_compile")
LIB_TARGET = "lib"

DOCUMENT_FIELDS = {"schema_version", "merge_base", "head", "tests", "scenarios", "tools",
                   "platform"}
TEST_FIELDS = {"package", "target", "name", "features", "status", "base", "head", "reason"}
SCENARIO_FIELDS = {"id", "exit_code", "signature_digest"}
TOOLS_FIELDS = {"rustc", "spacetime"}
PLATFORM_FIELDS = {"os", "arch"}
SIGNATURE_FIELDS = {"rule", "character", "checkpoint", "stream", "opcode", "field_path"}

COMMIT = re.compile(r"[0-9a-f]{40}")
DIGEST = re.compile(r"[0-9a-f]{64}")
PACKAGE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*")
TARGET = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*")
TEST_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*")
FEATURE = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_-]*")
SCENARIO = re.compile(r"[a-z0-9_]+")
TOOL_VERSION = re.compile(r"[0-9][0-9A-Za-z.+() -]{0,99}")
PLATFORM_NAME = re.compile(r"[a-z0-9_]{1,32}")
REASON_LIMIT = 500


class EvidenceError(ValueError):
    pass


def is_test_file(path):
    """Return whether a repo-relative path is a test file the merge-base run applies."""
    parts = PurePosixPath(path).parts
    name = parts[-1] if parts else ""
    return "tests" in parts[:-1] or name == "tests.rs" or name.endswith("_tests.rs")


def test_status(base, head):
    return "shown" if base in SHOWN_BASE_OUTCOMES and head == "passed" else "not_shown"


def _signature(entry):
    required = SIGNATURE_FIELDS - {"character"}
    if not isinstance(entry, dict) or not required <= set(entry) <= SIGNATURE_FIELDS:
        raise EvidenceError(f"invalid signature {entry!r}")
    signature = {key: entry[key] for key in required}
    if entry.get("character") is not None:
        signature["character"] = entry["character"]
    return json.dumps(signature, sort_keys=True, separators=(",", ":"), ensure_ascii=True)


def signature_digest(scenario_sha256, harness_revision, rule, data):
    """Return the hex SHA-256 over a scenario's provenance and its rule and data signature sets."""
    payload = {
        "scenario_sha256": scenario_sha256,
        "harness_revision": harness_revision,
        "rule": sorted({_signature(entry) for entry in rule}),
        "data": sorted({_signature(entry) for entry in data}),
    }
    canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(canonical.encode()).hexdigest()


def baseline_signature_digest(baseline, scenario):
    """Return `scenario`'s digest from a parsed `tier1-baseline.json`; None without an entry."""
    for entry in baseline["scenarios"]:
        if entry["scenario"] == scenario:
            return signature_digest(entry["scenario_sha256"], entry["harness_revision"],
                                    entry["rule"], entry["data"])
    return None


def _fail(path, message):
    raise EvidenceError(f"{path}: {message}")


def _object(value, path, fields):
    if not isinstance(value, dict):
        _fail(path, "must be an object")
    if set(value) != fields:
        unknown = sorted(set(value) - fields)
        missing = sorted(fields - set(value))
        _fail(path, f"unknown fields {unknown}, missing fields {missing}")
    return value


def _string(value, path, pattern):
    if not isinstance(value, str) or not pattern.fullmatch(value):
        _fail(path, f"must match {pattern.pattern}")
    return value


def _choice(value, path, choices):
    if not isinstance(value, str) or value not in choices:
        _fail(path, f"must be one of {list(choices)}")
    return value


def _validate_test(test, path):
    _object(test, path, TEST_FIELDS)
    _string(test["package"], f"{path}.package", PACKAGE)
    _string(test["target"], f"{path}.target", TARGET)
    _string(test["name"], f"{path}.name", TEST_NAME)
    features = test["features"]
    if not isinstance(features, list):
        _fail(f"{path}.features", "must be an array")
    for index, feature in enumerate(features):
        _string(feature, f"{path}.features[{index}]", FEATURE)
    if features != sorted(set(features)):
        _fail(f"{path}.features", "must be sorted and unique")
    base = _choice(test["base"], f"{path}.base", BASE_OUTCOMES)
    head = _choice(test["head"], f"{path}.head", HEAD_OUTCOMES)
    status = _choice(test["status"], f"{path}.status", ("shown", "not_shown"))
    if status != test_status(base, head):
        _fail(f"{path}.status",
              f"must be {test_status(base, head)} for base {base} and head {head}")
    reason = test["reason"]
    if status == "shown":
        if reason is not None:
            _fail(f"{path}.reason", "must be null for a shown test")
    elif (not isinstance(reason, str) or not reason or len(reason) > REASON_LIMIT
          or not reason.isprintable()):
        _fail(f"{path}.reason", f"must be printable text of 1 to {REASON_LIMIT} characters")
    return (test["package"], test["target"], test["name"], tuple(features))


def _validate_scenario(scenario, path):
    _object(scenario, path, SCENARIO_FIELDS)
    _string(scenario["id"], f"{path}.id", SCENARIO)
    exit_code = scenario["exit_code"]
    if exit_code is not None and (type(exit_code) is not int or exit_code not in (0, 1, 2)):
        _fail(f"{path}.exit_code", "must be 0, 1, 2 or null")
    digest = scenario["signature_digest"]
    if exit_code in (0, 1):
        _string(digest, f"{path}.signature_digest", DIGEST)
    elif digest is not None:
        _fail(f"{path}.signature_digest", "must be null without a completed comparison")
    return scenario["id"]


def validate(document):
    """Return `document` when it is a valid evidence document; raise EvidenceError otherwise."""
    _object(document, "$", DOCUMENT_FIELDS)
    version = document["schema_version"]
    if type(version) is not int or version != SCHEMA_VERSION:
        _fail("$.schema_version", f"unsupported version {version!r}")
    _string(document["merge_base"], "$.merge_base", COMMIT)
    _string(document["head"], "$.head", COMMIT)
    for field, check in (("tests", _validate_test), ("scenarios", _validate_scenario)):
        entries = document[field]
        if not isinstance(entries, list):
            _fail(f"$.{field}", "must be an array")
        keys = [check(entry, f"$.{field}[{index}]") for index, entry in enumerate(entries)]
        if len(keys) != len(set(keys)):
            _fail(f"$.{field}", "must not repeat an entry")
    if not document["tests"] and not document["scenarios"]:
        _fail("$", "must name at least one test or scenario")
    tools = _object(document["tools"], "$.tools", TOOLS_FIELDS)
    _string(tools["rustc"], "$.tools.rustc", TOOL_VERSION)
    if tools["spacetime"] is not None:
        _string(tools["spacetime"], "$.tools.spacetime", TOOL_VERSION)
    platform = _object(document["platform"], "$.platform", PLATFORM_FIELDS)
    _string(platform["os"], "$.platform.os", PLATFORM_NAME)
    _string(platform["arch"], "$.platform.arch", PLATFORM_NAME)
    return document


def _unique_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise EvidenceError(f"duplicate field {key!r}")
        result[key] = value
    return result


def parse(text):
    """Parse and validate an evidence document from JSON text."""
    try:
        document = json.loads(text, object_pairs_hook=_unique_keys,
                              parse_constant=lambda name: _fail("$", f"invalid number {name}"))
    except json.JSONDecodeError as error:
        raise EvidenceError(f"invalid JSON: {error}") from error
    return validate(document)


def render(document):
    return json.dumps(validate(document), indent=2) + "\n"


def render_section(document):
    return "\n".join([
        SECTION_START,
        "<details>",
        f"<summary>{SECTION_SUMMARY}</summary>",
        "",
        "```json",
        render(document).rstrip("\n"),
        "```",
        "",
        "</details>",
        SECTION_END,
    ])


def _section_spans(lines):
    spans = []
    start = None
    for index, line in enumerate(lines):
        if line.strip() == SECTION_START:
            if start is not None:
                raise EvidenceError("evidence section starts twice without an end")
            start = index
        elif line.strip() == SECTION_END:
            if start is None:
                raise EvidenceError("evidence section ends without a start")
            spans.append((start, index))
            start = None
    if start is not None:
        raise EvidenceError("evidence section has no end marker")
    return spans


def extract(body):
    """Return the validated document in a pull request body, or None when it has no section."""
    lines = (body or "").replace("\r\n", "\n").split("\n")
    spans = _section_spans(lines)
    if not spans:
        return None
    if len(spans) > 1:
        raise EvidenceError("pull request body has more than one evidence section")
    start, end = spans[0]
    section = lines[start + 1:end]
    try:
        fence = section.index("```json")
        close = section.index("```", fence + 1)
    except ValueError as error:
        raise EvidenceError("evidence section has no ```json block") from error
    return parse("\n".join(section[fence + 1:close]))


def replace_section(body, document):
    """Return `body` with every evidence section removed and `document`'s section appended."""
    lines = (body or "").replace("\r\n", "\n").split("\n")
    kept = []
    previous = 0
    for start, end in _section_spans(lines):
        kept.extend(lines[previous:start])
        previous = end + 1
    kept.extend(lines[previous:])
    remainder = "\n".join(kept).strip("\n")
    section = render_section(document)
    return f"{remainder}\n\n{section}\n" if remainder else f"{section}\n"
