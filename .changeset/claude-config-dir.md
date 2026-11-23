---
"skills-manager": patch
---

MCP inspect and `skills-manager-cli mcp register --client claude` now use `CLAUDE_CONFIG_DIR` when it is set, so they read and write the same `.claude.json` that Claude Code uses. Global instructions still go to `~/.claude`.
