---
"skills-manager": minor
---

Connecting to a host with an older CLI now installs the matching one without a click, and
the first start after an app update installs it on every host that has one. A newer host
asks you to update the app instead of being downgraded, and an older Agents Manager on the
host must be updated first. Hosts that could not be reached show "Update pending". The
install keeps the current and previous CLI versions, and also works on a host with no CLI.
