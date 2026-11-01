# Agents Manager

<!-- impeccable:product-schema 1 -->

## Platform

web

The interface is a React web frontend packaged as a Tauri desktop app for macOS, Windows, and Linux. It uses one shared interface across those platforms. The Rust backend provides filesystem, agent, host, and library operations; a browser alone does not provide those capabilities.

## Users

Developers using several coding agents across projects and hosts. They need to see and control what each agent loads from one shared library, without maintaining every agent's folders and configuration separately.

## Product Purpose

Manage the skills, instructions, and MCP definitions that coding agents load, on this computer and on SSH hosts. Success means users understand which resources are in the library, which are on each agent or in each project, and what a proposed change will affect.

## Positioning

One library connects reusable agent resources to agent-specific global folders and project workspaces. The desktop app and CLI share the Rust core and library state, so coding agents can manage resources through Agents Manager while preserving source metadata, presets, update tracking, and deployment state.

## Operating Context

- Users install skills from Git repositories, local folders, archives, or the marketplace; they also manage instruction bundles and MCP definitions.
- The selected host scopes library and workspace operations. Projects and agents provide additional scope; consequential actions must make their target clear.
- Global Workspaces show what agents actually have, including skills added outside Agents Manager. Project Workspaces compare project copies with the library. Linked Workspaces support other skill roots.
- Presets group library skills with per-agent toggles. Switching the active preset changes which skills are on agents; it is not merely a library filter.
- Sync connects participating hosts through one Git remote so they share skills, presets, and the active preset. Hosts retain local operational state and credentials.
- The CLI and desktop app can be used together. Supported Instructions and MCP operations vary by agent and scope.

## Capabilities and Constraints

- Support inspecting resource content and sources, searching and filtering the library, tracking upstream updates, and reviewing differences before resolving edits.
- Support adding skills to agents by link or copy where the platform permits. Preserve agent-specific paths and capability boundaries.
- Instruction and MCP changes have deployment previews and recovery workflows. The target agent handles MCP process startup and authentication.
- Protect local edits. An edited copy requires an explicit user choice rather than silent replacement.
- Keep credentials out of portable library data. Do not imply that a shared definition includes its credentials or that adding it proves a working connection.
- Preserve explicit host, project, agent, and resource scope throughout review and execution.
- Existing implementation: React, TypeScript, Vite, Tailwind CSS, Tauri, Rust, and SQLite. English is the only shipped locale; interface strings use i18next for future localization.
- Desktop development uses `pnpm tauri:dev`. Browser fixtures use the fake backend enabled by Vite's `e2e` mode; they demonstrate interface behavior, not real host or filesystem operations.

## Brand Commitments

The product name is **Agents Manager**. Repository, package, and CLI identifiers may retain `skills-manager`.

Use the domain vocabulary in `CONTEXT.md` as the terminology authority. In particular:

- **Install** adds a skill to the library. **Add to agent** and **Remove from agent** change what an agent has.
- **Host** means this computer or a registered SSH host.
- **Sync**, **Sync remote**, and **Join** describe the shared library workflow. Older Backup terminology in documentation, routes, or internal identifiers does not establish preferred product wording.
- **Edited here** describes a changed local copy; **Up to date**, **Update available**, and **Both changed** communicate project comparison states.

Use direct language that identifies the resource, target, consequence, and available recovery when relevant.

## Evidence on Hand

- `CONTEXT.md`: current domain vocabulary and Sync semantics.
- `README.md` and `AGENTS-MANAGER.md`: documented capabilities, operational boundaries, CLI workflows, and recovery guidance. Some README wording predates the current Sync terminology.
- `src/router.tsx`, `src/views/`, and `src/components/`: implemented interface and workflows.
- `src/index.css`, `assets/icon.png`, and `public/agent-icons/`: existing interface styles and product and agent assets.
- `assets/demo/`: committed interface screenshots. Check them against current source before treating them as current visual evidence.
- `e2e/`: browser scenarios backed by fixtures. These do not establish live-operation or production acceptance.

## Product Principles

1. Make it clear what each agent loads and where each resource comes from.
2. Keep host, project, and agent scope explicit before a change takes effect.
3. Give users a review step before consequential changes, with the affected resources and targets visible.
4. Protect local edits and preserve deliberate user choices when resolving differences.
5. Share reusable library data while keeping credentials and host-specific operational state local.

## Open Decisions

No product-specific accessibility standard or additional inclusion requirements have been established. This does not establish an accessibility compliance claim.
