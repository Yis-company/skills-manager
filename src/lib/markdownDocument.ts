import { MarkdownManager } from "@tiptap/markdown";
import StarterKit from "@tiptap/starter-kit";

/** Extensions shared by the rich editor and the round-trip check. */
export const markdownExtensions = [StarterKit];

const FRONTMATTER = /^---\r?\n[\s\S]*?\r?\n---(?:\r?\n|$)/;

/** Splits a leading YAML block (used by `.mdc` rules) from the Markdown body. */
export function splitFrontmatter(text: string) {
  const match = text.match(FRONTMATTER);

  return match
    ? { frontmatter: match[0], body: text.slice(match[0].length) }
    : { frontmatter: "", body: text };
}

/** Tiptap output has no trailing newline; keep the file's own. */
export function withTrailingNewline(serialized: string, original: string) {
  return original.endsWith("\n") ? `${serialized}\n` : serialized;
}

let manager: MarkdownManager | undefined;

/**
 * True when loading the body into the rich editor and saving it unchanged
 * gives back the same text. Anything else would silently reformat the file.
 */
export function richEditingIsLossless(text: string) {
  const { body } = splitFrontmatter(text);
  manager ??= new MarkdownManager({ extensions: markdownExtensions });

  return withTrailingNewline(manager.serialize(manager.parse(body)), body) === body;
}
