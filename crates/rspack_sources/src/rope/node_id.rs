use std::{fmt, num::NonZeroU32};

/// Stable address of a node in an append-only rope arena.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NodeId(NonZeroU32);

impl NodeId {
  pub(crate) fn from_index(index: usize) -> Option<Self> {
    let index = u32::try_from(index).ok()?;
    let raw = index.checked_add(1)?;
    NonZeroU32::new(raw).map(Self)
  }

  pub(crate) fn index(self) -> usize {
    (self.0.get() - 1) as usize
  }

  pub(crate) fn raw(self) -> u32 {
    self.0.get()
  }
}

impl fmt::Debug for NodeId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("NodeId")
      .field("raw", &self.raw())
      .field("index", &self.index())
      .finish()
  }
}

static_assertions::const_assert_eq!(std::mem::size_of::<Option<NodeId>>(), 4);
