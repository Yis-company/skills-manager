---
"skills-manager": minor
---

Let coding agents set up a project through `skills-manager-cli mcp`, a local MCP server with three tools: discover a checkout's saved setup, apply it in one confirmed call, and inspect an earlier apply by its operation ID. A first setup is proposed before anything is written, a retry with the same operation ID returns the recorded result, and a lost reply is reported as outcome unknown instead of being run again. `skills-manager-cli mcp register --client claude|codex` adds the server to a client's config and keeps its other settings.
