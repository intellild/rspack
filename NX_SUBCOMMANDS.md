# Nx 管理子命令依赖关系指南

## 概述

Nx 可以完美管理子命令依赖关系，例如 `test:wasm` 自动依赖 `build:wasm`。

```
test:wasm
    ↓ dependsOn
build:wasm
    ↓ dependsOn
cargo build (wasm target)
```

---

## 一、基础配置

### 1.1 全局配置（nx.json）

```json
{
  "targetDefaults": {
    "test:wasm": {
      "dependsOn": ["build:wasm"]
    },
    "test:node": {
      "dependsOn": ["build:node"]
    },
    "test": {
      "dependsOn": ["build"]
    }
  }
}
```

### 1.2 项目级配置（project.json）

```json
{
  "name": "@rspack/core",
  "targets": {
    "build:wasm": {
      "executor": "nx:run-commands",
      "options": {
        "command": "pnpm run build:browser"
      },
      "inputs": [
        "{workspaceRoot}/crates/**/*.rs",
        "{projectRoot}/**/*"
      ],
      "outputs": [
        "{projectRoot}/*.wasm"
      ]
    },
    "test:wasm": {
      "executor": "nx:run-commands",
      "options": {
        "command": "pnpm run test:browser"
      },
      "dependsOn": ["build:wasm"]
    }
  }
}
```

### 1.3 package.json 内联配置

```json
{
  "name": "@rspack/core",
  "scripts": {
    "build:wasm": "napi build --target wasm32-wasip1-threads",
    "test:wasm": "jest --config jest.wasm.config.js"
  },
  "nx": {
    "targets": {
      "build:wasm": {
        "inputs": ["{workspaceRoot}/crates/**/*.rs"],
        "outputs": ["{projectRoot}/*.wasm"]
      },
      "test:wasm": {
        "dependsOn": ["build:wasm"]
      }
    }
  }
}
```

---

## 二、运行效果

```bash
# 运行 test:wasm 会自动先执行 build:wasm
nx test:wasm @rspack/core

# 控制台输出：
# > nx run @rspack/core:build:wasm
#   ⠙ Building WASM target...
#   ✔ Build succeeded (15s)
#
# > nx run @rspack/core:test:wasm
#   ⠙ Running WASM tests...
#   ✔ Tests passed (8s)
#
#  NX   Successfully ran target test:wasm for project @rspack/core (23s)
```

### 缓存加速

```bash
# 第二次运行（没有文件变化）
nx test:wasm @rspack/core

# > nx run @rspack/core:build:wasm  [existing outputs match the cache, left as is]
# > nx run @rspack/core:test:wasm
#
#  NX   Successfully ran target test:wasm for project @rspack/core (8s)
```

---

## 三、Rspack 多平台完整配置

```json
// nx.json
{
  "namedInputs": {
    "wasmSources": [
      "{workspaceRoot}/crates/**/*.rs",
      "{workspaceRoot}/wasm/**/*"
    ],
    "browserSources": [
      "{workspaceRoot}/crates/rspack_browser/**/*"
    ],
    "nodeBindingSources": [
      "{workspaceRoot}/crates/node_binding/**/*",
      "{workspaceRoot}/crates/rspack_binding_api/**/*"
    ]
  },
  
  "targetDefaults": {
    "==========================================": "WASM 相关",
    "build:wasm": {
      "inputs": ["wasmSources", "{projectRoot}/**/*"],
      "outputs": ["{projectRoot}/*.wasm"],
      "cache": true
    },
    "test:wasm": {
      "dependsOn": ["build:wasm"],
      "inputs": [
        "{projectRoot}/tests/**/*",
        "{projectRoot}/*.wasm"
      ]
    },
    
    "==========================================": "Browser 相关",
    "build:browser": {
      "inputs": ["browserSources", "{projectRoot}/**/*"],
      "outputs": ["{projectRoot}/dist/browser/**/*"],
      "cache": true
    },
    "test:browser": {
      "dependsOn": ["build:browser"],
      "inputs": [
        "{projectRoot}/tests/**/*",
        "{projectRoot}/dist/browser/**/*"
      ]
    },
    
    "==========================================": "Node 相关",
    "build:node": {
      "dependsOn": ["^build:node"],
      "inputs": ["nodeBindingSources", "{projectRoot}/**/*"],
      "outputs": ["{projectRoot}/*.node"],
      "cache": true
    },
    "test:node": {
      "dependsOn": ["build:node"],
      "inputs": ["default", "^production"]
    },
    
    "==========================================": "通用映射",
    "build": {
      "dependsOn": ["^build"],
      "inputs": ["production", "^production"]
    },
    "test": {
      "dependsOn": ["build"],
      "inputs": ["default", "^production"]
    }
  }
}
```

