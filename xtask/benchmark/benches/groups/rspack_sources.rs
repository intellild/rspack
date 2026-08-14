#![allow(missing_docs)]
#![allow(clippy::zero_sized_map_values)]

#[path = "rspack_sources_complex_replace_source.rs"]
mod bench_complex_replace_source;
#[path = "rspack_sources_source_map.rs"]
mod bench_source_map;
#[path = "rspack_sources_repetitive_react_components.rs"]
mod benchmark_repetitive_react_components;

use std::collections::HashMap;

use bench_complex_replace_source::{
  benchmark_complex_replace_source_map,
  benchmark_complex_replace_source_map_cached_source_stream_chunks,
  benchmark_complex_replace_source_size, benchmark_complex_replace_source_source,
};
use bench_source_map::{benchmark_parse_source_map_from_json, benchmark_source_map_to_json};
use benchmark_repetitive_react_components::{
  benchmark_repetitive_react_components_map, benchmark_repetitive_react_components_source,
};
use criterion::{Bencher, BenchmarkId};
use rspack_benchmark::Criterion;
use rspack_sources::{
  BoxSource, CachedSource, ConcatSource, LegacyReplaceSourceBenchmark, MapOptions, ObjectPool,
  OriginalSource, PlaceholderKey, PlaceholderSource, RawStringSource, ReplaceSource, RopeSource,
  Source, SourceExt, SourceMap, SourceMapSource, SourceMapSourceOptions, TemplateRopeSource,
  replace_source_placeholders,
};

const HELLOWORLD_JS: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/benches/fixtures/rspack_sources/transpile-minify/files/helloworld.js"
));
const HELLOWORLD_JS_MAP: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/benches/fixtures/rspack_sources/transpile-minify/files/helloworld.js.map"
));
const HELLOWORLD_MIN_JS: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/benches/fixtures/rspack_sources/transpile-minify/files/helloworld.min.js"
));
const HELLOWORLD_MIN_JS_MAP: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/benches/fixtures/rspack_sources/transpile-minify/files/helloworld.min.js.map"
));
const BUNDLE_JS: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/benches/fixtures/rspack_sources/transpile-rollup/files/bundle.js"
));
const BUNDLE_JS_MAP: &str = include_str!(concat!(
  env!("CARGO_MANIFEST_DIR"),
  "/benches/fixtures/rspack_sources/transpile-rollup/files/bundle.js.map"
));

fn benchmark_concat_generate_string(b: &mut Bencher) {
  let sms_minify = SourceMapSource::new(SourceMapSourceOptions {
    value: HELLOWORLD_MIN_JS,
    name: "helloworld.min.js",
    source_map: SourceMap::from_json(HELLOWORLD_MIN_JS_MAP.to_string()).unwrap(),
    original_source: Some(HELLOWORLD_JS.to_string().into()),
    inner_source_map: Some(SourceMap::from_json(HELLOWORLD_JS_MAP.to_string()).unwrap()),
    remove_original_source: false,
  });

  let sms_rollup = SourceMapSource::new(SourceMapSourceOptions {
    value: BUNDLE_JS,
    name: "bundle.js",
    source_map: SourceMap::from_json(BUNDLE_JS_MAP.to_string()).unwrap(),
    original_source: None,
    inner_source_map: None,
    remove_original_source: false,
  });

  let concat = ConcatSource::new([sms_minify, sms_rollup]);

  b.iter(|| {
    concat
      .map(&ObjectPool::default(), &MapOptions::default())
      .unwrap()
      .to_json();
  })
}

