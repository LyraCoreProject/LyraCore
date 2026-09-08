#!/usr/bin/env python3
"""Render the local execution record into the published playerbots report."""

import argparse
import html
import json
import re
from pathlib import Path


START = "<!-- PLAYERBOTS_EXECUTION_START -->"
END = "<!-- PLAYERBOTS_EXECUTION_END -->"
FINISHED = {"complete", "merged"}
STATUSES = {"ready", "planned", "implementing", "review", "approved", "merged", "complete", "blocked", "waiting-for-client"}
RELEASE_OBSERVATIONS = {"imported_world", "attended_client"}


def escape(value):
    return html.escape(str(value), quote=True)


def validate(plan):
    """Refuse broken ownership and dependency graphs before publishing them."""
    spec = plan["spec"]
    rows = [spec, *plan["groups"], *plan["tickets"]]
    by_id = {row["id"]: row for row in rows}
    if len(by_id) != len(rows):
        raise ValueError("Execution identifiers must be unique")
    tickets = {row["id"]: row for row in plan["tickets"]}
    for row in rows:
        for pr in row.get("prs", []):
            if not re.fullmatch(r"https://github\.com/[\w.-]+/[\w.-]+/pull/[1-9]\d*", pr["url"]):
                raise ValueError(f"PR links must name GitHub pull requests for {row['id']}")
    for row in rows[1:]:
        if row["parent"] not in by_id:
            raise ValueError(f"Unknown parent for {row['id']}")
        seen = {row["id"]}
        current = row
        while current.get("parent"):
            current = by_id[current["parent"]]
            if current["id"] in seen:
                raise ValueError("Parent cycle")
            seen.add(current["id"])
        if current["id"] != spec["id"]:
            raise ValueError("Every row must belong to the root spec")
    visiting, visited = set(), set()

    def visit(identifier):
        if identifier in visiting:
            raise ValueError("Blocking cycle")
        if identifier in visited:
            return
        visiting.add(identifier)
        for blocker in tickets[identifier]["blocked_by"]:
            if blocker not in tickets:
                raise ValueError(f"Unknown blocker {blocker}")
            visit(blocker)
        visiting.remove(identifier)
        visited.add(identifier)

    for row in tickets.values():
        if row["status"] not in STATUSES:
            raise ValueError(f"Unknown status for {row['id']}")
        if not row["acceptance"]:
            raise ValueError(f"No acceptance criteria for {row['id']}")
        if row["id"] == "PB-012" and set(row.get("required_observations", [])) != RELEASE_OBSERVATIONS:
            raise ValueError("PB-012 must retain both release observation requirements")
        verified = {}
        for record in row.get("acceptance_evidence", []):
            criterion = record["criterion"]
            if criterion not in row["acceptance"] or criterion in verified:
                raise ValueError(f"Unknown or duplicate acceptance evidence for {row['id']}")
            if not record.get("reviewer", "").strip() or not record.get("evidence") or not all(item.strip() for item in record["evidence"]):
                raise ValueError(f"Acceptance evidence for {row['id']} needs a reviewer and sources")
            verified[criterion] = record
        if row["status"] in FINISHED:
            if set(verified) != set(row["acceptance"]):
                raise ValueError(f"Completed ticket {row['id']} needs reviewed evidence for every criterion")
            if not row.get("prs") or any(pr.get("status") != "merged" or not re.fullmatch(r"[0-9a-f]{40}", pr.get("merge_commit", "")) for pr in row["prs"]):
                raise ValueError(f"Completed ticket {row['id']} needs merged PRs with commit identities")
            for kind in row.get("required_observations", []):
                observation = row.get("observations", {}).get(kind, {})
                required = ["observer", "observed_at", "core_revision", "package_revision", "content_revision", "geometry_revision"]
                if not all(isinstance(observation.get(key), str) and observation[key].strip() for key in required):
                    raise ValueError(f"Completed ticket {row['id']} needs {kind} observation evidence")
                sources = observation.get("evidence")
                if not isinstance(sources, list) or not sources or not all(isinstance(source, str) and source.strip() for source in sources):
                    raise ValueError(f"Observation {kind} needs nonempty evidence sources")
                if kind == "attended_client" and observation.get("client_build") != "1.12.1.5875":
                    raise ValueError("Attended observation must name the real 1.12.1.5875 client")
                if kind == "imported_world" and (observation.get("minutes", 0) < 60 or observation.get("bots", 0) < 25):
                    raise ValueError("Imported-world observation needs at least 60 minutes with 25 bots")
        visit(row["id"])
        if row["status"] in {"implementing", "review", "approved", *FINISHED}:
            if any(tickets[key]["status"] not in FINISHED for key in row["blocked_by"]):
                raise ValueError(f"Ticket {row['id']} started before its blockers completed")
    return by_id, tickets


def state(ticket, tickets):
    status = ticket["status"]
    if status == "ready" and any(tickets[key]["status"] not in FINISHED for key in ticket["blocked_by"]):
        return "planned"
    return status


