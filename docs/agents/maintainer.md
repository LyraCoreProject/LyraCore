# Maintainer runbook for agents

These rules bind an agent working for a maintainer. `AGENTS.md` still applies in full.

## Realm operations

- For a production realm update or a read-only realm diagnosis, use the `lyracore-operator` skill.
  Its authoritative safety boundary is [`docs/danger-zones.md`](../danger-zones.md). Require a named
  host before any mutation.

## The `./lyracore` CLI

- Before you change `./lyracore` behaviour or its pin, read
  [`cross-repo-cli.md`](./cross-repo-cli.md). The CLI source is the sibling `lyracore-cli`
  repository, not the installed cache in `.lyracore/cli/`.

## Visual and design work

- For any non-trivial UI, layout or copy change, build several distinct static mocks first, publish
  them with the `html-communication` skill, report the URL, and stop. Edit real components only
  after the user picks one.
- Standing constraints: information-dense, no decorative card or pill chrome, no light-gray subtitle
  lines above sections, minimal copy, no em dashes.
