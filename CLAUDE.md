# meshling — notes for agents

Architecture doc (Chinese): https://claude.ai/artifact/JT2ebirqjgr4ohbUfwYCS3

## Rules that CI enforces
- Layering: `cargo xtask layers`. A crate depends only on lower layers (table in `xtask/src/main.rs`; the only same-layer edges are render-cpu/render-gpu → plan). A new crate must be added to that table.
- L0–L5 never depend on egui/eframe/winit/rfd and must build for `wasm32-unknown-unknown` (`cargo xtask wasm`).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`. `unwrap()` is denied outside tests; `unsafe` is forbidden.

## Design rules (not yet machine-checked)
- Everything is a command: every edit goes through the engine's command registry and produces a reversible transaction. The UI, CLI and MCP server are thin shells; no editing logic in them.
- Rendering: `rig` (CPU, deterministic) → `plan` (backend-independent pass list) → `render-gpu` (wgpu) or `render-cpu` (reference/oracle). Blend, clipping and draw-order semantics live only in `plan`. GPU output must match CPU within 2/255 inside meshes.
- Model data is immutable and `Arc`-shared; a new field that can hold a large payload uses `Arc`.
- Every object has a stable `ObjId`, a name, tags and `Provenance`; never key anything by name or index.
- Project JSON must stay deterministic (same project → same bytes). A format change bumps `FORMAT_VERSION` and adds a step to `model::migrate`.
