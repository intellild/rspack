use std::sync::Arc;

use rspack_core::{
  AssetInfo, ChunkCodeTemplate, ChunkInitFragments, ChunkUkey, CodeGenerationDataFilename,
  Compilation, CompilationParams, CompilerCompilation, DependencyId, ImportMeta,
  JavascriptParserUrl, ManifestAssetType, Module, ModuleType, NormalModuleFactoryParser,
  ParserAndGenerator, ParserOptions, PathData, Plugin, SourceType, URLStaticMode,
  get_css_chunk_filename_template, rspack_sources::ReplaceSource,
};
use rspack_error::Result;
use rspack_hook::{plugin, plugin_hook};

use crate::{
  JavascriptModulesRenderModuleContent, JsPlugin, RenderSource,
  dependency::{URL_STATIC_PLACEHOLDER, URL_STATIC_PLACEHOLDER_RE},
  parser_and_generator::JavaScriptParserAndGenerator,
};

#[plugin]
#[derive(Debug, Default)]
pub struct URLPlugin {}

#[plugin_hook(CompilerCompilation for URLPlugin)]
async fn compilation(
  &self,
  compilation: &mut Compilation,
  _params: &mut CompilationParams,
) -> Result<()> {
  let hooks = JsPlugin::get_compilation_hooks_mut(compilation.id());
  hooks
    .write()
    .await
    .render_module_content
    .tap(render_module_content::new(self));
  Ok(())
}
#[plugin_hook(NormalModuleFactoryParser for URLPlugin)]
async fn normal_module_factory_parser(
  &self,
  _module_type: &ModuleType,
  parser: &mut Box<dyn ParserAndGenerator>,
  parser_options: Option<&ParserOptions>,
) -> Result<()> {
  if let Some(parser) = parser.downcast_mut::<JavaScriptParserAndGenerator>() {
    let options = parser_options
      .and_then(|p| p.get_javascript())
      .expect("should at least have a global javascript parser options");

    if !matches!(options.url, Some(JavascriptParserUrl::Disable)) {
      let mode = if matches!(options.import_meta, Some(ImportMeta::Disabled))
        && matches!(options.url, None | Some(JavascriptParserUrl::Enable))
      {
        Some(JavascriptParserUrl::NewUrlRelative)
      } else {
        options.url
      };

      parser.add_parser_plugin(Box::new(crate::parser_plugin::URLPlugin { mode }));
    }
  }

  Ok(())
}

#[plugin_hook(JavascriptModulesRenderModuleContent for URLPlugin,tracing=false)]
async fn render_module_content(
  &self,
  compilation: &Compilation,
  chunk_ukey: &ChunkUkey,
  module: &dyn Module,
  render_source: &mut RenderSource,
  _init_fragments: &mut ChunkInitFragments,
  _runtime_template: &ChunkCodeTemplate,
) -> Result<()> {
  let runtime = compilation
    .build_chunk_graph_artifact
    .chunk_by_ukey
    .expect_get(chunk_ukey)
    .runtime();
  let module_graph = compilation.get_module_graph();
  let codegen_result = compilation
    .code_generation_results
    .get(&module.identifier(), Some(runtime));
  if codegen_result.data.contains::<URLStaticMode>() {
    let content = render_source.source.source().into_string_lossy();
    let mut replace_source = ReplaceSource::new(render_source.source.clone());
    let replacement = URL_STATIC_PLACEHOLDER_RE
      .find_iter(&content)
      .map(|cap| (cap.start(), cap.end()));

    for (start, end) in replacement {
      let dep_id = &content[start + URL_STATIC_PLACEHOLDER.len()..end];
      let dep_id: DependencyId = dep_id
        .parse::<u32>()
        .unwrap_or_else(|_| panic!("should be valid dependency id \"{dep_id}\""))
        .into();
      let Some(module) = module_graph.module_identifier_by_dependency_id(&dep_id) else {
        continue;
      };
      let codegen_result = compilation
        .code_generation_results
        .get(module, Some(runtime));
      let filename = if let Some(filename) = codegen_result.data.get::<CodeGenerationDataFilename>()
      {
        filename.filename().to_string()
      } else {
        let module = module_graph
          .module_by_identifier(module)
          .expect("module should exist");
        if module.source_types(module_graph).contains(&SourceType::Css)
          || matches!(
            module.module_type(),
            ModuleType::Css | ModuleType::CssAuto | ModuleType::CssModule | ModuleType::CssGlobal
          )
        {
          let chunk = compilation
            .build_chunk_graph_artifact
            .chunk_by_ukey
            .expect_get(chunk_ukey);
          let filename_template = get_css_chunk_filename_template(
            chunk,
            &compilation.options.output,
            &compilation.build_chunk_graph_artifact.chunk_group_by_ukey,
          );
          let mut asset_info = AssetInfo::default().with_asset_type(ManifestAssetType::Css);
          compilation
            .get_path_with_info(
              filename_template,
              PathData::default()
                .chunk_id_optional(chunk.id().map(|id| id.as_str()))
                .chunk_hash_optional(chunk.rendered_hash(
                  &compilation.chunk_hashes_artifact,
                  compilation.options.output.hash_digest_length,
                ))
                .chunk_name_optional(chunk.name_for_filename_template())
                .content_hash_optional(chunk.rendered_content_hash_by_source_type(
                  &compilation.chunk_hashes_artifact,
                  &SourceType::Css,
                  compilation.options.output.hash_digest_length,
                ))
                .runtime(chunk.runtime().as_str()),
              &mut asset_info,
            )
            .await?
        } else {
          unreachable!()
        }
      };

      replace_source.replace(start as u32, end as u32, filename, None);
    }

    render_source.source = Arc::new(replace_source);
  }
  Ok(())
}

impl Plugin for URLPlugin {
  fn name(&self) -> &'static str {
    "rspack.URLPlugin"
  }

  fn apply(&self, ctx: &mut rspack_core::ApplyContext<'_>) -> Result<()> {
    ctx.compiler_hooks.compilation.tap(compilation::new(self));
    ctx
      .normal_module_factory_hooks
      .parser
      .tap(normal_module_factory_parser::new(self));
    Ok(())
  }
}
