use std::{
  borrow::Cow,
  fmt,
  hash::{Hash, Hasher},
  num::NonZeroU32,
  sync::{Arc, OnceLock},
};

use rustc_hash::FxHashMap;

use crate::{
  BoxSource, Error, MapOptions, ObjectPool, RawStringSource, ReplaceSource, Result, RopeSource,
  Source, SourceEvent, SourceExt, SourceMap, SourceValue,
  helpers::{Chunks, StreamChunks},
};

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

/// An immutable typed placeholder leaf with a compatibility fallback.
///
/// The fallback preserves source output for callers that inspect an intermediate
/// source. Resolution uses the typed key emitted by
/// [`Source::rope_with_placeholders`], never the fallback bytes, so identical
/// user-authored text is not replaced.
#[derive(Clone, PartialEq, Eq)]
pub struct PlaceholderSource {
  key: PlaceholderKey,
  fallback: RawStringSource,
}

impl PlaceholderSource {
  /// Create a placeholder with an owned compatibility fallback.
  pub fn new(key: PlaceholderKey, fallback: impl Into<String>) -> Self {
    Self {
      key,
      fallback: RawStringSource::from(fallback.into()),
    }
  }

  /// Create a placeholder with a static compatibility fallback.
  pub fn from_static(key: PlaceholderKey, fallback: &'static str) -> Self {
    Self {
      key,
      fallback: RawStringSource::from_static(fallback),
    }
  }

  /// The semantic key used by a resolver.
  pub fn key(&self) -> &PlaceholderKey {
    &self.key
  }

  /// The bytes emitted before the placeholder is resolved.
  pub fn fallback(&self) -> &str {
    self.fallback.value()
  }
}

impl Source for PlaceholderSource {
  fn source(&self) -> SourceValue<'_> {
    self.fallback.source()
  }

  fn rope<'a>(&'a self, on_chunk: &mut dyn FnMut(&'a str)) {
    self.fallback.rope(on_chunk);
  }

  fn rope_with_placeholders<'a>(&'a self, on_event: &mut dyn FnMut(SourceEvent<'a>)) {
    on_event(SourceEvent::Placeholder(&self.key, self.fallback()));
  }

  fn buffer(&self) -> Cow<'_, [u8]> {
    self.fallback.buffer()
  }

  fn size(&self) -> usize {
    self.fallback.size()
  }

  fn map(&self, object_pool: &ObjectPool, options: &MapOptions) -> Option<SourceMap<'_>> {
    self.fallback.map(object_pool, options)
  }

  fn map_static(self: Arc<Self>, _: &ObjectPool, _: &MapOptions) -> Option<SourceMap<'static>> {
    None
  }

  fn to_writer(&self, writer: &mut dyn std::io::Write) -> std::io::Result<()> {
    self.fallback.to_writer(writer)
  }
}

impl StreamChunks for PlaceholderSource {
  fn stream_chunks<'a>(&'a self) -> Box<dyn Chunks<'a> + 'a> {
    self.fallback.stream_chunks()
  }
}

impl Hash for PlaceholderSource {
  fn hash<H: Hasher>(&self, state: &mut H) {
    "PlaceholderSource".hash(state);
    self.key.hash(state);
    self.fallback.hash(state);
  }
}

impl fmt::Debug for PlaceholderSource {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("PlaceholderSource")
      .field("key", &self.key)
      .field("fallback", &self.fallback())
      .finish()
  }
}

/// Byte range and semantic key of one typed placeholder in rendered source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderOccurrence {
  key: PlaceholderKey,
  start: u32,
  end: u32,
}

impl PlaceholderOccurrence {
  /// The semantic placeholder key.
  pub fn key(&self) -> &PlaceholderKey {
    &self.key
  }

  /// Start byte offset in compatibility output.
  pub fn start(&self) -> u32 {
    self.start
  }

  /// End byte offset in compatibility output.
  pub fn end(&self) -> u32 {
    self.end
  }
}

/// Collect typed placeholder ranges by traversing source nodes, without
/// materializing or scanning their text.
pub fn collect_placeholder_occurrences(source: &dyn Source) -> Result<Vec<PlaceholderOccurrence>> {
  let mut offset = 0usize;
  let mut occurrences = Vec::new();
  let mut overflow = false;
  source.rope_with_placeholders(&mut |event| match event {
    SourceEvent::Text(text) => {
      offset = offset.checked_add(text.len()).unwrap_or_else(|| {
        overflow = true;
        usize::MAX
      });
    }
    SourceEvent::Placeholder(key, fallback) => {
      let start = offset;
      offset = offset.checked_add(fallback.len()).unwrap_or_else(|| {
        overflow = true;
        usize::MAX
      });
      match (u32::try_from(start), u32::try_from(offset)) {
        (Ok(start), Ok(end)) => occurrences.push(PlaceholderOccurrence {
          key: key.clone(),
          start,
          end,
        }),
        _ => overflow = true,
      }
    }
  });
  if overflow {
    return Err(Error::ArenaOverflow);
  }
  Ok(occurrences)
}

