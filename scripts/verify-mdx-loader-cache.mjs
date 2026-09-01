#!/usr/bin/env zx

import { strict as assert } from 'node:assert';
import { readFile, stat, unlink, utimes, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { $, chalk } from 'zx';

const SCRIPT_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT_DIR = path.resolve(SCRIPT_DIR, '..');
const WEBSITE_DIR = path.join(ROOT_DIR, 'website');
const CACHE_MDX_PATH = path.join(WEBSITE_DIR, 'docs/zh/config/cache.mdx');
const ENTRY_MDX_PATH = path.join(WEBSITE_DIR, 'docs/zh/config/entry.mdx');
const FILE_DEPENDENCY_PATH = path.join(WEBSITE_DIR, 'docs/zh/config/file.js');
const CACHE_MDX_RELATIVE_PATH = 'zh/config/cache.mdx';
const ENTRY_MDX_RELATIVE_PATH = 'zh/config/entry.mdx';
const CACHE_MODULE_REQUEST =
  './docs/zh/config/cache.mdx!lazy-compilation-proxy';
const ENTRY_MODULE_REQUEST =
  './docs/zh/config/entry.mdx!lazy-compilation-proxy';
const PROBE_PREFIX = '[mdx-loader-cache-probe]';
const DEPENDENCY_PROBE_VALUE = 'zx-mdx-loader-cache-dependency';
const TIMEOUT_MS = 60_000;
const TRACE_PATH = path.join(
  tmpdir(),
  `rspack-mdx-loader-cache-probe-${process.pid}.log`,
);

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const rspressRoot = path.join(WEBSITE_DIR, 'node_modules/@rspress/core');
const mdxLoaderPath = path.join(rspressRoot, 'dist/node/mdx/loader.js');
const rspressBin = path.join(WEBSITE_DIR, 'node_modules/.bin/rspress');
const rspressConfigPath = path.join(
  WEBSITE_DIR,
  'node_modules/rstack/dist/rspressConfig.js',
);
const rspackPackagePath = path.join(
  WEBSITE_DIR,
  'node_modules/@rspack/core/package.json',
);

const trackedFiles = new Map();
let originalLoaderSource;
let activeDevServer;
let cleanupStarted = false;
let sourceVersion = 0;

async function rememberFile(filePath) {
  const fileStat = await stat(filePath);
  trackedFiles.set(filePath, {
    content: await readFile(filePath),
    atime: fileStat.atime,
    mtime: fileStat.mtime,
  });
}

async function restoreFile(filePath, snapshot) {
  await writeFile(filePath, snapshot.content);
  await utimes(filePath, snapshot.atime, snapshot.mtime);
}

async function cleanup() {
  if (cleanupStarted) return;
  cleanupStarted = true;

  if (activeDevServer) {
    await activeDevServer.stop();
    activeDevServer = undefined;
  }

  if (originalLoaderSource !== undefined) {
    await writeFile(mdxLoaderPath, originalLoaderSource);
  }

  for (const [filePath, snapshot] of trackedFiles) {
    await restoreFile(filePath, snapshot);
  }

  await unlink(TRACE_PATH).catch((error) => {
    if (error.code !== 'ENOENT') throw error;
  });
}

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => {
    void cleanup().finally(() => {
      process.exit(signal === 'SIGINT' ? 130 : 143);
    });
  });
}

async function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      assert(address && typeof address === 'object');
      server.close((error) => {
        if (error) reject(error);
        else resolve(address.port);
      });
    });
  });
}

async function waitFor(predicate, description, getOutput) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < TIMEOUT_MS) {
    if (await predicate()) return;
    if (activeDevServer?.exited) {
      throw new Error(
        `Dev server exited while waiting for ${description}.\n${await getOutput()}`,
      );
    }
    await delay(50);
  }
  throw new Error(
    `Timed out waiting for ${description}.\n${await getOutput()}`,
  );
}

async function fetchText(url, options) {
  let lastError;
  const startedAt = Date.now();
  while (Date.now() - startedAt < TIMEOUT_MS) {
    try {
      const response = await fetch(url, options);
      if (response.ok) return response.text();
      lastError = new Error(`${response.status} ${response.statusText}`);
    } catch (error) {
      lastError = error;
    }
    await delay(100);
  }
  throw new Error(`Failed to fetch ${url}: ${lastError}`);
}

function lazyRouteId(indexSource, moduleRequest) {
  const line = indexSource
    .split('\n')
    .find((candidate) => candidate.includes(moduleRequest));
  assert(line, `Cannot find lazy route for ${moduleRequest}`);
  const match = line.match(/__webpack_require__\.e\([^\n]*?"([^"]+)"\)/);
  assert(match, `Cannot parse lazy route id from: ${line}`);
  return match[1];
}

