use serde::{Deserialize, Serialize};

use crate::ObjId;

/// One parameter a keyform grid varies along, with the parameter values its keys sit at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamBinding {
    pub param: ObjId,
    /// Strictly increasing parameter values.
    pub keys: Vec<f32>,
}

/// Forms of an object laid out on a grid over 0–3 parameters.
///
/// `forms` holds one entry per grid point, in row-major order with the **first** binding varying
/// fastest. With no bindings there is exactly one form (the object never changes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyformGrid<T> {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ParamBinding>,
    pub forms: Vec<T>,
}

impl<T> KeyformGrid<T> {
    /// A grid bound to no parameter.
    pub fn single(form: T) -> Self {
        Self { bindings: Vec::new(), forms: vec![form] }
    }

    /// How many forms the bindings call for.
    pub fn expected_len(&self) -> usize {
        self.bindings.iter().map(|b| b.keys.len()).product()
    }

    /// Flat index of the form at grid coordinates `coords` (one key index per binding).
    pub fn index(&self, coords: &[usize]) -> Option<usize> {
        if coords.len() != self.bindings.len() {
            return None;
        }
        let mut index = 0;
        let mut stride = 1;
        for (c, b) in coords.iter().zip(&self.bindings) {
            if *c >= b.keys.len() {
                return None;
            }
            index += c * stride;
            stride *= b.keys.len();
        }
        Some(index)
    }

    pub fn get(&self, coords: &[usize]) -> Option<&T> {
        self.index(coords).and_then(|i| self.forms.get(i))
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.forms.iter()
    }
}
