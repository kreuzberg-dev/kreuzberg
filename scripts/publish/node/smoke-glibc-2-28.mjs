import { existsSync } from "node:fs";
import { createRequire } from "node:module";

const nodePath = process.env.NODE_PATH;
if (!nodePath) {
  console.error("NODE_PATH not set (path to the .node file)");
  process.exit(2);
}
if (!existsSync(nodePath)) {
  console.error(`NODE_PATH does not exist: ${nodePath}`);
  process.exit(2);
}

console.log(`=== Loading ${nodePath} ===`);

const require_ = createRequire(import.meta.url);
let native;
try {
  native = require_(nodePath);
} catch (e) {
  console.error("FAIL: dlopen of .node failed (glibc symbol miss?):");
  console.error(e?.message || e);
  process.exit(1);
}

const failures = [];
function check(name, fn) {
  try {
    fn();
    console.log(`  OK  ${name}`);
  } catch (e) {
    console.error(`  FAIL ${name}: ${e?.message || e}`);
    failures.push(name);
  }
}

check("module is an object", () => {
  if (typeof native !== "object" || native === null) {
    throw new Error(`got ${typeof native}`);
  }
});

const required = ["extract", "extractBatch", "listSupportedFormats"];
for (const name of required) {
  check(`export ${name} is function`, () => {
    if (typeof native[name] !== "function") {
      throw new Error(`got ${typeof native[name]}`);
    }
  });
}

check("listSupportedFormats returns non-empty array", () => {
  const formats = native.listSupportedFormats();
  if (!Array.isArray(formats) || formats.length === 0) {
    throw new Error(`got ${typeof formats} length=${Array.isArray(formats) ? formats.length : "n/a"}`);
  }
});

console.log(`\n=== Summary ===`);
if (failures.length === 0) {
  console.log(`OK: ${nodePath} loads and responds on glibc 2.28.`);
  process.exit(0);
} else {
  console.error(`FAIL: ${failures.length} smoke check(s) failed: ${failures.join(", ")}`);
  process.exit(1);
}
