use rspack_core::rspack_sources::{PlaceholderKey, PlaceholderSource};

pub const ESM_CHUNK_PLACEHOLDER_PREFIX: &str = "__RSPACK_ESM_CHUNK_";
pub const ESM_CHUNK_PLACEHOLDER_KEY_PREFIX: &str = "rspack:esm-library:chunk:";

pub fn esm_chunk_placeholder(chunk_id: &str) -> PlaceholderSource {
  PlaceholderSource::new(
    PlaceholderKey::new(format!("{ESM_CHUNK_PLACEHOLDER_KEY_PREFIX}{chunk_id}")),
    format!("{ESM_CHUNK_PLACEHOLDER_PREFIX}{chunk_id}"),
  )
}

pub fn chunk_id_from_placeholder(key: &PlaceholderKey) -> Option<&str> {
  key.as_str().strip_prefix(ESM_CHUNK_PLACEHOLDER_KEY_PREFIX)
}
