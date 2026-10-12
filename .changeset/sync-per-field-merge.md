---
"skills-manager": minor
---

Presets and their per-agent toggles merge field by field (release 2). A rename
on one host and an icon change on another both survive. When two hosts change
the same field, the newer change wins and the Sync history names what it
replaced. Deleting a preset loses to another host editing it or switching to
it, and taking a skill out of a preset loses to another host changing its
toggles. Two hosts adding a preset with the same name keep both: one becomes
"Name (2)" and waits in Needs review, where you can keep both, combine them or
keep one. Pulling such a pair no longer fails on the duplicate name.