fn benchmark_concat_generate_string_with_cache(b: &mut Bencher) {
  let sms_minify = SourceMapSource::new(SourceMapSourceOptions {
    value: HELLOWORLD_MIN_JS,
    name: "helloworld.min.js",
    source_map: SourceMap::from_json(HELLOWORLD_MIN_JS_MAP.to_string()).unwrap(),
    original_source: Some(HELLOWORLD_JS.to_string().into()),
    inner_source_map: Some(SourceMap::from_json(HELLOWORLD_JS_MAP.to_string()).unwrap()),
    remove_original_source: false,
  });
  let sms_rollup = SourceMapSource::new(SourceMapSourceOptions {
    value: BUNDLE_JS,
    name: "bundle.js",
    source_map: SourceMap::from_json(BUNDLE_JS_MAP.to_string()).unwrap(),
    original_source: None,
    inner_source_map: None,
    remove_original_source: false,
  });
  let concat = ConcatSource::new([sms_minify, sms_rollup]);
  let cached = CachedSource::new(concat);

  b.iter(|| {
    cached
      .map(&ObjectPool::default(), &MapOptions::default())
      .unwrap()
      .to_json();
  })
}

fn benchmark_cached_source_hash(b: &mut Bencher) {
  let sms_minify = SourceMapSource::new(SourceMapSourceOptions {
    value: HELLOWORLD_MIN_JS,
    name: "helloworld.min.js",
    source_map: SourceMap::from_json(HELLOWORLD_MIN_JS_MAP.to_string()).unwrap(),
    original_source: Some(HELLOWORLD_JS.to_string().into()),
    inner_source_map: Some(SourceMap::from_json(HELLOWORLD_JS_MAP.to_string()).unwrap()),
    remove_original_source: false,
  });
  let sms_rollup = SourceMapSource::new(SourceMapSourceOptions {
    value: BUNDLE_JS,
    name: "bundle.js",
    source_map: SourceMap::from_json(BUNDLE_JS_MAP.to_string()).unwrap(),
    original_source: None,
    inner_source_map: None,
    remove_original_source: false,
  });
  let concat = ConcatSource::new([sms_minify, sms_rollup]);
  let cached = CachedSource::new(concat).boxed();

  b.iter(|| {
    let mut m = HashMap::<BoxSource, ()>::new();
    m.insert(cached.clone(), ());
    let _ = std::hint::black_box(|| m.get(&cached));
    let _ = std::hint::black_box(|| m.get(&cached));
  })
}

fn benchmark_concat_source_add_many(b: &mut Bencher) {
  // Mimic rspack's concatenated_module / runtime hot path: build a ConcatSource
  // by adding many small children sequentially. 500 matches the scale of a
  // typical concatenated module (rspack chains 300+ adds per module).
  let pieces: Vec<BoxSource> = (0..500)
    .map(|i| RawStringSource::from(format!("// piece {i}\n")).boxed())
    .collect();

  b.iter(|| {
    let mut concat = ConcatSource::default();
    for piece in &pieces {
      concat.add(piece.clone());
    }
    std::hint::black_box(concat);
  })
}

fn benchmark_concat_source_add_few(b: &mut Bencher) {
  // Smaller scale: closer to runtime module assembly (~10-15 adds per chunk).
  let pieces: Vec<BoxSource> = (0..16)
    .map(|i| RawStringSource::from(format!("// piece {i}\n")).boxed())
    .collect();

  b.iter(|| {
    let mut concat = ConcatSource::default();
    for piece in &pieces {
      concat.add(piece.clone());
    }
    std::hint::black_box(concat);
  })
}

fn replacement_order(count: usize, pattern: &str) -> Vec<u32> {
  let mut order = (0..count as u32).collect::<Vec<_>>();
  match pattern {
    "ascending" => {}
    "descending" => order.reverse(),
    "random" => {
      let mut state = 0x9e37_79b9_u32;
      for i in (1..order.len()).rev() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        order.swap(i, state as usize % (i + 1));
      }
    }
    "runs" => {
      let run_count = 6;
      let run_len = count.div_ceil(run_count);
      let run_order = [3, 0, 5, 1, 4, 2];
      order.clear();
      for run in run_order {
        let start = run * run_len;
        let end = (start + run_len).min(count);
        order.extend((start..end).map(|value| value as u32));
      }
    }
    _ => unreachable!(),
  }
  order
}

fn build_replace_source(order: &[u32]) -> ReplaceSource {
  let mut source = ReplaceSource::new(RawStringSource::from_static(""));
  for &position in order {
    source.insert_static(position * 2, "x", None);
  }
  source
}

