#!/usr/bin/env python3
"""Claim and release issues by comment, and release stale claims (CONTRIBUTING.md, "Picking an issue")."""

import json
import os
import re
import subprocess
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from urllib.parse import quote


READY = "ready-for-agent"
IN_PROGRESS = "in progress"
CLAIM_BOT = "github-actions[bot]"
MAINTAINER_ASSOCIATIONS = frozenset({"OWNER", "MEMBER", "COLLABORATOR"})
MAX_CLAIMS = 2
CLAIM_LIFETIME = timedelta(hours=24)
LOGIN = re.compile(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?")
CLOSING_REFERENCE = re.compile(
    r"\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\b:?[ \t]+#(\d+)\b", re.IGNORECASE
)

ASSIGN = "assign"
UNASSIGN = "unassign"
ADD_LABEL = "add_label"
REMOVE_LABEL = "remove_label"
COMMENT = "comment"


@dataclass(frozen=True)
class Action:
    kind: str
    value: str


@dataclass(frozen=True)
class CommentFacts:
    body: str
    author: str
    author_type: str
    association: str
    issue_number: int
    is_pull_request: bool
    is_open: bool
    labels: tuple
    assignees: tuple
    author_claims: tuple


@dataclass(frozen=True)
class ExpiryFacts:
    issue_number: int
    labels: tuple
    assignees: tuple
    claimed_at: datetime
    claimed_by: str
    linked_pulls: tuple
    now: datetime


def command_of(body):
    first_line = body.split("\n", 1)[0].strip().lower()
    return first_line if first_line in ("/claim", "/release") else None


def closing_numbers(body):
    return {int(number) for number in CLOSING_REFERENCE.findall(body or "")}


def mentions(logins):
    return ", ".join(f"@{login}" for login in logins)


def decide_comment(facts):
    if (
        facts.is_pull_request
        or facts.author_type == "Bot"
        or not LOGIN.fullmatch(facts.author)
    ):
        return []
    command = command_of(facts.body)
    if command == "/claim":
        return decide_claim(facts)
    if command == "/release":
        return decide_release(facts)
    return []


def reply(facts, text):
    return [Action(COMMENT, f"@{facts.author} {text}")]


def decide_claim(facts):
    number = facts.issue_number
    if not facts.is_open:
        return reply(facts, "This issue is closed, so it cannot be claimed.")
    if facts.author in facts.assignees:
        return reply(
            facts,
            f"You already hold this issue. Open a draft pull request that says `Closes #{number}`.",
        )
    if facts.assignees:
        return reply(
            facts,
            f"This issue is held by {mentions(facts.assignees)}. "
            f"Pick another issue with the `{READY}` label.",
        )
    if IN_PROGRESS in facts.labels:
        return reply(
            facts,
            f"This issue is in progress with a maintainer. Pick another issue with the `{READY}` label.",
        )
    if READY not in facts.labels:
        return reply(
            facts,
            f"This issue cannot be claimed: it has no `{READY}` label. "
            f"Pick an issue that has it.",
        )
    if len(facts.author_claims) >= MAX_CLAIMS:
        held = ", ".join(f"#{held}" for held in facts.author_claims)
        return reply(
            facts,
            f"You already hold {len(facts.author_claims)} claims ({held}). "
            f"Open a pull request for one, or comment `/release` on it, then claim again.",
        )
    return [
        Action(ASSIGN, facts.author),
        Action(ADD_LABEL, IN_PROGRESS),
        *reply(
            facts,
            f"Claimed. Open a draft pull request that says `Closes #{number}` within 24 hours, "
            f"or the claim ends. Comment `/release` to give it up.",
        ),
    ]


def decide_release(facts):
    if not facts.is_open:
        return reply(facts, "This issue is closed, so there is nothing to release.")
    if not facts.assignees and IN_PROGRESS not in facts.labels:
        return reply(facts, "Nobody holds this issue.")
    is_holder = facts.author in facts.assignees
    if not is_holder and facts.association not in MAINTAINER_ASSOCIATIONS:
        return reply(facts, "Only the holder or a maintainer can release this issue.")
    return [
        *(Action(UNASSIGN, login) for login in facts.assignees),
        *([Action(REMOVE_LABEL, IN_PROGRESS)] if IN_PROGRESS in facts.labels else []),
        *reply(facts, "Released. Anyone can claim this issue with `/claim`."),
    ]


def decide_expiry(facts):
    if (
        IN_PROGRESS not in facts.labels
        or not facts.assignees
        or facts.claimed_by != CLAIM_BOT
        or facts.linked_pulls
        or facts.now - facts.claimed_at <= CLAIM_LIFETIME
    ):
        return []
    return [
        *(Action(UNASSIGN, login) for login in facts.assignees),
        Action(REMOVE_LABEL, IN_PROGRESS),
        Action(
            COMMENT,
            f"Released {mentions(facts.assignees)}: the claim is older than 24 hours "
            f"and no open pull request says `Closes #{facts.issue_number}`. "
            f"Comment `/claim` to take this issue again.",
        ),
    ]


class Github:
    def __init__(self, repository):
        self.repository = repository

    def api(self, *args):
        result = subprocess.run(
            ["gh", "api", *args], check=True, capture_output=True, text=True
        )
        return result.stdout


def json_lines(text):
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def list_issues(gh, *fields, jq):
    """Open issues (never pull requests) matching `fields`, projected by `jq`."""
    query = [item for name, value in fields for item in ("-f", f"{name}={value}")]
    return json_lines(
        gh.api(
            "--paginate", "--method", "GET", f"repos/{gh.repository}/issues",
            "-f", "state=open", "-f", "per_page=100", *query,
            "--jq", f".[] | select(.pull_request == null) | {jq}",
        )
    )


def perform(gh, number, actions):
    base = f"repos/{gh.repository}/issues/{number}"
    for action in actions:
        if action.kind == ASSIGN:
            assigned = json.loads(
                gh.api(
                    "--method", "POST", f"{base}/assignees", "-f", f"assignees[]={action.value}",
                    "--jq", "[.assignees[].login]",
                )
            )
            if action.value not in assigned:
                raise RuntimeError(f"GitHub did not assign {action.value} to #{number}")
        elif action.kind == UNASSIGN:
            gh.api("--method", "DELETE", f"{base}/assignees", "-f", f"assignees[]={action.value}")
        elif action.kind == ADD_LABEL:
            gh.api("--method", "POST", f"{base}/labels", "-f", f"labels[]={action.value}")
        elif action.kind == REMOVE_LABEL:
            gh.api("--method", "DELETE", f"{base}/labels/{quote(action.value)}")
        elif action.kind == COMMENT:
            gh.api("--method", "POST", f"{base}/comments", "-f", f"body={action.value}")
        else:
            raise ValueError(f"unknown action {action.kind}")


def handle_comment(gh, env):
    body = env["COMMENT_BODY"]
    author = env["COMMENT_AUTHOR"]
    number = int(env["ISSUE_NUMBER"])
    if command_of(body) is None:
        return
    issue = json.loads(
        gh.api(
            f"repos/{gh.repository}/issues/{number}",
            "--jq",
            "{open: (.state == \"open\"), pull_request: (.pull_request != null),"
            " labels: [.labels[].name], assignees: [.assignees[].login]}",
        )
    )
    claims = ()
    if command_of(body) == "/claim" and LOGIN.fullmatch(author):
        claims = tuple(
            issue_number
            for issue_number in list_issues(
                gh, ("assignee", author), ("labels", IN_PROGRESS), jq=".number"
            )
            if issue_number != number
        )
    facts = CommentFacts(
        body=body,
        author=author,
        author_type=env["COMMENT_AUTHOR_TYPE"],
        association=env["COMMENT_ASSOCIATION"],
        issue_number=number,
        is_pull_request=issue["pull_request"],
        is_open=issue["open"],
        labels=tuple(issue["labels"]),
        assignees=tuple(issue["assignees"]),
        author_claims=claims,
    )
    perform(gh, number, decide_comment(facts))


def parse_time(text):
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


def handle_expiry(gh, now):
    linked = {}
    pulls = json_lines(
        gh.api(
            "--paginate", "--method", "GET", f"repos/{gh.repository}/pulls",
            "-f", "state=open", "-f", "per_page=100",
            "--jq", ".[] | {number, body}",
        )
    )
    for pull in pulls:
        for issue_number in closing_numbers(pull["body"]):
            linked.setdefault(issue_number, []).append(pull["number"])
    for issue in list_issues(
        gh, ("labels", IN_PROGRESS), jq="{number, labels: [.labels[].name], assignees: [.assignees[].login]}"
    ):
        events = json_lines(
            gh.api(
                "--paginate", f"repos/{gh.repository}/issues/{issue['number']}/events",
                "--jq",
                f".[] | select(.event == \"labeled\" and .label.name == \"{IN_PROGRESS}\")"
                " | {at: .created_at, by: (.actor.login // \"\")}",
            )
        )
        if not events:
            continue
        facts = ExpiryFacts(
            issue_number=issue["number"],
            labels=tuple(issue["labels"]),
            assignees=tuple(issue["assignees"]),
            claimed_at=parse_time(events[-1]["at"]),
            claimed_by=events[-1]["by"],
            linked_pulls=tuple(linked.get(issue["number"], ())),
            now=now,
        )
        perform(gh, issue["number"], decide_expiry(facts))


def main(env, gh, now):
    if env["GITHUB_EVENT_NAME"] == "issue_comment":
        handle_comment(gh, env)
    else:
        handle_expiry(gh, now)


if __name__ == "__main__":
    main(
        os.environ,
        Github(os.environ["GITHUB_REPOSITORY"]),
        datetime.now(timezone.utc),
    )
