# LyraCore on Windows

LyraCore does not run on native Windows. On Windows, run the Realm in WSL2 with Ubuntu and keep
the game client on Windows. The client connects to the Realm on `127.0.0.1`.

The maintainers do not test this route on every change. If a step below fails, open an issue with
the exact error and your `./lyracore doctor` output.

Each command block below says where it runs: **PowerShell** on Windows, or **Ubuntu** in the WSL
terminal.

## 1. Install WSL2 and Ubuntu

**PowerShell**, as Administrator:

```powershell
wsl --install -d Ubuntu-24.04
```

Restart Windows if it asks you to. Then open **Ubuntu** from the Start menu and create your Linux
user. Check that the distribution uses WSL 2:

```powershell
wsl --list --verbose
```

The `VERSION` column must say `2`. If it says `1`, run `wsl --set-version Ubuntu-24.04 2`.

## 2. Install LyraCore inside Ubuntu

Keep the checkout in your Linux home directory, for example `~/LyraCore`. Do not clone it under
`/mnt/c/...` and do not clone it with Git for Windows. Builds on the Windows drive are much slower,
and Linux file permissions do not work there. Microsoft
[recommends the Linux file system](https://learn.microsoft.com/en-us/windows/wsl/filesystems) for
this kind of project.

**Ubuntu**:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev openssl python3 git curl
cd ~
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh
cd LyraCore
```

The installer offers to install Rust and SpacetimeDB 2.7.1. After it finishes, open a new Ubuntu
terminal, or run the `export` or `. "$HOME/.cargo/env"` line that it prints. Then check the
machine:

```bash
lyracore doctor
```

`doctor` must end with `ready for lyracore dev up`. Fix each `✗` line before you continue.

## 3. Start the Realm

**Ubuntu**, in `~/LyraCore`:

```bash
lyracore dev up
lyracore account create admin
```

The first `dev up` installs the Native Host's Rust toolchain if it is missing, then builds the
Native Host and the Module. This takes a long time. Later starts reuse the build.

## 4. Connect the client on Windows

On Windows, open `realmlist.wtf` in your 1.12.1 client folder and set:

```
set realmlist 127.0.0.1
```

Start `WoW.exe` and log in with the account from step 3. WSL forwards `127.0.0.1` from Windows to
the Realm in Ubuntu. See
[WSL networking](https://learn.microsoft.com/en-us/windows/wsl/networking).

## 5. Import the world from your Windows client

Ubuntu sees the Windows drives under `/mnt`. Convert the Windows path of the client's `Data`
folder with `wslpath`, and keep the quotes, because the path can contain spaces.

**Ubuntu**:

```bash
lyracore config set client-data "$(wslpath 'C:\Games\World of Warcraft\Data')"
lyracore import
```

The client data can stay on Windows. Only the checkout must be in the Linux file system.

## Troubleshooting

### `./lyracore` fails with Windows line endings

The checkout has Windows line endings if `./lyracore` prints
`/bin/sh^M: bad interpreter: No such file or directory`. With `sh ./lyracore`, the same checkout
prints `set: Illegal option -`, `$'\r': command not found`, or
`.lyracore-cli-rev must hold one 40-character lowercase hex commit SHA`.

This happens when Git for Windows made the clone. Make a new
clone inside Ubuntu, in `~`, as step 2 shows.

### The build is very slow, or fails with permission errors

Run `pwd` in Ubuntu. If the path starts with `/mnt/`, the checkout is on the Windows drive. Make a
new clone in `~`.

### `doctor` reports `Python` or `Host toolchain`

Run the command that `doctor` prints, then run `lyracore doctor` again.

### The client says "Unable to connect"

Check the Realm in **Ubuntu** with `lyracore dev status`. If it is healthy, check that
`realmlist.wtf` says `127.0.0.1` and that no other program on Windows uses port 3724 or 8085.

A VPN or a custom `.wslconfig` can stop the forwarding of `127.0.0.1`. On Windows 11, you can set
`networkingMode=mirrored` under `[wsl2]` in `%UserProfile%\.wslconfig`, then run `wsl --shutdown`
in **PowerShell** and start Ubuntu again.

The [quickstart troubleshooting](quickstart.md#8-troubleshooting) has the other failures. They are
the same on WSL.
