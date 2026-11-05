---
"skills-manager": minor
---

Create a new skill straight into the library, from `skills create` in the CLI or the new Create tab on the Install page, and optionally add it to agents, a preset or a project on any host. A created skill has no source path, and `skills update --all` now skips local skills without one instead of reporting an error. Skill descriptions containing `---` are now read correctly.
