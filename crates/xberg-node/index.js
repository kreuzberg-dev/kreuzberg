"use strict";

const { platform, arch } = process;
const isMusl = () => {
  // Prefer the report-header `glibcVersion` string when present — fastest and
  // unambiguous on Node builds that populate it. On Node 22+, certain CI
  // environments leave `glibcVersion` undefined even on glibc systems, so the
  // `=== undefined` branch from older napi-rs templates produces a false
  // "is musl" positive. Fall through to the filesystem heuristic instead: on
  // glibc systems `/lib64/ld-musl-x86_64.so.1` does not exist; on musl systems
  // it always does. statSync errors → not musl.
  if (typeof process.report === "object" && typeof process.report.getReport === "function") {
    const report = process.report.getReport();
    if (report && report.header && typeof report.header.glibcVersion === "string") {
      return false;
    }
  }
  try {
    require("fs").statSync("/lib64/ld-musl-x86_64.so.1");
    return true;
  } catch {
    return false;
  }
};

let nativeBinding = null;
const loadErrors = [];

function requireOptionalDependency(name) {
  try {
    return require(name);
  } catch (e) {
    loadErrors.push(`Optional dependency ${name}: ${e.message}`);
    return null;
  }
}

const tryLoadBinding = () => {
  // Local `.node` files are named after `napi.binaryName` (binary file name on disk).
  // Optional-dep packages are named after `napi.packageName` (npm subpackage names),
  // which inherits any scope prefix from the parent package.
  const targets = [
    ["linux", "x64", "gnu", "./xberg-node.linux-x64-gnu.node", "@xberg-io/xberg-linux-x64-gnu"],
    ["linux", "arm64", "gnu", "./xberg-node.linux-arm64-gnu.node", "@xberg-io/xberg-linux-arm64-gnu"],
    ["linux", "x64", "musl", "./xberg-node.linux-x64-musl.node", "@xberg-io/xberg-linux-x64-musl"],
    ["linux", "arm64", "musl", "./xberg-node.linux-arm64-musl.node", "@xberg-io/xberg-linux-arm64-musl"],
    ["darwin", "arm64", null, "./xberg-node.darwin-arm64.node", "@xberg-io/xberg-darwin-arm64"],
    ["win32", "x64", null, "./xberg-node.win32-x64-msvc.node", "@xberg-io/xberg-win32-x64-msvc"],
  ];

  for (const [plat, a, abi, localPath, optionalDep] of targets) {
    if (platform !== plat || arch !== a) {
      continue;
    }

    if (plat === "linux" && abi) {
      const isCurMusl = isMusl();
      if ((abi === "musl") !== isCurMusl) {
        continue;
      }
    }

    try {
      nativeBinding = require(localPath);
      if (nativeBinding) {
        return;
      }
    } catch (e) {
      loadErrors.push(e.message);
    }

    try {
      const optBinding = requireOptionalDependency(optionalDep);
      if (optBinding) {
        nativeBinding = optBinding;
        return;
      }
    } catch (e) {
      loadErrors.push(e.message);
    }
  }
};

tryLoadBinding();

if (!nativeBinding) {
  throw new Error(`Failed to load native binding for ${platform}-${arch}. Errors: ${loadErrors.join(", ")}`);
}

module.exports = nativeBinding;
