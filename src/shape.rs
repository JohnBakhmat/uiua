use std::{
    fmt,
    hash::Hash,
    ops::{Deref, DerefMut, Index, RangeBounds},
};

use serde::*;
use tinyvec::TinyVec;

/// Uiua's array shape type
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(from = "ShapeRep", into = "ShapeRep")]
pub struct Shape {
    sizes: TinyVec<[usize; 3]>,
    markers: Vec<Marker>,
}

/// A marker for a dimension
pub type Marker = char;

/// The empty marker
pub const EMPTY_MARKER: Marker = '\0';

/// A dimension in an array shape
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Dimension {
    /// The size of the dimension
    pub size: usize,
    /// The marker of the dimension
    pub marker: Marker,
}

impl From<usize> for Dimension {
    fn from(size: usize) -> Self {
        Self {
            size,
            marker: EMPTY_MARKER,
        }
    }
}

impl Shape {
    /// Create a new shape with no dimensions
    pub fn scalar() -> Self {
        Shape {
            sizes: TinyVec::new(),
            markers: Vec::new(),
        }
    }
    /// Create a new scalar shape with the given capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Shape {
            sizes: TinyVec::with_capacity(capacity),
            markers: Vec::new(),
        }
    }
    /// Remove dimensions in the given range
    pub fn drain(&mut self, range: impl RangeBounds<usize>) {
        self.sizes.drain(range);
    }
    /// Add a trailing dimension
    pub fn push(&mut self, dim: impl Into<Dimension>) {
        let dim = dim.into();
        self.sizes.push(dim.size);
        if dim.marker != EMPTY_MARKER {
            self.markers.push(dim.marker);
        } else if !self.markers.is_empty() {
            self.markers.push(EMPTY_MARKER);
        }
    }
    /// Remove the last dimension
    pub fn pop(&mut self) -> Option<Dimension> {
        let size = self.sizes.pop()?;
        let marker = if self.markers.is_empty() {
            EMPTY_MARKER
        } else {
            self.markers.pop()?
        };
        Some(Dimension { size, marker })
    }
    /// Insert a dimension at the given index
    pub fn insert(&mut self, index: usize, dim: impl Into<Dimension>) {
        let dim = dim.into();
        self.sizes.insert(index, dim.size);
        if dim.marker != EMPTY_MARKER {
            self.markers.insert(index, dim.marker);
        } else if !self.markers.is_empty() {
            self.markers.insert(index, EMPTY_MARKER);
        }
    }
    /// Remove the dimension at the given index
    pub fn remove(&mut self, index: usize) -> Dimension {
        let size = self.sizes.remove(index);
        let marker = if self.markers.is_empty() {
            EMPTY_MARKER
        } else {
            self.markers.remove(index)
        };
        Dimension { size, marker }
    }
    /// Extend the shape with the given dimensions and markers
    pub fn extend_from_shape<R>(&mut self, shape: &Shape, range: R)
    where
        [usize]: Index<R, Output = [usize]>,
        [Marker]: Index<R, Output = [Marker]>,
        R: Clone,
    {
        self.sizes
            .extend_from_slice(&shape.sizes[..][range.clone()]);
        if !shape.markers.is_empty() {
            self.markers.extend_from_slice(&shape.markers[..][range]);
        }
    }
    /// Split the shape at the given index
    pub fn split_off(&mut self, at: usize) -> Self {
        Shape {
            sizes: self.sizes.split_off(at),
            markers: self.markers.split_off(at.min(self.markers.len())),
        }
    }
    /// Get a reference to the dimension sizes
    pub fn sizes(&self) -> &[usize] {
        &self.sizes
    }
    /// Get a reference to the dimension markers
    pub fn markers(&self) -> &[Marker] {
        if self.markers.is_empty() {
            &NO_MARKERS[..self.sizes.len()]
        } else {
            &self.markers
        }
    }
    /// Get an iterator over the dimensions
    pub fn dims(&self) -> impl Iterator<Item = Dimension> + '_ {
        self.sizes
            .iter()
            .enumerate()
            .map(move |(i, &size)| Dimension {
                size,
                marker: self.markers.get(i).copied().unwrap_or(EMPTY_MARKER),
            })
    }
}

static NO_MARKERS: [Marker; 32] = ['\0'; 32];

impl From<Vec<Dimension>> for Shape {
    fn from(dims: Vec<Dimension>) -> Self {
        Self::from_iter(dims)
    }
}

impl From<Shape> for Vec<Dimension> {
    fn from(shape: Shape) -> Self {
        shape
            .sizes
            .into_iter()
            .map(|size| Dimension {
                size,
                marker: EMPTY_MARKER,
            })
            .collect()
    }
}

