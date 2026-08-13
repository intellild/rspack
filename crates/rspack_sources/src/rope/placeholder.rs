use std::{borrow::Cow, fmt, num::NonZeroU32};

use rustc_hash::FxHashMap;

use crate::{BoxSource, Error, RawStringSource, Result, RopeSource, Source, SourceExt};

/// Semantic key for a template placeholder.
///
/// User text with the same bytes remains a text node and is never resolved.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaceholderKey(Cow<'static, str>);

impl PlaceholderKey {
  /// Create a key backed by an owned string.
  pub fn new(value: impl Into<String>) -> Self {
    Self(Cow::Owned(value.into()))
  }

  /// Create a key from a static semantic name without allocating.
  pub const fn from_static(value: &'static str) -> Self {
    Self(Cow::Borrowed(value))
  }

  /// The semantic name used in diagnostics.
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl From<String> for PlaceholderKey {
  fn from(value: String) -> Self {
    Self(Cow::Owned(value))
  }
}

/// Compact stable ID of a placeholder slot in one template.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaceholderId(NonZeroU32);

impl PlaceholderId {
  fn from_index(index: usize) -> Option<Self> {
    let index = u32::try_from(index).ok()?;
    NonZeroU32::new(index.checked_add(1)?).map(Self)
  }

  fn index(self) -> usize {
    (self.0.get() - 1) as usize
  }
}

static_assertions::const_assert_eq!(std::mem::size_of::<Option<PlaceholderId>>(), 4);

#[derive(Clone)]
enum ResolvedPlaceholder {
  Text(Cow<'static, str>),
  Source(BoxSource),
}

impl PartialEq for ResolvedPlaceholder {
  fn eq(&self, other: &Self) -> bool {
    match (self, other) {
      (Self::Text(left), Self::Text(right)) => left == right,
      (Self::Source(left), Self::Source(right)) => left == right,
      _ => false,
    }
  }
}

struct PlaceholderSlot {
  key: PlaceholderKey,
  value: Option<ResolvedPlaceholder>,
}

enum TemplatePart {
  Source(BoxSource),
  Text(Cow<'static, str>),
  Placeholder(PlaceholderId),
}

/// Mutable source composition that may contain unresolved typed placeholders.
///
/// This type intentionally does not implement [`Source`]. Only [`freeze`](Self::freeze)
/// produces an immutable [`RopeSource`] that can be emitted, mapped, hashed or cached.
pub struct TemplateRopeSource {
  parts: Vec<TemplatePart>,
  ids: FxHashMap<PlaceholderKey, PlaceholderId>,
  slots: Vec<PlaceholderSlot>,
  occurrences: Vec<Vec<usize>>,
}

impl Default for TemplateRopeSource {
  fn default() -> Self {
    Self::new()
  }
}

impl TemplateRopeSource {
  /// Create an empty template.
  pub fn new() -> Self {
    Self {
      parts: Vec::new(),
      ids: FxHashMap::default(),
      slots: Vec::new(),
      occurrences: Vec::new(),
    }
  }

  /// Append ordinary text. It is never interpreted as a placeholder marker.
  pub fn append_text(&mut self, text: impl Into<String>) {
    self.parts.push(TemplatePart::Text(Cow::Owned(text.into())));
  }

  /// Append static ordinary text without allocating.
  pub fn append_static(&mut self, text: &'static str) {
    self.parts.push(TemplatePart::Text(Cow::Borrowed(text)));
  }

  /// Append an already frozen source child.
  pub fn append_source<T: Source + 'static>(&mut self, source: T) {
    self.parts.push(TemplatePart::Source(source.boxed()));
  }

  /// Register a semantic placeholder key, returning the existing ID on duplicates.
  pub fn register(&mut self, key: PlaceholderKey) -> PlaceholderId {
    if let Some(id) = self.ids.get(&key) {
      return *id;
    }
    let id = PlaceholderId::from_index(self.slots.len())
      .expect("template cannot contain more than u32::MAX placeholders");
    self.ids.insert(key.clone(), id);
    self.slots.push(PlaceholderSlot { key, value: None });
    self.occurrences.push(Vec::new());
    id
  }

  /// Append one occurrence of a semantic placeholder.
  pub fn append_placeholder(&mut self, key: PlaceholderKey) -> PlaceholderId {
    let id = self.register(key);
    self.append_placeholder_id(id);
    id
  }

  /// Append another occurrence of an already registered placeholder.
  pub fn append_placeholder_id(&mut self, id: PlaceholderId) {
    self
      .slots
      .get(id.index())
      .expect("PlaceholderId belongs to this template");
    self.occurrences[id.index()].push(self.parts.len());
    self.parts.push(TemplatePart::Placeholder(id));
  }

  /// Resolve a placeholder to generated text.
  pub fn resolve_text(&mut self, id: PlaceholderId, value: impl Into<String>) -> Result<()> {
    self.resolve(id, ResolvedPlaceholder::Text(Cow::Owned(value.into())))
  }

  /// Resolve a placeholder to static generated text without allocating.
  pub fn resolve_static(&mut self, id: PlaceholderId, value: &'static str) -> Result<()> {
    self.resolve(id, ResolvedPlaceholder::Text(Cow::Borrowed(value)))
  }

  /// Resolve a placeholder to an already frozen source.
  pub fn resolve_source<T: Source + 'static>(&mut self, id: PlaceholderId, value: T) -> Result<()> {
    self.resolve(id, ResolvedPlaceholder::Source(value.boxed()))
  }

  fn resolve(&mut self, id: PlaceholderId, value: ResolvedPlaceholder) -> Result<()> {
    let slot = self
      .slots
      .get_mut(id.index())
      .expect("PlaceholderId belongs to this template");
    match &slot.value {
      Some(existing) if existing != &value => {
        Err(Error::ConflictingPlaceholder(slot.key.as_str().to_owned()))
      }
      Some(_) => Ok(()),
      None => {
        slot.value = Some(value);
        Ok(())
      }
    }
  }

  /// Validate all slots and freeze this builder into an immutable source.
  pub fn freeze(self) -> Result<RopeSource> {
    for slot in &self.slots {
      if slot.value.is_none() {
        return Err(Error::UnresolvedPlaceholder(slot.key.as_str().to_owned()));
      }
    }

    let mut children = Vec::with_capacity(self.parts.len());
    let mut pending_text = String::new();
    let flush_text = |pending_text: &mut String, children: &mut Vec<BoxSource>| {
      if !pending_text.is_empty() {
        children.push(RawStringSource::from(std::mem::take(pending_text)).boxed());
      }
    };
    for part in self.parts {
      match part {
        TemplatePart::Source(source) => {
          flush_text(&mut pending_text, &mut children);
          children.push(source);
        }
        TemplatePart::Text(text) => pending_text.push_str(&text),
        TemplatePart::Placeholder(id) => {
          let value = self.slots[id.index()]
            .value
            .as_ref()
            .expect("all slots validated before freezing");
          match value {
            ResolvedPlaceholder::Text(text) => pending_text.push_str(text),
            ResolvedPlaceholder::Source(source) => {
              flush_text(&mut pending_text, &mut children);
              children.push(source.clone());
            }
          }
        }
      }
    }
    flush_text(&mut pending_text, &mut children);
    Ok(RopeSource::from_boxed(children))
  }
}

impl fmt::Debug for TemplateRopeSource {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("TemplateRopeSource")
      .field("parts", &self.parts.len())
      .field("placeholders", &self.slots.len())
      .field(
        "unresolved",
        &self
          .slots
          .iter()
          .filter(|slot| slot.value.is_none())
          .count(),
      )
      .finish()
  }
}
