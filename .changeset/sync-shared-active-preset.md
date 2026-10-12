---
"skills-manager": major
---

Sync format 3 (release 2; every device needs this release before any of them
writes it). The active preset is shared: a click on a preset in the global
workspace or the tray switches it on every host, and each host puts exactly that
preset's skills on its own agents, removing the rest, including skills added
directly. The first switch shows what it removes. Toggles store only the
choices you set (a missing toggle means on), and adding or removing a skill
directly no longer changes them. Creating a preset doesn't activate it, the
active preset can't be deleted, and "Default" has one fixed id everywhere. A
host that joins takes Sync's active preset.
