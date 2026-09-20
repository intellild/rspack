import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { rspack } from "@rspack/core";
import { createFsFromVolume, Volume } from "memfs";
import { closeCompiler, runCompiler } from "./helpers.mjs";

const require = createRequire(import.meta.url);
const threads = require("node:worker_threads");
const OriginalWorker = threads.Worker;
threads.Worker = class extends OriginalWorker {
  constructor() {
    super('throw new Error("intentional loader worker startup failure")', { eval: true });
  }
};

async function build(parallel) {
  const compiler = rspack({
    context: import.meta.dirname,
    mode: "development",
    entry: "./entry.js",
    module: { rules: [{ test: /entry\.js$/, use: [{
      loader: require.resolve("./subscription-loader.cjs"),
      options: { name: "startup", parallel },
      parallel: parallel ? { maxWorkers: 1 } : false,
    }] }] },
    name: "startup",
    output: { path: "/", filename: "bundle.js" },
  });
  compiler.outputFileSystem = createFsFromVolume(new Volume());
  try {
    const stats = await runCompiler(compiler);
    if (stats.hasErrors()) throw new Error(stats.toString());
  } finally {
    await closeCompiler(compiler);
  }
}

try {
  await assert.rejects(build(true), /intentional loader worker startup failure/);
} finally {
  threads.Worker = OriginalWorker;
}
// A failed worker pool must not prevent main-only compilers from consuming tasks.
await build(false);
