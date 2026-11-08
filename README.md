# meshling

把分层的 2D 立绘做成可交互、带物理联动的实时形象的编辑器。Rust + wgpu，编辑器、命令行和 MCP 服务共用同一个引擎：每个编辑操作都是一条命令，人在界面里点和 agent 通过 MCP 调用走的是同一条路径。

架构与里程碑：[2D 交互形象编辑器：架构与首个里程碑](https://claude.ai/artifact/JT2ebirqjgr4ohbUfwYCS3)

## 状态

M1 第 1 周：workspace 骨架、分层检查、CI、工程数据模型。其余 crate 目前是空壳，各自的文档注释写明了在 M1 的第几周实现。

## 开发

```sh
cargo test --workspace      # 全部测试
cargo xtask layers          # 分层依赖检查
cargo xtask wasm            # L0–L5 能否编译到 wasm32（需先 rustup target add wasm32-unknown-unknown）
```

## 结构

| 层 | crate |
| --- | --- |
| L0 | `geom` |
| L1 | `model` |
| L2 | `rig` `physics` `mesh` `ops` |
| L3 | `plan` `render-cpu` `render-gpu` |
| L4 | `io` `validate` `ai` |
| L5 | `engine` |
| L6 | `ui-egui` `automation` |
| L7 | `apps/editor` `apps/cli` |

只允许依赖更低层的 crate（`render-cpu`、`render-gpu` 可依赖同层的 `plan`）；L5 及以下不得依赖 egui、winit 等界面库，且必须能编译到 wasm32。