def paragraphs(items):
    return "".join(f"<p>{escape(item)}</p>" for item in items)


def bullets(items, ordered=False):
    tag = "ol" if ordered else "ul"
    return f"<{tag}>" + "".join(f"<li>{escape(item)}</li>" for item in items) + f"</{tag}>"


def row_detail(row, kind):
    if kind == "Spec":
        parts = ["<h3>Problem statement</h3>", paragraphs([row["problem"]]), "<h3>Solution</h3>", paragraphs([row["solution"]])]
        for title, key, ordered in [
            ("User stories", "user_stories", True),
            ("Implementation decisions", "implementation_decisions", False),
            ("Testing decisions", "testing_decisions", False),
            ("Out of scope", "out_of_scope", False),
            ("Further notes", "notes", False),
        ]:
            parts.extend([f"<h3>{title}</h3>", bullets(row[key], ordered)])
        return "".join(parts)
    if kind == "Workstream":
        return paragraphs([row.get("delivery", row["title"])])
    parts = [paragraphs([row["delivery"]]), "<h3>Acceptance criteria</h3>", bullets(row["acceptance"])]
    verified = row.get("acceptance_evidence", [])
    parts.append(paragraphs([f"Verified criteria: {len(verified)}/{len(row['acceptance'])}."]))
    for record in verified:
        parts.extend([paragraphs([record["criterion"], f"Reviewed by {record['reviewer']}."]), bullets(record["evidence"])])
    for kind in row.get("required_observations", []):
        observation = row.get("observations", {}).get(kind)
        label = kind.replace("_", " ")
        parts.extend([f"<h3>{escape(label.capitalize())} observation</h3>", bullets([f"{key.replace('_', ' ')}: {value}" for key, value in observation.items()]) if observation else paragraphs(["Pending. Required before completion."])])
    parts.append(paragraphs([f"Execution model: {row['model']}, {row.get('effort', 'high')} effort. Repositories: {', '.join(row['repo_scope'])}."]))
    if row.get("evidence"):
        parts.extend(["<h3>Recorded evidence</h3>", bullets(row["evidence"])])
    if row.get("notes"):
        parts.extend(["<h3>Current notes</h3>", bullets(row["notes"])])
    return "".join(parts)


