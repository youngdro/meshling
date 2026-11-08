//! Geometry primitives shared by every crate: vectors, sizes, rectangles and 2D affine maps.
//! Re-exports `glam` so the rest of the workspace uses one version of it.

pub use glam::{Affine2, Mat2, Vec2};
use serde::{Deserialize, Serialize};

/// A size in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// An axis-aligned rectangle in canvas coordinates (y down).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.y >= self.min.y && p.x <= self.max.x && p.y <= self.max.y
    }

    /// The smallest rectangle containing every point, or `None` for no points.
    pub fn bounding(points: impl IntoIterator<Item = Vec2>) -> Option<Self> {
        let mut it = points.into_iter();
        let first = it.next()?;
        let (min, max) = it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
        Some(Self { min, max })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_box() {
        let r = Rect::bounding([Vec2::new(1.0, 5.0), Vec2::new(-2.0, 3.0), Vec2::new(4.0, -1.0)]);
        assert_eq!(r, Some(Rect::new(Vec2::new(-2.0, -1.0), Vec2::new(4.0, 5.0))));
        assert_eq!(Rect::bounding([]), None);
    }
}