---

## 四、与 pnpm scripts 集成

### 方案 A：Nx 调度 + pnpm 执行（推荐）

保持 `package.json` 不变，Nx 只负责依赖调度：

```json
// packages/rspack/package.json
{
  "scripts": {
    "build:wasm": "pnpm run build:dev:wasm",
    "build:browser": "pnpm run build:dev:browser",
    "build:node": "napi build --platform",
    "test:wasm": "node --experimental-wasm64 node_modules/.bin/jest --config jest.wasm.config.js",
    "test:browser": "jest --config jest.browser.config.js",
    "test:node": "rstest"
  },
  "nx": {
    "targets": {
      "test:wasm": {
        "dependsOn": ["build:wasm"]
      },
      "test:browser": {
        "dependsOn": ["build:browser"]
      },
      "test:node": {
        "dependsOn": ["build:node"]
      }
    }
  }
}
```

### 方案 B：完全迁移到 Nx Executor

```json
{
  "targets": {
    "build:wasm": {
      "executor": "nx:run-commands",
      "options": {
        "commands": [
          "cd crates/node_binding && napi build --platform --target wasm32-wasip1-threads"
        ],
        "parallel": false
      },
      "inputs": ["{workspaceRoot}/crates/**/*.rs"],
      "outputs": ["{projectRoot}/*.wasm"]
    },
    "test:wasm": {
      "executor": "@nx/jest:jest",
      "options": {
        "jestConfig": "jest.wasm.config.js",
        "testPathPattern": "wasm"
      },
      "dependsOn": ["build:wasm"],
      "inputs": [
        "{projectRoot}/tests/**/*",
        "{projectRoot}/*.wasm"
      ]
    }
  }
}
```

---

## 五、跨项目依赖

### 5.1 同类型依赖

```json
{
  "targets": {
    "test:wasm": {
      "dependsOn": [
        "build:wasm",
        {
          "projects": ["@rspack/binding"],
          "target": "build:wasm"
        }
      ]
    }
  }
}
```

执行顺序：
1. `@rspack/binding:build:wasm`
2. `@rspack/core:build:wasm`
3. `@rspack/core:test:wasm`

### 5.2 链式依赖

```json
{
  "targets": {
    "build:all": {
      "dependsOn": ["build:node", "build:wasm", "build:browser"]
    },
    "test:all": {
      "dependsOn": [
        "test:node",
        "test:wasm",
        "test:browser",
        { "projects": "dependencies", "target": "test" }
      ]
    }
  }
}
```

---

## 六、配置验证

```bash
# 1. 查看项目依赖图
nx graph

# 2. 查看特定 target 的依赖
nx graph --target=test:wasm

# 3. 模拟运行（不实际执行）
nx test:wasm @rspack/core --dry-run

# 4. 查看详细任务计划
nx test:wasm @rspack/core --verbose

# 5. 跳过缓存强制运行
nx test:wasm @rspack/core --skip-nx-cache

# 6. 只运行特定项目
nx run-many -t test:wasm -p @rspack/core @rspack/binding
```

---

## 七、与 x.mjs 的对比

| 特性 | x.mjs (zx) | Nx |
|------|------------|-----|
| **定义依赖** | 手动编码在脚本中 | 声明式配置 |
| **增量构建** | ❌ 不支持 | ✅ 基于 inputs 哈希 |
| **并行执行** | 需手动实现 | ✅ 自动并行化 |
| **缓存** | ❌ 不支持 | ✅ 本地 + 远程缓存 |
| **可视化** | ❌ 无 | ✅ `nx graph` |
| **调试能力** | ✅ LLDB 集成 | ⚠️ 需额外配置 |
| **学习成本** | 低（shell 脚本） | 中（Nx 概念） |
| **灵活性** | 高（任意代码） | 中（配置驱动） |

