---
"skills-manager": minor
---

A skill copy you edited in an agent's folder is never overwritten or removed
automatically, not even by a preset switch. When Sync changes that skill, the
copy is flagged "Edited here" instead. "Use Sync version" moves your edit to a
backup folder and puts the library version back; "Keep my edit" leaves the copy
alone until the skill changes again. Edits are spotted by file times, and
Python caches don't count.
