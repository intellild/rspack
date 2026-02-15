# Rspack Monorepo 依赖关系分析与 nx 管理可行性报告

## 一、项目依赖关系现状

### 1.1 架构概览

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Rspack Monorepo                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│  Rust Layer (crates/)                                                        │
│  ├── Core: rspack_core (核心编译逻辑)                                         │
│  ├── Binding: rspack_binding_api → rspack_node (node_binding/)              │
│  ├── Plugins: rspack_plugin_* (50+ 插件)                                     │
│  └── Support: rspack_*, swc_plugin_* (工具库)                                │
├─────────────────────────────────────────────────────────────────────────────┤
│  JavaScript Layer (packages/)                                                │
│  ├── @rspack/binding (crates/node_binding 的 NAPI 包装)                      │
│  ├── @rspack/core (依赖 @rspack/binding)                                     │
│  ├── @rspack/cli (依赖 @rspack/core)                                         │
│  ├── @rspack/test-tools (测试工具)                                           │
│  └── create-rspack (脚手架)                                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│  Test Layer (tests/)                                                         │
│  ├── @rspack/tests (集成测试，依赖 @rspack/cli, @rspack/core)                │
│  └── e2e-test (端到端测试)                                                   │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 1.2 关键依赖链

| 依赖方向  | 路径                                                             | 说明             |
| --------- | ---------------------------------------------------------------- | ---------------- |
| Rust → JS | `crates/*` → `node_binding` → `@rspack/binding` → `@rspack/core` | 通过 NAPI 绑定   |
| JS → JS   | `@rspack/core` → `@rspack/cli`                                   | workspace 依赖   |
| Test → JS | `@rspack/tests` → `@rspack/cli`/`@rspack/core`                   | 测试依赖构建产物 |

### 1.3 当前构建流程

```bash
# 当前在 package.json 中的构建顺序
1. build:binding:dev    # 构建 Rust binding (pnpm --filter @rspack/binding run build:dev)
2. build:js            # 构建 JS 包 (先 @rspack/core, 后其他)

# 隐含依赖：build:binding:dev 内部执行 cargo build + napi-rs 生成 .node/.wasm
```

---

## 二、Nx 管理方案可行性分析

### 2.1 Nx 核心能力匹配度

| 能力       | Rspack 需求                 | 匹配度                   |
| ---------- | --------------------------- | ------------------------ |
| 任务依赖图 | Rust crates + JS 包混合依赖 | ✅ 支持                  |
| 增量构建   | 只构建修改的部分            | ✅ 支持 (需配置输入输出) |
| 远程缓存   | CI 构建加速                 | ✅ 支持                  |
| 分布式执行 | 大规模并行构建              | ✅ 支持                  |
| Rust 支持  | Cargo workspace             | ⚠️ 需插件                |

### 2.2 Rust 相关插件选项

#### 选项 A: @monodon/rust (官方推荐)

```bash
npm install -D @monodon/rust
```

**功能**: 解析 `Cargo.toml`，为每个 crate 生成 Nx project

**优点**:

- 官方维护，与 Nx Release 集成
- 支持 `cargo build`/`cargo test` 作为 Nx targets
- 支持 changelog 生成和 crates.io 发布

**缺点**:

- 需要 `useLegacyVersioning: true` (已知限制)
- 对 napi-rs 的特殊构建流程支持有限

**适用场景**: 纯 Rust crate 管理，发布流程

#### 选项 B: @nxrs/cargo (社区插件)

```bash
npm install -D @nxrs/cargo
```

**功能**: 提供 generators 和 executors 管理 Rust 项目

**状态**: WIP，功能较基础

**评价**: 不推荐用于生产环境

#### 选项 C: 自定义 Project Graph Plugin

```javascript
// tools/rust-plugin.js
const { execSync } = require('child_process');
const { readFileSync } = require('fs');
const toml = require('@iarna/toml');

// 解析 Cargo.toml 生成 Nx project nodes 和 dependencies
module.exports = {
  processProjectGraph: (graph, context) => {
    // 读取所有 crates/*/Cargo.toml
    // 为每个 crate 创建 project node
    // 解析 dependencies 创建 edges
    return graph;
  },
};
```

**优点**: 完全自定义，可处理 napi-rs 特殊流程

**缺点**: 开发维护成本高

### 2.3 JS 包在 Rust 修改时更新的可行性

**挑战**: `@rspack/binding` 是一个特殊包：

1. 它的构建产物来自 Rust `node_binding` crate
2. `.node` 文件是二进制文件，需要重新编译
3. `@rspack/core` 依赖 `@rspack/binding`

**解决方案**:

```json
// nx.json 配置示意
{
  "targetDefaults": {
    "build": {
      "dependsOn": ["^build"],
      "inputs": [
        "{projectRoot}/src/**/*",
        // 关键：让 @rspack/binding 监听 Rust 源码
        {
          "dependentTasksOutputFiles": "**/*",
          "transitive": true
        }
      ]
    }
  },
  "plugins": [
    {
      "plugin": "@monodon/rust",
      "options": {
        "cargoWorkspaceRoot": "."
      }
    }
  ]
}
```