---

## 八、推荐迁移路径

### 阶段 1：共存期（低风险）

```json
// package.json - 保持 scripts 不变，添加 nx 配置
{
  "scripts": {
    "build:wasm": "pnpm run build:dev:wasm",
    "test:wasm": "jest --config jest.wasm.config.js"
  },
  "nx": {
    "targets": {
      "test:wasm": {
        "dependsOn": ["build:wasm"]
      }
    }
  }
}
```

验证命令：
```bash
# 新旧命令并行使用
./x test wasm      # 旧命令
nx test:wasm @rspack/core  # 新命令
```

### 阶段 2：优化期（中风险）

1. 添加 `inputs` 和 `outputs` 优化缓存
2. 将 shell 命令迁移到 Nx executors
3. 配置远程缓存

### 阶段 3：完全迁移（高风险）

```json
// 最终状态 - 可以移除 scripts
{
  "nx": {
    "targets": {
      "build:wasm": {
        "executor": "nx:run-commands",
        "options": { "command": "napi build --target wasm32..." },
        "inputs": ["{workspaceRoot}/crates/**/*.rs"],
        "outputs": ["{projectRoot}/*.wasm"]
      },
      "test:wasm": {
        "executor": "@nx/jest:jest",
        "options": { "jestConfig": "jest.wasm.config.js" },
        "dependsOn": ["build:wasm"]
      }
    }
  }
}
```

---

## 九、常见问题

### Q: 如何传递额外参数？

```bash
# x.mjs 风格
./x test:wasm -- --verbose

# Nx 风格
nx test:wasm @rspack/core -- --verbose
```

配置：
```json
{
  "test:wasm": {
    "executor": "nx:run-commands",
    "options": {
      "command": "jest --config jest.wasm.config.js {args.extra}"
    }
  }
}
```

### Q: 如何条件触发依赖？

```json
{
  "test:wasm": {
    "dependsOn": [
      {
        "target": "build:wasm",
        "projects": "self",
        "params": "ignore"
      }
    ]
  }
}
```

### Q: 环境变量如何处理？

```json
{
  "build:wasm": {
    "executor": "nx:run-commands",
    "options": {
      "command": "napi build",
      "env": {
        "RUST_TARGET": "wasm32-wasip1-threads"
      }
    }
  }
}
```

---

## 十、参考配置

完整的 `project.json` 示例：

```json
{
  "name": "@rspack/binding",
  "sourceRoot": "crates/node_binding",
  "projectType": "library",
  "targets": {
    "build:node": {
      "executor": "nx:run-commands",
      "options": {
        "command": "napi build --platform"
      },
      "inputs": [
        "{workspaceRoot}/crates/**/*.rs",
        "{workspaceRoot}/Cargo.lock"
      ],
      "outputs": [
        "{projectRoot}/*.node",
        "{projectRoot}/binding.js",
        "{projectRoot}/binding.d.ts"
      ]
    },
    "build:wasm": {
      "executor": "nx:run-commands",
      "options": {
        "command": "napi build --platform --target wasm32-wasip1-threads"
      },
      "inputs": [
        "{workspaceRoot}/crates/**/*.rs",
        "!{workspaceRoot}/crates/**/target/**/*"
      ],
      "outputs": [
        "{projectRoot}/*.wasm"
      ]
    },
    "build:browser": {
      "executor": "nx:run-commands",
      "options": {
        "command": "napi build --platform --target wasm32-wasip1-threads --features browser"
      },
      "inputs": ["{workspaceRoot}/crates/**/*.rs"],
      "outputs": ["{projectRoot}/browser/*.wasm"]
    },
    "build": {
      "dependsOn": ["build:node", "build:wasm", "build:browser"]
    },
    "test:wasm": {
      "dependsOn": ["build:wasm"],
      "executor": "@nx/jest:jest",
      "options": {
        "jestConfig": "jest.wasm.config.js"
      }
    }
  }
}
```
