use std::sync::Arc;

use napi::{
  Either,
  bindgen_prelude::{Buffer, FnArgs},
};
use rspack_core::{
  AssetGeneratorDataUrl, AssetGeneratorDataUrlOptions, CssParserImport, Filename,
  ModuleOptionsContext, ParseOption, PublicPath,
};
use rspack_napi::owned_value::{FromObject, FromValue, Value, ValueRef};

use super::{RawAssetGeneratorDataUrlFnCtx, RawCssImportFn, convert_import_option};
use crate::{
  compiler_scoped_tsfn::CompilerScopedTsFnHandle as ThreadsafeFunction, filename::JsFilename,
};

pub(super) struct ModuleOptionsDecoder;

impl ModuleOptionsContext for ModuleOptionsDecoder {
  fn filename(value: ValueRef<'_>) -> Option<Filename> {
    JsFilename::from_value(value).map(Into::into)
  }
  fn public_path(value: ValueRef<'_>) -> Option<PublicPath> {
    JsFilename::from_value(value).map(Into::into)
  }
  fn css_import(value: ValueRef<'_>) -> Option<CssParserImport> {
    convert_import_option(Some(Either::<bool, RawCssImportFn>::from_value(value)?))
  }
  fn json_parse(value: ValueRef<'_>) -> Option<ParseOption> {
    if matches!(value.value(), Value::Null) {
      return Some(ParseOption::None);
    }
    let callback = ThreadsafeFunction::<String, String>::from_value(value)?;
    Some(ParseOption::Func(Arc::new(move |source: String| {
      let callback = callback.clone();
      Box::pin(async move { callback.call_with_sync(source).await })
    })))
  }
  fn data_url(value: ValueRef<'_>) -> Option<AssetGeneratorDataUrl> {
    match value.value() {
      Value::Function(callback) => {
        let callback =
        ThreadsafeFunction::<FnArgs<(Buffer, RawAssetGeneratorDataUrlFnCtx)>, String>::from_dynamic(
          callback.clone(),
        );
        Some(AssetGeneratorDataUrl::Func(Arc::new(move |source, ctx| {
          let callback = callback.clone();
          let source = source.into();
          let ctx = ctx.into();
          Box::pin(async move { callback.call_with_sync((source, ctx).into()).await })
        })))
      }
      _ => <AssetGeneratorDataUrlOptions as FromObject<Self>>::from_object(value.object()?)
        .ok()
        .map(AssetGeneratorDataUrl::Options),
    }
  }
}
