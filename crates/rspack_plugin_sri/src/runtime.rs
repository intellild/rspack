use rspack_core::{
  ChunkUkey, Compilation, CompilationAdditionalTreeRuntimeRequirements, CrossOriginLoading,
  ManifestAssetType, RuntimeGlobals, RuntimeModule, RuntimeModuleExt, RuntimeModuleGenerateContext,
  RuntimeTemplate, SourceType,
  chunk_graph_chunk::ChunkId,
  impl_runtime_module,
  rspack_sources::{BoxSource, ConcatSource, RawStringSource, Source, SourceExt},
};
use rspack_error::{Result, error};
use rspack_hook::plugin_hook;
use rspack_plugin_runtime::{
  CreateLinkData, CreateScriptData, LinkPreloadData, RuntimePluginCreateLink,
  RuntimePluginCreateScript, RuntimePluginLinkPreload,
};
use rustc_hash::FxHashMap as HashMap;

use crate::{
  SubresourceIntegrityHashFunction, SubresourceIntegrityPlugin, SubresourceIntegrityPluginInner,
  util::{find_chunks, get_hash_variable, make_placeholder_source},
};

fn add_attribute(
  tag: &str,
  variable_ref: &str,
  code: &str,
  cross_origin_loading: &CrossOriginLoading,
) -> String {
  format!(
    r#"{code}
{tag}.integrity = {variable_ref}[chunkId];
{tag}.crossOrigin = '{cross_origin_loading}';"#
  )
}

#[impl_runtime_module]
#[derive(Debug)]
struct SRIHashVariableRuntimeModule {
  hash_funcs: Vec<SubresourceIntegrityHashFunction>,
}

impl SRIHashVariableRuntimeModule {
  pub fn new(
    runtime_template: &RuntimeTemplate,
    hash_funcs: Vec<SubresourceIntegrityHashFunction>,
  ) -> Self {
    Self::with_default(runtime_template, hash_funcs)
  }
  async fn render_source(&self, context: &RuntimeModuleGenerateContext<'_>) -> Result<BoxSource> {
    let compilation = context.compilation;
    let Some(chunk) = self
      .chunk()
      .as_ref()
      .and_then(|c| compilation.build_chunk_graph_artifact.chunk_by_ukey.get(c))
    else {
      return Err(error!(
        "Generate sri runtime module failed: chunk not found"
      ));
    };

    let include_chunks = chunk
      .get_all_async_chunks(&compilation.build_chunk_graph_artifact.chunk_group_by_ukey)
      .iter()
      .filter_map(|c| {
        let chunk = compilation
          .build_chunk_graph_artifact
          .chunk_by_ukey
          .get(c)?;
        let id = chunk.id()?;
        let rendered_hash = chunk.rendered_hash(
          &compilation.chunk_hashes_artifact,
          compilation.options.output.hash_digest_length,
        )?;
        Some((id, rendered_hash))
      })
      .collect::<HashMap<_, _>>();

    let module_graph = compilation.get_module_graph();

    let runtime_template = context.runtime_template;
    let runtime_require_name = runtime_template.render_runtime_globals(&RuntimeGlobals::REQUIRE);
    let source_types = vec![
      (
        SourceType::JavaScript,
        get_hash_variable(&runtime_require_name, SourceType::JavaScript),
      ),
      (
        SourceType::Css,
        get_hash_variable(&runtime_require_name, SourceType::Css),
      ),
      (
        SourceType::Custom("css/mini-extract".into()),
        get_hash_variable(
          &runtime_require_name,
          SourceType::Custom("css/mini-extract".into()),
        ),
      ),
    ];

    let all_chunks = find_chunks(&self.chunk().expect("should attached chunk"), compilation)
      .into_iter()
      .filter(|c| {
        compilation
          .build_chunk_graph_artifact
          .chunk_graph
          .get_chunk_modules(c, module_graph)
          .iter()
          .any(|m| {
            let result = compilation.code_generation_results.get_one(&m.identifier());
            result.inner.values().any(|v| v.size() != 0)
          })
      })
      .collect::<Vec<_>>();

    let mut code = ConcatSource::default();

    for (source_type, variable_ref) in source_types {
      let chunk_with_source_type = all_chunks
        .iter()
        .filter(|c| {
          compilation
            .build_chunk_graph_artifact
            .chunk_graph
            .has_chunk_module_by_source_type(c, source_type, module_graph)
        })
        .map(|c| {
          compilation
            .build_chunk_graph_artifact
            .chunk_by_ukey
            .expect_get(c)
            .expect_id()
        })
        .filter(|c| include_chunks.contains_key(c))
        .collect::<Vec<_>>();

      if !chunk_with_source_type.is_empty() {
        let asset_type = match source_type {
          SourceType::JavaScript => ManifestAssetType::JavaScript,
          SourceType::Css => ManifestAssetType::Css,
          SourceType::Custom(name) if name == "css/mini-extract" => {
            ManifestAssetType::Custom("extract-css".into())
          }
          _ => ManifestAssetType::Unknown,
        };
        code.add(RawStringSource::from(format!(
          "\n          {variable_ref} = "
        )));
        code.add(generate_sri_hash_placeholders(
          asset_type,
          chunk_with_source_type,
          &self.hash_funcs,
        ));
        code.add(RawStringSource::from_static(";\n          "));
      }
    }

    Ok(code.boxed())
  }
  fn get_runtime_requirements(
    &self,
    _compilation: &Compilation,
  ) -> rspack_core::RuntimeModuleRuntimeRequirements {
    rspack_core::RuntimeModuleRuntimeRequirements {
      dependencies: { RuntimeGlobals::REQUIRE_SCOPE },
      ..Default::default()
    }
  }
}

