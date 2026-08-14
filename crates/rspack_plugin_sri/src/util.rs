use std::borrow::Cow;

use cow_utils::CowUtils;
use rspack_core::{
  AssetInfo, ChunkGroupUkey, ChunkUkey, Compilation, ManifestAssetType, SourceType,
  rspack_sources::{PlaceholderKey, PlaceholderSource},
};
use rspack_util::fx_hash::FxIndexSet;

use crate::{SubresourceIntegrityHashFunction, integrity::compute_integrity};

pub const PLACEHOLDER_PREFIX: &str = "*-*-*-CHUNK-SRI-HASH-";
pub const PLACEHOLDER_KEY_PREFIX: &str = "rspack:sri:chunk:";

pub fn get_hash_variable(runtime_require_name: &str, source_type: SourceType) -> String {
  match source_type {
    SourceType::JavaScript => format!("{runtime_require_name}.sriHashes"),
    SourceType::Css => format!("{runtime_require_name}.sriCssHashes"),
    SourceType::Custom(t) if t == "css/mini-extract" => {
      format!("{runtime_require_name}.sriExtractCssHashes")
    }
    _ => unreachable!(),
  }
}

pub fn find_chunks(chunk: &ChunkUkey, compilation: &Compilation) -> FxIndexSet<ChunkUkey> {
  let mut all_chunks = FxIndexSet::default();
  let mut visited_groups = FxIndexSet::default();
  recurse_chunk(chunk, &mut all_chunks, &mut visited_groups, compilation);
  all_chunks
}

fn recurse_chunk_group(
  group: &ChunkGroupUkey,
  all_chunks: &mut FxIndexSet<ChunkUkey>,
  visited_groups: &mut FxIndexSet<ChunkGroupUkey>,
  compilation: &Compilation,
) {
  if visited_groups.contains(group) {
    return;
  }
  visited_groups.insert(*group);

  if let Some(chunk_group) = compilation
    .build_chunk_graph_artifact
    .chunk_group_by_ukey
    .get(group)
  {
    for chunk in chunk_group.chunks.iter() {
      recurse_chunk(chunk, all_chunks, visited_groups, compilation);
    }
    for child in chunk_group.children.iter() {
      recurse_chunk_group(child, all_chunks, visited_groups, compilation);
    }
  }
}

fn recurse_chunk(
  chunk: &ChunkUkey,
  all_chunks: &mut FxIndexSet<ChunkUkey>,
  visited_groups: &mut FxIndexSet<ChunkGroupUkey>,
  compilation: &Compilation,
) {
  if all_chunks.contains(chunk) {
    return;
  }
  all_chunks.insert(*chunk);

  if let Some(chunk) = compilation
    .build_chunk_graph_artifact
    .chunk_by_ukey
    .get(chunk)
  {
    for group in chunk.groups() {
      recurse_chunk_group(group, all_chunks, visited_groups, compilation);
    }
  }
}

pub fn make_placeholder(
  asset_type: ManifestAssetType,
  hash_funcs: &Vec<SubresourceIntegrityHashFunction>,
  id: &str,
) -> String {
  let placeholder_source = format!("{PLACEHOLDER_PREFIX}{asset_type}{id}");
  let filler = compute_integrity(hash_funcs, &placeholder_source);
  format!(
    "{}{}",
    PLACEHOLDER_PREFIX,
    &filler[PLACEHOLDER_PREFIX.len()..]
  )
}

pub fn make_placeholder_key(asset_type: &ManifestAssetType, id: &str) -> PlaceholderKey {
  PlaceholderKey::new(format!("{PLACEHOLDER_KEY_PREFIX}{asset_type}:{id}"))
}

pub fn make_placeholder_source(
  asset_type: ManifestAssetType,
  hash_funcs: &Vec<SubresourceIntegrityHashFunction>,
  id: &str,
) -> PlaceholderSource {
  let key = make_placeholder_key(&asset_type, id);
  let fallback = rspack_util::json_stringify_str(&make_placeholder(asset_type, hash_funcs, id));
  PlaceholderSource::new(key, fallback)
}

pub fn normalize_path(path: &str) -> Cow<'_, str> {
  path.split('?').next().unwrap_or("").cow_replace('\\', "/")
}

pub fn use_any_hash(info: &AssetInfo) -> bool {
  !info.chunk_hash.is_empty() || !info.full_hash.is_empty() || !info.content_hash.is_empty()
}
