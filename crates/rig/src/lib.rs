//! Rig evaluation (architecture: "渲染管线" step 1).
//!
//! `evaluate(&Project, &ParamValues) -> Pose`: keyform interpolation per deformer and mesh, then
//! the deformer hierarchy top down (warp lattices, rotations) into final vertex positions,
//! opacity and draw order. CPU only, pure and deterministic. Implemented in week 2 of M1.
