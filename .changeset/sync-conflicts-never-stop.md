---
"skills-manager": minor
---

Conflicts no longer stop a device from syncing. Automatic backup keeps merging
when another device changes a skill that is waiting for your decision: the skill
keeps your version and the conflict moves to the newest remote version. A skill
flagged on two devices with different versions is flagged again instead of
failing the sync. The CLI's `git pull` now finishes or rolls back an interrupted
sync first, as the app does at startup.
