use super::{NodeId, summary::NodeSummary};
use crate::{BoxSource, Error, Result};

#[derive(Clone)]
pub(crate) enum Node {
  Branch {
    parent: Option<NodeId>,
    #[allow(dead_code)] // Retained arena topology; production leaf traversal is contiguous.
    left: NodeId,
    #[allow(dead_code)] // Retained arena topology; production leaf traversal is contiguous.
    right: NodeId,
    height: u8,
    summary: NodeSummary,
  },
  ChildSource {
    parent: Option<NodeId>,
    source: BoxSource,
    summary: NodeSummary,
  },
}

impl Node {
  #[cfg(feature = "codspeed")]
  pub(crate) fn parent(&self) -> Option<NodeId> {
    match self {
      Self::Branch { parent, .. } | Self::ChildSource { parent, .. } => *parent,
    }
  }

  pub(crate) fn set_parent(&mut self, parent: NodeId) {
    match self {
      Self::Branch {
        parent: node_parent,
        ..
      }
      | Self::ChildSource {
        parent: node_parent,
        ..
      } => *node_parent = Some(parent),
    }
  }

  pub(crate) fn summary(&self) -> NodeSummary {
    match self {
      Self::Branch { summary, .. } | Self::ChildSource { summary, .. } => *summary,
    }
  }
}

#[derive(Clone, Default)]
pub(crate) struct RopeArena {
  nodes: Vec<Node>,
}

impl RopeArena {
  pub(crate) fn with_capacity(capacity: usize) -> Self {
    Self {
      nodes: Vec::with_capacity(capacity),
    }
  }

  pub(crate) fn alloc(&mut self, node: Node) -> Result<NodeId> {
    let id = NodeId::from_index(self.nodes.len()).ok_or(Error::ArenaOverflow)?;
    self.nodes.push(node);
    Ok(id)
  }

  pub(crate) fn get(&self, id: NodeId) -> &Node {
    self
      .nodes
      .get(id.index())
      .expect("NodeId belongs to this rope arena")
  }

  pub(crate) fn get_mut(&mut self, id: NodeId) -> &mut Node {
    self
      .nodes
      .get_mut(id.index())
      .expect("NodeId belongs to this rope arena")
  }

  pub(crate) fn leaves(&self, len: usize) -> ArenaLeaves {
    ArenaLeaves::new(len)
  }

  #[cfg(feature = "codspeed")]
  pub(crate) fn parent_leaves(&self, root: Option<NodeId>, len: usize) -> ParentArenaLeaves<'_> {
    ParentArenaLeaves::new(self, root, len)
  }

  #[cfg(feature = "codspeed")]
  pub(crate) fn stack_leaves(&self, root: Option<NodeId>, len: usize) -> StackArenaLeaves<'_> {
    StackArenaLeaves::new(self, root, len)
  }

  #[cfg(feature = "codspeed")]
  pub(crate) fn benchmark_stats(&self, root: Option<NodeId>) -> (usize, u8, usize) {
    let height = root.map_or(0, |root| match self.get(root) {
      Node::Branch { height, .. } => *height,
      Node::ChildSource { .. } => 1,
    });
    (self.nodes.len(), height, std::mem::size_of::<Node>())
  }

  #[cfg(feature = "codspeed")]
  fn leftmost_leaf(&self, mut id: NodeId) -> NodeId {
    loop {
      match self.get(id) {
        Node::Branch { left, .. } => id = *left,
        Node::ChildSource { .. } => return id,
      }
    }
  }

  #[cfg(feature = "codspeed")]
  fn next_leaf(&self, mut id: NodeId) -> Option<NodeId> {
    loop {
      let parent = self.get(id).parent()?;
      match self.get(parent) {
        Node::Branch { left, right, .. } if *left == id => {
          return Some(self.leftmost_leaf(*right));
        }
        Node::Branch { right, .. } => {
          debug_assert_eq!(*right, id);
          id = parent;
        }
        Node::ChildSource { .. } => unreachable!("a child source cannot be a parent"),
      }
    }
  }
}

/// Iterates the canonical leaf sequence using the bulk builder's invariant
/// that all leaves are allocated first in input order.
pub(crate) struct ArenaLeaves {
  next: usize,
  end: usize,
}

impl ArenaLeaves {
  fn new(end: usize) -> Self {
    Self { next: 0, end }
  }
}

impl Iterator for ArenaLeaves {
  type Item = NodeId;

  fn next(&mut self) -> Option<Self::Item> {
    if self.next == self.end {
      return None;
    }
    let id = NodeId::from_index(self.next).expect("leaf count is checked by the arena builder");
    self.next += 1;
    Some(id)
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    let remaining = self.end - self.next;
    (remaining, Some(remaining))
  }
}

impl ExactSizeIterator for ArenaLeaves {}

#[cfg(feature = "codspeed")]
pub(crate) struct ParentArenaLeaves<'a> {
  arena: &'a RopeArena,
  next: Option<NodeId>,
  remaining: usize,
}

#[cfg(feature = "codspeed")]
impl<'a> ParentArenaLeaves<'a> {
  fn new(arena: &'a RopeArena, root: Option<NodeId>, remaining: usize) -> Self {
    Self {
      arena,
      next: root.map(|root| arena.leftmost_leaf(root)),
      remaining,
    }
  }
}

#[cfg(feature = "codspeed")]
impl Iterator for ParentArenaLeaves<'_> {
  type Item = NodeId;

  fn next(&mut self) -> Option<Self::Item> {
    let id = self.next?;
    self.next = self.arena.next_leaf(id);
    self.remaining -= 1;
    Some(id)
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    (self.remaining, Some(self.remaining))
  }
}

#[cfg(feature = "codspeed")]
impl ExactSizeIterator for ParentArenaLeaves<'_> {}

#[cfg(feature = "codspeed")]
pub(crate) struct StackArenaLeaves<'a> {
  arena: &'a RopeArena,
  stack: [Option<NodeId>; 64],
  stack_len: usize,
  remaining: usize,
}

#[cfg(feature = "codspeed")]
impl<'a> StackArenaLeaves<'a> {
  fn new(arena: &'a RopeArena, root: Option<NodeId>, remaining: usize) -> Self {
    let mut stack = [None; 64];
    let stack_len = usize::from(root.is_some());
    stack[0] = root;
    Self {
      arena,
      stack,
      stack_len,
      remaining,
    }
  }
}

#[cfg(feature = "codspeed")]
impl Iterator for StackArenaLeaves<'_> {
  type Item = NodeId;

  fn next(&mut self) -> Option<Self::Item> {
    while self.stack_len > 0 {
      self.stack_len -= 1;
      let id = self.stack[self.stack_len]
        .take()
        .expect("arena traversal stack entry initialized");
      match self.arena.get(id) {
        Node::Branch { left, right, .. } => {
          self.stack[self.stack_len] = Some(*right);
          self.stack[self.stack_len + 1] = Some(*left);
          self.stack_len += 2;
        }
        Node::ChildSource { .. } => {
          self.remaining -= 1;
          return Some(id);
        }
      }
    }
    None
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    (self.remaining, Some(self.remaining))
  }
}

#[cfg(feature = "codspeed")]
impl ExactSizeIterator for StackArenaLeaves<'_> {}
