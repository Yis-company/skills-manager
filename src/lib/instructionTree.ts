export type FileTreeNode<T> =
  | { kind: "folder"; name: string; path: string; children: FileTreeNode<T>[] }
  | { kind: "file"; name: string; path: string; file: T };

/**
 * Nests flat scan paths into folders. Folders sort before files, and a folder
 * whose only child is another folder is shown as one row (`.claude/rules`).
 */
export function buildFileTree<T extends { path: string }>(
  files: T[],
): FileTreeNode<T>[] {
  const root: FileTreeNode<T>[] = [];
  for (const file of files) {
    const parts = file.path.split("/");
    let level = root;
    parts.slice(0, -1).forEach((name, index) => {
      let folder = level.find(
        (node): node is Extract<FileTreeNode<T>, { kind: "folder" }> =>
          node.kind === "folder" && node.name === name,
      );
      if (!folder) {
        folder = {
          kind: "folder",
          name,
          path: parts.slice(0, index + 1).join("/"),
          children: [],
        };
        level.push(folder);
      }
      level = folder.children;
    });
    level.push({
      kind: "file",
      name: parts[parts.length - 1],
      path: file.path,
      file,
    });
  }
  return finish(root);
}

function finish<T>(nodes: FileTreeNode<T>[]): FileTreeNode<T>[] {
  return nodes
    .map((node) => {
      if (node.kind === "file") return node;
      let folder = { ...node, children: finish(node.children) };
      while (
        folder.children.length === 1 &&
        folder.children[0].kind === "folder"
      ) {
        const only = folder.children[0];
        folder = { ...only, name: `${folder.name}/${only.name}` };
      }
      return folder;
    })
    .sort((a, b) =>
      a.kind === b.kind
        ? a.name.localeCompare(b.name)
        : a.kind === "folder"
          ? -1
          : 1,
    );
}
