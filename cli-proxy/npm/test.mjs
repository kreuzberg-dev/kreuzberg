import assert from "node:assert/strict";
import { test } from "node:test";

import { isNonCliArtifact, selectArchiveName } from "./install.js";

const TRIPLE = "aarch64-apple-darwin";

test("isNonCliArtifact rejects bindings/native-lib/bottle artifacts", () => {
  const rejected = [
    "html-to-markdown-rs-ffi-v3.6.18-aarch64-apple-darwin.tar.gz",
    "liter_llm_ffi-v1.7.4-aarch64-apple-darwin.tar.gz",
    "libliter_llm_nif-v1.7.4-nif-2.17-aarch64-apple-darwin.so.tar.gz",
    "libtree_sitter_language_pack_nif-v1.9.1-nif-2.16-aarch64-apple-darwin.so.tar.gz",
    "xberg-0.1.0.bottle.tar.gz",
    "foo.dylib.tar.gz",
    "foo.dll.zip",
    "Foo.artifactbundle.zip",
    "xberg-node-darwin-arm64.tar.gz",
    "xberg-1.0-cp312-napi.whl",
  ];
  for (const name of rejected) {
    assert.equal(isNonCliArtifact(name), true, `should reject ${name}`);
  }
});

test("isNonCliArtifact accepts standalone CLI archives", () => {
  for (const name of [
    "xberg-cli-aarch64-apple-darwin.tar.gz",
    "cli-aarch64-apple-darwin.tar.gz",
    "liter-llm-1.7.4-aarch64-apple-darwin.tar.gz",
  ]) {
    assert.equal(isNonCliArtifact(name), false, `should accept ${name}`);
  }
});

test("selectArchiveName picks the cli archive over an ffi archive", () => {
  const names = [
    "html-to-markdown-rs-ffi-v3.6.18-aarch64-apple-darwin.tar.gz",
    "cli-aarch64-apple-darwin.tar.gz",
    "cli-x86_64-apple-darwin.tar.gz",
  ];
  assert.equal(selectArchiveName(names, TRIPLE), "cli-aarch64-apple-darwin.tar.gz");
});

test("selectArchiveName prefers a bin-name/cli archive among survivors", () => {
  const names = ["xberg-1.0.0-aarch64-apple-darwin.tar.gz", "xberg-cli-aarch64-apple-darwin.tar.gz"];
  assert.equal(selectArchiveName(names, TRIPLE), "xberg-cli-aarch64-apple-darwin.tar.gz");
});

test("selectArchiveName returns null when only non-CLI artifacts exist", () => {
  const names = ["crawlberg-ffi-v0.3.0-aarch64-apple-darwin.tar.gz", "libfoo-nif-2.17-aarch64-apple-darwin.so.tar.gz"];
  assert.equal(selectArchiveName(names, TRIPLE), null);
});
