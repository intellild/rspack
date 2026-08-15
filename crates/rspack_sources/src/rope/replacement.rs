use super::NodeId;
use crate::Replacement;

// Below this point the compact Vec wins even for adversarial insertion order.
// Once the edit list is larger, moving Replacement values dominates and the
// arena tree has a clear crossover advantage.
pub(crate) const TREE_CROSSOVER: usize = 256;

// Keeping the fixed traversal stack inline avoids a heap allocation every time
// a tree-backed ReplaceSource is rendered or serialized.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ReplacementIter<'a> {
  Ordered(std::slice::Iter<'a, Replacement>),
  Tree(TreeIter<'a>),
}

impl<'a> Iterator for ReplacementIter<'a> {
  type Item = &'a Replacement;

  fn next(&mut self) -> Option<Self::Item> {
    match self {
      Self::Ordered(iter) => iter.next(),
      Self::Tree(iter) => iter.next(),
    }
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    match self {
      Self::Ordered(iter) => iter.size_hint(),
      Self::Tree(iter) => iter.size_hint(),
    }
  }
}

impl ExactSizeIterator for ReplacementIter<'_> {}

#[derive(Clone)]
struct ReplacementNode {
  replacement: Replacement,
  left: Option<NodeId>,
  right: Option<NodeId>,
  height: u8,
}

#[derive(Clone, Default)]
pub(crate) struct ReplacementTree {
  nodes: Vec<ReplacementNode>,
  root: Option<NodeId>,
  len: usize,
}

impl ReplacementTree {
  pub(crate) fn rewrite_text_contents(
    &mut self,
    rewrite: &mut impl FnMut(&mut std::borrow::Cow<'static, str>),
  ) {
    for node in &mut self.nodes {
      node.replacement.rewrite_text_content(rewrite);
    }
  }

  pub(crate) fn from_sorted(replacements: Vec<Replacement>) -> Self {
    let len = replacements.len();
    let mut replacements = replacements.into_iter().map(Some).collect::<Vec<_>>();
    let mut tree = Self {
      nodes: Vec::with_capacity(len),
      root: None,
      len,
    };
    tree.root = tree.build_sorted(&mut replacements, 0, len);
    tree
  }

  fn build_sorted(
    &mut self,
    replacements: &mut [Option<Replacement>],
    start: usize,
    end: usize,
  ) -> Option<NodeId> {
    if start == end {
      return None;
    }
    let middle = start + (end - start) / 2;
    let left = self.build_sorted(replacements, start, middle);
    let right = self.build_sorted(replacements, middle + 1, end);
    let replacement = replacements[middle]
      .take()
      .expect("bulk-built replacement consumed once");
    Some(self.alloc(ReplacementNode {
      replacement,
      left,
      right,
      height: 1 + self.height(left).max(self.height(right)),
    }))
  }

  fn alloc(&mut self, node: ReplacementNode) -> NodeId {
    let id = NodeId::from_index(self.nodes.len())
      .expect("replacement arena cannot contain more than u32::MAX nodes");
    self.nodes.push(node);
    id
  }

  fn node(&self, id: NodeId) -> &ReplacementNode {
    self
      .nodes
      .get(id.index())
      .expect("NodeId belongs to this replacement arena")
  }

  fn node_mut(&mut self, id: NodeId) -> &mut ReplacementNode {
    self
      .nodes
      .get_mut(id.index())
      .expect("NodeId belongs to this replacement arena")
  }

  fn height(&self, id: Option<NodeId>) -> u8 {
    id.map_or(0, |id| self.node(id).height)
  }

  fn update_height(&mut self, id: NodeId) {
    let (left, right) = {
      let node = self.node(id);
      (node.left, node.right)
    };
    self.node_mut(id).height = 1 + self.height(left).max(self.height(right));
  }

  fn balance_factor(&self, id: NodeId) -> i16 {
    let node = self.node(id);
    self.height(node.left) as i16 - self.height(node.right) as i16
  }

  fn rotate_left(&mut self, root: NodeId) -> NodeId {
    let pivot = self
      .node(root)
      .right
      .expect("left rotation has right child");
    let middle = self.node(pivot).left;
    self.node_mut(root).right = middle;
    self.node_mut(pivot).left = Some(root);
    self.update_height(root);
    self.update_height(pivot);
    pivot
  }

  fn rotate_right(&mut self, root: NodeId) -> NodeId {
    let pivot = self.node(root).left.expect("right rotation has left child");
    let middle = self.node(pivot).right;
    self.node_mut(root).left = middle;
    self.node_mut(pivot).right = Some(root);
    self.update_height(root);
    self.update_height(pivot);
    pivot
  }

  fn rebalance(&mut self, root: NodeId) -> NodeId {
    self.update_height(root);
    let balance = self.balance_factor(root);
    if balance > 1 {
      let left = self
        .node(root)
        .left
        .expect("left-heavy node has left child");
      if self.balance_factor(left) < 0 {
        let new_left = self.rotate_left(left);
        self.node_mut(root).left = Some(new_left);
      }
      self.rotate_right(root)
    } else if balance < -1 {
      let right = self
        .node(root)
        .right
        .expect("right-heavy node has right child");
      if self.balance_factor(right) > 0 {
        let new_right = self.rotate_right(right);
        self.node_mut(root).right = Some(new_right);
      }
      self.rotate_left(root)
    } else {
      root
    }
  }

  pub(crate) fn insert(&mut self, replacement: Replacement) {
    self.root = Some(match self.root {
      Some(root) => self.insert_at(root, replacement),
      None => self.alloc(ReplacementNode {
        replacement,
        left: None,
        right: None,
        height: 1,
      }),
    });
    self.len += 1;
  }

  pub(crate) fn len(&self) -> usize {
    self.len
  }

  #[cfg(feature = "codspeed")]
  pub(crate) fn benchmark_stats(&self) -> (usize, u8, usize) {
    (
      self.len,
      self.root.map_or(0, |root| self.node(root).height),
      std::mem::size_of::<ReplacementNode>(),
    )
  }

  fn insert_at(&mut self, root: NodeId, replacement: Replacement) -> NodeId {
    if replacement < self.node(root).replacement {
      let left = match self.node(root).left {
        Some(left) => self.insert_at(left, replacement),
        None => self.alloc(ReplacementNode {
          replacement,
          left: None,
          right: None,
          height: 1,
        }),
      };
      self.node_mut(root).left = Some(left);
    } else {
      let right = match self.node(root).right {
        Some(right) => self.insert_at(right, replacement),
        None => self.alloc(ReplacementNode {
          replacement,
          left: None,
          right: None,
          height: 1,
        }),
      };
      self.node_mut(root).right = Some(right);
    }
    self.rebalance(root)
  }
}

pub(crate) struct TreeIter<'a> {
  tree: &'a ReplacementTree,
  stack: [Option<NodeId>; 64],
  stack_len: usize,
  next: Option<NodeId>,
  remaining: usize,
}

impl<'a> TreeIter<'a> {
  pub(crate) fn new(tree: &'a ReplacementTree) -> Self {
    Self {
      tree,
      stack: [None; 64],
      stack_len: 0,
      next: tree.root,
      remaining: tree.len,
    }
  }
}

impl<'a> Iterator for TreeIter<'a> {
  type Item = &'a Replacement;

  fn next(&mut self) -> Option<Self::Item> {
    while let Some(id) = self.next {
      self.stack[self.stack_len] = Some(id);
      self.stack_len += 1;
      self.next = self.tree.node(id).left;
    }
    if self.stack_len == 0 {
      return None;
    }
    self.stack_len -= 1;
    let id = self.stack[self.stack_len]
      .take()
      .expect("iterator stack entry initialized");
    let node = self.tree.node(id);
    self.next = node.right;
    self.remaining -= 1;
    Some(&node.replacement)
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    (self.remaining, Some(self.remaining))
  }
}

impl ExactSizeIterator for TreeIter<'_> {}
