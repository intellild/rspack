mod bridge;
mod context;
mod resolver;
mod scheduler;
pub(crate) mod worker;

use std::fmt::Debug;

pub use context::{JsLoaderContext, JsLoaderDependencies, JsLoaderItem};
use rspack_core::{Compilation, CompilationParams, CompilerThisCompilation, Plugin};
use rspack_error::Result;
use rspack_hook::{plugin, plugin_hook};
use rustc_hash::FxHashSet;
use tokio::sync::RwLock;

#[plugin]
pub(crate) struct JsLoaderRspackPlugin {
  pub(crate) main_object_handle: u32,
  pub(crate) resolved_loaders: resolver::LoaderResolveCache,
  pub(crate) loaders_without_pitch: RwLock<FxHashSet<String>>,
}

impl JsLoaderRspackPlugin {
  pub fn new(main_object_handle: u32) -> Self {
    Self::new_inner(
      main_object_handle,
      Default::default(),
      RwLock::new(FxHashSet::default()),
    )
  }
}

impl Debug for JsLoaderRspackPlugin {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_tuple("LoaderResolver").finish()
  }
}

#[plugin_hook(CompilerThisCompilation for JsLoaderRspackPlugin)]
async fn this_compilation(
  &self,
  _compilation: &mut Compilation,
  _params: &mut CompilationParams,
) -> Result<()> {
  self
    .resolved_loaders
    .lock()
    .expect("loader resolution cache lock poisoned")
    .clear();
  Ok(())
}

impl Plugin for JsLoaderRspackPlugin {
  fn name(&self) -> &'static str {
    "rspack.JsLoaderRspackPlugin"
  }

  fn apply(&self, ctx: &mut rspack_core::ApplyContext<'_>) -> Result<()> {
    ctx
      .compiler_hooks
      .this_compilation
      .tap(this_compilation::new(self));

    ctx
      .normal_module_factory_hooks
      .resolve_loader
      .tap(resolver::resolve_loader::new(self));

    ctx
      .normal_module_hooks
      .loader_yield
      .tap(scheduler::loader_yield::new(self));

    Ok(())
  }
}
