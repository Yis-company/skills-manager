---
"skills-manager": minor
---

Backups no longer create `sm-v-*` snapshot tags. The Backup history now lists
every commit, newest first, with the device that made it, and any of them can be
restored. Syncing, resolving a conflict or restoring commits the current state
first, so it stays in the history. Existing snapshot tags are kept and still
restore. The CLI's `git versions` prints the same history, `git restore` accepts
a commit from it, and `git commit` reports the commit instead of a tag.
