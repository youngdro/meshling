//! CPU reference rasterizer: executes a [`FramePlan`] exactly as the compositing table in
//! `meshling-plan` describes. It is the oracle `render-gpu` is tested against, and the fallback
//! when no GPU is available.
//!
//! Sampling rules (the GPU backend must match them):
//! - A pixel is covered when its centre `(x + 0.5, y + 0.5)` is inside the triangle. A centre
//!   exactly on an edge belongs to the triangle for which that edge is a top or left edge, so
//!   triangles sharing an edge never both cover, nor both miss, a pixel.
//! - Texture coordinates are interpolated linearly (no perspective) and sampled bilinearly with
//!   clamp-to-edge addressing, texel centres at `(i + 0.5) / size`.
//! - Each pass's result is clamped to `[0, 1]`, as a `Unorm` render target does.

use meshling_geom::{Size, Vec2};
use meshling_model::{Blend, ObjId, Project};
use meshling_plan::{FramePlan, Pass, Pixels, Textures};
use meshling_rig::Pose;

/// A rendered frame: premultiplied RGBA in `[0, 1]`, row major, top row first.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub size: Size,
    pub pixels: Vec<[f32; 4]>,
}

impl Image {
    pub fn transparent(size: Size) -> Self {
        Self { size, pixels: vec![[0.0; 4]; size.width as usize * size.height as usize] }
    }

    pub fn get(&self, x: u32, y: u32) -> [f32; 4] {
        self.pixels[y as usize * self.size.width as usize + x as usize]
    }

    /// Premultiplied RGBA8, rounded to nearest.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)).collect()
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum RenderError {
    #[error("mesh {0} is in the plan but not in the project or pose")]
    MissingMesh(ObjId),
    #[error("texture {texture} of mesh {mesh} has no pixels")]
    MissingTexture { mesh: ObjId, texture: ObjId },
}

/// Render `plan` (made from `project` and `pose`) with `textures`.
pub fn render(project: &Project, pose: &Pose, plan: &FramePlan, textures: &Textures) -> Result<Image, RenderError> {
    let size = plan.canvas;
    let mut image = Image::transparent(size);
    let mut masks: Vec<Vec<f32>> = vec![Vec::new(); plan.mask_slots as usize];
    for pass in &plan.passes {
        match pass {
            Pass::Mask { slot, meshes } => {
                let mut cover = vec![0.0f32; size.width as usize * size.height as usize];
                for &id in meshes {
                    let geo = Geometry::of(project, pose, textures, id)?;
                    geo.raster(size, |i, sample| {
                        let a = sample[3];
                        cover[i] = (a + cover[i] * (1.0 - a)).clamp(0.0, 1.0);
                    });
                }
                if let Some(m) = masks.get_mut(*slot as usize) {
                    *m = cover;
                }
            }
            Pass::Draw { mesh, blend, opacity, mask } => {
                let geo = Geometry::of(project, pose, textures, *mesh)?;
                let mask = mask.and_then(|s| masks.get(s as usize));
                geo.raster(size, |i, sample| {
                    let k = opacity * mask.map_or(1.0, |m| m[i]);
                    let src = sample.map(|c| c * k);
                    let dst = &mut image.pixels[i];
                    *dst = composite(*blend, src, *dst);
                });
            }
        }
    }
    Ok(image)
}

/// One blend step of the table in `meshling-plan`, clamped like a Unorm target.
pub fn composite(blend: Blend, s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let inv = 1.0 - s[3];
    let out = match blend {
        Blend::Normal => [s[0] + d[0] * inv, s[1] + d[1] * inv, s[2] + d[2] * inv, s[3] + d[3] * inv],
        Blend::Add => [s[0] + d[0], s[1] + d[1], s[2] + d[2], d[3]],
        Blend::Multiply => [s[0] * d[0] + d[0] * inv, s[1] * d[1] + d[1] * inv, s[2] * d[2] + d[2] * inv, d[3]],
    };
    out.map(|c| c.clamp(0.0, 1.0))
}

struct Geometry<'a> {
    positions: &'a [Vec2],
    uvs: &'a [Vec2],
    indices: &'a [u32],
    texture: &'a Pixels,
}

