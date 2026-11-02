import { defineRule, eslintCompatPlugin } from "@oxlint/plugins";

const tmpPath = /^(\/private)?\/tmp\//;

/** Ban machine-specific temp paths; tests write artifacts under testInfo.outputPath(). */
const noTmpPathsRule = defineRule({
  meta: {
    type: "problem",
    docs: { description: "Disallow absolute /tmp paths in tests." },
    messages: {
      tmpPath:
        "Write test artifacts with `testInfo.outputPath()`; absolute /tmp paths break on other machines and CI.",
    },
  },
  createOnce(context) {
    return {
      Literal(node) {
        if (tmpPath.test(String(node.value))) {
          context.report({ node, messageId: "tmpPath" });
        }
      },
      TemplateLiteral(node) {
        if (tmpPath.test(node.quasis[0]?.value.cooked ?? "")) {
          context.report({ node, messageId: "tmpPath" });
        }
      },
    };
  },
});

/** Project-specific Oxlint rules, kept apart from the vendored anti-slop plugin. */
export default eslintCompatPlugin({
  meta: { name: "local" },
  rules: { "no-tmp-paths": noTmpPathsRule },
});
