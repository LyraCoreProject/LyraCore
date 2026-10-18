# Wave Defense browser hosting

The browser address is `https://159.69.88.70`. Use a browser with WebGPU support.
The host serves Manila with the WaveDefense addon installed. Native Benilla remains usable
through the existing game ports.
See [browser verification](BROWSER-VERIFICATION.md) for the deployment checks and remaining limits.

## Accounts

The browser has its own web logins. An admin invites testers at `/admin/users`; each tester
must change their temporary password at first sign-in. The host creates a separate LyraCore
Account and supplies its credentials to the client through the authenticated play endpoint.
Existing Discord and native game credentials do not sign into the website.

The initial admin handoff is stored only on Argus. Read it through the authorized SSH account:

```sh
ssh root@159.69.88.70 'cat /root/wave-defense-browser-admin.txt'
```

The web panel supports invitations and password resets. Gameplay administration remains with
the LyraCore tools; MaNGOS controls are hidden. Use a dedicated tester Account for verification.

## Sources and installed paths

| Component | Location |
| --- | --- |
| Browser and hosting source | `Mathih13/benilla`, branch `t3/wave-defense-web` |
| Manila base | `Adrinalin4ik/manila`, commit `ea2dddd2a707c6b36f78e7dbd723f1ebfb7c6fcb` |
| Remote source checkout | `/home/wave-defense/manila` |
| Hosted executable and account command | `/opt/wave-defense-web/bin` |
| Browser files | `/opt/wave-defense-web/www` |
| Addon root | `/opt/wave-defense-web/addons` |
| Client archive view | `/opt/wave-defense-web/client-data` |
| Web state and credential encryption key | `/var/lib/wave-defense-web` |
| Protected configuration | `/etc/wave-defense/web.env` |
| Service | `wave-defense-web.service`, Unix account `wave-defense-web` |
| Reverse proxy | `/etc/nginx/sites-available/wave-defense` |

The archive view links to the existing `/srv/wowclient/Data` archives. It contains a readable
copy of the small `patch-3.MPQ`, whose original mode is 0600. Original files and permissions
remain unchanged. Updating that archive requires refreshing the copy and restarting the web
service. The rest of the game data is reused without another import or full client copy.

The web process binds only `127.0.0.1:8090`. Nginx terminates HTTPS at the public IP and relays
WebSockets. The relay dials only the existing local logon and world ports. Client assets,
addon files and sockets require a web session. The original Tailscale HTTPS listener remains
bound to its own address.

The host needs the matching Gateway command and additive `create_account` reducer from Core
commit `718daae2`. New Accounts use atomic create-only provisioning. Password rotation retains
the existing provisioning behavior. The web service copies its account command separately from
the live Gateway executable; update both when deploying an account-command change.

## Certificates

Certbot 5.8.0 is installed in `/opt/wave-defense-certbot`. The certificate uses the Let's Encrypt
short-lived profile and includes the public IP in its subject alternative names. The
`wave-defense-certificate.timer` checks renewal every four hours with up to fifteen minutes
of jitter. Successful renewal reloads Nginx.

Keep inbound TCP 80 and 443 open in both UFW and the Hetzner firewall. Port 80 serves the ACME
challenge directory and redirects other requests to HTTPS. Do not replace the explicit public
IP HTTPS bind with a wildcard bind; Tailscale already owns port 443 on its addresses.

## Updates and checks

See the browser branch's `docs/LYRACORE.md` for build and configuration details. Build the Wasm
client on the workstation with `CARGO_TARGET_DIR` unset, then transfer `web/dist`. Build only
the native hosting crate on Argus to preserve disk reserve. Restart the web service after
changing addons because their index is built at startup.

```sh
curl --fail https://159.69.88.70/healthz
systemctl status wave-defense-web wave-defense-certificate.timer
systemctl list-timers wave-defense-certificate.timer
```

Health must report `backend: lyracore`, `sqlite: true`, `client_data: true`, and no client-data
error. MySQL and SOAP are null in this mode. Health alone does not prove account provisioning
or world entry. Verify those with a dedicated web login and a real browser.

Keep the state directory and its encryption key together in any backup. Do not print the
service environment, setup token or unfiltered first-start logs. The existing realm's lack
of an off-host backup also applies to this new web state.
