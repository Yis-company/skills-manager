import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { devNull, homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { afterEach, expect, test } from "vitest";

const root = resolve(import.meta.dirname, "..");

const temporary: string[] = [];

afterEach(() => {
  for (const path of temporary.splice(0)) rmSync(path, { recursive: true, force: true });
});

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "skills-manager-hooks-"));
  temporary.push(dir);

  // Git exports repository settings to hooks. Never carry those into a fixture.
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_") && key !== "CI"),
  );

  env.GIT_CONFIG_GLOBAL = devNull;
  env.GIT_CONFIG_SYSTEM = devNull;
  execFileSync("git", ["init", "--quiet", dir], { env });
  mkdirSync(join(dir, "scripts"));

  for (const file of ["install-hooks.mjs", "pre-push-hook.sh", "pre-push.sh"]) {
    copyFileSync(join(root, "scripts", file), join(dir, "scripts", file));
  }

  const install = () =>
    spawnSync(process.execPath, ["scripts/install-hooks.mjs"], { cwd: dir, env, encoding: "utf8" });

  return { dir, env, install, hook: join(dir, ".git/hooks/pre-push") };
}

test("CI skips hook installation; removing its guard makes this check fail", () => {
  const f = fixture();
  f.env.CI = "true";
  expect(f.install().status).toBe(0);
  expect(existsSync(f.hook)).toBe(false);
});

test("installation and reinstallation preserve the hook", () => {
  const f = fixture();
  expect(f.install().status).toBe(0);
  chmodSync(f.hook, 0o644);
  expect(f.install().status).toBe(0);
  expect(readFileSync(f.hook)).toEqual(readFileSync(join(root, "scripts/pre-push-hook.sh")));
  expect(process.platform === "win32" || (statSync(f.hook).mode & 0o111) === 0o111).toBe(true);
});

test("foreign hooks and configured hook paths are refused without overwriting them", () => {
  const f = fixture();
  writeFileSync(f.hook, "personal hook");
  expect(f.install().status).toBe(1);
  expect(readFileSync(f.hook, "utf8")).toBe("personal hook");
  execFileSync("git", ["config", "core.hooksPath", "personal-hooks"], { cwd: f.dir, env: f.env });
  const result = f.install();
  expect(result.status).toBe(1);
  expect(result.stderr).toContain("core.hooksPath is configured");
  expect(existsSync(join(f.dir, "personal-hooks"))).toBe(false);
});

test("fixtures ignore inherited hook Git settings and CI", () => {
  const gitPath = (path: string) =>
    execFileSync("git", ["rev-parse", "--path-format=absolute", "--git-path", path], {
      cwd: root,
      encoding: "utf8",
    }).trim();

  const protectedPaths = [
    gitPath("config"),
    gitPath("config.worktree"),
    gitPath("hooks/pre-push"),
    process.env.GIT_CONFIG_GLOBAL ?? join(homedir(), ".gitconfig"),
    join(process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config"), "git/config"),
  ];

  const before = protectedPaths.map((path) => (existsSync(path) ? readFileSync(path) : null));
  const inherited = { ...process.env };

  try {
    process.env.GIT_DIR = join(root, ".git");
    process.env.GIT_WORK_TREE = root;
    process.env.GIT_CONFIG_COUNT = "1";
    process.env.GIT_CONFIG_KEY_0 = "core.hooksPath";
    process.env.GIT_CONFIG_VALUE_0 = "hostile-hooks";
    process.env.CI = "true";
    const f = fixture();
    expect(f.install().status).toBe(0);
    expect(existsSync(f.hook)).toBe(true);
    execFileSync("git", ["config", "core.hooksPath", "fixture-hooks"], { cwd: f.dir, env: f.env });
    expect(f.install().status).toBe(1);
  } finally {
    for (const key of Object.keys(process.env)) if (!(key in inherited)) delete process.env[key];
    Object.assign(process.env, inherited);
  }

  // Compare bytes without printing potentially private Git configuration on failure.
  expect(
    protectedPaths.every((path, index) => {
      const original = before[index];

      return original ? existsSync(path) && readFileSync(path).equals(original) : !existsSync(path);
    }),
  ).toBe(true);
});

// These isolated PATH fixtures use POSIX shell executables and symlinks.
test.skipIf(process.platform === "win32").each([
  ["missing cargo", "1.99.1", "cargo"],
  ["missing rustc", "1.99.1", "rustc"],
  ["outdated Rust", "1.98.0", ""],
  ["supported Rust", "1.99.1", ""],
  ["three-digit minor", "1.100.0", ""],
])("pre-push checks prerequisites before JS: %s", (_, version, missing) => {
  const f = fixture();
  const bin = join(f.dir, "bin");
  const log = join(f.dir, "calls");
  mkdirSync(bin);
  mkdirSync(join(f.dir, "src-tauri"));
  copyFileSync(join(root, "src-tauri/Cargo.toml"), join(f.dir, "src-tauri/Cargo.toml"));
  symlinkSync(execFileSync("which", ["awk"], { encoding: "utf8" }).trim(), join(bin, "awk"));

  const scripts = {
    git: 'printf "%s\\n" "$FIXTURE_ROOT"',
    rustc: `printf '%s\\n' 'rustc ${version} (fixture)'`,
    cargo: 'printf "cargo %s\\n" "$*" >> "$CALL_LOG"',
    bun: 'printf "bun %s\\n" "$*" >> "$CALL_LOG"',
  };

  for (const [name, body] of Object.entries(scripts)) {
    if (name !== missing) writeFileSync(join(bin, name), `#!/bin/sh\n${body}\n`, { mode: 0o755 });
  }

  const result = spawnSync("/bin/sh", ["scripts/pre-push.sh"], {
    cwd: f.dir,
    env: { ...f.env, PATH: bin, FIXTURE_ROOT: f.dir, CALL_LOG: log },
    encoding: "utf8",
  });

  const supported = !missing && version !== "1.98.0";
  const calls = existsSync(log) ? readFileSync(log, "utf8").trim().split("\n") : [];
  expect(result.status).toBe(supported ? 0 : 1);
  expect(result.stderr).toContain(supported ? "" : missing || "1.99");
  expect(calls).toEqual(
    supported
      ? [
          "bun run format:check",
          "bun run lint",
          "bun run typecheck",
          "bun run test",
          "cargo fmt --manifest-path src-tauri/Cargo.toml --check",
          "cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings",
        ]
      : [],
  );
});
