import { Markdown } from "@tiptap/markdown";
import { EditorContent, useEditor } from "@tiptap/react";
import { useRef, useState } from "react";

import {
  markdownExtensions,
  richEditingIsLossless,
  splitFrontmatter,
  withTrailingNewline,
} from "../lib/markdownDocument";
import { cn } from "../utils";

type Mode = "rich" | "source";

/**
 * Markdown editor with a Tiptap rich mode and a raw source mode. Files that
 * would not survive a rich round-trip open in source mode, and `onChange`
 * fires only on real edits, so opening and saving never rewrites a file.
 */
export function MarkdownEditor({
  value,
  onChange,
  ariaLabel,
  rows = 12,
}: {
  value: string;
  onChange: (value: string) => void;
  ariaLabel: string;
  rows?: number;
}) {
  const [lossless, setLossless] = useState(() => richEditingIsLossless(value));
  const [mode, setMode] = useState<Mode>(lossless ? "rich" : "source");
  // The last value this editor knows about. Anything else is a new file.
  const [synced, setSynced] = useState(value);
  const [version, setVersion] = useState(0);

  if (value !== synced) {
    const next = richEditingIsLossless(value);
    setSynced(value);
    setLossless(next);
    setMode(next ? "rich" : "source");
    setVersion((old) => old + 1);
  }

  const emit = (next: string) => {
    setSynced(next);
    onChange(next);
  };

  return (
    <div className="mt-2 space-y-2">
      <div className="flex items-center gap-2 text-[11px]">
        <div className="app-segmented" role="group" aria-label={`${ariaLabel} mode`}>
          {(["rich", "source"] as const).map((option) => (
            <button
              key={option}
              type="button"
              aria-pressed={mode === option}
              className={cn(
                "rounded-md px-2 py-0.5",
                mode === option ? "bg-surface-active text-primary" : "text-muted",
              )}
              onClick={() => setMode(option)}
            >
              {option === "rich" ? "Rich" : "Source"}
            </button>
          ))}
        </div>
        {!lossless && (
          <span className="text-amber-600">Rich editing would reformat this file.</span>
        )}
      </div>
      {mode === "rich" ? (
        <RichMarkdown key={version} value={value} onChange={emit} ariaLabel={ariaLabel} />
      ) : (
        <textarea
          aria-label={ariaLabel}
          className="app-input h-auto w-full py-2 font-mono text-[12px]"
          rows={rows}
          value={value}
          onChange={(event) => emit(event.target.value)}
        />
      )}
    </div>
  );
}

function RichMarkdown({
  value,
  onChange,
  ariaLabel,
}: {
  value: string;
  onChange: (value: string) => void;
  ariaLabel: string;
}) {
  const [{ frontmatter, body }] = useState(() => splitFrontmatter(value));
  const [header, setHeader] = useState(frontmatter);
  // Read by the editor's update callback, which Tiptap keeps from creation.
  const headerRef = useRef(frontmatter);

  const editor = useEditor({
    extensions: [...markdownExtensions, Markdown],
    content: body,
    contentType: "markdown",
    editorProps: {
      attributes: {
        role: "textbox",
        "aria-multiline": "true",
        "aria-label": ariaLabel,
        class: "markdown-editor app-input h-auto min-h-40 w-full py-2",
      },
    },
    onUpdate: ({ editor }) =>
      onChange(headerRef.current + withTrailingNewline(editor.getMarkdown(), body)),
  });

  return (
    <>
      {frontmatter && (
        <textarea
          aria-label={`${ariaLabel} frontmatter`}
          className="app-input h-auto w-full py-2 font-mono text-[12px]"
          rows={header.split("\n").length}
          value={header}
          onChange={(event) => {
            setHeader(event.target.value);
            headerRef.current = event.target.value;
            onChange(
              event.target.value +
                (editor ? withTrailingNewline(editor.getMarkdown(), body) : body),
            );
          }}
        />
      )}
      <EditorContent editor={editor} />
    </>
  );
}
