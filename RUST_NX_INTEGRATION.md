# Rust ↔ Nx 构建集成方案

## 需求理解

**目标**：任何 Rust 代码修改时，触发 `@rspack/binding` 重新构建，但**不**触发其他 crates 的 `cargo build`。

```
修改 crates/rspack_core/src/*.rs 
    ↓
触发 @rspack/binding:build (napi-rs 构建)
    ↓
触发 @rspack/core:build (JS 包重新编译)
    ↓
不触发其他 lib crates 的构建
```

---

## 方案一：纯配置方案（推荐）

不需要写插件，只需在 `nx.json` 中配置 `targetDefaults`。

### 1. 配置 namedInputs

```json
{
  "namedInputs": {
    "rustSources": [
      "{workspaceRoot}/crates/**/*.rs",
      "{workspaceRoot}/crates/**/Cargo.toml",
      "{workspaceRoot}/Cargo.lock",
      "!{workspaceRoot}/crates/**/target/**/*"
    ]
  }
}
```

### 2. 配置 targetDefaults

```json
{
  "targetDefaults": {
    "@rspack/binding:build": {
      "dependsOn": [],
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

### 3. 效果

| 操作 | 触发构建 |
|------|---------|
| 修改 `crates/rspack_core/src/*.rs` | ✅ `@rspack/binding` → `@rspack/core` |
| 修改 `crates/rspack_plugin_*/src/*.rs` | ✅ `@rspack/binding` → `@rspack/core` |
| 修改 `crates/rspack_util/src/*.rs` | ✅ `@rspack/binding` → `@rspack/core` |
| 运行 `cargo build -p rspack_core` | ❌ 不触发（这是 cargo 命令，非 Nx） |

---

## 方案二：自定义插件方案

如果需要更灵活的控制（如按 crate 类型区分），使用提供的插件。

### 1. 安装插件

```bash
# 插件已包含在 tools/ 目录，无需安装
```

### 2. 配置 nx.json

```json
{
  "plugins": [
    {
      "plugin": "./tools/rust-binding-sync-plugin.js"
    }
  ]
}
```

### 3. 插件行为

```javascript
// 插件逻辑
for (每个 Cargo.toml) {
  if (crate 是 rspack_node) {
    // 创建完整项目，有 build/test/lint targets
    projects[root] = { targets: { build: {...} } }
  } else {
    // 其他 crate：创建 implicit 项目（无 targets）
    projects[root] = { implicit: true, targets: {} }
  }
}
```

---

## 方案对比

| 特性 | 纯配置方案 | 插件方案 |
|------|-----------|---------|
| 复杂度 | 低 | 中 |
| 灵活性 | 中 | 高 |
| 维护成本 | 低 | 中 |
| 可按 crate 定制 | ❌ | ✅ |
| 适合 Rspack | ✅ | 可选 |

---

## 关键配置详解

### `inputs` 语法

```json
"inputs": [
  "{projectRoot}/src/**/*",           // 本项目的 src 目录
  "{workspaceRoot}/crates/**/*.rs",   // 所有 Rust 源文件
  "^production",                       // 依赖项目的 production 输出
  "!{workspaceRoot}/crates/**/target/**/*" // 排除 target 目录
]
```

### `dependsOn` 语法

```json
"dependsOn": [
  "^build",                    // 所有依赖项目的 build
  "@rspack/binding:build",     // 特定项目的 build
  { "projects": ["a", "b"], "target": "build" } // 多个项目
]
```

---

## 验证配置

```bash
# 1. 查看项目图
nx graph

# 2. 模拟构建（查看哪些任务会被执行）
nx build @rspack/binding --dry-run

# 3. 修改一个 Rust 文件，测试增量构建
touch crates/rspack_core/src/lib.rs
nx build @rspack/core --skip-nx-cache

# 应该输出：
# - @rspack/binding:build (因为 rustSources 变化)
# - @rspack/core:build (因为 binding 变化)
```

---

## 进阶：精确控制触发范围

如果只想让**特定 crate** 的修改触发构建：

```json
{
  "targetDefaults": {
    "@rspack/binding:build": {
      "inputs": [
        "{projectRoot}/**/*",
        // 只监听核心 crates
        "{workspaceRoot}/crates/rspack_core/**/*.rs",
        "{workspaceRoot}/crates/rspack_binding_api/**/*.rs",
        "{workspaceRoot}/crates/node_binding/**/*.rs",
        "{workspaceRoot}/crates/rspack_plugin_javascript/**/*.rs",
        "{workspaceRoot}/crates/rspack_plugin_asset/**/*.rs",
        "{workspaceRoot}/crates/rspack_loader_swc/**/*.rs"
      ]
    }
  }
}
```

---

## Troubleshooting

### 问题：修改 Rust 文件后 JS 包没有重建

检查：
1. `inputs` 配置是否正确包含 Rust 文件路径
2. 是否使用了 `--skip-nx-cache` 测试
3. `nx graph` 中 `@rspack/binding` 是否依赖 Rust 文件

### 问题：所有 crate 都被构建了

检查：
1. 是否配置了 `@monodon/rust` 插件（它会为所有 crate 添加 targets）
2. 目标 crate 是否设置为 `implicit: true`

### 问题：Cargo.lock 变化没有触发构建

添加：
```json
"inputs": [
  "{workspaceRoot}/Cargo.lock"
]
```