def render(plan):
    by_id, tickets = validate(plan)
    children = {key: [] for key in by_id}
    for row in [*plan["groups"], *plan["tickets"]]:
        children[row["parent"]].append(row["id"])

    def descendants(identifier):
        found = []
        for child in children[identifier]:
            found.extend([child] if child in tickets else descendants(child))
        return found

    def table_rows(identifier, depth=0):
        row = by_id[identifier]
        kind = "Ticket" if identifier in tickets else "Spec" if depth == 0 else "Workstream"
        if kind == "Ticket":
            status = state(row, tickets)
            progress = status.replace("-", " ")
        else:
            all_tickets = descendants(identifier)
            complete = sum(tickets[key]["status"] in FINISHED for key in all_tickets)
            progress = f"{complete}/{len(all_tickets)} complete"
            status = "complete" if all_tickets and complete == len(all_tickets) else "planned"
        blockers = row.get("blocked_by", [])
        blocked_text = ", ".join(f'<a href="#work-{escape(key)}">{escape(key)}</a>' for key in blockers) or "None"
        links = "<br>".join(f'<a href="{escape(pr["url"])}" rel="noopener noreferrer">{escape(pr.get("label", "PR"))}</a>' for pr in row.get("prs", [])) or "Pending"
        detail = row_detail(row, kind)
        classes = "ticket-row" if kind == "Ticket" else "parent-row"
        parent = row.get("parent", "")
        output = [f'<tr id="work-{escape(identifier)}" class="{classes}" data-work-status="{status}" data-work-kind="{kind}" data-parent="{escape(parent)}">',
                  f'<td><code>{escape(identifier)}</code><br>{kind}</td>',
                  f'<td style="padding-left:{12 + depth * 8}px"><details><summary>{escape(row["title"])}</summary><div>{detail}</div></details><small>Parent: {escape(parent) if parent else "Root"}</small></td>',
                  f'<td>{escape(progress)}</td><td>{escape(row.get("owner", "Orchestrator" if kind != "Ticket" else "Unassigned"))}</td>',
                  f'<td>{blocked_text}</td><td>{links}</td></tr>']
        for child in children[identifier]:
            output.append(table_rows(child, depth + 1))
        return "".join(output)

    ready = [row["id"] for row in tickets.values() if state(row, tickets) == "ready"]
    content = f'''{START}
<section id="execution">
<h2>Execution tracker</h2>
<p>The spec and tickets below drive implementation. Expand a title for the behavior, acceptance criteria, and recorded evidence. Parent totals include every child. Completion requires reviewed evidence for every criterion and merged PRs. The final ticket also requires imported-world and attended-client observation records.</p>
<p><strong>Updated {escape(plan['updated_at'])}.</strong> {escape(plan.get('summary', 'Implementation is starting.'))}</p>
<p>Ready to start: <span id="execution-ready">{escape(', '.join(ready) or 'No unassigned work is ready.')}</span>. The orchestrator republishes progress to this URL. Filters operate on this published snapshot.</p>
<div class="execution-controls">
<label for="execution-search">Find work <input id="execution-search" type="search" placeholder="Ticket, behavior, owner, evidence"></label>
<label for="execution-status">Show <select id="execution-status"><option value="all">All work</option><option value="ready">Ready to start</option><option value="active">Implementation and review</option><option value="unfinished">Unfinished</option><option value="complete">Complete</option><option value="blocked">Blocked or needs client evidence</option></select></label>
<button type="button" id="execution-expand">Expand all details</button>
<button type="button" id="execution-collapse">Collapse details</button>
</div>
<p id="execution-count" aria-live="polite">{len(tickets)} tickets</p>
<div class="table-scroll execution-table"><table><thead><tr><th>Identifier</th><th>Spec and ticket hierarchy</th><th>Status</th><th>Owner</th><th>Blocked by</th><th>Pull requests</th></tr></thead><tbody>{table_rows(plan['spec']['id'])}</tbody></table></div>
</section>
<style>
.execution-controls{{display:flex;align-items:end;flex-wrap:wrap;gap:12px}}.execution-controls label{{display:grid;gap:4px;font-size:14px}}.execution-controls input,.execution-controls select,.execution-controls button{{font:inherit;border:1px solid #718a95;background:white;color:#18252d;padding:7px 9px;border-radius:0}}.execution-controls input{{max-width:100%;width:260px}}.execution-table table{{min-width:900px}}.execution-table td:nth-child(2){{min-width:300px;max-width:490px}}.execution-table details{{border:0;padding:0}}.execution-table details h3{{font-size:1rem}}.execution-table details p,.execution-table details li{{font-weight:400}}.execution-table .parent-row{{background:#edf3f4}}.execution-table small{{font-weight:400}}.execution-table tr[hidden]{{display:none}}@media print{{.execution-controls,#execution-count{{display:none}}.execution-table table{{min-width:0}}.execution-table tr[hidden]{{display:table-row}}}}
</style>
<script>
(() => {{
  const section = document.querySelector('#execution');
  const rows = Array.from(section.querySelectorAll('tbody tr'));
  const byId = new Map(rows.map(row => [row.id.slice(5), row]));
  const search = section.querySelector('#execution-search');
  const status = section.querySelector('#execution-status');
  const completed = value => value === 'complete' || value === 'merged';
  const matchesStatus = value => {{
    switch (status.value) {{
      case 'ready': return value === 'ready';
      case 'active': return ['implementing', 'review', 'approved'].includes(value);
      case 'unfinished': return !completed(value);
      case 'complete': return completed(value);
      case 'blocked': return ['blocked', 'waiting-for-client'].includes(value);
      default: return true;
    }}
  }};
  function filter() {{
    const query = search.value.trim().toLowerCase();
    const visible = new Set();
    let count = 0;
    rows.forEach(row => {{
      if (row.dataset.workKind !== 'Ticket') return;
      if (!row.textContent.toLowerCase().includes(query) || !matchesStatus(row.dataset.workStatus)) return;
      count += 1;
      let current = row;
      while (current) {{
        visible.add(current);
        current = byId.get(current.dataset.parent);
      }}
    }});
    rows.forEach(row => {{ row.hidden = !visible.has(row); }});
    section.querySelector('#execution-count').textContent = `${{count}} of {len(tickets)} tickets shown`;
  }}
  search.addEventListener('input', filter);
  status.addEventListener('change', filter);
  section.querySelector('#execution-expand').addEventListener('click', () => rows.filter(row => !row.hidden).forEach(row => {{ row.querySelector('details').open = true; }}));
  section.querySelector('#execution-collapse').addEventListener('click', () => rows.forEach(row => {{ row.querySelector('details').open = false; }}));
  filter();
}})();
</script>
{END}'''
    return content


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path, default=Path(__file__).with_name("playerbots-execution.json"))
    parser.add_argument("--report", type=Path, default=Path(__file__).with_name("playerbots-rewrite.html"))
    args = parser.parse_args()
    plan = json.loads(args.plan.read_text())
    section = render(plan)
    report = args.report.read_text()
    if START in report:
        before, rest = report.split(START, 1)
        _, after = rest.split(END, 1)
        report = before + section + after
    else:
        report = report.replace('<section id="decision">', section + '\n<section id="decision">', 1)
        report = report.replace('<p>Read or decide</p>', '<p>Read or decide</p>\n<a href="#execution">Execution tracker</a>', 1)
    report = report.replace(' target="_blank"', '')
    report = report.replace("Prepared for LyraCore. Research and proposal only.", "Prepared for LyraCore. Research and implementation tracker.")
    if len(report.encode()) > 512 * 1024:
        raise ValueError("Report exceeds the 512 KB publication limit")
    args.report.write_text(report)
    print(f"Rendered {len(plan['tickets'])} tickets into {args.report.name}")


if __name__ == "__main__":
    main()
