---
"skills-manager": minor
---

SSH hosts can join Sync on their own access. The host creates its own deploy key
(the private key never leaves it); the app registers it with GitHub using your
sign-in, or shows it to paste under the repository's Deploy keys. The host
trusts only GitHub's published host keys and falls back to port 443 when port 22
is blocked. Joining installs a systemd timer that syncs the host every 5
minutes, warning when linger is off. Removing a host asks "Also remove from
Sync", checked by default, which deletes its key on GitHub at once. The app
checks every host's Sync at startup and on the Hosts screen and says when one is
stuck. Skill auto-update is now on by default in the desktop app.
