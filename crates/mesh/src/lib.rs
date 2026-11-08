//! Automatic mesh generation from a layer's alpha.
//!
//! M1 uses an *occupancy grid*: the layer is divided into square cells, every cell holding any
//! ink becomes two triangles, and cells share their corner vertices. The result:
//! - covers every inked pixel exactly (cell edges lie on pixel edges, so nothing is clipped),
//! - never has flipped, degenerate or overlapping triangles,
//! - spreads vertices evenly, which is what warp and rotation deformers bend well,
//! - is deterministic: the same pixels and options give the same mesh.
//!
//! Contour-following meshes (fewer transparent triangles, better edges under strong bends) can
//! replace it later behind the same [`auto_mesh`] signature.

use meshling_geom::{Size, Vec2};

/// How finely to mesh a layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshOptions {
    /// Cells along the layer's longer side. Rig layers use 6–12; the cell side is at least 1 px.
    pub cells: u32,
    /// A pixel counts as ink when its alpha is above this.
    pub alpha_threshold: u8,
}

impl Default for MeshOptions {
    fn default() -> Self {
        Self { cells: 12, alpha_threshold: 0 }
    }
}

/// A triangle mesh in the layer's pixel space (origin at its top-left corner, y down).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeneratedMesh {
    pub positions: Vec<Vec2>,
    /// `positions` divided by the layer size: texture coordinates in `[0, 1]`.
    pub uvs: Vec<Vec2>,
    /// Triangle list, each triangle clockwise on screen (positive area in y-down coordinates).
    pub indices: Vec<u32>,
}

impl GeneratedMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The same mesh moved by `offset` (layer space to canvas space).
    pub fn translated(&self, offset: Vec2) -> Vec<Vec2> {
        self.positions.iter().map(|&p| p + offset).collect()
    }
}

/// Mesh the inked part of a `size` layer whose alpha at `(x, y)` is `alpha(x, y)`.
/// An empty mesh means the layer has no ink.
pub fn auto_mesh(size: Size, opts: MeshOptions, alpha: impl Fn(u32, u32) -> u8) -> GeneratedMesh {
    let (w, h) = (size.width, size.height);
    if w == 0 || h == 0 {
        return GeneratedMesh::default();
    }
    let cell = w.max(h).div_ceil(opts.cells.max(1)).max(1);
    let (cols, rows) = (w.div_ceil(cell), h.div_ceil(cell));

    let mut ink = vec![false; (cols * rows) as usize];
    for y in 0..h {
        for x in 0..w {
            if alpha(x, y) > opts.alpha_threshold {
                ink[((y / cell) * cols + x / cell) as usize] = true;
            }
        }
    }

    let mut mesh = GeneratedMesh::default();
    let mut vertex = vec![u32::MAX; ((cols + 1) * (rows + 1)) as usize];
    let mut corner = |mesh: &mut GeneratedMesh, i: u32, j: u32| -> u32 {
        let slot = &mut vertex[(j * (cols + 1) + i) as usize];
        if *slot == u32::MAX {
            let p = Vec2::new((i * cell).min(w) as f32, (j * cell).min(h) as f32);
            *slot = mesh.positions.len() as u32;
            mesh.positions.push(p);
            mesh.uvs.push(Vec2::new(p.x / w as f32, p.y / h as f32));
        }
        *slot
    };
    for j in 0..rows {
        for i in 0..cols {
            if !ink[(j * cols + i) as usize] {
                continue;
            }
            let a = corner(&mut mesh, i, j);
            let b = corner(&mut mesh, i + 1, j);
            let c = corner(&mut mesh, i + 1, j + 1);
            let d = corner(&mut mesh, i, j + 1);
            // Alternate the diagonal in a checkerboard so bends stay symmetric.
            if (i + j) % 2 == 0 {
                mesh.indices.extend([a, b, c, a, c, d]);
            } else {
                mesh.indices.extend([a, b, d, b, c, d]);
            }
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(m: &GeneratedMesh, t: usize) -> f32 {
        let [a, b, c] = [0, 1, 2].map(|k| m.positions[m.indices[t * 3 + k] as usize]);
        (b - a).perp_dot(c - a) / 2.0
    }

    #[test]
    fn a_full_layer_becomes_a_grid() {
        let m = auto_mesh(Size::new(40, 20), MeshOptions { cells: 4, alpha_threshold: 0 }, |_, _| 255);
        assert_eq!(m.positions.len(), 5 * 3);
        assert_eq!(m.triangle_count(), 4 * 2 * 2);
        let total: f32 = (0..m.triangle_count()).map(|t| area(&m, t)).sum();
        assert_eq!(total, 800.0);
        assert!((0..m.triangle_count()).all(|t| area(&m, t) > 0.0), "all clockwise, none degenerate");
        assert!(m.uvs.iter().all(|uv| (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)));
    }

    #[test]
    fn transparent_cells_are_dropped_but_every_inked_pixel_is_covered() {
        // A ring: ink only near the border of a 64 × 64 layer.
        let size = Size::new(64, 64);
        let ink = |x: u32, y: u32| if x.min(y).min(63 - x).min(63 - y) < 6 { 255 } else { 0 };
        let m = auto_mesh(size, MeshOptions { cells: 8, alpha_threshold: 0 }, ink);
        assert_eq!(m.triangle_count(), (64 - 36) * 2, "8×8 cells minus the 6×6 empty middle");
        for y in 0..64 {
            for x in 0..64 {
                if ink(x, y) == 0 {
                    continue;
                }
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let covered = (0..m.triangle_count()).any(|t| {
                    let [a, b, c] = [0, 1, 2].map(|k| m.positions[m.indices[t * 3 + k] as usize]);
                    [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).perp_dot(p - *u) >= 0.0)
                });
                assert!(covered, "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn uneven_and_tiny_layers_keep_cells_inside_the_layer() {
        let m = auto_mesh(Size::new(2, 3), MeshOptions::default(), |_, _| 200);
        assert!(m.positions.iter().all(|p| p.x <= 2.0 && p.y <= 3.0));
        let total: f32 = (0..m.triangle_count()).map(|t| area(&m, t)).sum();
        assert_eq!(total, 6.0);

        let m = auto_mesh(Size::new(25, 7), MeshOptions { cells: 3, alpha_threshold: 0 }, |_, _| 1);
        let total: f32 = (0..m.triangle_count()).map(|t| area(&m, t)).sum();
        assert_eq!(total, 175.0);
    }

    #[test]
    fn empty_and_faint_layers_give_no_triangles() {
        assert_eq!(auto_mesh(Size::new(0, 5), MeshOptions::default(), |_, _| 255), GeneratedMesh::default());
        let faint = MeshOptions { alpha_threshold: 10, ..MeshOptions::default() };
        assert_eq!(auto_mesh(Size::new(9, 9), faint, |_, _| 10).triangle_count(), 0);
    }

    #[test]
    fn deterministic() {
        let f = |x: u32, y: u32| (x * 7 + y * 13).is_multiple_of(5) as u8 * 255;
        let o = MeshOptions { cells: 10, alpha_threshold: 0 };
        assert_eq!(auto_mesh(Size::new(50, 30), o, f), auto_mesh(Size::new(50, 30), o, f));
    }
}
