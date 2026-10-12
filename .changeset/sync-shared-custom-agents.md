---
"skills-manager": minor
---

Custom agents are shared by Sync (release 2). An agent added on one host
appears on every host, with its folder kept relative to home. It counts as
installed only where that folder's parent exists. A folder outside home stays
this host's own path, and editing an agent's path sets it for this host only,
with Reset going back to the shared one. A host's custom agents from before
join Sync on its first pull, and Join matches agents by key, with Sync's
definition winning.
