import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight, FileText, Folder } from "lucide-react";
import { cn } from "../utils";
import { buildFileTree, type FileTreeNode } from "../lib/instructionTree";

export interface InstructionTreeFile {
  path: string;
  kind: string;
  managed: boolean;
  applicable?: boolean;
  conflict?: boolean;
  symlink_target?: string;
}

/** Scanned instruction files as a collapsible folder tree. Folders start open. */
export function InstructionFileTree<T extends InstructionTreeFile>({
  files,
  selectedPath,
  onOpen,
}: {
  files: T[];
  selectedPath: string;
  onOpen: (file: T) => void;
}) {
  const tree = useMemo(() => buildFileTree(files), [files]);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const toggle = (path: string) =>
    setCollapsed((old) => {
      const next = new Set(old);
      if (!next.delete(path)) next.add(path);
      return next;
    });

  const render = (nodes: FileTreeNode<T>[], depth: number) =>
    nodes.map((node) => {
      const indent = { paddingLeft: `${12 + depth * 14}px` };
      if (node.kind === "folder") {
        const open = !collapsed.has(node.path);
        return (
          <div key={node.path} role="treeitem" aria-expanded={open}>
            <button
              className="flex w-full items-center gap-1.5 py-1.5 pr-3 text-left text-[12px] font-medium text-muted hover:bg-surface-hover"
              style={indent}
              onClick={() => toggle(node.path)}
            >
              {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
              <Folder size={12} />
              <span className="break-all">{node.name}</span>
            </button>
            {open && <div role="group">{render(node.children, depth + 1)}</div>}
          </div>
        );
      }
      const file = node.file;
      return (
        <button
          key={node.path}
          role="treeitem"
          aria-selected={selectedPath === node.path}
          title={node.path}
          className={cn(
            "block w-full py-1.5 pr-3 text-left hover:bg-surface-hover",
            selectedPath === node.path && "bg-surface-active",
          )}
          style={indent}
          onClick={() => onOpen(file)}
        >
          <span className="flex items-center gap-1.5 text-[12px] font-medium">
            <FileText size={12} className="shrink-0 text-muted" />
            <span className="break-all">{node.name}</span>
          </span>
          <span className="block pl-[18px] text-[10px] text-muted">
            {file.applicable === false ? "Referenced document" : file.kind}
            {file.managed ? " · managed" : " · local"}
            {file.conflict ? " · changed since deployment" : ""}
            {file.symlink_target && (
              <span className="block break-all">
                Link target: {file.symlink_target}
              </span>
            )}
          </span>
        </button>
      );
    });

  return (
    <div role="tree" aria-label="Instruction files" className="py-1">
      {render(tree, 0)}
    </div>
  );
}