impl fmt::Debug for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        for (i, dim) in self.sizes.iter().enumerate() {
            if i > 0 {
                write!(f, " × ")?;
            }
            write!(f, "{}", dim)?;
        }
        write!(f, "]")
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<usize> for Shape {
    fn from(dim: usize) -> Self {
        Self::from([dim])
    }
}

impl From<&[usize]> for Shape {
    fn from(dims: &[usize]) -> Self {
        Self {
            sizes: dims.iter().copied().collect(),
            markers: Vec::new(),
        }
    }
}

impl<const N: usize> From<[usize; N]> for Shape {
    fn from(dims: [usize; N]) -> Self {
        dims.as_slice().into()
    }
}

impl Deref for Shape {
    type Target = [usize];
    fn deref(&self) -> &Self::Target {
        &self.sizes
    }
}

impl DerefMut for Shape {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sizes
    }
}

impl IntoIterator for Shape {
    type Item = Dimension;
    type IntoIter = Box<dyn Iterator<Item = Self::Item>>;
    fn into_iter(self) -> Self::IntoIter {
        if self.markers.is_empty() {
            Box::new(self.sizes.into_iter().map(|size| Dimension {
                size,
                marker: EMPTY_MARKER,
            }))
        } else {
            Box::new(
                self.sizes
                    .into_iter()
                    .zip(self.markers)
                    .map(|(size, marker)| Dimension { size, marker }),
            )
        }
    }
}

impl<'a> IntoIterator for &'a Shape {
    type Item = &'a usize;
    type IntoIter = <&'a [usize] as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.sizes.iter()
    }
}

impl FromIterator<usize> for Shape {
    fn from_iter<I: IntoIterator<Item = usize>>(iter: I) -> Self {
        Self {
            sizes: iter.into_iter().collect(),
            markers: Vec::new(),
        }
    }
}

impl FromIterator<Dimension> for Shape {
    fn from_iter<I: IntoIterator<Item = Dimension>>(iter: I) -> Self {
        let mut sizes = TinyVec::new();
        let mut markers = Vec::new();
        for (i, dim) in iter.into_iter().enumerate() {
            sizes.push(dim.size);
            if dim.marker != EMPTY_MARKER {
                markers.resize(i, EMPTY_MARKER);
                markers.push(dim.marker);
            }
        }
        if !markers.is_empty() {
            markers.resize(sizes.len(), EMPTY_MARKER);
        }
        Self { sizes, markers }
    }
}

impl Extend<usize> for Shape {
    fn extend<I: IntoIterator<Item = usize>>(&mut self, iter: I) {
        self.sizes.extend(iter);
    }
}

impl PartialEq<usize> for Shape {
    fn eq(&self, other: &usize) -> bool {
        self == [*other]
    }
}

impl PartialEq<usize> for &Shape {
    fn eq(&self, other: &usize) -> bool {
        *self == [*other]
    }
}

impl<const N: usize> PartialEq<[usize; N]> for Shape {
    fn eq(&self, other: &[usize; N]) -> bool {
        self == other.as_slice()
    }
}

impl<const N: usize> PartialEq<[usize; N]> for &Shape {
    fn eq(&self, other: &[usize; N]) -> bool {
        *self == other.as_slice()
    }
}

impl PartialEq<[usize]> for Shape {
    fn eq(&self, other: &[usize]) -> bool {
        self.sizes == other
    }
}

impl PartialEq<[usize]> for &Shape {
    fn eq(&self, other: &[usize]) -> bool {
        *self == other
    }
}

impl PartialEq<&[usize]> for Shape {
    fn eq(&self, other: &&[usize]) -> bool {
        self.sizes == *other
    }
}

impl PartialEq<Shape> for &[usize] {
    fn eq(&self, other: &Shape) -> bool {
        other == self
    }
}

impl PartialEq<Shape> for [usize] {
    fn eq(&self, other: &Shape) -> bool {
        other == self
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum ShapeRep {
    Unmarked(Vec<usize>),
    Marked {
        sizes: Vec<usize>,
        markers: Vec<Marker>,
    },
}

impl From<Shape> for ShapeRep {
    fn from(shape: Shape) -> Self {
        if shape.markers.is_empty() {
            Self::Unmarked(shape.sizes.into_iter().collect())
        } else {
            Self::Marked {
                sizes: shape.sizes.into_iter().collect(),
                markers: shape.markers,
            }
        }
    }
}

impl From<ShapeRep> for Shape {
    fn from(rep: ShapeRep) -> Self {
        match rep {
            ShapeRep::Unmarked(sizes) => Shape {
                sizes: sizes.into_iter().collect(),
                markers: Vec::new(),
            },
            ShapeRep::Marked { sizes, markers } => Shape {
                sizes: sizes.into_iter().collect(),
                markers,
            },
        }
    }
}