fn build_reference_vec(order: &[u32]) -> LegacyReplaceSourceBenchmark {
  let mut source = LegacyReplaceSourceBenchmark::new(RawStringSource::from_static(""));
  for &position in order {
    source.insert_static(position * 2, "x", None);
  }
  source
}

fn assert_replacement_differential(order: &[u32]) {
  let original = (0..order.len())
    .map(|index| format!("value_{index:04};\n"))
    .collect::<String>();
  let mut expected = ReplaceSource::new(OriginalSource::new(original.clone(), "ordered.js"));
  let mut actual = ReplaceSource::new(OriginalSource::new(original, "ordered.js"));
  for position in 0..order.len() as u32 {
    let start = position * 12;
    expected.replace(start, start + 5, format!("item_{position}"), None);
  }
  for &position in order {
    let start = position * 12;
    actual.replace(start, start + 5, format!("item_{position}"), None);
  }
  assert_eq!(actual.source(), expected.source());
  assert_eq!(actual.size(), expected.size());
  assert_eq!(
    actual.map(&ObjectPool::default(), &MapOptions::new(true)),
    expected.map(&ObjectPool::default(), &MapOptions::new(true))
  );
  assert_eq!(
    actual.map(&ObjectPool::default(), &MapOptions::new(false)),
    expected.map(&ObjectPool::default(), &MapOptions::new(false))
  );
}

fn benchmark_replacement_build(
  group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
) {
  for count in [16, 128, 1024, 6144] {
    for pattern in ["ascending", "descending", "random", "runs"] {
      let order = replacement_order(count, pattern);
      if count == 1024 {
        assert_replacement_differential(&order);
      }
      if count == 6144 {
        let stats = build_replace_source(&order).benchmark_replacement_stats();
        println!(
          "replacement {pattern}: nodes={}, height={}, bytes/node={}",
          stats.0, stats.1, stats.2
        );
      }
      group.bench_with_input(
        BenchmarkId::new(format!("replacement_build/store/{pattern}"), count),
        &order,
        |b, order| {
          b.iter(|| std::hint::black_box(build_replace_source(std::hint::black_box(order))));
        },
      );
      group.bench_with_input(
        BenchmarkId::new(format!("replacement_build/reference_vec/{pattern}"), count),
        &order,
        |b, order| {
          b.iter(|| std::hint::black_box(build_reference_vec(std::hint::black_box(order))));
        },
      );
    }
  }
}

fn composition_children(count: usize) -> Vec<BoxSource> {
  (0..count)
    .map(|index| {
      let text = format!("const value_{index} = {index};\n");
      if index % 2 == 0 {
        RawStringSource::from(text).boxed()
      } else {
        OriginalSource::new(text, format!("source-{index}.js")).boxed()
      }
    })
    .collect()
}

