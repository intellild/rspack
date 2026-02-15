/**
 * Rust Binding Sync Plugin
 *
 * 功能：任何 Rust 代码修改时，触发 @rspack/binding 重新构建（napi-rs）
 * 但不触发其他 crates 的 cargo build
 *
 * 原理：
 * 1. 使用 createNodesV2 识别所有 Cargo.toml（建立项目图）
 * 2. 为 @rspack/binding 的 build target 添加全局 Rust 文件作为 inputs
 * 3. 其他 crates 保持 implicit（无 targets，不参与构建）
 */

const { dirname, relative, join } = require('path');
const { existsSync } = require('fs');

const PLUGIN_NAME = '@rspack/rust-binding-sync';

/**
 * 定义哪些 crate 需要作为独立 Nx 项目（有 targets）
 */
const EXPLICIT_CRATES = ['rspack_node']; // node_binding 的 crate 名

/**
 * 监听所有 Rust 源码文件
 * 这些将作为 @rspack/binding build 的 inputs
 */
const RUST_SOURCE_GLOBS = [
  '{workspaceRoot}/crates/**/*.rs',
  '!{workspaceRoot}/crates/**/target/**/*',
  '!{workspaceRoot}/crates/**/*.gen.rs',
];

/**
 * 获取 Cargo metadata
 */
function getCargoMetadata(workspaceRoot) {
  try {
    const { execSync } = require('child_process');
    const output = execSync('cargo metadata --format-version 1 --no-deps', {
      cwd: workspaceRoot,
      encoding: 'utf-8',
      maxBuffer: 10 * 1024 * 1024,
    });
    return JSON.parse(output);
  } catch (e) {
    console.warn(`[${PLUGIN_NAME}] Failed to get cargo metadata:`, e.message);
    return null;
  }
}

/**
 * createNodesV2 - 处理 Cargo.toml 文件
 */
const createNodesV2 = [
  '*/**/Cargo.toml',

  async (configFilePaths, options, context) => {
    const { workspaceRoot, nxJsonConfiguration } = context;
    const metadata = getCargoMetadata(workspaceRoot);

    if (!metadata) {
      return { projects: {}, externalNodes: {} };
    }

    const projects = {};

    for (const pkg of metadata.packages) {
      const manifestDir = dirname(pkg.manifest_path);
      const relativeRoot = relative(workspaceRoot, manifestDir);
      const isExplicit = EXPLICIT_CRATES.includes(pkg.name);

      // 只处理 workspace 内的 crates
      if (pkg.source) continue; // 跳过外部依赖

      if (isExplicit) {
        // 核心 crate（如 node_binding）：创建完整项目配置
        projects[relativeRoot] = {
          root: relativeRoot,
          name:
            pkg.name === 'rspack_node'
              ? '@rspack/node-binding-crate'
              : pkg.name,
          sourceRoot: join(relativeRoot, 'src'),
          projectType: 'library',
          targets: {
            // napi-rs 构建任务
            build: {
              executor: 'nx:run-commands',
              options: {
                command:
                  'napi build --platform --js binding.js --dts binding.d.ts',
                cwd: relativeRoot,
              },
              // 关键：监听所有 Rust 源码
              inputs: ['{projectRoot}/**/*', ...RUST_SOURCE_GLOBS],
              outputs: [
                '{projectRoot}/binding.js',
                '{projectRoot}/binding.d.ts',
                '{projectRoot}/*.node',
                '{projectRoot}/*.wasm',
              ],
            },
            test: {
              executor: '@monodon/rust:test',
            },
            lint: {
              executor: '@monodon/rust:lint',
            },
          },
          tags: ['rust', 'binding', 'napi'],
        };
      } else {
        // 其他 lib crates：创建 implicit 项目（仅用于依赖图，无 targets）
        projects[relativeRoot] = {
          root: relativeRoot,
          name: `rust:${pkg.name}`,
          sourceRoot: join(relativeRoot, 'src'),
          projectType: 'library',
          // implicit: true 表示不生成 runnable targets
          implicit: true,
          targets: {},
          tags: ['rust', 'lib'],
        };
      }
    }

    return { projects, externalNodes: {} };
  },
];

/**
 * createDependencies - 创建 crate 间的依赖关系（用于正确的构建顺序）
 */
const createDependencies = (graph, context) => {
  const dependencies = [];
  const { projects } = graph;
  const metadata = getCargoMetadata(context.workspaceRoot);

  if (!metadata) return dependencies;

  // 建立名称到项目的映射
  const projectByName = new Map();
  for (const [root, project] of Object.entries(projects)) {
    projectByName.set(project.name, project);
    // 同时记录 rust: 前缀的名称
    if (project.name.startsWith('rust:')) {
      projectByName.set(project.name.replace('rust:', ''), project);
    }
  }

  // 解析 Cargo 依赖
  for (const pkg of metadata.packages) {
    if (pkg.source) continue;

    const sourceProject =
      projectByName.get(pkg.name) || projectByName.get(`rust:${pkg.name}`);

    if (!sourceProject) continue;

    for (const dep of pkg.dependencies) {
      const targetProject =
        projectByName.get(dep.name) || projectByName.get(`rust:${dep.name}`);

      if (targetProject && targetProject !== sourceProject) {
        dependencies.push({
          source: sourceProject.name,
          target: targetProject.name,
          type: 'static',
          sourceFile: relative(context.workspaceRoot, pkg.manifest_path),
        });
      }
    }
  }

  return dependencies;
};

/**
 * 生成推荐的 nx.json 配置
 * 此函数可以独立运行，输出建议配置
 */
function generateNxJsonConfig(workspaceRoot) {
  return {
    plugins: [
      {
        plugin: './tools/rust-binding-sync-plugin.js',
        options: {},
      },
    ],
    targetDefaults: {
      // 确保 @rspack/binding（JS 包）依赖 node-binding-crate 的构建
      '@rspack/binding:build': {
        dependsOn: ['@rspack/node-binding-crate:build'],
        inputs: [
          '{projectRoot}/**/*',
          // 关键：任何 Rust 文件变化都触发
          '{workspaceRoot}/crates/**/*.rs',
          '!{workspaceRoot}/crates/**/target/**/*',
        ],
      },
      // @rspack/core 依赖 binding
      '@rspack/core:build': {
        dependsOn: ['@rspack/binding:build'],
        inputs: [
          '{projectRoot}/src/**/*',
          '{workspaceRoot}/crates/node_binding/**/*',
        ],
      },
    },
    namedInputs: {
      // 定义可复用的 Rust 源码输入
      rustSources: [
        '{workspaceRoot}/crates/**/*.rs',
        '{workspaceRoot}/crates/**/Cargo.toml',
        '!{workspaceRoot}/crates/**/target/**/*',
      ],
    },
  };
}

module.exports = {
  name: PLUGIN_NAME,
  createNodesV2,
  createDependencies,
  generateNxJsonConfig,
};
