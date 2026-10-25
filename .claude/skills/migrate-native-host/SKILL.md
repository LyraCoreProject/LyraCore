---
name: migrate-native-host
description: Move a checkout, branch or Package from the Gateway runtime to the Native Host.
disable-model-invocation: true
---

# Migrate to the Native Host

The Gateway is gone. The Module now handles requests, Relays and Refusals, and the Native Host
handles transport. Read [`docs/migrating-to-native-host.md`](../../../docs/migrating-to-native-host.md)
first. It holds the file map and the patterns this skill uses. Read `AGENTS.md` too, and follow it.

## 1. Find what needs to move

Run these checks from the user's checkout and report the result before you change anything:

- **Branch.** `git fetch origin`, then
  `git diff --stat $(git merge-base HEAD origin/main) HEAD -- gateway/`. Any output means the branch
  changes Gateway code. Note whether `origin/main` still has `gateway/`. If it does, the Native Host
  has not landed yet. Stop and tell the user.
- **Local Realm.** `./lyracore dev status`, and check for `.lyracore/coordinator-token` and
  `.lyracore/native-data/`. A coordinator token without native data means an old local Realm.
- **Packages.** Search the branch's Package code for `emit_bot_invite_intent`,
  `emit_bot_leave_intent`, `set_sessionless_action_consent`, `enter_sessionless_areatrigger`, and
  any wait on a Group Intent or Transfer Intent.
- **Realm with players.** Ask whether the checkout deploys a Realm that has players. If it does,
  stop. The doc's "Multi-Shard Realms" section explains why. Do not touch that Realm.

Then do only the sections that apply.

## 2. Local Realm

Follow the doc's "Local Realm" steps. Tell the user before you run `./lyracore dev down`: the
old local Accounts and Characters do not come across. Do not delete old data directories. Finish
with `./lyracore dev smoke` and report the result.

## 3. Port a branch

1. Record the branch head so the user can return to it: `git branch backup/<branch> HEAD`.
2. Save the old Gateway diff to a file outside the repository:
   `git diff $(git merge-base HEAD origin/main) HEAD -- gateway/ > /tmp/<branch>-gateway.diff`.
   This is the specification for the port.
3. Rebase onto `origin/main`. For each Gateway file in a delete/modify conflict, run `git rm` on it.
   Resolve the other conflicts as usual.
4. For each hunk in the saved diff, find the new location in the doc's file map. Read the new code
   before you change it, because the runtime change already contains some fixes. When a hunk
   already exists in the new code, tell the user and skip it.
5. Port the hunk by its behaviour, not by its text. Use the doc's patterns:
   request handlers in `module/src/native/<area>.rs`, Refusals through `rollback_silent` or
   `rollback_reply`, Relays called from the Module code that writes the Relay row, codecs in
   `crates/lyracore-protocol`.
6. When the old change added or changed a `gw_*` reducer only for the Gateway, put the logic in the
   native handler. Keep the `gw_*` change only when durable tests or `native-host/check.py` need it.
7. Move each old Gateway test to a Module unit test, a `module/tests/` durable test or a
   `native-host/check.py` case. A test that checked wire bytes or packet order goes in
   `check.py`. Do not delete a test without a replacement. If you cannot port it, tell the user.
8. Run the checks in `docs/testing.md`: formatting, Clippy, the Module tests, the WASM build and
   the native protocol check. Report failures with their output.
9. Rewrite the PR description where it names the Gateway, `call_reducer!` or Gateway files.

## 4. Packages

Apply the doc's "Package changes" section. Remove waits on Group Intents and Transfer Intents,
because the action is complete when the call returns. Run `package_api_lint` and the Package
tests.

## 5. Report

Tell the user:

- which sections applied,
- each ported hunk, with its old and new file,
- each hunk you skipped because the new code already had it,
- tests you moved, and tests you could not port,
- check results, and the name of the backup branch.

Complete the [comment review](../../../CODING_STANDARDS.md#comment-review) on the ported code and
include the result. Do not push or open a PR unless the user asks.
