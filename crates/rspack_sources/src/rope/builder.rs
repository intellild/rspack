use super::{
  NodeId,
  arena::{Node, RopeArena},
  summary::NodeSummary,
};
use crate::{BoxSource, helpers::utf16_len};

pub(crate) fn build(sources: Vec<BoxSource>) -> (RopeArena, Option<NodeId>, Vec<NodeId>) {
  let node_capacity = sources.len().saturating_mul(2).saturating_sub(1);
  let mut arena = RopeArena::with_capacity(node_capacity);
  let mut leaves = Vec::with_capacity(sources.len());
  for source in sources {
    let summary = summarize(&source);
    leaves.push(
      arena
        .alloc(Node::ChildSource { source, summary })
        .expect("bulk-build capacity is checked by NodeId"),
    );
  }
  let root = build_balanced(&mut arena, &leaves);
  (arena, root, leaves)
}

fn summarize(source: &BoxSource) -> NodeSummary {
  let mut generated_line = 1;
  let mut generated_column = 0;
  let mut bytes = 0;
  let mut is_ascii = true;
  source.rope(&mut |chunk| {
    bytes += chunk.len();
    is_ascii &= chunk.is_ascii();
    let mut remaining = chunk;
    while let Some(newline) = memchr::memchr(b'\n', remaining.as_bytes()) {
      generated_line += 1;
      generated_column = 0;
      remaining = &remaining[newline + 1..];
    }
    generated_column += utf16_len(remaining) as u32;
  });
  NodeSummary {
    bytes,
    generated_line,
    generated_column,
    is_ascii,
  }
}

fn build_balanced(arena: &mut RopeArena, nodes: &[NodeId]) -> Option<NodeId> {
  match nodes {
    [] => None,
    [node] => Some(*node),
    _ => {
      let middle = nodes.len() / 2;
      let left = build_balanced(arena, &nodes[..middle]).expect("left half is non-empty");
      let right = build_balanced(arena, &nodes[middle..]).expect("right half is non-empty");
      let summary = NodeSummary::combine(arena.get(left).summary(), arena.get(right).summary());
      let height = 1 + height(arena, left).max(height(arena, right));
      Some(
        arena
          .alloc(Node::Branch {
            left,
            right,
            height,
            summary,
          })
          .expect("bulk-build capacity is checked by NodeId"),
      )
    }
  }
}

fn height(arena: &RopeArena, id: NodeId) -> u8 {
  match arena.get(id) {
    Node::Branch { height, .. } => *height,
    Node::ChildSource { .. } => 1,
  }
}