function lazyRequest(routeSource, moduleRequest) {
  const moduleIndex = routeSource.indexOf(`"${moduleRequest}"`);
  assert(moduleIndex >= 0, `Cannot find lazy proxy for ${moduleRequest}`);
  const sourceAfterModule = routeSource.slice(moduleIndex);
  const match = sourceAfterModule.match(/var data = ("(?:[^"\\]|\\.)*");/);
  assert(match, `Cannot parse lazy request for ${moduleRequest}`);
  return JSON.parse(match[1]);
}

async function resolveLazyModule(origin, indexSource, moduleRequest) {
  const routeId = lazyRouteId(indexSource, moduleRequest);
  const routeUrl = `${origin}/static/js/async/${routeId}.js`;
  const routeSource = await fetchText(routeUrl);
  return {
    request: lazyRequest(routeSource, moduleRequest),
    routeUrl,
  };
}

async function readProbeOutput() {
  return readFile(TRACE_PATH, 'utf8').catch((error) => {
    if (error.code === 'ENOENT') return '';
    throw error;
  });
}

async function probeCount(resourcePath) {
  const output = await readProbeOutput();
  const probe = `${PROBE_PREFIX} ${resourcePath}`;
  return output.split(probe).length - 1;
}

async function prepareUniqueSources() {
  sourceVersion += 1;
  const marker = `zx-mdx-loader-cache-baseline-${process.pid}-${sourceVersion}`;

  for (const filePath of [CACHE_MDX_PATH, ENTRY_MDX_PATH]) {
    const originalSource = trackedFiles.get(filePath).content.toString();
    await writeFile(
      filePath,
      `${originalSource.trimEnd()}\n\n{/* ${marker} */}\n`,
    );
  }
}

async function startDevServer(cacheEnabled) {
  await prepareUniqueSources();
  await writeFile(TRACE_PATH, '');
  const port = await getFreePort();
  const origin = `http://127.0.0.1:${port}`;
  let output = '';
  let exited = false;

  const child = $({
    cwd: WEBSITE_DIR,
    env: {
      ...process.env,
      RSPACK_MDX_LOADER_CACHE: String(cacheEnabled),
    },
    nothrow: true,
    quiet: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  })`${rspressBin} dev --config ${rspressConfigPath} --host 127.0.0.1 --port ${port}`;

  const appendOutput = (chunk) => {
    const text = chunk.toString();
    output += text;
    process.stdout.write(text);
  };
  child.stdout.on('data', appendOutput);
  child.stderr.on('data', appendOutput);
  child.run();
  void child.then(() => {
    exited = true;
  });

  const server = {
    get exited() {
      return exited;
    },
    get output() {
      return output;
    },
    async stop() {
      if (exited) return;
      await child.kill('SIGINT');
      await Promise.race([child, delay(5_000)]);
      if (!exited) await child.kill('SIGKILL');
    },
  };
  activeDevServer = server;

  await waitFor(
    () => output.includes(`Local:    ${origin}/`),
    `${origin} to become ready`,
    () => output,
  );

  const indexSource = await fetchText(`${origin}/static/js/index.js`);
  const cacheModule = await resolveLazyModule(
    origin,
    indexSource,
    CACHE_MODULE_REQUEST,
  );
  const entryModule = await resolveLazyModule(
    origin,
    indexSource,
    ENTRY_MODULE_REQUEST,
  );
  await fetchText(`${origin}/_rspack/lazy/trigger`, {
    method: 'POST',
    headers: { 'Content-Type': 'text/plain' },
    body: `${cacheModule.request}\n${entryModule.request}`,
  });

  await waitFor(
    async () =>
      (await probeCount(CACHE_MDX_PATH)) === 1 &&
      (await probeCount(ENTRY_MDX_PATH)) === 1,
    'the initial MDX loader executions',
    async () => `${output}\nLoader trace:\n${await readProbeOutput()}`,
  );

  return { server, origin, cacheModule };
}

async function waitForBuild(server, outputOffset, resourcePath) {
  await waitFor(
    () => {
      const segment = server.output.slice(outputOffset);
      const buildStart = segment.indexOf(`building ${resourcePath}`);
      return (
        buildStart >= 0 &&
        segment.slice(buildStart).includes('ready   built in')
      );
    },
    `${resourcePath} to rebuild`,
    () => server.output.slice(outputOffset),
  );
  await delay(100);
}

async function touch(filePath) {
  const now = new Date();
  await utimes(filePath, now, now);
}

async function runCacheDisabledCase() {
  console.log(chalk.bold('\n[1/2] Cache disabled'));
  const { server } = await startDevServer(false);
  assert.equal(await probeCount(CACHE_MDX_PATH), 1);

  const outputOffset = server.output.length;
  await touch(CACHE_MDX_PATH);
  await waitForBuild(server, outputOffset, CACHE_MDX_RELATIVE_PATH);
  assert.equal(
    await probeCount(CACHE_MDX_PATH),
    2,
    'cache.mdx loader should run again when loader cache is disabled',
  );

  console.log(chalk.green('✓ touch(cache.mdx) executes the MDX loader'));
  await server.stop();
  activeDevServer = undefined;
}

