use std::{
  borrow::Cow,
  hash::{Hash, Hasher},
  sync::Arc,
};

use crate::{
  BoxSource, MapOptions, Source, SourceExt, SourceMap, SourceValue,
  concat_source::CombinedSourceChunks,
  helpers::{Chunks, GeneratedInfo, StreamChunks, get_map},
  object_pool::ObjectPool,
  rope::{
    NodeId,
    arena::{Node, RopeArena},
    builder,
  },
};

/// An immutable, balanced composition of source children.
///
/// The arena topology is an implementation detail: equality and hashing use
/// canonical in-order child sequence, so balancing never changes identity.
#[derive(Clone)]
pub struct RopeSource {
  arena: RopeArena,
  root: Option<NodeId>,
  leaves: Vec<NodeId>,
}

impl RopeSource {
  /// Bulk-build a balanced rope from an ordered sequence of sources.
  pub fn new<S, T>(sources: S) -> Self
  where
    S: IntoIterator<Item = T>,
    T: Source + 'static,
  {
    Self::from_boxed(sources.into_iter().map(SourceExt::boxed).collect())
  }

  /// Bulk-build a balanced rope from already boxed sources.
  pub fn from_boxed(sources: Vec<BoxSource>) -> Self {
    let (arena, root, leaves) = builder::build(sources);
    debug_assert!(arena.leaves(root).eq(leaves.iter().copied()));
    Self {
      arena,
      root,
      leaves,
    }
  }

  /// Number of logical child sources in this rope.
  pub fn len(&self) -> usize {
    self.leaves.len()
  }

  /// Whether the rope has no children.
  pub fn is_empty(&self) -> bool {
    self.leaves.is_empty()
  }

  /// Whether every emitted text chunk is ASCII.
  pub fn is_ascii(&self) -> bool {
    self.summary().is_none_or(|summary| summary.is_ascii)
  }

  /// Position immediately after the generated output.
  pub fn generated_info(&self) -> GeneratedInfo {
    self.summary().map_or(
      GeneratedInfo {
        generated_line: 1,
        generated_column: 0,
      },
      |summary| GeneratedInfo {
        generated_line: summary.generated_line,
        generated_column: summary.generated_column,
      },
    )
  }

  fn summary(&self) -> Option<crate::rope::summary::NodeSummary> {
    self.root.map(|root| self.arena.get(root).summary())
  }

  fn children(&self) -> RopeChildren<'_> {
    RopeChildren {
      arena: &self.arena,
      leaves: self.leaves.iter(),
    }
  }

  #[cfg(feature = "rspack_cacheable")]
  pub(crate) fn child_sources(&self) -> impl ExactSizeIterator<Item = &BoxSource> {
    self.children()
  }
}

impl Source for RopeSource {
  fn source(&self) -> SourceValue<'_> {
    let mut children = self.children();
    if self.leaves.len() == 1 {
      return children
        .next()
        .expect("single-leaf rope has a child")
        .source();
    }
    let mut result = String::with_capacity(self.size());
    self.rope(&mut |chunk| result.push_str(chunk));
    SourceValue::String(Cow::Owned(result))
  }

  fn rope<'a>(&'a self, on_chunk: &mut dyn FnMut(&'a str)) {
    for child in self.children() {
      child.rope(on_chunk);
    }
  }

  fn buffer(&self) -> Cow<'_, [u8]> {
    let mut children = self.children();
    if self.leaves.len() == 1 {
      return children
        .next()
        .expect("single-leaf rope has a child")
        .buffer();
    }
    let mut result = Vec::with_capacity(self.size());
    self
      .to_writer(&mut result)
      .expect("writing to Vec cannot fail");
    Cow::Owned(result)
  }

  fn size(&self) -> usize {
    self.summary().map_or(0, |summary| summary.bytes)
  }

  fn map<'a>(&'a self, object_pool: &ObjectPool, options: &MapOptions) -> Option<SourceMap<'a>> {
    let chunks = self.stream_chunks();
    get_map(object_pool, chunks.as_ref(), options).map(SourceMap::from_fields)
  }

  fn map_static(
    self: Arc<Self>,
    object_pool: &ObjectPool,
    options: &MapOptions,
  ) -> Option<SourceMap<'static>> {
    let owner = self.clone();
    self
      .as_ref()
      .map(object_pool, options)
      .map(|map| map.into_static(owner))
  }

  fn to_writer(&self, writer: &mut dyn std::io::Write) -> std::io::Result<()> {
    for child in self.children() {
      child.to_writer(writer)?;
    }
    Ok(())
  }
}

impl StreamChunks for RopeSource {
  fn stream_chunks<'a>(&'a self) -> Box<dyn Chunks<'a> + 'a> {
    let chunks = self.children().map(|child| child.stream_chunks()).collect();
    Box::new(CombinedSourceChunks::from_chunks(chunks))
  }
}

impl Hash for RopeSource {
  fn hash<H: Hasher>(&self, state: &mut H) {
    "RopeSource".hash(state);
    self.leaves.len().hash(state);
    for child in self.children() {
      child.hash(state);
    }
  }
}

impl PartialEq for RopeSource {
  fn eq(&self, other: &Self) -> bool {
    self.leaves.len() == other.leaves.len()
      && self
        .children()
        .zip(other.children())
        .all(|(left, right)| left == right)
  }
}

impl Eq for RopeSource {}

impl std::fmt::Debug for RopeSource {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let mut list = f.debug_list();
    for child in self.children() {
      list.entry(child);
    }
    list.finish()
  }
}

struct RopeChildren<'a> {
  arena: &'a RopeArena,
  leaves: std::slice::Iter<'a, NodeId>,
}

impl<'a> Iterator for RopeChildren<'a> {
  type Item = &'a BoxSource;

  fn next(&mut self) -> Option<Self::Item> {
    self.leaves.next().map(|id| match self.arena.get(*id) {
      Node::ChildSource { source, .. } => source,
      Node::Branch { .. } => {
        unreachable!("freeze leaf index only contains ChildSource nodes")
      }
    })
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    self.leaves.size_hint()
  }
}

impl ExactSizeIterator for RopeChildren<'_> {}
