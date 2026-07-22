import { execFileSync } from "node:child_process";
import { isAbsolute, join, resolve } from "node:path";

export function resolveOndaCli(repoDir) {
  const override = process.env.ONDA_CLI?.trim();
  if (override) {
    return isAbsolute(override) ? override : resolve(repoDir, override);
  }

  const cargo = process.env.CARGO?.trim() || "cargo";
  execFileSync(cargo, ["build", "-q", "-p", "onda_cli"], {
    cwd: repoDir,
    stdio: "inherit",
  });

  const targetDir = process.env.CARGO_TARGET_DIR
    ? resolve(repoDir, process.env.CARGO_TARGET_DIR)
    : join(repoDir, "target");
  return join(
    targetDir,
    "debug",
    process.platform === "win32" ? "onda.exe" : "onda",
  );
}
