use super::{NodeId, summary::NodeSummary};
use crate::{BoxSource, Error, Result};

#[derive(Clone)]
pub(crate) enum Node {
  Branch {
    left: NodeId,
    right: NodeId,
    height: u8,
    summary: NodeSummary,
  },
  ChildSource {
    source: BoxSource,
    summary: NodeSummary,
  },
}

impl Node {
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

  pub(crate) fn leaves(&self, root: Option<NodeId>) -> ArenaLeaves<'_> {
    ArenaLeaves::new(self, root)
  }
}

pub(crate) struct ArenaLeaves<'a> {
  arena: &'a RopeArena,
  stack: [Option<NodeId>; 64],
  stack_len: usize,
}

impl<'a> ArenaLeaves<'a> {
  fn new(arena: &'a RopeArena, root: Option<NodeId>) -> Self {
    let mut stack = [None; 64];
    let stack_len = usize::from(root.is_some());
    stack[0] = root;
    Self {
      arena,
      stack,
      stack_len,
    }
  }
}

impl Iterator for ArenaLeaves<'_> {
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
        Node::ChildSource { .. } => return Some(id),
      }
    }
    None
  }
}
