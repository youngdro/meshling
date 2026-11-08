//! Frame planner (architecture: "渲染管线" step 2).
//!
//! `plan(&Project, &Pose, &ViewOptions) -> FramePlan`: a linear list of passes (mask, draw mesh,
//! overlay) that both backends execute. Draw order, clipping and blending semantics live here
//! and nowhere else. Implemented in week 2 of M1.
