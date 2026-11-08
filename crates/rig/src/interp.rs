//! Multilinear interpolation over keyform grids.

use meshling_geom::Vec2;
use meshling_model::{KeyformGrid, Project};

use crate::ParamValues;

/// A value that keyforms blend: a weighted sum of forms.
pub trait Lerp: Copy {
    const ZERO: Self;
    fn add_scaled(self, other: Self, w: f32) -> Self;
}

impl Lerp for f32 {
    const ZERO: Self = 0.0;
    fn add_scaled(self, other: Self, w: f32) -> Self {
        self + other * w
    }
}

impl Lerp for Vec2 {
    const ZERO: Self = Vec2::ZERO;
    fn add_scaled(self, other: Self, w: f32) -> Self {
        self + other * w
    }
}

/// Which forms of a grid contribute at the current parameters, and how much (weights sum to 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Weights {
    pub terms: Vec<(usize, f32)>,
}

impl Weights {
    /// Blend one value per form (`values` yields them in form order).
    pub fn blend<T: Lerp>(&self, values: impl Iterator<Item = T>) -> T {
        let values: Vec<T> = values.collect();
        self.terms.iter().fold(T::ZERO, |acc, &(i, w)| acc.add_scaled(values[i], w))
    }

    /// Blend one point list per form, each `n` long.
    pub fn blend_points<'a>(&self, forms: impl Iterator<Item = &'a [Vec2]>, n: usize) -> Vec<Vec2> {
        let forms: Vec<&[Vec2]> = forms.collect();
        let mut out = vec![Vec2::ZERO; n];
        for &(i, w) in &self.terms {
            for (o, p) in out.iter_mut().zip(forms[i]) {
                *o += *p * w;
            }
        }
        out
    }
}

/// The contributing forms of `grid` at `values`: per binding, the two keys around the value
/// (clamped to the first and last key, never extrapolated), combined multilinearly.
/// `None` when the grid's form count doesn't match its bindings.
pub fn weights<T>(project: &Project, values: &ParamValues, grid: &KeyformGrid<T>) -> Option<Weights> {
    if grid.forms.len() != grid.expected_len() || grid.forms.is_empty() {
        return None;
    }
    // Per binding: (lower key index, fraction towards the next key, stride in `forms`).
    let mut axes = Vec::with_capacity(grid.bindings.len());
    let mut stride = 1;
    for b in &grid.bindings {
        let keys = &b.keys;
        let v = values.get(project, b.param);
        let last = keys.len() - 1;
        let (i, t) = if v <= keys[0] {
            (0, 0.0)
        } else if v >= keys[last] {
            (last, 0.0)
        } else {
            let i = keys.windows(2).position(|w| v < w[1]).unwrap_or(last.saturating_sub(1));
            (i, (v - keys[i]) / (keys[i + 1] - keys[i]))
        };
        axes.push((i, t, stride));
        stride *= keys.len();
    }
    let mut terms = Vec::with_capacity(1 << axes.len());
    for corner in 0..(1usize << axes.len()) {
        let mut index = 0;
        let mut w = 1.0;
        for (bit, &(i, t, stride)) in axes.iter().enumerate() {
            let upper = corner >> bit & 1 == 1;
            if upper {
                index += (i + 1) * stride;
                w *= t;
            } else {
                index += i * stride;
                w *= 1.0 - t;
            }
        }
        if w > 0.0 {
            terms.push((index, w));
        }
    }
    Some(Weights { terms })
}
