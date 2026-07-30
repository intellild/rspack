use std::{
  hash::{DefaultHasher, Hash, Hasher},
  path::PathBuf,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use rspack_cacheable::{cacheable, cacheable_dyn};
use rspack_collections::Identifiable;
use rspack_error::Result;
use rspack_fs::IntermediateFileSystem;
use rspack_hash::{HashFunction, RspackHasher};
use rspack_loader_runner::{AdditionalData, Content, Loader, LoaderContext, Scheme};
use rspack_sources::SourceMap;
use rspack_util::{Timestamp, fx_hash::FxDashMap};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OnceCell};

use crate::{
  CacheOptions, CompilerOptions, RunnerContext,
  cache::persistent::storage::{BoxStorage, StorageOptions, Version, create_storage},
};

pub(crate) const INTERNAL_CACHE_LOADER_IDENTIFIER: &str = "builtin:cache-loader";

const FORMAT_VERSION: u8 = 1;
const STORAGE_SCOPE: &str = "loader-cache-v1";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct LoaderCacheKey {
  rspack_version: String,
  compiler_scope: String,
  module_identifier: String,
  remaining_request: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct ResourceStamp {
  mtime_ms: Timestamp,
  size: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DependencyDelta {
  added: FxHashSet<PathBuf>,
  removed: FxHashSet<PathBuf>,
}

#[derive(Debug, Clone)]
struct LoaderCacheEntry {
  resource: ResourceStamp,
  content: Option<Content>,
  source_map: Option<String>,
  additional_data: Option<AdditionalData>,
  file_dependencies: DependencyDelta,
  context_dependencies: DependencyDelta,
  missing_dependencies: DependencyDelta,
  build_dependencies: DependencyDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum PersistedContent {
  String(String),
  Buffer(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedPayload {
  version: u8,
  identity: LoaderCacheKey,
  resource: ResourceStamp,
  content: PersistedContent,
  source_map: Option<String>,
  file_dependencies: DependencyDelta,
  context_dependencies: DependencyDelta,
  missing_dependencies: DependencyDelta,
  build_dependencies: DependencyDelta,
}

#[derive(Debug, Serialize, Deserialize)]
struct PitchData {
  key: LoaderCacheKey,
  digest: String,
  resource: ResourceStamp,
  started_at_ms: Timestamp,
  diagnostics_len: usize,
  file_dependencies: FxHashSet<PathBuf>,
  context_dependencies: FxHashSet<PathBuf>,
  missing_dependencies: FxHashSet<PathBuf>,
  build_dependencies: FxHashSet<PathBuf>,
}

#[derive(Debug)]
struct LoaderCacheStorage {
  storage: Mutex<BoxStorage>,
  readonly: bool,
  dirty: AtomicBool,
}

impl LoaderCacheStorage {
  fn new(storage: BoxStorage, readonly: bool) -> Self {
    Self {
      storage: Mutex::new(storage),
      readonly,
      dirty: AtomicBool::new(false),
    }
  }

  async fn load(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
    let storage = self.storage.lock().await;
    storage.cleanup_stale();
    storage.load(STORAGE_SCOPE).await.unwrap_or_default()
  }

  async fn set(&self, key: Vec<u8>, value: Vec<u8>) {
    if self.readonly {
      return;
    }
    let mut storage = self.storage.lock().await;
    storage.set(STORAGE_SCOPE, key, value);
    self.dirty.store(true, Ordering::Release);
  }

  async fn remove(&self, key: &[u8]) {
    if self.readonly {
      return;
    }
    let mut storage = self.storage.lock().await;
    storage.remove(STORAGE_SCOPE, key);
    self.dirty.store(true, Ordering::Release);
  }

  async fn save(&self) {
    if self.readonly || !self.dirty.swap(false, Ordering::AcqRel) {
      return;
    }
    self.storage.lock().await.save();
  }

  async fn flush(&self) {
    self.storage.lock().await.flush().await;
  }
}

#[derive(Debug)]
pub(crate) struct LoaderCacheService {
  compiler_scope: String,
  entries: FxDashMap<LoaderCacheKey, LoaderCacheEntry>,
  storage: Option<LoaderCacheStorage>,
  storage_loaded: OnceCell<()>,
}

impl LoaderCacheService {
  pub(crate) fn from_compiler_options(
    compiler_path: &str,
    options: &CompilerOptions,
    intermediate_filesystem: Arc<dyn IntermediateFileSystem>,
  ) -> Option<Self> {
    if !options.experiments.loader_cache {
      return None;
    }

    let (cache_version, storage) = match &options.cache {
      CacheOptions::Persistent(option) => {
        let storage_options = match &option.storage {
          StorageOptions::FileSystem { directory } => StorageOptions::FileSystem {
            directory: directory.join("loader-cache"),
          },
        };
        let compiler_scope_hash = hash_value(&compiler_path);
        let storage_version = Version::new(
          compiler_scope_hash,
          hash_value(&(
            FORMAT_VERSION,
            env!("CARGO_PKG_VERSION"),
            option.version.as_str(),
            options.name.as_deref().unwrap_or_default(),
            options.mode,
            options.context.as_str(),
          )),
        );
        let storage = create_storage(
          storage_options,
          storage_version,
          option.max_age,
          option.max_versions,
          intermediate_filesystem,
        );
        (
          option.version.as_str(),
          Some(LoaderCacheStorage::new(storage, option.readonly)),
        )
      }
      CacheOptions::Disabled | CacheOptions::Memory { .. } => ("", None),
    };
    let compiler_scope = format!(
      "{}\0{}\0{:?}\0{}\0{}",
      compiler_path,
      options.name.as_deref().unwrap_or_default(),
      options.mode,
      options.context,
      cache_version
    );

    Some(Self::new(compiler_scope, storage))
  }

  fn new(compiler_scope: String, storage: Option<LoaderCacheStorage>) -> Self {
    Self {
      compiler_scope,
      entries: FxDashMap::default(),
      storage,
      storage_loaded: OnceCell::new(),
    }
  }

  async fn ensure_storage_loaded(&self) {
    self
      .storage_loaded
      .get_or_init(|| async {
        let Some(storage) = &self.storage else {
          return;
        };
        for (_, bytes) in storage.load().await {
          if let Ok((key, entry)) = decode_stored_entry(&bytes) {
            self.entries.insert(key, entry);
          }
        }
      })
      .await;
  }

  async fn lookup(
    &self,
    key: &LoaderCacheKey,
    digest: &str,
    resource: ResourceStamp,
  ) -> Option<LoaderCacheEntry> {
    self.ensure_storage_loaded().await;
    if let Some(entry) = self.entries.get(key).map(|entry| entry.value().clone())
      && entry.resource == resource
    {
      return Some(entry);
    }
    self.entries.remove(key);
    if let Some(storage) = &self.storage {
      storage.remove(digest.as_bytes()).await;
    }
    None
  }

  async fn store(&self, key: LoaderCacheKey, digest: String, entry: LoaderCacheEntry) {
    self.ensure_storage_loaded().await;
    self.entries.insert(key.clone(), entry.clone());

    let Some(storage) = &self.storage else {
      return;
    };
    // AdditionalData has no stable serialization contract. This entry remains
    // useful in L1, but any older disk entry must not be reused.
    if entry.additional_data.is_some() {
      storage.remove(digest.as_bytes()).await;
      return;
    }
    let Some(bytes) = encode_entry(key, entry) else {
      return;
    };
    storage.set(digest.into_bytes(), bytes).await;
  }

  async fn remove(&self, key: &LoaderCacheKey, digest: &str) {
    self.ensure_storage_loaded().await;
    self.entries.remove(key);
    if let Some(storage) = &self.storage {
      storage.remove(digest.as_bytes()).await;
    }
  }

  pub(crate) async fn close(&self) {
    if let Some(storage) = &self.storage {
      storage.save().await;
      storage.flush().await;
    }
  }

  pub(crate) async fn save(&self) {
    if let Some(storage) = &self.storage {
      storage.save().await;
    }
  }
}

fn hash_value(value: &impl Hash) -> String {
  let mut hasher = DefaultHasher::new();
  value.hash(&mut hasher);
  hex::encode(hasher.finish().to_ne_bytes())
}

fn encode_entry(key: LoaderCacheKey, entry: LoaderCacheEntry) -> Option<Vec<u8>> {
  let content = match entry.content? {
    Content::String(value) => PersistedContent::String(value),
    Content::Buffer(value) => PersistedContent::Buffer(value),
  };
  let payload = PersistedPayload {
    version: FORMAT_VERSION,
    identity: key,
    resource: entry.resource,
    content,
    source_map: entry.source_map,
    file_dependencies: entry.file_dependencies,
    context_dependencies: entry.context_dependencies,
    missing_dependencies: entry.missing_dependencies,
    build_dependencies: entry.build_dependencies,
  };
  serde_json::to_vec(&payload).ok()
}

fn decode_stored_entry(
  bytes: &[u8],
) -> std::result::Result<(LoaderCacheKey, LoaderCacheEntry), ()> {
  let payload: PersistedPayload = serde_json::from_slice(bytes).map_err(|_| ())?;
  if payload.version != FORMAT_VERSION {
    return Err(());
  }
  let key = payload.identity;
  let content = match payload.content {
    PersistedContent::String(value) => Content::String(value),
    PersistedContent::Buffer(value) => Content::Buffer(value),
  };
  Ok((
    key,
    LoaderCacheEntry {
      resource: payload.resource,
      content: Some(content),
      source_map: payload.source_map,
      additional_data: None,
      file_dependencies: payload.file_dependencies,
      context_dependencies: payload.context_dependencies,
      missing_dependencies: payload.missing_dependencies,
      build_dependencies: payload.build_dependencies,
    },
  ))
}

fn now_ms() -> Option<Timestamp> {
  let millis: u64 = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .ok()?
    .as_millis()
    .try_into()
    .ok()?;
  Some(millis.into())
}

fn hash_bytes(value: &[u8]) -> u64 {
  let mut hasher = RspackHasher::new(&HashFunction::Xxhash64);
  hasher.write(value);
  hasher.finish()
}

fn cache_key(loader_context: &LoaderContext<RunnerContext>) -> (LoaderCacheKey, String) {
  let loader_cache = loader_context
    .context
    .loader_cache
    .as_ref()
    .expect("cache loader should only run when loader cache is enabled");
  let key = LoaderCacheKey {
    rspack_version: env!("CARGO_PKG_VERSION").to_string(),
    compiler_scope: loader_cache.compiler_scope.clone(),
    module_identifier: loader_context
      .context
      .module
      .identifier()
      .as_str()
      .to_owned(),
    remaining_request: loader_context.remaining_request().to_string(),
  };
  let bytes = serde_json::to_vec(&key).expect("loader cache key should be serializable");
  let digest = format!("{:016x}", hash_bytes(&bytes));
  (key, digest)
}

async fn resource_stamp(loader_context: &LoaderContext<RunnerContext>) -> Option<ResourceStamp> {
  if loader_context.resource_data().get_scheme() != &Scheme::None {
    return None;
  }
  let path = loader_context.resource_path()?;
  if path.as_str().is_empty() {
    return None;
  }
  let metadata = loader_context.context.fs.metadata(path).await.ok()?;
  if !metadata.is_file {
    return None;
  }
  Some(ResourceStamp {
    mtime_ms: metadata.mtime_ms.into(),
    size: metadata.size,
  })
}

fn mtime_is_reliable(mtime: Timestamp, started_at: Timestamp) -> bool {
  // Match cache-loader's conservative handling for coarse filesystems: when
  // the resource mtime is in the same second as this cache attempt (or later),
  // it may have changed without producing a distinguishable timestamp.
  mtime.as_millis() / 1000 < started_at.as_millis() / 1000
}

fn dependency_delta(
  baseline: &FxHashSet<PathBuf>,
  current: &FxHashSet<PathBuf>,
) -> DependencyDelta {
  DependencyDelta {
    added: current.difference(baseline).cloned().collect(),
    removed: baseline.difference(current).cloned().collect(),
  }
}

fn replay_dependency_delta(dependencies: &mut FxHashSet<PathBuf>, delta: &DependencyDelta) {
  dependencies.retain(|dependency| !delta.removed.contains(dependency));
  dependencies.extend(delta.added.iter().cloned());
}

fn record_pitch_data(
  loader_context: &mut LoaderContext<RunnerContext>,
  key: LoaderCacheKey,
  digest: String,
  resource: ResourceStamp,
) {
  let data = PitchData {
    key,
    digest,
    resource,
    started_at_ms: now_ms().expect("system time should be after unix epoch"),
    diagnostics_len: loader_context.diagnostics.len(),
    file_dependencies: loader_context.file_dependencies.clone(),
    context_dependencies: loader_context.context_dependencies.clone(),
    missing_dependencies: loader_context.missing_dependencies.clone(),
    build_dependencies: loader_context.build_dependencies.clone(),
  };
  let index = loader_context.loader_index as usize;
  loader_context.loader_items[index]
    .set_data(serde_json::to_value(data).expect("cache loader pitch data should be serializable"));
}

#[cacheable]
#[derive(Debug, Default)]
pub(crate) struct CacheLoader;

#[async_trait]
#[cacheable_dyn]
impl Loader<RunnerContext> for CacheLoader {
  fn identifier(&self) -> rspack_collections::Identifier {
    INTERNAL_CACHE_LOADER_IDENTIFIER.into()
  }

  async fn pitch(&self, loader_context: &mut LoaderContext<RunnerContext>) -> Result<()> {
    let Some(resource) = resource_stamp(loader_context).await else {
      return Ok(());
    };
    let (key, digest) = cache_key(loader_context);
    let entry = loader_context
      .context
      .loader_cache
      .as_ref()
      .expect("cache loader should only run when loader cache is enabled")
      .lookup(&key, &digest, resource)
      .await;
    if let Some(entry) = entry {
      let source_map = match entry.source_map {
        Some(source_map) => match SourceMap::from_json(source_map) {
          Ok(source_map) => Some(source_map),
          Err(_) => {
            loader_context
              .context
              .loader_cache
              .as_ref()
              .expect("cache loader should only run when loader cache is enabled")
              .remove(&key, &digest)
              .await;
            record_pitch_data(loader_context, key, digest, resource);
            return Ok(());
          }
        },
        None => None,
      };
      replay_dependency_delta(
        &mut loader_context.file_dependencies,
        &entry.file_dependencies,
      );
      replay_dependency_delta(
        &mut loader_context.context_dependencies,
        &entry.context_dependencies,
      );
      replay_dependency_delta(
        &mut loader_context.missing_dependencies,
        &entry.missing_dependencies,
      );
      replay_dependency_delta(
        &mut loader_context.build_dependencies,
        &entry.build_dependencies,
      );
      loader_context.finish_with((entry.content, source_map, entry.additional_data));
      return Ok(());
    }

    record_pitch_data(loader_context, key, digest, resource);
    Ok(())
  }

  async fn run(&self, loader_context: &mut LoaderContext<RunnerContext>) -> Result<()> {
    let pitch_data =
      serde_json::from_value::<PitchData>(loader_context.current_loader().data().clone()).ok();
    if let Some(pitch_data) = pitch_data
      && loader_context.cacheable
      && loader_context.diagnostics.len() == pitch_data.diagnostics_len
      && resource_stamp(loader_context).await == Some(pitch_data.resource)
      && mtime_is_reliable(pitch_data.resource.mtime_ms, pitch_data.started_at_ms)
    {
      let file_dependencies = dependency_delta(
        &pitch_data.file_dependencies,
        &loader_context.file_dependencies,
      );
      let context_dependencies = dependency_delta(
        &pitch_data.context_dependencies,
        &loader_context.context_dependencies,
      );
      let missing_dependencies = dependency_delta(
        &pitch_data.missing_dependencies,
        &loader_context.missing_dependencies,
      );
      let build_dependencies = dependency_delta(
        &pitch_data.build_dependencies,
        &loader_context.build_dependencies,
      );
      let mut loader_files = build_dependencies.added.clone();
      loader_files.extend(
        loader_context
          .loader_items
          .iter()
          .filter(|item| item.path().is_absolute())
          .map(|item| item.path().to_path_buf().into_std_path_buf()),
      );
      let build_dependencies = DependencyDelta {
        added: loader_files,
        removed: build_dependencies.removed,
      };
      loader_context
        .context
        .loader_cache
        .as_ref()
        .expect("cache loader should only run when loader cache is enabled")
        .store(
          pitch_data.key,
          pitch_data.digest,
          LoaderCacheEntry {
            resource: pitch_data.resource,
            content: loader_context.content().cloned(),
            source_map: loader_context.source_map().map(SourceMap::to_json),
            additional_data: loader_context.additional_data().cloned(),
            file_dependencies,
            context_dependencies,
            missing_dependencies,
            build_dependencies,
          },
        )
        .await;
    }

    loader_context.current_loader().set_finish_called();
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use rspack_loader_runner::Content;
  use rspack_storage::MemoryStorage;
  use rustc_hash::FxHashSet;

  use super::{
    DependencyDelta, LoaderCacheEntry, LoaderCacheKey, LoaderCacheStorage, ResourceStamp,
    decode_stored_entry, dependency_delta, encode_entry, mtime_is_reliable,
    replay_dependency_delta,
  };

  fn dependencies(values: &[&str]) -> FxHashSet<PathBuf> {
    values.iter().map(PathBuf::from).collect()
  }

  fn key() -> LoaderCacheKey {
    LoaderCacheKey {
      rspack_version: "test".to_string(),
      compiler_scope: "scope".to_string(),
      module_identifier: "module".to_string(),
      remaining_request: "loader!resource".to_string(),
    }
  }

  fn entry() -> LoaderCacheEntry {
    LoaderCacheEntry {
      resource: ResourceStamp {
        mtime_ms: 1.into(),
        size: 3,
      },
      content: Some(Content::Buffer(vec![1, 2, 3])),
      source_map: None,
      additional_data: None,
      file_dependencies: DependencyDelta::default(),
      context_dependencies: DependencyDelta::default(),
      missing_dependencies: DependencyDelta::default(),
      build_dependencies: DependencyDelta::default(),
    }
  }

  #[test]
  fn replays_dependency_additions_and_removals() {
    let baseline = dependencies(&["resource.js", "removed.js"]);
    let current = dependencies(&["resource.js", "added.js"]);
    let delta = dependency_delta(&baseline, &current);

    let mut replayed = baseline;
    replay_dependency_delta(&mut replayed, &delta);

    assert_eq!(replayed, current);
  }

  #[test]
  fn json_entry_round_trips_and_rejects_bad_data() {
    let key = key();
    let bytes = encode_entry(key.clone(), entry()).unwrap();
    let (decoded_key, decoded) = decode_stored_entry(&bytes).unwrap();
    assert_eq!(decoded_key, key);
    assert_eq!(decoded.content, Some(Content::Buffer(vec![1, 2, 3])));

    assert!(decode_stored_entry(b"{").is_err());
    let mut payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    payload["version"] = serde_json::json!(0);
    assert!(decode_stored_entry(&serde_json::to_vec(&payload).unwrap()).is_err());
  }

  #[tokio::test]
  async fn storage_adapter_round_trips_bytes() {
    let storage = LoaderCacheStorage::new(Box::<MemoryStorage>::default(), false);
    storage.set(b"digest".to_vec(), b"cached".to_vec()).await;

    assert_eq!(
      storage.load().await,
      vec![(b"digest".to_vec(), b"cached".to_vec())]
    );
  }

  #[test]
  fn rejects_mtime_from_the_cache_attempt_second_or_later() {
    assert!(mtime_is_reliable(9_999.into(), 10_000.into()));
    assert!(!mtime_is_reliable(10_000.into(), 10_999.into()));
    assert!(!mtime_is_reliable(11_000.into(), 10_999.into()));
  }
}
