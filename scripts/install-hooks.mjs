import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));

const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

try {
  if (realpathSync(git("rev-parse", "--show-toplevel")) !== realpathSync(root)) {
    throw new Error("The project root is not the Git repository root.");
  }

  const configured = spawnSync("git", ["config", "--get", "core.hooksPath"], {
    cwd: root,
    encoding: "utf8",
  });

  if (configured.error || (configured.status !== 0 && configured.status !== 1)) {
    throw configured.error ?? new Error(configured.stderr || "Could not inspect core.hooksPath");
  }

  if (configured.status === 0) {
    throw new Error(
      "core.hooksPath is configured; integrate scripts/pre-push.sh with your existing hooks manually.",
    );
  }

  const hooks = git("rev-parse", "--path-format=absolute", "--git-path", "hooks");
  const target = join(hooks, "pre-push");
  const hook = readFileSync(join(root, "scripts", "pre-push-hook.sh"));

  mkdirSync(hooks, { recursive: true });

  try {
    writeFileSync(target, hook, { flag: "wx", mode: 0o755 });
  } catch (error) {
    if (
      error.code !== "EEXIST" ||
      !lstatSync(target).isFile() ||
      !readFileSync(target).equals(hook)
    ) {
      throw new Error(
        `Will not overwrite ${target}; integrate scripts/pre-push.sh with your existing hook manually.`,
      );
    }

    chmodSync(target, 0o755);
  }

  console.log(`Pre-push hook installed at ${target}`);
} catch (error) {
  console.error(`Could not install pre-push hook: ${error.message}`);
  process.exitCode = 1;
}
