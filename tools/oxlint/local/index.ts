import { defineRule, eslintCompatPlugin } from "@oxlint/plugins";

const tmpPath = /^(?:(\/private)?\/tmp\/|(?:\.\/)?\.impeccable\/review(?:\/|$))/;

/** Ban machine-specific temp paths; tests write artifacts under testInfo.outputPath(). */
const noTmpPathsRule = defineRule({
  meta: {
    type: "problem",
    docs: { description: "Keep test artifacts in Playwright output paths." },
    messages: {
      tmpPath:
        "Write test artifacts with `testInfo.outputPath()`; absolute temp paths and tracked review screenshots are not test output directories.",
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
