#!/usr/bin/env python3
"""Render the maintained issue assessments as a self-contained report."""

import argparse
import html
import json
import subprocess
from pathlib import Path


STATES = {
    "implementing": "In progress",
    "review": "PR review",
    "ready": "Ready",
    "verify": "Verify existing work",
    "dependency": "Dependency",
    "decision": "Design first",
    "human": "Human acceptance",
    "superseded": "Retired or shipped",
    "pending": "Audit pending",
    "done": "Done",
}


def escape(value):
    return html.escape(str(value), quote=True)


def link(url, label):
    url = url.split()[0]
    if not url.startswith("https://github.com/"):
        return escape(label)
    return f'<a href="{escape(url)}">{escape(label)}</a>'


def score(issue):
    return round(issue["importance"] * issue["gain"] / issue["effort"], 1)


def order(issue):
    state = issue["disposition"]
    band = {
        "implementing": 0, "review": 1, "ready": 2, "verify": 3,
        "dependency": 4, "decision": 5, "human": 6,
        "superseded": 7, "pending": 8, "done": 9,
    }[state]
    urgent = issue["importance"] == 5
    return band, not urgent, -score(issue), -issue["importance"], issue["number"]


def issue_row(issue, rank):
    state = issue["disposition"]
    pending = state == "pending"
    values = "<td>?</td>" * 4 if pending else "".join(
        f'<td class="number">{value}</td>' for value in (
            issue["importance"], issue["gain"], issue["effort"], score(issue)
        )
    )
    evidence = "".join(
        f"<li>{link(item, item) if item.startswith('https://') else escape(item)}</li>"
        for item in issue["evidence"]
    )
    blockers = ""
    if issue["blockers"]:
        blockers = '<p>Blocked by</p><ul>' + "".join(
            f"<li>{link(item, item) if item.startswith('https://') else escape(item)}</li>"
            for item in issue["blockers"]
        ) + "</ul>"
    prs = " ".join(
        link(f"https://github.com/LyraCoreProject/LyraCore/pull/{number}", f"PR #{number}")
        for number in issue["related_prs"]
    )
    summary = issue["summary"]
    details = (
        f'<details><summary>{escape(summary)}</summary><p>{escape(issue["rationale"])}</p>'
        f'<p>Confidence: {escape(issue["confidence"])}. Issue updated '
        f'{escape(issue["updatedAt"][:10])}.</p>{blockers}<ul>{evidence}</ul>'
        f'<p>{prs}</p></details>'
    )
    return (
        f'<tr data-state="{state}" data-rank="{rank}" '
        f'data-score="{0 if pending else score(issue)}">'
        f'<td class="number">{rank}</td><td>'
        f'{link(issue["url"], "#" + str(issue["number"]))}{details}</td>'
        f'{values}<td>{STATES[state]}<br>{escape(issue["owner"])}</td>'
        f'<td>{escape(issue["next_action"])}</td></tr>'
    )


def render(queue):
    issues = sorted(queue["issues"], key=order)
    assessed = sum(issue["disposition"] != "pending" for issue in issues)
    rows = "".join(issue_row(issue, rank) for rank, issue in enumerate(issues, 1))
    options = "".join(
        f'<option value="{state}">{label}</option>' for state, label in STATES.items()
    )
    pulls = "".join(
        f'<tr><td>{link(pr["url"], "#" + str(pr["number"]))}</td>'
        f'<td>{escape(pr["title"])}</td><td>{escape(pr["owner"])}</td>'
        f'<td>{escape(pr["status"])}</td><td>{escape(pr["next_action"])}</td></tr>'
        for pr in queue["pull_requests"]
    )
    events = "".join(
        f'<li><time>{escape(event["at"])}</time> {escape(event["text"])}</li>'
        for event in queue["history"][-12:][::-1]
    )
    focus = "".join(f'<li>{escape(item)}</li>' for item in queue["focus"])
    return f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>LyraCore issue order and delivery</title>
