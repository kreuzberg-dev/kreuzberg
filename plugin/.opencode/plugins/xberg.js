// AI-RULEZ :: GENERATED FILE — DO NOT EDIT
// Content-Hash: blake3:0c4b20b2428ca03b76e22085bdcc856fbac85091cd3f6d157655d6c25c02714f
// Source-Hash: blake3:e627b65881d01f835095c09b8ff3f8e15d61895139679b904925a8aa19500a12
// Schema-Version: v1

import { tool } from "@opencode-ai/plugin";
import { spawn } from "node:child_process";

const schema = tool.schema;

const wireFormat = schema.enum(["text", "json", "toon"]).default("json").describe("CLI output format.");

const contentFormat = schema
  .enum(["plain", "markdown", "djot", "html", "json"])
  .optional()
  .describe("Document content rendering format.");

function hasValue(value) {
  return value !== undefined && value !== null && value !== "";
}

function pushOption(args, name, value) {
  if (hasValue(value)) {
    args.push(name, String(value));
  }
}

function validateJson(value, name) {
  if (!hasValue(value)) {
    return;
  }

  try {
    JSON.parse(value);
  } catch (error) {
    throw new Error(`${name} must be valid JSON: ${error.message}`, { cause: error });
  }
}

function runCli(args, context) {
  const directory = context?.directory ?? context?.worktree ?? process.cwd();

  return new Promise((resolve, reject) => {
    const child = spawn("xberg", args, {
      cwd: directory,
      env: process.env,
      signal: context?.abort,
      stdio: ["ignore", "pipe", "pipe"],
    });

    const stdout = [];
    const stderr = [];

    child.stdout.on("data", (chunk) => stdout.push(chunk));
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.on("error", (error) => {
      if (error.code === "ENOENT") {
        resolve({
          title: "xberg CLI not found",
          output:
            "Install the xberg CLI with `brew install xberg-io/tap/xberg`, or run it via `npx -y @xberg-io/xberg-cli` / `uvx --from xberg-cli xberg`.",
          metadata: { exitCode: 127, command: "xberg", subcommand: args[0] },
        });
        return;
      }
      reject(error);
    });
    child.on("close", (exitCode, signal) => {
      const stdoutText = Buffer.concat(stdout).toString("utf8").trim();
      const stderrText = Buffer.concat(stderr).toString("utf8").trim();
      const output = [stdoutText, stderrText && `stderr:\n${stderrText}`].filter(Boolean).join("\n\n");

      resolve({
        title: exitCode === 0 ? `xberg ${args[0]}` : `xberg ${args[0]} failed`,
        output: output || "(no output)",
        metadata: {
          exitCode,
          signal,
          command: "xberg",
          subcommand: args[0],
        },
      });
    });
  });
}

export const XbergPlugin = () =>
  Promise.resolve({
    tool: {
      xberg_extract: tool({
        description: "Extract text, tables, metadata, and images from a local document with the xberg CLI.",
        args: {
          path: schema.string().min(1).describe("Path to the local document."),
          format: wireFormat,
          content_format: contentFormat,
          mime_type: schema.string().min(1).optional().describe("Optional MIME type hint."),
          config_json: schema.string().min(2).optional().describe("Optional ExtractionConfig JSON."),
        },
        async execute(args, context) {
          validateJson(args.config_json, "config_json");

          const cliArgs = ["extract", args.path, "--format", args.format];
          pushOption(cliArgs, "--content-format", args.content_format);
          pushOption(cliArgs, "--mime-type", args.mime_type);
          pushOption(cliArgs, "--config-json", args.config_json);

          return await runCli(cliArgs, context);
        },
      }),
      xberg_detect: tool({
        description: "Detect the MIME type for a local file with the xberg CLI.",
        args: {
          path: schema.string().min(1).describe("Path to the local file."),
          format: wireFormat,
        },
        async execute(args, context) {
          return await runCli(["detect", args.path, "--format", args.format], context);
        },
      }),
      xberg_formats: tool({
        description: "List document formats supported by the xberg CLI.",
        args: {
          format: wireFormat,
        },
        async execute(args, context) {
          return await runCli(["formats", "--format", args.format], context);
        },
      }),
    },
  });

export default XbergPlugin;
