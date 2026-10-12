---
"skills-manager": minor
---

Connecting to an existing backup now joins it instead of replacing your library.
Items match by name: where both sides have one, the shared version wins, and
items only this device has are added for everyone. A full copy of your library
and database from before joining is kept under `pre-sync-<date>`, and "Undo
join" restores it. The CLI gets `sync join <url>` (shows the review list and
asks first), `sync undo-join` and `sync leave`; leaving keeps your library and
the skills on your agents. "Fix Sync Setup" joins again instead of re-cloning.