/// Resolve typed placeholders with ordinary generated text.
///
/// User text is never inspected, so bytes identical to a compatibility
/// fallback remain unchanged.
pub fn replace_source_placeholders(
  source: BoxSource,
  mut resolver: impl FnMut(&PlaceholderKey) -> Option<String>,
) -> Result<BoxSource> {
  let mut original_offset = 0usize;
  let mut resolved_size = source.size();
  let mut resolutions = Vec::new();
  let mut overflow = false;
  let mut resolved = false;
  source.rope_with_placeholders(&mut |event| match event {
    SourceEvent::Text(text) => {
      original_offset = original_offset.checked_add(text.len()).unwrap_or_else(|| {
        overflow = true;
        usize::MAX
      });
    }
    SourceEvent::Placeholder(key, fallback) => {
      original_offset = original_offset
        .checked_add(fallback.len())
        .unwrap_or_else(|| {
          overflow = true;
          usize::MAX
        });
      if u32::try_from(original_offset).is_err() {
        overflow = true;
      }
      let value = resolver(key);
      if let Some(value) = &value {
        resolved = true;
        resolved_size = resolved_size
          .checked_sub(fallback.len())
          .and_then(|size| size.checked_add(value.len()))
          .unwrap_or_else(|| {
            overflow = true;
            usize::MAX
          });
      }
      resolutions.push(value);
    }
  });
  if overflow {
    return Err(Error::ArenaOverflow);
  }
  if resolutions.is_empty() || !resolved {
    return Ok(source);
  }
  Ok(
    ResolvedPlaceholderSource {
      inner: source,
      resolutions,
      resolved_size,
      replacement_source: Arc::new(OnceLock::new()),
    }
    .boxed(),
  )
}

/// A resolved placeholder view optimized for the common generated-source path.
///
/// Rendering streams typed events once. Source-map operations lazily construct
/// the equivalent ReplaceSource, preserving the original mappings.
#[derive(Clone)]
pub(crate) struct ResolvedPlaceholderSource {
  inner: BoxSource,
  resolutions: Vec<Option<String>>,
  resolved_size: usize,
  replacement_source: Arc<OnceLock<BoxSource>>,
}

impl ResolvedPlaceholderSource {
  fn for_each_event<'a>(&'a self, on_event: &mut dyn FnMut(SourceEvent<'a>)) {
    let mut index = 0usize;
    self.inner.rope_with_placeholders(&mut |event| match event {
      SourceEvent::Text(text) => on_event(SourceEvent::Text(text)),
      SourceEvent::Placeholder(key, fallback) => {
        let resolution = self
          .resolutions
          .get(index)
          .expect("placeholder topology changed after resolution");
        index += 1;
        match resolution {
          Some(value) => on_event(SourceEvent::Text(value)),
          None => on_event(SourceEvent::Placeholder(key, fallback)),
        }
      }
    });
    debug_assert_eq!(index, self.resolutions.len());
  }

  pub(crate) fn replacement_source(&self) -> &BoxSource {
    self.replacement_source.get_or_init(|| {
      let occurrences = collect_placeholder_occurrences(self.inner.as_ref())
        .expect("placeholder offsets validated when resolutions were created");
      debug_assert_eq!(occurrences.len(), self.resolutions.len());
      let mut output = ReplaceSource::new(self.inner.clone());
      for (occurrence, resolution) in occurrences.iter().zip(&self.resolutions) {
        if let Some(value) = resolution {
          output.replace(occurrence.start(), occurrence.end(), value.clone(), None);
        }
      }
      output.boxed()
    })
  }
}

impl Source for ResolvedPlaceholderSource {
  fn source(&self) -> SourceValue<'_> {
    let mut output = String::with_capacity(self.resolved_size);
    self.rope(&mut |text| output.push_str(text));
    SourceValue::String(Cow::Owned(output))
  }

  fn rope<'a>(&'a self, on_chunk: &mut dyn FnMut(&'a str)) {
    self.for_each_event(&mut |event| match event {
      SourceEvent::Text(text) | SourceEvent::Placeholder(_, text) => on_chunk(text),
    });
  }

  fn rope_with_placeholders<'a>(&'a self, on_event: &mut dyn FnMut(SourceEvent<'a>)) {
    self.for_each_event(on_event);
  }

  fn buffer(&self) -> Cow<'_, [u8]> {
    self.source().into_bytes()
  }

  fn size(&self) -> usize {
    self.resolved_size
  }

  fn map<'a>(&'a self, object_pool: &ObjectPool, options: &MapOptions) -> Option<SourceMap<'a>> {
    self.replacement_source().map(object_pool, options)
  }

  fn map_static(
    self: Arc<Self>,
    object_pool: &ObjectPool,
    options: &MapOptions,
  ) -> Option<SourceMap<'static>> {
    self
      .replacement_source()
      .clone()
      .map_static(object_pool, options)
  }

  fn to_writer(&self, writer: &mut dyn std::io::Write) -> std::io::Result<()> {
    let mut result = Ok(());
    self.rope(&mut |text| {
      if result.is_ok() {
        result = writer.write_all(text.as_bytes());
      }
    });
    result
  }
}

impl StreamChunks for ResolvedPlaceholderSource {
  fn stream_chunks<'a>(&'a self) -> Box<dyn Chunks<'a> + 'a> {
    self.replacement_source().stream_chunks()
  }
}

impl Hash for ResolvedPlaceholderSource {
  fn hash<H: Hasher>(&self, state: &mut H) {
    self.replacement_source().hash(state);
  }
}

impl PartialEq for ResolvedPlaceholderSource {
  fn eq(&self, other: &Self) -> bool {
    self.inner.as_ref() == other.inner.as_ref() && self.resolutions == other.resolutions
  }
}

impl Eq for ResolvedPlaceholderSource {}

impl fmt::Debug for ResolvedPlaceholderSource {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    self.replacement_source().fmt(f)
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