<style>
:root{{color-scheme:light;font:15px/1.5 system-ui,sans-serif;color:#18232d;background:#fff}}
body{{margin:0 auto;padding:24px;max-width:1500px}}h1{{font-size:1.65rem;margin:0 0 10px}}
h2{{font-size:1.15rem;margin:28px 0 10px}}p{{max-width:105ch;margin:8px 0}}
a{{color:#075b9f}}a:hover{{text-decoration-thickness:2px}}table{{border-collapse:collapse;width:100%}}
th,td{{text-align:left;vertical-align:top;padding:9px 10px;border-bottom:1px solid #cad3da}}
th{{background:#edf2f5;font-size:.85rem}}td{{font-size:.9rem}}.number{{font-variant-numeric:tabular-nums;text-align:right}}
#issues td:nth-child(2){{min-width:240px;width:29%}}#issues td:last-child{{min-width:245px;width:29%}}
details summary{{cursor:pointer;margin-top:4px;font-weight:600}}details p,details li{{font-weight:400;font-size:.85rem}}
details ul{{padding-left:18px;overflow-wrap:anywhere}}.scroll{{overflow-x:auto}}
.controls{{display:flex;gap:16px;flex-wrap:wrap;align-items:end;margin:16px 0}}label{{display:flex;flex-direction:column;gap:4px}}
input,select,button{{font:inherit;padding:6px 8px;background:#fff;border:1px solid #657784;border-radius:0}}
[hidden]{{display:none!important}}
input{{width:min(320px,70vw)}}button{{cursor:pointer}}nav{{display:flex;gap:18px;margin:16px 0}}
time{{font-variant-numeric:tabular-nums}}li{{margin:5px 0}}.notice{{border-left:3px solid #926514;padding-left:12px}}
@media(max-width:650px){{body{{padding:14px}}h1{{font-size:1.35rem}}th,td{{padding:7px}}}}
@media print{{.controls{{display:none}}body{{max-width:none;padding:0;font-size:10pt}}.scroll{{overflow:visible}}a{{color:inherit}}}}
</style></head><body>
<header><h1>LyraCore issue order and delivery</h1>
<p>Owner: Codex in the triage and delivery thread. Updated <time>{escape(queue["updated_at"])}</time>.
{assessed} of {len(issues)} issues assessed. Source revision {escape(queue["base_revision"])}.</p>
<p>{escape(queue["status_note"])}</p></header>
<nav aria-label="Report sections"><a href="#order">Issue order</a><a href="#reviews">PRs</a><a href="#maintenance">Ownership</a><a href="#changes">Updates</a></nav>
<section><h2>Current work</h2><ul>{focus}</ul></section>
<section><h2>How I set the order</h2>
<p>Importance measures the cost of leaving the problem open. Gain measures reach and the work it unlocks.
Both use 1 to 5, with 5 highest. Effort uses 1 for a small local change, 3 for a substantial slice,
and 5 for several dependent Tickets. Return is importance × gain ÷ effort. These are estimates, not measured benefits.</p>
<p>Work in progress and PR review come first. Ready issues follow, with importance 5 before the return score.
Existing-work verification, dependencies, decisions and human acceptance follow in separate groups.
Retired or shipped proposals stay visible until their GitHub issues are resolved. An importance score remains high even when execution is blocked.</p>
<p>Open an issue summary for its reasoning, evidence and blockers. GitHub remains the source of truth for issue and PR state.</p></section>
<section id="order"><h2>Issue order</h2>
<div class="controls" hidden id="controls">
<label>Find an issue<input type="search" id="search" placeholder="Number, topic or owner"></label>
<label>Work state<select id="state"><option value="all">All states</option>{options}</select></label>
<label>Order<select id="sort"><option value="rank">Recommended order</option><option value="score">Estimated return</option></select></label>
<button id="reset" type="button">Reset</button><span id="count" role="status"></span></div>
<div class="scroll"><table id="issues"><thead><tr><th scope="col">Order</th><th scope="col">Issue and assessment</th>
<th scope="col">Importance</th><th scope="col">Gain</th><th scope="col">Effort</th><th scope="col">Return</th>
<th scope="col">State and owner</th><th scope="col">Next action</th></tr></thead><tbody>{rows}</tbody></table></div></section>
<section id="reviews"><h2>PRs to completion</h2>
<p>Each implementer owns their PR until it merges or has a named blocker. Codex checks CI, CodeRabbit reviews,
human comments and unresolved threads after each push. Accepted findings get a fix and fresh checks; disputed findings get a reasoned response.
Rebase before opening a PR and recheck the reviewed commit before merge.</p>
<div class="scroll"><table><thead><tr><th>PR</th><th>Change</th><th>Owner</th><th>State</th><th>Next action</th></tr></thead><tbody>{pulls}</tbody></table></div></section>
<section id="maintenance"><h2>Ownership and updates</h2>
<p>This report is the working dispatch list. Codex updates the same published URL after triage, assignment,
PR creation, review changes and merge. Scores change when new evidence changes the expected benefit or effort.
New issues enter as audit pending. A merged PR does not satisfy an outstanding real-client acceptance check.</p>
<p>{escape(queue["monitoring_note"])}</p>
<p>Production changes require a named host and explicit authorization. Work that crosses section 1 of
<a href="https://github.com/LyraCoreProject/LyraCore/blob/main/docs/danger-zones.md">danger-zones.md</a>
needs human review before shipping. Product UI changes follow the repository's mock-selection requirement.</p></section>
<section id="changes"><h2>Recent updates</h2><ul>{events}</ul></section>
<script>
const body=document.querySelector('#issues tbody');
const rows=Array.from(body.rows);
const search=document.querySelector('#search');
const state=document.querySelector('#state');
const sort=document.querySelector('#sort');
function update(){{
 const query=search.value.toLowerCase().trim();let count=0;
 rows.sort((a,b)=>sort.value==='score'?Number(b.dataset.score)-Number(a.dataset.score)||Number(a.dataset.rank)-Number(b.dataset.rank):Number(a.dataset.rank)-Number(b.dataset.rank));
 for(const row of rows){{row.hidden=!((state.value==='all'||state.value===row.dataset.state)&&row.textContent.toLowerCase().includes(query));if(!row.hidden)count++;body.append(row);}}
 document.querySelector('#count').textContent=count+' of '+rows.length+' issues';
}}
search.addEventListener('input',update);state.addEventListener('change',update);sort.addEventListener('change',update);
document.querySelector('#reset').addEventListener('click',()=>{{search.value='';state.value='all';sort.value='rank';update();}});
document.querySelector('#controls').hidden=false;update();
</script></body></html>'''


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, default=root / "docs/triage/issues.json")
    parser.add_argument("--output", type=Path, default=root / "docs/triage/report.html")
    parser.add_argument("--publish", action="store_true", help="Update the existing Postplan draft")
    args = parser.parse_args()
    queue = json.loads(args.input.read_text())
    numbers = [issue["number"] for issue in queue["issues"]]
    if len(numbers) != len(set(numbers)):
        raise ValueError("Each issue must have exactly one assessment")
    document = render(queue)
    if len(document.encode()) > 512 * 1024:
        raise ValueError("Report exceeds Postplan's 512 KB limit")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(document)
    print(f"Rendered {len(numbers)} issues to {args.output}")
    if args.publish:
        publisher = root / "docs/triage/node_modules/.bin/postplan"
        if not publisher.is_file():
            parser.error("Install the pinned publisher with: npm ci --prefix docs/triage --ignore-scripts")
        subprocess.run(
            [str(publisher), "upload", str(args.output.resolve()),
             "--draft", queue["postplan_draft"]],
            check=True,
        )


if __name__ == "__main__":
    main()
