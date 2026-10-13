---
"skills-manager": minor
---

Instruction and MCP conflicts no longer stop a sync (release 2). When two hosts
change the same instruction bundle or MCP, Sync keeps this host's version and
records the conflict in its history, so every host lists it in Needs review
and in the Library, named by host. Any desktop app can resolve it, and every
host applies the choice on its next sync. A choice no longer expires when
another host syncs in the meantime.
