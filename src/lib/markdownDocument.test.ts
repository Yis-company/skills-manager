import { describe, expect, it } from "vitest";
import {
  richEditingIsLossless,
  splitFrontmatter,
  withTrailingNewline,
} from "./markdownDocument";

describe("splitFrontmatter", () => {
  it("separates a leading YAML block and joins back to the original", () => {
    const text = "---\nglobs: src/**\nalwaysApply: false\n---\n# Rule\n";
    const { frontmatter, body } = splitFrontmatter(text);
    expect(frontmatter).toBe("---\nglobs: src/**\nalwaysApply: false\n---\n");
    expect(body).toBe("# Rule\n");
    expect(frontmatter + body).toBe(text);
  });

  it("leaves text without frontmatter as the body", () => {
    expect(splitFrontmatter("# Title\n\n---\n")).toEqual({
      frontmatter: "",
      body: "# Title\n\n---\n",
    });
  });
});

describe("withTrailingNewline", () => {
  it("follows the original file", () => {
    expect(withTrailingNewline("a", "x\n")).toBe("a\n");
    expect(withTrailingNewline("a", "x")).toBe("a");
  });
});

describe("richEditingIsLossless", () => {
  it("accepts plain headings, lists, code and links", () => {
    expect(
      richEditingIsLossless(
        "# Rules\n\n- Run `npm test`\n- See [guide](docs/guide.md)\n\n```ts\nx\n```\n",
      ),
    ).toBe(true);
  });

  it("checks only the body of a file with frontmatter", () => {
    expect(richEditingIsLossless("---\nalwaysApply: true\n---\n# Rule\n")).toBe(
      true,
    );
  });

  it("rejects syntax the rich editor would rewrite", () => {
    expect(richEditingIsLossless("<!-- keep -->\nText\n")).toBe(false);
    expect(richEditingIsLossless("* star\n* list\n")).toBe(false);
  });
});