```json
// packages/rspack/package.json (project.json)
{
  "name": "@rspack/core",
  "targets": {
    "build": {
      "dependsOn": ["@rspack/binding:build"],
      "inputs": [
        "{projectRoot}/src/**/*",
        // 监听 binding 变化
        "{workspaceRoot}/crates/node_binding/**/*.rs",
        "{workspaceRoot}/crates/rspack_binding_api/**/*.rs"
      ]
    }
  }
}
```

**可行性**: ✅ **可行但复杂**

- 需要为 `@rspack/binding` 创建自定义 executor 处理 napi-rs 构建
- 需要配置 Rust 源码作为 JS 包的输入依赖

---

## 三、Nx 完整配置方案

### 3.1 推荐的混合架构

```
├── nx.json                 # Nx 配置
├── project.json            # Root 项目 (可选)
├── crates/
│   ├── rspack_core/        # @monodon/rust 自动生成 project
│   ├── rspack_binding_api/ # @monodon/rust 自动生成 project
│   └── node_binding/       # 自定义 project (napi-rs 特殊处理)
│       └── project.json
├── packages/
│   ├── rspack/             # Nx JS project
│   │   └── project.json
│   ├── rspack-cli/         # Nx JS project
│   │   └── project.json
│   └── rspack-test-tools/
│       └── project.json
└── tests/
    └── rspack-test/        # 测试项目
        └── project.json
```

### 3.2 关键配置示例

```json
// nx.json
{
  "extends": "nx/presets/npm.json",
  "npmScope": "rspack",
  "tasksRunnerOptions": {
    "default": {
      "runner": "nx/tasks-runners/default",
      "options": {
        "cacheableOperations": ["build", "test", "lint"],
        "parallel": 8
      }
    }
  },
  "targetDefaults": {
    "build": {
      "dependsOn": ["^build"],
      "inputs": ["production", "^production"]
    },
    "test": {
      "dependsOn": ["build"],
      "inputs": ["default", "^production"]
    }
  },
  "namedInputs": {
    "default": ["{projectRoot}/**/*"],
    "production": ["!{projectRoot}/**/*.spec.ts", "!{projectRoot}/**/*.test.ts"]
  },
  "plugins": ["@monodon/rust"],
  "release": {
    "projects": ["crates/*", "packages/*"],
    "version": {
      "useLegacyVersioning": true
    }
  }
}
```

```json
// crates/node_binding/project.json
{
  "name": "@rspack/node-binding",
  "sourceRoot": "crates/node_binding",
  "projectType": "library",
  "targets": {
    "build": {
      "executor": "nx:run-commands",
      "options": {
        "command": "napi build --platform",
        "cwd": "crates/node_binding"
      },
      "inputs": [
        "{workspaceRoot}/crates/**/*.rs",
        "!{workspaceRoot}/crates/**/target/**/*"
      ],
      "outputs": [
        "{projectRoot}/binding.js",
        "{projectRoot}/binding.d.ts",
        "{projectRoot}/*.node"
      ]
    }
  }
}
```

```json
// packages/rspack/project.json
{
  "name": "@rspack/core",
  "sourceRoot": "packages/rspack/src",
  "projectType": "library",
  "targets": {
    "build": {
      "executor": "@nx/js:tsc",
      "dependsOn": ["@rspack/node-binding:build"],
      "options": {
        "main": "packages/rspack/src/index.ts",
        "tsConfig": "packages/rspack/tsconfig.json",
        "outputPath": "packages/rspack/dist"
      },
      "inputs": [
        "{projectRoot}/src/**/*",
        // 关键：Rust 修改触发 JS 重新构建
        "{workspaceRoot}/crates/node_binding/**/*.rs",
        "{workspaceRoot}/crates/rspack_binding_api/**/*.rs"
      ]
    }
  }
}
```

---

## 四、可行性结论

| 场景                    | 可行性 | 复杂度 | 推荐方案             |
| ----------------------- | ------ | ------ | -------------------- |
| 纯 Rust crate 管理      | ✅ 高  | 低     | @monodon/rust        |
| Rust + JS 混合构建      | ⚠️ 中  | 高     | 自定义配置           |
| Test 依赖部分项目 Build | ✅ 高  | 中     | Nx task dependencies |
| Rust 修改触发 JS 更新   | ⚠️ 中  | 高     | 配置 inputs 监听     |
| 发布流程管理            | ✅ 高  | 低     | Nx Release           |

**总体评估**:

- **短期**: 可以引入 Nx 管理 JS 包和测试流程，Rust 部分保持 cargo workspace
- **长期**: 需要开发自定义插件才能完全发挥 Nx 能力

---

## 五、脚本管理工具推荐 (替代 zx)

当前 `x.mjs` 使用 zx，以下是替代方案：

