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

Run these checks from the user's checkout. Report the result before you change anything.

- **Branch.** Run `git fetch origin`. The branch changes Gateway code when either command prints
  something:
  - `git diff --stat $(git merge-base HEAD origin/main) HEAD -- gateway/`
  - `git log --full-history --no-merges --oneline origin/main..HEAD -- gateway/`

  When only the second prints something, the branch already merged the new `main`, and the merge
  dropped the Gateway changes. Tell the user. Step 3 of "Port a branch" saves them differently.
- **Local Realm.** Run `./lyracore dev status`, and check for `.lyracore/coordinator-token` and
  `.lyracore/native-data/`. A coordinator token without native data means an old local Realm.
- **Packages.** Search the branch's Package code for `emit_bot_invite_intent`,
  `emit_bot_leave_intent`, `set_sessionless_action_consent`, `enter_sessionless_areatrigger`,
  `party_facts`, and any wait on a Group Intent or Transfer Intent.
- **Realm with players.** Ask whether the checkout deploys a Realm that has players. If it does,
  stop. The doc's "Multi-Shard Realms" section explains why. Do not touch that Realm.

Then do only the sections that apply.

## 2. Local Realm

Follow the doc's "Local Realm" steps. Before you run `./lyracore dev down`, tell the user that the
old local Accounts and Characters do not come across. Do not delete old data directories.

`account create` and `dev smoke` read a password. Ask the user for one, then pass it on stdin:

```bash
printf '%s\n' "$PASSWORD" | ./lyracore account create TEST --password-stdin
printf '%s\n' "$PASSWORD" | ./lyracore dev smoke TEST Tester --password-stdin
```

Report the smoke result.

## 3. Port a branch

1. Check that `git status --porcelain` prints nothing. If it prints something, ask the user to
   commit or stash. Do not stash or discard their work yourself.
2. Record the branch head so the user can return to it: `git branch backup/<branch> HEAD`. If that
   branch exists, pick another name. On a detached HEAD, ask the user for a name.
3. Save the old Gateway diff outside the repository. This is the specification for the port.

   ```bash
   name=$(git rev-parse --abbrev-ref HEAD | tr / -)
   git diff $(git merge-base HEAD origin/main) HEAD -- gateway/ > "/tmp/$name-gateway.diff"
   ```

   If the branch already merged the new `main`, that diff is empty. Use this instead:

   ```bash
   git log --full-history --no-merges -p origin/main..HEAD -- gateway/ > "/tmp/$name-gateway.diff"
   ```

   Check that the file is not empty before you continue.

4. Run `git rebase -X no-renames origin/main`. Rename detection must stay off. With it on, Git
   applies some Gateway changes to moved codec files and moves added Gateway files into
   `module/src/native/` or the protocol crate. At each stop:
   - run `git rm -r -f -q --ignore-unmatch gateway/`,
   - resolve the other conflicts as usual,
   - run `git rebase --continue`, or `git rebase --skip` when the commit is now empty.

   Conflicts come back for each commit that changed the Gateway. Repeat until the rebase ends.
5. Check that `git ls-files gateway/` prints nothing. Read `git diff --stat origin/main HEAD` and
   check that it lists only the branch's own changes.
6. For each hunk in the saved diff, find the new location in the doc's file map. Read the new code
   before you change it, because the runtime change already contains some fixes. When a hunk
   already exists in the new code, tell the user and skip it.
7. Port the hunk by its behaviour, not by its text, in new commits. Use the doc's patterns:
   - Request handlers go in `module/src/native/<area>.rs`.
   - A Refusal returns `Err(super::rollback_silent(cause))` or `Err(super::rollback_reply(message, cause))`.
     Gate helpers return a plain `String` and the handler maps it with `.map_err(super::rollback_silent)`.
     Any other `Err` that leaves the handler ends the World Session with a Transport Loss.
   - A Relay is called from the Module code that writes the Relay row, and is re-exported in
     `native/mod.rs`.
   - Codecs go in `crates/lyracore-protocol`.
8. When the old change added or changed a `gw_*` reducer only for the Gateway, put the logic in the
   native handler. Keep the `gw_*` change only when durable tests or `native-host/check.py` need it.
9. Move each old Gateway test to a Module unit test, a `module/tests/` durable test or a
   `native-host/check.py` case. A test that checked wire bytes or packet order goes in
   `check.py`. Do not delete a test without a replacement. If you cannot port it, tell the user.
10. Run the checks in `docs/testing.md`: formatting, Clippy, the Module tests, the WASM build and
    the native protocol check. Report failures with their output.
11. Rewrite the PR description where it names the Gateway, `call_reducer!` or Gateway files.

## 4. Packages

Apply the doc's "Package changes" section. Remove waits on Group Intents and Transfer Intents,
because the action is complete when the call returns. Fix any lookup that treats the
`enter_sessionless_areatrigger` result as a Transfer Intent id. Then run
`cargo test -p lyracore-module --test package_api_lint` and the Package tests.

## 5. Report

Tell the user:

- which sections applied,
- each ported hunk, with its old and new file,
- each hunk you skipped because the new code already had it,
- tests you moved, and tests you could not port,
- check results, and the name of the backup branch.

Complete the [comment review](../../../CODING_STANDARDS.md#comment-review) on the ported code and
include the result. Do not push or open a PR unless the user asks.
