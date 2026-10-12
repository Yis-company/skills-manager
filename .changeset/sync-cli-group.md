---
"skills-manager": minor
---

The CLI gets a `sync` command group: `sync now`, `sync status`, `sync history`,
`sync restore`, `sync pause`, `sync resume`, `sync conflicts` and
`sync resolve <skill> --keep <host>|--both`. `sync now` exits 1 with the reason
when it can't sync, and `sync status` reports the state, the reason, conflicts,
edited agent copies, the last sync, the version and the Sync format. The old
`git` commands keep working and point to their `sync` names. The app runs the
same commands over its SSH session to a host.
