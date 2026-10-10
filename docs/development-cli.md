# LyraCore development CLI

`./lyracore` is a pinned shim. Its source and its command reference live in the
[`LyraCoreProject/lyracore-cli`](https://github.com/LyraCoreProject/lyracore-cli) repository:
[`docs/commands.md`](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/commands.md) lists every command, flag and safety rule.

`.lyracore-cli-rev` in this checkout names the CLI commit that `./lyracore` installs into
`.lyracore/cli/<rev>/` and runs. To bump the CLI, commit a new SHA to that file.
[`agents/cross-repo-cli.md`](./agents/cross-repo-cli.md) gives the steps for a change that spans both
repositories.
