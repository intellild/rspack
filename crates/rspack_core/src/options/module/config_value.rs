use rspack_napi::owned_value::{
  FromObject, FromValue, Value, ValueRef, false_or_string, from_str, stringified,
};

use super::*;

/// Binding-specific callback bridges used when decoding module options.
pub trait ModuleOptionsContext {
  fn filename(value: ValueRef<'_>) -> Option<Filename>;
  fn public_path(value: ValueRef<'_>) -> Option<PublicPath>;
  fn css_import(value: ValueRef<'_>) -> Option<CssParserImport>;
  fn json_parse(value: ValueRef<'_>) -> Option<ParseOption>;
  fn data_url(value: ValueRef<'_>) -> Option<AssetGeneratorDataUrl>;
}

pub(super) fn order(value: ValueRef<'_>) -> Option<JavascriptParserOrder> {
  stringified(value).map(|value| JavascriptParserOrder::from(value.as_str()))
}

pub(super) fn url(value: ValueRef<'_>) -> Option<JavascriptParserUrl> {
  stringified(value).map(|value| JavascriptParserUrl::from(value.as_str()))
}

pub(super) fn presence(value: ValueRef<'_>) -> Option<ExportPresenceMode> {
  false_or_string(value).map(|value| ExportPresenceMode::from(value.as_str()))
}

pub(super) fn encoding(value: ValueRef<'_>) -> Option<DataUrlEncoding> {
  false_or_string(value).map(Into::into)
}

pub(super) fn hash_salt(value: ValueRef<'_>) -> Option<HashSalt> {
  Option::<String>::from_value(value).map(Into::into)
}

pub(super) fn worker_url(value: ValueRef<'_>) -> Option<Option<JavascriptParserWorkerUrl>> {
  Option::<String>::from_value(value)
    .map(|value| value.and_then(|value| JavascriptParserWorkerUrl::from(value.as_str())))
}

pub(super) fn wrapped_context_reg_exp(value: ValueRef<'_>) -> Option<RspackRegex> {
  let fields = value.object()?;
  let (Value::String(source), Value::String(flags)) =
    (fields.get("source")?.value(), fields.get("flags")?.value())
  else {
    return None;
  };
  RspackRegex::with_flags(source, flags).ok()
}

pub(super) fn import_meta(value: ValueRef<'_>) -> Option<ImportMeta> {
  match value.value() {
    Value::Object(_) => Some(ImportMeta::Granular(ImportMetaOptions::new(
      value
        .object()?
        .iter()
        .filter_map(|(key, value)| bool::from_value(value).map(|value| (key.clone(), value)))
        .collect(),
    ))),
    Value::Bool(value) => Some(ImportMeta::from(value.to_string().as_str())),
    _ => from_str(value),
  }
}

pub(super) fn create_require(value: ValueRef<'_>) -> Option<JavascriptParserCreateRequire> {
  match value.value() {
    Value::Bool(true) => Some(JavascriptParserCreateRequire::Enabled(
      "createRequire from module".into(),
    )),
    Value::Bool(false) => Some(JavascriptParserCreateRequire::Disabled),
    _ => String::from_value(value).map(JavascriptParserCreateRequire::Enabled),
  }
}

pub(super) fn worker(value: ValueRef<'_>) -> Option<JavascriptParserWorkerOptions> {
  match value.value() {
    Value::Bool(flag) => Some(JavascriptParserWorkerOptions::new(
      if *flag {
        vec!["...".to_owned()]
      } else {
        vec![]
      },
      None,
    )),
    Value::Array(_) => {
      Vec::<String>::from_value(value).map(|alias| JavascriptParserWorkerOptions::new(alias, None))
    }
    _ => <JavascriptParserWorkerOptions as FromObject>::from_object(value.object()?).ok(),
  }
}

pub(super) fn commonjs_exports(
  value: ValueRef<'_>,
) -> Option<JavascriptParserCommonjsExportsOption> {
  match value.value() {
    Value::Null => Some(JavascriptParserCommonjsExportsOption::Enable),
    Value::Bool(flag) => Some(JavascriptParserCommonjsExportsOption::from(*flag)),
    Value::String(value) if value == "skipInEsm" => {
      Some(JavascriptParserCommonjsExportsOption::SkipInEsm)
    }
    _ => None,
  }
}

pub(super) fn commonjs(value: ValueRef<'_>) -> Option<JavascriptParserCommonjsOptions> {
  match value.value() {
    Value::Bool(flag) => Some(JavascriptParserCommonjsOptions {
      exports: JavascriptParserCommonjsExportsOption::from(*flag),
    }),
    _ => <JavascriptParserCommonjsOptions as FromObject>::from_object(value.object()?).ok(),
  }
}

pub(super) fn asset_parser_data_url(value: ValueRef<'_>) -> Option<AssetParserDataUrl> {
  <AssetParserDataUrlOptions as FromObject>::from_object(value.object()?)
    .ok()
    .map(AssetParserDataUrl::Options)
}