impl<'a> Geometry<'a> {
    fn of(project: &'a Project, pose: &'a Pose, textures: &'a Textures, id: ObjId) -> Result<Self, RenderError> {
        let mesh = project.mesh(id).ok_or(RenderError::MissingMesh(id))?;
        let posed = pose.mesh(id).ok_or(RenderError::MissingMesh(id))?;
        let texture =
            textures.get(&mesh.texture).ok_or(RenderError::MissingTexture { mesh: id, texture: mesh.texture })?;
        Ok(Self { positions: &posed.positions, uvs: &mesh.uvs, indices: &mesh.indices, texture })
    }

    /// Call `f(pixel index, texture sample)` for every covered pixel of every triangle.
    fn raster(&self, size: Size, mut f: impl FnMut(usize, [f32; 4])) {
        for tri in self.indices.chunks_exact(3) {
            let [ia, ib, ic] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let (Some(&a), Some(&b), Some(&c)) =
                (self.positions.get(ia), self.positions.get(ib), self.positions.get(ic))
            else {
                continue;
            };
            let (Some(&ua), Some(&ub), Some(&uc)) = (self.uvs.get(ia), self.uvs.get(ib), self.uvs.get(ic)) else {
                continue;
            };
            // Normalize winding so the triangle's area is positive.
            let (b, c, ub, uc) = if edge(a, b, c) < 0.0 { (c, b, uc, ub) } else { (b, c, ub, uc) };
            let area = edge(a, b, c);
            if area <= 0.0 || !area.is_finite() {
                continue;
            }
            let lo = a.min(b).min(c);
            let hi = a.max(b).max(c);
            let x0 = (lo.x - 0.5).ceil().max(0.0) as u32;
            let y0 = (lo.y - 0.5).ceil().max(0.0) as u32;
            let x1 = ((hi.x - 0.5).floor() as i64).min(size.width as i64 - 1);
            let y1 = ((hi.y - 0.5).floor() as i64).min(size.height as i64 - 1);
            if x1 < x0 as i64 || y1 < y0 as i64 {
                continue;
            }
            let (tl_bc, tl_ca, tl_ab) = (top_left(b, c), top_left(c, a), top_left(a, b));
            for y in y0..=y1 as u32 {
                for x in x0..=x1 as u32 {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let (w0, w1, w2) = (edge(b, c, p), edge(c, a, p), edge(a, b, p));
                    if !(inside(w0, tl_bc) && inside(w1, tl_ca) && inside(w2, tl_ab)) {
                        continue;
                    }
                    let uv = (ua * w0 + ub * w1 + uc * w2) / area;
                    f(y as usize * size.width as usize + x as usize, sample(self.texture, uv));
                }
            }
        }
    }
}

/// Twice the signed area of `(a, b, p)`; positive when `p` is on the inner side of `a → b`.
fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

/// Top or left edge of a positive-area triangle in y-down coordinates.
fn top_left(a: Vec2, b: Vec2) -> bool {
    let d = b - a;
    (d.y == 0.0 && d.x > 0.0) || d.y < 0.0
}

fn inside(w: f32, top_left: bool) -> bool {
    w > 0.0 || (w == 0.0 && top_left)
}

/// Bilinear, clamp-to-edge sample of premultiplied pixels at normalized `uv`.
pub fn sample(tex: &Pixels, uv: Vec2) -> [f32; 4] {
    let (w, h) = (tex.size.width as i64, tex.size.height as i64);
    if w == 0 || h == 0 {
        return [0.0; 4];
    }
    let x = uv.x * w as f32 - 0.5;
    let y = uv.y * h as f32 - 0.5;
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let texel =
        |ix: i64, iy: i64| tex.get(ix.clamp(0, w - 1) as u32, iy.clamp(0, h - 1) as u32).map(|c| c as f32 / 255.0);
    let (ix, iy) = (fx as i64, fy as i64);
    let (p00, p10, p01, p11) = (texel(ix, iy), texel(ix + 1, iy), texel(ix, iy + 1), texel(ix + 1, iy + 1));
    std::array::from_fn(|k| {
        let top = p00[k] + (p10[k] - p00[k]) * tx;
        let bottom = p01[k] + (p11[k] - p01[k]) * tx;
        top + (bottom - top) * ty
    })
}
