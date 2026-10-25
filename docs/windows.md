# LyraCore on Windows

LyraCore does not run on native Windows. Run the Realm in WSL2 with Ubuntu and keep the game
client on Windows. This page covers what differs on WSL2. For everything else, follow the
[quickstart](quickstart.md) inside Ubuntu.

The maintainers do not test this route on every change. If a step fails, open an issue with the
exact error and your `./lyracore doctor` output.

## 1. Install WSL2 and Ubuntu

In **PowerShell**, as Administrator:

```powershell
wsl --install -d Ubuntu-24.04
wsl --list --verbose
```

Restart Windows if it asks. Open **Ubuntu** from the Start menu and create your Linux user. The
`VERSION` column must say `2`; if it says `1`, run `wsl --set-version Ubuntu-24.04 2`.

## 2. Keep the checkout in the Linux file system

In **Ubuntu**, install the [Ubuntu prerequisites](quickstart.md#ubuntu-2404) and run the installer
from your Linux home directory, so the checkout is `~/LyraCore`. Do not clone under `/mnt/c/...`
and do not clone with Git for Windows. Builds on the Windows drive are much slower, and Linux file
permissions do not work there. Microsoft
[recommends the Linux file system](https://learn.microsoft.com/en-us/windows/wsl/filesystems) for
this kind of project.

Then start the Realm and create an Account as the quickstart shows, from
[§3](quickstart.md#3-start-the-realm).

## 3. Connect the client on Windows

Set `realmlist.wtf` in the Windows client folder to `set realmlist 127.0.0.1` and start `WoW.exe`.
WSL forwards `127.0.0.1` from Windows to the Realm in Ubuntu; see
[WSL networking](https://learn.microsoft.com/en-us/windows/wsl/networking).

## 4. Import from the Windows client

Ubuntu sees the Windows drives under `/mnt`. Convert the path of the client's `Data` folder with
`wslpath`, and keep the quotes, because the path can contain spaces:

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
`.lyracore-cli-rev must hold one 40-character lowercase hex commit SHA`. Git for Windows made that
clone. Make a new clone inside Ubuntu, in `~`.

### The build is very slow, or fails with permission errors

Run `pwd` in Ubuntu. If the path starts with `/mnt/`, the checkout is on the Windows drive. Make a
new clone in `~`.

### The client says "Unable to connect"

Check the Realm in **Ubuntu** with `lyracore dev status`. If it is healthy, check that
`realmlist.wtf` says `127.0.0.1` and that no other Windows program uses port 3724 or 8085.

A VPN or a custom `.wslconfig` can stop the forwarding of `127.0.0.1`. On Windows 11, set
`networkingMode=mirrored` under `[wsl2]` in `%UserProfile%\.wslconfig`, then run `wsl --shutdown`
in **PowerShell** and start Ubuntu again.

The [quickstart troubleshooting](quickstart.md#8-troubleshooting) covers the other failures. They
are the same on WSL.