async function runCacheEnabledCase() {
  console.log(chalk.bold('\n[2/2] Cache enabled'));
  const { server, origin, cacheModule } = await startDevServer(true);
  assert.equal(await probeCount(CACHE_MDX_PATH), 1);
  assert.equal(await probeCount(ENTRY_MDX_PATH), 1);

  let outputOffset = server.output.length;
  await touch(CACHE_MDX_PATH);
  await waitForBuild(server, outputOffset, CACHE_MDX_RELATIVE_PATH);
  assert.equal(
    await probeCount(CACHE_MDX_PATH),
    1,
    'cache.mdx loader should be skipped on a cache hit',
  );
  console.log(chalk.green('✓ touch(cache.mdx) hits the loader cache'));

  outputOffset = server.output.length;
  const originalEntry = trackedFiles.get(ENTRY_MDX_PATH).content.toString();
  await writeFile(
    ENTRY_MDX_PATH,
    `${originalEntry.trimEnd()}\n\n{/* zx-mdx-loader-cache-unrelated */}\n`,
  );
  await waitForBuild(server, outputOffset, ENTRY_MDX_RELATIVE_PATH);
  assert.equal(
    await probeCount(ENTRY_MDX_PATH),
    2,
    'entry.mdx loader should run after its content changes',
  );
  assert.equal(
    await probeCount(CACHE_MDX_PATH),
    1,
    'cache.mdx loader should not run after an unrelated MDX change',
  );
  console.log(chalk.green('✓ changing entry.mdx does not execute cache.mdx'));

  outputOffset = server.output.length;
  await writeFile(
    FILE_DEPENDENCY_PATH,
    `console.log('${DEPENDENCY_PROBE_VALUE}');\n`,
  );
  await waitForBuild(server, outputOffset, 'zh/config/file.js');

  const updatedRouteSource = await fetchText(cacheModule.routeUrl);
  const compiledChunkMatch = updatedRouteSource.match(
    /__webpack_require__\.e\("([^"]*docs_zh_config_cache_mdx[^"]*)"\)/,
  );
  assert(compiledChunkMatch, 'Cannot find the compiled cache.mdx chunk');
  const compiledSource = await fetchText(
    `${origin}/static/js/async/${compiledChunkMatch[1]}.js`,
  );
  const dependencyRefreshResult = {
    cacheMdxLoaderExecutions: await probeCount(CACHE_MDX_PATH),
    compiledOutputUpdated: compiledSource.includes(DEPENDENCY_PROBE_VALUE),
    entryMdxLoaderExecutions: await probeCount(ENTRY_MDX_PATH),
  };
  console.log('Dependency refresh result:', dependencyRefreshResult);
  assert.deepEqual(
    dependencyRefreshResult,
    {
      cacheMdxLoaderExecutions: 2,
      compiledOutputUpdated: true,
      entryMdxLoaderExecutions: 2,
    },
    'file.js should invalidate only cache.mdx and update its compiled output',
  );
  console.log(
    chalk.green('✓ changing file.js invalidates cache.mdx and updates output'),
  );

  await server.stop();
  activeDevServer = undefined;
}

async function installLoaderProbe() {
  originalLoaderSource = await readFile(mdxLoaderPath, 'utf8');
  const anchor = '    const filepath = this.resourcePath;';
  assert(
    originalLoaderSource.includes(anchor),
    `Cannot find the probe anchor in ${mdxLoaderPath}`,
  );
  assert(
    !originalLoaderSource.includes(PROBE_PREFIX),
    `A loader cache probe is already installed in ${mdxLoaderPath}`,
  );

  const traceImport = `import { appendFileSync as __appendMdxLoaderCacheTrace } from 'node:fs';`;
  const probe = `${anchor}\n    __appendMdxLoaderCacheTrace(${JSON.stringify(TRACE_PATH)}, '${PROBE_PREFIX} ' + filepath + '\\n');`;
  await writeFile(TRACE_PATH, '');
  await writeFile(
    mdxLoaderPath,
    `${traceImport}\n${originalLoaderSource.replace(anchor, probe)}`,
  );
}

async function main() {
  await Promise.all(
    [CACHE_MDX_PATH, ENTRY_MDX_PATH, FILE_DEPENDENCY_PATH].map(rememberFile),
  );
  await installLoaderProbe();
  const rspackPackage = JSON.parse(await readFile(rspackPackagePath, 'utf8'));
  console.log(`Rspack version: ${rspackPackage.version}`);
  console.log(`MDX loader: ${mdxLoaderPath}`);

  try {
    await runCacheDisabledCase();
    await runCacheEnabledCase();
    console.log(chalk.bold.green('\nAll MDX loader cache checks passed.'));
  } finally {
    await cleanup();
  }
}

await main();
