import { describe, expect, it } from "vitest";
import { buildFileTree, type FileTreeNode } from "./instructionTree";

type Shape = string | { [folder: string]: Shape[] };

function shape(nodes: FileTreeNode<{ path: string }>[]): Shape[] {
  return nodes.map((node) =>
    node.kind === "file" ? node.name : { [node.name]: shape(node.children) },
  );
}

describe("buildFileTree", () => {
  it("nests paths with folders before files, sorted by name", () => {
    const tree = buildFileTree(
      [
        "CLAUDE.md",
        "packages/web/AGENTS.md",
        "AGENTS.md",
        "packages/api/AGENTS.md",
      ].map((path) => ({ path })),
    );
    expect(shape(tree)).toEqual([
      { packages: [{ api: ["AGENTS.md"] }, { web: ["AGENTS.md"] }] },
      "AGENTS.md",
      "CLAUDE.md",
    ]);
  });

  it("collapses single-folder chains into one row", () => {
    const tree = buildFileTree(
      [".claude/rules/testing.md", ".claude/rules/style.md"].map((path) => ({
        path,
      })),
    );
    expect(shape(tree)).toEqual([
      { ".claude/rules": ["style.md", "testing.md"] },
    ]);
    expect(tree[0].path).toBe(".claude/rules");
  });

  it("keeps the scanned entry on each file node", () => {
    const entry = { path: "docs/AGENTS.md", managed: true };
    const [folder] = buildFileTree([entry]);
    expect(folder.kind === "folder" && folder.children[0]).toMatchObject({
      kind: "file",
      path: "docs/AGENTS.md",
      file: entry,
    });
  });
});
