use crate::utils::GPosIdx;
use std::fmt::Display;
use std::hash::Hash;

#[derive(Debug, Clone)]
/// A type that contains several position objects and contains and inner value.
pub struct Loc<T: Clone> {
    inner: T,
    pos: GPosIdx,
}

impl<T: Clone> Loc<T> {
    /// Construct a new `Loc` with the given inner value and no positions.
    pub fn new(inner: T, pos: GPosIdx) -> Self {
        Self { inner, pos }
    }

    /// A value with no position information.
    pub fn unknown(inner: T) -> Self {
        Self {
            inner,
            pos: GPosIdx::UNKNOWN,
        }
    }

    /// Get the position associted with this `Loc`.
    pub fn pos(&self) -> GPosIdx {
        self.pos
    }

    /// Set the position at the index `i` to the given position.
    /// Should be wrapped by the type to represent the semantic meaning of the position.
    pub fn set_pos(&mut self, pos: GPosIdx) {
        self.pos = pos;
    }

    /// Get a reference to  inner value.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner value.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Get the inner value and the position
    pub fn split(self) -> (T, GPosIdx) {
        (self.inner, self.pos)
    }

    /// Get the inner value and drop the position
    pub fn take(self) -> T {
        self.inner
    }

    /// Map over the inner value.
    pub fn map<U: Clone, F>(self, mut f: F) -> Loc<U>
    where
        F: FnMut(T) -> U,
    {
        Loc {
            inner: f(self.inner),
            pos: self.pos,
        }
    }

    pub fn map_into<B>(self) -> Loc<B>
    where
        T: Into<B>,
        B: Clone,
    {
        Loc {
            inner: self.inner.into(),
            pos: self.pos,
        }
    }
}

impl<T: Copy> Loc<T> {
    /// Get a copy of the inner value.
    pub fn copy(&self) -> T {
        self.inner
    }
}

impl<T: Clone> From<T> for Loc<T> {
    fn from(inner: T) -> Self {
        Self::unknown(inner)
    }
}

impl<T: Clone> std::ops::Deref for Loc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: Clone> std::ops::DerefMut for Loc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T: Display + Clone> Display for Loc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<T: PartialEq + Clone> PartialEq for Loc<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
impl<T: Eq + Clone> Eq for Loc<T> {}

impl<T: PartialOrd + Clone> PartialOrd for Loc<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.inner.partial_cmp(&other.inner)
    }
}

impl<T: Hash + Clone> Hash for Loc<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.inner.hash(state)
    }
}
