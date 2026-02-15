# Rust → JS 构建联动快速开始

## 核心需求

修改任何 Rust 代码 → 自动触发 `@rspack/binding` 和 `@rspack/core` 构建

```
┌─────────────────────────────────────────────────────────────┐
│  修改 crates/*/src/*.rs (任意 Rust 文件)                      │
└───────────────────────┬─────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────┐
│  @rspack/binding:build (napi-rs 重新编译 .node)              │
│  - inputs: 所有 crates/**/*.rs                               │
└───────────────────────┬─────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────┐
│  @rspack/core:build (重新编译 JS 包)                         │
│  - dependsOn: @rspack/binding:build                          │
└─────────────────────────────────────────────────────────────┘
```

---

## 第一步：安装 Nx

```bash
pnpm add -D nx
```

---

## 第二步：创建 nx.json

```json
{
  "namedInputs": {
    "rustSources": [
      "{workspaceRoot}/crates/**/*.rs",
      "{workspaceRoot}/crates/**/Cargo.toml",
      "!{workspaceRoot}/crates/**/target/**/*"
    ]
  },

  "targetDefaults": {
    "build": {
      "dependsOn": ["^build"],
      "cache": true
    },
    
    "@rspack/binding:build": {
      "inputs": [
        "{projectRoot}/**/*",
        "rustSources"
      ]
    },

    "@rspack/core:build": {
      "dependsOn": ["@rspack/binding:build"],
      "inputs": [
        "{projectRoot}/src/**/*",
        "{workspaceRoot}/crates/node_binding/**/*"
      ]
    }
  }
}
```

---

## 第三步：为关键包添加 project.json

### crates/node_binding/project.json

```json
{
  "name": "@rspack/binding",
  "sourceRoot": "crates/node_binding/src",
  "projectType": "library",
  "targets": {
    "build": {
      "executor": "nx:run-commands",
      "options": {
        "command": "napi build --platform",
        "cwd": "crates/node_binding"
      }
    }
  }
}
```

### packages/rspack/project.json

```json
{
  "name": "@rspack/core",
  "sourceRoot": "packages/rspack/src",
  "projectType": "library",
  "targets": {
    "build": {
      "executor": "nx:run-commands",
      "options": {
        "command": "pnpm rslib build",
        "cwd": "packages/rspack"
      }
    }
  }
}
```

---

## 第四步：测试

```bash
# 查看项目图
nx graph

# 测试构建（首次会构建 binding）
nx build @rspack/core

# 修改任意 Rust 文件
touch crates/rspack_core/src/lib.rs

# 再次构建（应该触发 binding 和 core）
nx build @rspack/core

# 输出应该包含：
# - @rspack/binding:build
# - @rspack/core:build
```

---

## 工作原理

```
Rust 文件修改
    │
    ├── inputs: rustSources ──┐
    │                          ▼
    │              ┌───────────────────┐
    └─────────────►│ 计算文件哈希变化  │
                   └─────────┬─────────┘
                             │
              是 ◄── 变化？ ──┤
              │              │ 否
              ▼              │
    ┌───────────────────┐    │
    │ 标记为需重建      │    │
    │ @rspack/binding   │    │
    └─────────┬─────────┘    │
              │              │
              ▼              │
    ┌───────────────────┐    │
    │ 执行 napi build   │    │
    │ 生成 .node 文件   │    │
    └─────────┬─────────┘    │
              │              │
              ├── dependsOn ─┘
              │
              ▼
    ┌───────────────────┐
    │ 构建 @rspack/core │
    └───────────────────┘
```

---

## 进阶：排除不需要监听的 crate

如果某些 crate 的修改不应该触发重建（如纯测试 crate）：

```json
{
  "namedInputs": {
    "rustSources": [
      "{workspaceRoot}/crates/**/*.rs",
      "!{workspaceRoot}/crates/**/target/**/*",
      "!{workspaceRoot}/crates/rspack_test*/**/*.rs",
      "!{workspaceRoot}/crates/*_test/**/*.rs"
    ]
  }
}
```

---

## 常见问题

**Q: 修改 `crates/rspack_util` 会触发构建吗？**
A: 会。因为 `rustSources` 包含 `crates/**/*.rs`，任何 crate 的修改都会触发。

**Q: 如何只让特定 crate 触发构建？**
A: 明确列出要监听的 crate：

```json
"inputs": [
  "{workspaceRoot}/crates/rspack_core/**/*.rs",
  "{workspaceRoot}/crates/rspack_binding_api/**/*.rs",
  "{workspaceRoot}/crates/node_binding/**/*.rs"
]
```

**Q: 这会触发其他 crates 的 `cargo build` 吗？**
A: 不会。Nx 不会自动为其他 crates 添加 `cargo build` 任务。