### 5.1 推荐工具列表

| 工具           | 语言 | 特点                             | 适用场景                 |
| -------------- | ---- | -------------------------------- | ------------------------ |
| **just**       | Rust | Makefile 语法，无 build 系统开销 | 简单命令别名、任务链     |
| **cargo-make** | Rust | Rust 原生，Cargo 集成            | Rust 项目任务管理        |
| **moonrepo**   | Rust | 完整 monorepo 方案，类 Nx        | 替代 Nx 的轻量方案       |
| **Taskfile**   | Go   | 跨平台，YAML 配置                | 团队协作，CI 友好        |
| **pnpm 内置**  | JS   | 无需额外依赖                     | 简单脚本，利用 pnpm 过滤 |

### 5.2 推荐配置示例

#### 方案 A: just (最推荐，简单任务)

```justfile
# Justfile
set shell := ["bash", "-cu"]

# 默认显示帮助
default:
    @just --list

# 变量
export CARGO_TERM_COLOR := "always"
export FORCE_COLOR := "3"

# 任务
ready:
    cargo check
    cargo lint
    cargo test
    pnpm install
    pnpm run build:cli:release
    pnpm run test:unit
    echo "All passed."

build mode="dev":
    pnpm --filter @rspack/binding build:{{mode}}
    pnpm --filter "@rspack/*" build

test target="unit":
    #!/usr/bin/env bash
    if [ "{{target}}" = "unit" ]; then
        ./x build js
        pnpm --filter "@rspack/*" test
    elif [ "{{target}}" = "rust" ]; then
        cargo test
    fi

clean:
    cargo clean
```

#### 方案 B: cargo-make (Rust 深度集成)

```toml
# Makefile.toml
[config]
default_to_workspace = false

[env]
CARGO_TERM_COLOR = "always"
FORCE_COLOR = "3"

[tasks.ready]
dependencies = ["check", "lint", "test-rust", "install", "build-release", "test-unit"]

[tasks.build-binding]
command = "pnpm"
args = ["--filter", "@rspack/binding", "build:dev"]

[tasks.build-js]
command = "pnpm"
args = ["--filter", "@rspack/core", "build"]

[tasks.build]
dependencies = ["build-binding", "build-js"]
```

#### 方案 C: moonrepo (Nx 替代)

```yaml
# .moon/workspace.yml
$schema: 'https://moonrepo.dev/schemas/workspace.json'
projects:
  - 'packages/*'
  - 'crates/*'

# .moon/tasks.yml
tasks:
  build:
    command: 'pnpm run build'
    inputs:
      - 'src/**/*'
    outputs:
      - 'dist'

  test:
    command: 'pnpm run test'
    deps: ['build']
```

#### 方案 D: pnpm 内置 (最小改动)

```json
// package.json 利用 pnpm 的 --filter 和 -r
{
  "scripts": {
    "x:ready": "pnpm run '/^x:(check|lint|test-rs|setup|build:cli:release|test:unit)$/'",
    "x:check": "cargo check",
    "x:lint": "cargo lint",
    "x:test-rs": "cargo test",
    "x:setup": "pnpm install",
    "x:build:cli:release": "pnpm run build:cli:release",
    "x:test:unit": "pnpm run test:unit",
    "x:build": "pnpm -r --filter @rspack/binding --filter \"@rspack/*\" run build",
    "x:test": "pnpm -r --filter \"@rspack/*\" run test"
  }
}
```

---

## 六、实施路线图

```
Phase 1: 脚本迁移 (低风险)
├── 引入 just/cargo-make 替代 zx
└── 保留现有 package.json scripts

Phase 2: JS 层 Nx 化 (中风险)
├── 配置 Nx 管理 packages/*
├── 利用 pnpm workspace 集成
└── 实现测试依赖部分项目构建

Phase 3: Rust 集成 (高风险)
├── 评估 @monodon/rust
├── 开发 napi-rs 自定义 executor
└── 实现 Rust→JS 依赖链

Phase 4: 发布流程
├── 迁移到 Nx Release
└── 统一 Rust crates 和 npm 包版本管理
```

---

## 七、总结建议

1. **脚本工具**: 推荐 **just** 作为 zx 替代，轻量且 Rust 生态友好
2. **Nx 采用**: 建议从 JS 层开始试点，Rust 集成需要更多调研
3. **Rust 触发 JS 更新**: 技术上可行，但需要精心配置 inputs/outputs
4. **渐进迁移**: 不需要一次性替换，可以共存逐步过渡

---

## 八、参考资源

- [Nx Release with Rust Guide](https://nx.dev/docs/guides/nx-release/publish-rust-crates)
- [@monodon/rust Plugin](https://www.npmjs.com/package/@monodon/rust)
- [moonrepo 文档](https://moonrepo.dev/docs)
- [just 命令运行器](https://github.com/casey/just)
- [cargo-make 文档](https://sagiegurari.github.io/cargo-make/)