#[async_trait::async_trait]
impl RuntimeModule for SRIHashVariableRuntimeModule {
  fn runtime_module_variables() -> &'static [&'static str] {
    &[]
  }

  async fn generate(&self, context: &RuntimeModuleGenerateContext<'_>) -> Result<String> {
    Ok(
      self
        .render_source(context)
        .await?
        .source()
        .into_string_lossy()
        .into_owned(),
    )
  }

  async fn generate_source(&self, context: &RuntimeModuleGenerateContext<'_>) -> Result<BoxSource> {
    self.render_source(context).await
  }

  fn runtime_requirements(
    &self,
    compilation: &Compilation,
  ) -> rspack_core::RuntimeModuleRuntimeRequirements {
    self.get_runtime_requirements(compilation)
  }
}
fn generate_sri_hash_placeholders(
  asset_type: ManifestAssetType,
  chunks: Vec<&ChunkId>,
  hash_funcs: &Vec<SubresourceIntegrityHashFunction>,
) -> BoxSource {
  let mut source = ConcatSource::default();
  source.add(RawStringSource::from_static("{"));
  for (index, chunk) in chunks.into_iter().enumerate() {
    if index > 0 {
      source.add(RawStringSource::from_static(","));
    }
    source.add(RawStringSource::from(format!(
      "{}: ",
      rspack_util::json_stringify(chunk)
    )));
    source.add(make_placeholder_source(
      asset_type,
      hash_funcs,
      chunk.as_str(),
    ));
  }
  source.add(RawStringSource::from_static("}"));
  source.boxed()
}

#[plugin_hook(RuntimePluginCreateScript for SubresourceIntegrityPlugin)]
pub async fn create_script(&self, mut data: CreateScriptData) -> Result<CreateScriptData> {
  let ctx = SubresourceIntegrityPlugin::get_compilation_sri_context(data.chunk.compilation_id);
  data.code = add_attribute(
    "script",
    &get_hash_variable(&ctx.runtime_require_name, SourceType::JavaScript),
    &data.code,
    &ctx.cross_origin_loading,
  );
  Ok(data)
}

#[plugin_hook(RuntimePluginCreateLink for SubresourceIntegrityPlugin)]
pub async fn create_link<'a>(
  &self,
  compilation: &Compilation,
  mut data: CreateLinkData<'a>,
) -> Result<CreateLinkData<'a>> {
  let ctx = SubresourceIntegrityPlugin::get_compilation_sri_context(compilation.id());
  let runtime_template = compilation
    .runtime_template
    .create_runtime_module_code_template();
  let runtime_require_name = runtime_template.render_runtime_globals(&RuntimeGlobals::REQUIRE);
  if data.code.contains("loadingAttribute") {
    data.code = add_attribute(
      "link",
      &get_hash_variable(&runtime_require_name, SourceType::Css),
      &data.code,
      &ctx.cross_origin_loading,
    );
  } else {
    data.code = add_attribute(
      "linkTag",
      &get_hash_variable(
        &runtime_require_name,
        SourceType::Custom("css/mini-extract".into()),
      ),
      &data.code,
      &ctx.cross_origin_loading,
    );
  }

  Ok(data)
}

#[plugin_hook(RuntimePluginLinkPreload for SubresourceIntegrityPlugin)]
pub async fn link_preload<'a>(
  &self,
  compilation: &Compilation,
  mut data: LinkPreloadData<'a>,
) -> Result<LinkPreloadData<'a>> {
  let ctx = SubresourceIntegrityPlugin::get_compilation_sri_context(compilation.id());
  let runtime_template = compilation
    .runtime_template
    .create_runtime_module_code_template();
  let runtime_require_name = runtime_template.render_runtime_globals(&RuntimeGlobals::REQUIRE);
  if data.code.contains(".as = \"style\"") {
    data.code = add_attribute(
      "link",
      (if data.code.contains(".miniCssF") {
        get_hash_variable(
          &runtime_require_name,
          SourceType::Custom("css/mini-extract".into()),
        )
      } else {
        get_hash_variable(&runtime_require_name, SourceType::Css)
      })
      .as_str(),
      &data.code,
      &ctx.cross_origin_loading,
    );
  } else {
    data.code = add_attribute(
      "link",
      &get_hash_variable(&runtime_require_name, SourceType::JavaScript),
      &data.code,
      &ctx.cross_origin_loading,
    );
  }

  Ok(data)
}

#[plugin_hook(CompilationAdditionalTreeRuntimeRequirements for SubresourceIntegrityPlugin)]
pub async fn handle_runtime(
  &self,
  compilation: &Compilation,
  _chunk_ukey: &ChunkUkey,
  _runtime_requirements: &mut RuntimeGlobals,
  runtime_modules: &mut Vec<Box<dyn RuntimeModule>>,
) -> Result<()> {
  runtime_modules.push(
    SRIHashVariableRuntimeModule::new(
      &compilation.runtime_template,
      self.options.hash_func_names.clone(),
    )
    .boxed(),
  );
  Ok(())
}
