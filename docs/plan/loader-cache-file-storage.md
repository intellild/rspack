# Simple persistent loader cache

## Scope

Keep the existing compiler-local `FxDashMap` loader cache and add an optional
storage backend. Reuse `rspack_storage`, but create a loader-cache-owned storage
instance instead of sharing the persistent cache instance. Do not reuse the
persistent cache context/snapshot/occasion machinery, add a new public option,
or change the loader-runner state machine.

The feature is guarded by `experiments.loaderCache`, which defaults to
`false`. When the experiment is disabled, `Rule.use.cache` is ignored for
loader execution: no internal cache loader is inserted and no cache storage is
created. Enabling the experiment is required for both memory and persistent
loader caching.

The backend is enabled only for `cache.type = "persistent"`. It uses a separate
subdirectory and storage version:

```text
<cache.storage.directory>/loader-cache/<loader-storage-version>/
```

Entries use the `loader-cache-v1` storage scope and the cache-key hash as the
KV key. The concrete pack-file layout is owned by `rspack_storage`.

## Responsibilities

`LoaderCacheService` remains responsible for cache identity, resource
validation, serialization, L1 lookup, and dependency replay.
`LoaderCacheStorage` wraps one independent `BoxStorage` and provides:

```text
load(scope) -> entries
set(hash, bytes)
remove(hash)
flush()
```

The wrapper serializes access to `Storage`'s mutable staging API. Updates are
staged during loader execution, then one `save()` is issued after a successful
compilation. `compiler.close()` performs a final `save()` and `flush()`.
Storage failures become misses or ignored writes and must not fail a
compilation.

## Entry and validity

The persisted JSON payload contains:

- format version and complete cache identity;
- compiler scope, including compiler path/name/mode/context and persistent
  cache version;
- resource `mtime_ms` and file size;
- content, source map, and supported dependency data;

The key hash is only a KV lookup key. The complete identity remains in the
payload.

A hit requires:

```text
current mtime_ms == stored mtime_ms
current size     == stored size
```

As in `cache-loader`, a miss records its start time. The result is not cached
when the resource mtime falls in the same second as that start time or later:

```text
resource_mtime_ms / 1000 >= cache_start_ms / 1000
```

This avoids trusting a coarse filesystem timestamp when the resource may have
changed during loader execution. A resource stamp change between pitch and
store also prevents the candidate from being written. Missing files, parse
errors, version mismatches, and identity mismatches are misses.

## Storage isolation

Loader cache and persistent compilation cache must not use the same
`FileSystemStorage` instance or version directory:

- each owns its own `BoxStorage`;
- loader cache appends `loader-cache` to the configured storage directory;
- loader cache derives its own `Version` from compiler identity, cache version,
  Rspack version, and loader-cache format version;
- loader cache uses its own static scope.

This prevents the two independent `updates` maps and `TaskQueue`s from writing
the same database directory. Locking, atomic commits, packs, stale-version
cleanup, and cross-process recovery remain `rspack_storage` responsibilities.

## Execution flow

```text
Compiler::new
  experiments.loaderCache?
       ↓
LoaderCacheService::from_compiler_options
  memory L1: FxDashMap
  persistent cache?
       ↓
create independent FileSystemStorage
  directory: <cache directory>/loader-cache
  scope: loader-cache-v1
       ↓
cache-loader first access
  bulk-load scope once into L1
       ↓
cache hit / loader execution
       ↓
stage set/remove
       ↓
compilation completes
  save staged updates
       ↓
Compiler::close
  save + flush loader storage
```

## Integration

- Add `experiments.loaderCache?: boolean` to the public TypeScript options,
  defaulting to `false`, and pass it through raw options into Rust
  `Experiments.loader_cache`.
- The NormalModuleFactory checks this experiment before inserting the internal
  cache loader. Disabled experiments retain ordinary loader behavior.
- The compiler derives the loader-cache root from the existing persistent
  filesystem cache options and constructs one compiler-local storage instance.
- `MemoryCache` and `DisableCache` continue using memory-only loader cache.
- Do not expose or share the persistent cache's `BoxStorage`.
- Keep the plugin/compiler ownership and compiler isolation unchanged.
- `compiler.close()` flushes pending loader-cache storage tasks.

## Limits of v1

- No TTL/LRU or size budget; versioned directories are the cleanup boundary.
- No remote backend or loader-specific cross-process generation protocol.
- Arbitrary `AdditionalData` stays memory-only when it cannot be encoded.
- mtime/size validation is intentionally weaker than content hashing and may
  miss externally preserved timestamps. Dependency changes are replayed for
  watch/build bookkeeping but are not additional cache-key inputs in v1.

## Tests

- cold process/compiler writes, then a second compiler hits the storage entry;
- unchanged mtime/size hits and changed mtime/size misses;
- resources modified in the cache-attempt second are not cached;
- malformed/truncated/version-invalid entries degrade to miss;
- storage write/read failures do not fail compilation;
- loader and persistent caches use distinct storage directories;
- memory-only cache paths do not touch disk.
