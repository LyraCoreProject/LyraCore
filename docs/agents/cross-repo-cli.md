# Cross-repo LyraCore CLI changes

`./lyracore` is a pinned shim. Its Rust source lives in the sibling `lyracore-cli` repository, not
in this checkout. `.lyracore-cli-rev` selects the commit installed on the shim's next invocation.

When command behaviour belongs in the CLI:

1. Fetch the sibling repository. Preserve its current branch and working tree; make a fresh
   worktree from `origin/main` for the change.
2. Implement and test through the CLI's `ProcessRunner` / `ProcessInspector` seams and fake
   adapters. Production checks must remain read-only unless the command explicitly owns a mutation.
3. Run `cargo test` and `cargo +1.85 check` in the CLI worktree.
4. Commit and push the CLI change so the full commit SHA is reachable from the CLI remote. Then
   update `.lyracore-cli-rev` in this repository to that SHA.
5. Invoke `./lyracore --help`, then the changed command's safe argument validation from this
   checkout. Verify it installs and exposes the pinned behaviour before claiming the integration
   works.

Treat `.lyracore/cli/<rev>/` as an installed cache. Editing it produces an unreviewable local binary
that disappears on the next pin change.