fn benchmark_rope_source(
  group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
) {
  for count in [16, 128, 1024] {
    let children = composition_children(count);
    let concat = ConcatSource::new(children.clone());
    let rope = RopeSource::from_boxed(children);
    assert_eq!(concat.source(), rope.source());
    assert_eq!(
      concat.source().into_string_lossy(),
      rope.benchmark_source_with_parent()
    );
    assert_eq!(
      concat.source().into_string_lossy(),
      rope.benchmark_source_with_stack()
    );
    assert_eq!(
      concat.source().into_string_lossy(),
      rope.benchmark_source_with_index()
    );
    assert_eq!(concat.size(), rope.size());
    let (nodes, height, node_bytes, old_index_bytes) = rope.benchmark_arena_stats();
    println!(
      "rope {count}: nodes={nodes}, height={height}, bytes/node={node_bytes}, old-index-bytes={old_index_bytes}"
    );
    for columns in [false, true] {
      assert_eq!(
        concat.map(&ObjectPool::default(), &MapOptions::new(columns)),
        rope.map(&ObjectPool::default(), &MapOptions::new(columns)),
      );
    }

    group.bench_with_input(
      BenchmarkId::new("composition/source/concat", count),
      &concat,
      |b, source| {
        b.iter(|| std::hint::black_box(source.source()));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/source/rope_contiguous", count),
      &rope,
      |b, source| {
        b.iter(|| std::hint::black_box(source.source()));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/source/rope_parent", count),
      &rope,
      |b, source| {
        b.iter(|| std::hint::black_box(source.benchmark_source_with_parent()));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/source/rope_stack", count),
      &rope,
      |b, source| {
        b.iter(|| std::hint::black_box(source.benchmark_source_with_stack()));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/source/rope_leaf_index", count),
      &rope,
      |b, source| {
        b.iter(|| std::hint::black_box(source.benchmark_source_with_index()));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/size/concat", count),
      &concat,
      |b, source| {
        b.iter(|| std::hint::black_box(source.size()));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/size/rope", count),
      &rope,
      |b, source| {
        b.iter(|| std::hint::black_box(source.size()));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/map_columns/concat", count),
      &concat,
      |b, source| {
        b.iter(|| std::hint::black_box(source.map(&ObjectPool::default(), &MapOptions::new(true))));
      },
    );
    group.bench_with_input(
      BenchmarkId::new("composition/map_columns/rope", count),
      &rope,
      |b, source| {
        b.iter(|| std::hint::black_box(source.map(&ObjectPool::default(), &MapOptions::new(true))));
      },
    );
  }
}

const BENCHMARK_PLACEHOLDER: &str = "__RSPACK_BENCHMARK_PLACEHOLDER__";

fn legacy_placeholder_source(count: usize) -> BoxSource {
  ConcatSource::new(
    (0..count)
      .map(|_| RawStringSource::from(format!("x{BENCHMARK_PLACEHOLDER}")))
      .collect::<Vec<_>>(),
  )
  .boxed()
}

fn resolve_legacy_placeholders(source: BoxSource) -> String {
  let materialized = source.source().into_string_lossy();
  let matches = materialized
    .match_indices(BENCHMARK_PLACEHOLDER)
    .map(|(start, marker)| (start, start + marker.len()))
    .collect::<Vec<_>>();
  let mut replaced = ReplaceSource::new(source);
  for (start, end) in matches {
    replaced.replace_static(start as u32, end as u32, "resolved", None);
  }
  replaced.source().into_string_lossy().into_owned()
}
fn typed_event_placeholder_source(count: usize) -> BoxSource {
  let mut source = ConcatSource::default();
  for _ in 0..count {
    source.add(RawStringSource::from_static("x"));
    source.add(PlaceholderSource::from_static(
      PlaceholderKey::from_static("benchmark"),
      BENCHMARK_PLACEHOLDER,
    ));
  }
  source.boxed()
}

fn resolve_typed_event_placeholders(source: BoxSource) -> String {
  replace_source_placeholders(source, |key| {
    (key.as_str() == "benchmark").then(|| "resolved".to_string())
  })
  .unwrap()
  .source()
  .into_string_lossy()
  .into_owned()
}

fn typed_placeholder_template(count: usize) -> (TemplateRopeSource, rspack_sources::PlaceholderId) {
  let mut template = TemplateRopeSource::new();
  let id = template.register(PlaceholderKey::from_static("benchmark"));
  for _ in 0..count {
    template.append_static("x");
    template.append_placeholder_id(id);
  }
  (template, id)
}

fn resolve_typed_placeholders(
  mut template: TemplateRopeSource,
  id: rspack_sources::PlaceholderId,
) -> String {
  template.resolve_static(id, "resolved").unwrap();
  template
    .freeze()
    .unwrap()
    .source()
    .into_string_lossy()
    .into_owned()
}

fn benchmark_placeholder_resolution(
  group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
) {
  for count in [1, 100, 1000] {
    let expected = resolve_legacy_placeholders(legacy_placeholder_source(count));
    let (expected_template, expected_id) = typed_placeholder_template(count);
    assert_eq!(
      expected,
      resolve_typed_placeholders(expected_template, expected_id)
    );
    assert_eq!(
      expected,
      resolve_typed_event_placeholders(typed_event_placeholder_source(count))
    );
    let source = legacy_placeholder_source(count);
    let typed_source = typed_event_placeholder_source(count);
    group.bench_with_input(
      BenchmarkId::new("placeholder/legacy_scan", count),
      &source,
      |b, source| {
        b.iter(|| {
          std::hint::black_box(resolve_legacy_placeholders(std::hint::black_box(
            source.clone(),
          )))
        })
      },
    );
    group.bench_with_input(
      BenchmarkId::new("placeholder/typed_source_events", count),
      &typed_source,
      |b, source| {
        b.iter(|| {
          std::hint::black_box(resolve_typed_event_placeholders(std::hint::black_box(
            source.clone(),
          )))
        })
      },
    );
    group.bench_with_input(
      BenchmarkId::new("placeholder/typed", count),
      &count,
      |b, _| {
        b.iter_batched(
          || typed_placeholder_template(count),
          |(mut template, id)| {
            template.resolve_static(id, "resolved").unwrap();
            let source = template.freeze().unwrap();
            std::hint::black_box(source.source().into_string_lossy().into_owned())
          },
          criterion::BatchSize::SmallInput,
        )
      },
    );
  }

  let mut collision = TemplateRopeSource::new();
  collision.append_static(BENCHMARK_PLACEHOLDER);
  let id = collision.append_placeholder(PlaceholderKey::from_static("collision-check"));
  collision.resolve_static(id, "resolved").unwrap();
  assert_eq!(
    collision.freeze().unwrap().source().into_string_lossy(),
    format!("{BENCHMARK_PLACEHOLDER}resolved")
  );

  let mut unresolved = TemplateRopeSource::new();
  unresolved.append_placeholder(PlaceholderKey::from_static("unresolved"));
  assert!(unresolved.freeze().is_err());

  let mut conflict = TemplateRopeSource::new();
  let id = conflict.append_placeholder(PlaceholderKey::from_static("conflict"));
  conflict.resolve_static(id, "first").unwrap();
  conflict.resolve_static(id, "first").unwrap();
  assert!(conflict.resolve_static(id, "second").is_err());
}

fn bench_rspack_sources(criterion: &mut Criterion) {
  let mut group = criterion.benchmark_group("rspack_sources");

  group.bench_function(
    "sources@concat_generate_string_with_cache",
    benchmark_concat_generate_string_with_cache,
  );
  group.bench_function(
    "sources@concat_generate_string",
    benchmark_concat_generate_string,
  );

  group.bench_function("sources@cached_source_hash", benchmark_cached_source_hash);

  group.bench_function(
    "sources@concat_source_add_many",
    benchmark_concat_source_add_many,
  );
  group.bench_function(
    "sources@concat_source_add_few",
    benchmark_concat_source_add_few,
  );

  group.bench_function(
    "sources@complex_replace_source_map",
    benchmark_complex_replace_source_map,
  );

  group.bench_function(
    "sources@complex_replace_source_map_cached_source_stream_chunks",
    benchmark_complex_replace_source_map_cached_source_stream_chunks,
  );

  group.bench_function(
    "sources@complex_replace_source_source",
    benchmark_complex_replace_source_source,
  );

  group.bench_function(
    "sources@complex_replace_source_size",
    benchmark_complex_replace_source_size,
  );

  group.bench_function(
    "sources@parse_source_map_from_json",
    benchmark_parse_source_map_from_json,
  );

  group.bench_function("sources@source_map_to_json", benchmark_source_map_to_json);

  group.bench_function(
    "sources@repetitive_react_components_map",
    benchmark_repetitive_react_components_map,
  );

  group.bench_function(
    "sources@repetitive_react_components_source",
    benchmark_repetitive_react_components_source,
  );

  benchmark_replacement_build(&mut group);
  benchmark_rope_source(&mut group);
  benchmark_placeholder_resolution(&mut group);

  group.finish();
}

pub fn rspack_sources_benchmark(c: &mut Criterion) {
  bench_rspack_sources(c);
}
