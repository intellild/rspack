mod config_value;
mod deferred;

use std::{
  fmt::Formatter,
  ptr::NonNull,
  sync::{Arc, LazyLock},
};

use deferred::{RawGeneratorOptionObject, RawParserOptionObject, RuleOptions};
pub use deferred::{RawGeneratorOptions, RawParserOptions};
use derive_more::Debug;
use futures::future::BoxFuture;
use napi::{
  Either,
  bindgen_prelude::{
    Either3, FromNapiValue, Object, ValidateNapiValue, from_raw_required_field,
    get_named_property_raw,
  },
  sys,
};
use napi_derive::napi;
use regex::Regex;
use rspack_core::{
  AssetGeneratorDataUrlFnCtx, CodeFilter, CssParserImport, CssParserImportContext, DescriptionData,
  FuncUseCtx, LoaderFilter, ModuleNoParseRule, ModuleNoParseRules, ModuleNoParseTestFn,
  ModuleOptions, ModuleRule, ModuleRuleEffect, ModuleRuleEnforce, ModuleRuleUse,
  ModuleRuleUseLoader,
};
use rspack_error::error;
use rspack_regex::RspackRegex;
use rustc_hash::FxHashMap as HashMap;

use crate::{
  compiler_scoped_tsfn::CompilerScopedTsFnHandle as ThreadsafeFunction, module::ModuleObject,
  options::raw_resolve::RawConfigValue,
};

/// `loader` is for both JS and Rust loaders.
/// `options` is
///   - a `None` on rust side and handled by js side `getOptions` when
/// using with `loader`.
///   - a `Some(string)` on rust side, deserialized by `serde_json::from_str`
/// and passed to rust side loader in [get_builtin_loader] when using with
/// `builtin_loader`.
#[napi(object)]
#[derive(Debug)]
pub struct RawModuleRuleUse {
  pub loader: String,
  pub options: Option<String>,
  pub cache: bool,
  pub options_cache_key: String,
  #[napi(ts_type = "string | RegExp")]
  pub code_filter: Option<Either<String, RspackRegex>>,
}

fn into_loader_filter(value: Either<String, RspackRegex>) -> Arc<LoaderFilter> {
  let code = match value {
    Either::A(value) => CodeFilter::String(value),
    Either::B(value) => CodeFilter::Regexp(value),
  };
  Arc::new(LoaderFilter::new(Some(code)))
}

#[rspack_napi_macros::tagged_union]
pub enum RawRuleSetCondition {
  string(String),
  #[napi(ts_type = "RegExp")]
  regexp(RspackRegex),
  logical(Vec<RawRuleSetLogicalConditions>),
  array(Vec<RawRuleSetCondition>),
  #[napi(ts_type = r#"(value: string) => boolean"#)]
  func(ThreadsafeFunction<serde_json::Value, bool>),
}

impl std::fmt::Debug for RawRuleSetCondition {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      RawRuleSetCondition::string(s) => write!(f, "RawRuleSetCondition::string({s:?})"),
      RawRuleSetCondition::regexp(r) => write!(f, "RawRuleSetCondition::regexp({r:?})"),
      RawRuleSetCondition::logical(l) => write!(f, "RawRuleSetCondition::logical({l:?})"),
      RawRuleSetCondition::array(a) => write!(f, "RawRuleSetCondition::array({a:?})"),
      RawRuleSetCondition::func(_) => write!(f, "RawRuleSetCondition::func(...)"),
    }
  }
}

#[derive(Debug, Default)]
#[napi(object, object_to_js = false)]
pub struct RawRuleSetLogicalConditions {
  pub and: Option<Vec<RawRuleSetCondition>>,
  pub or: Option<Vec<RawRuleSetCondition>>,
  pub not: Option<RawRuleSetCondition>,
}

impl TryFrom<RawRuleSetLogicalConditions> for rspack_core::RuleSetLogicalConditions {
  type Error = rspack_error::Error;

  fn try_from(value: RawRuleSetLogicalConditions) -> rspack_error::Result<Self> {
    let and = value
      .and
      .map(|i| {
        i.into_iter()
          .map(TryFrom::try_from)
          .collect::<rspack_error::Result<Vec<_>>>()
      })
      .transpose()?;
    let or = value
      .or
      .map(|i| {
        i.into_iter()
          .map(TryFrom::try_from)
          .collect::<rspack_error::Result<Vec<_>>>()
      })
      .transpose()?;
    let not = value.not.map(TryFrom::try_from).transpose()?;
    Ok(Self::new(and, or, not))
  }
}

impl TryFrom<RawRuleSetCondition> for rspack_core::RuleSetCondition {
  type Error = rspack_error::Error;

  fn try_from(x: RawRuleSetCondition) -> rspack_error::Result<Self> {
    let result = match x {
      RawRuleSetCondition::string(s) => Self::String(s),
      RawRuleSetCondition::regexp(r) => Self::Regexp(r),
      RawRuleSetCondition::logical(mut l) => {
        let l = l.get_mut(0).ok_or_else(|| {
          error!("TODO: use Box after https://github.com/napi-rs/napi-rs/issues/1500 landed")
        })?;
        let l = std::mem::take(l);
        Self::Logical(Box::new(rspack_core::RuleSetLogicalConditions::try_from(
          l,
        )?))
      }
      RawRuleSetCondition::array(a) => Self::array(
        a.into_iter()
          .map(|i| i.try_into())
          .collect::<rspack_error::Result<Vec<_>>>()?,
      ),
      RawRuleSetCondition::func(f) => Self::Func(Box::new(move |data| {
        let data = data.to_value();
        let f = f.clone();
        Box::pin(async move { f.call_with_sync(data).await })
      })),
    };

    Ok(result)
  }
}

impl TryFrom<RawRuleSetCondition> for rspack_core::RuleSetConditionWithEmpty {
  type Error = rspack_error::Error;

  fn try_from(x: RawRuleSetCondition) -> rspack_error::Result<Self> {
    Ok(Self::new(x.try_into()?))
  }
}

type ThreadsafeUse = ThreadsafeFunction<RawFuncUseCtx, Vec<RawModuleRuleUse>>;

#[derive(Debug, Default)]
#[napi(object, object_to_js = false)]
pub struct RawModuleRule {
  pub r#as: Option<String>,
  /// A conditional match matching an absolute path + query + fragment.
  /// Note:
  ///   This is a custom matching rule not initially designed by webpack.
  ///   Only for single-threaded environment interoperation purpose.
  pub rspack_resource: Option<RawRuleSetCondition>,
  /// A condition matcher matching an absolute path.
  pub test: Option<RawRuleSetCondition>,
  pub include: Option<RawRuleSetCondition>,
  pub exclude: Option<RawRuleSetCondition>,
  /// A condition matcher matching an absolute path.
  pub resource: Option<RawRuleSetCondition>,
  /// A condition matcher against the resource query.
  pub resource_query: Option<RawRuleSetCondition>,
  pub resource_fragment: Option<RawRuleSetCondition>,
  pub description_data: Option<HashMap<String, RawRuleSetCondition>>,
  pub with: Option<HashMap<String, RawRuleSetCondition>>,
  pub side_effects: Option<bool>,
  #[napi(ts_type = "RawModuleRuleUse[] | ((arg: RawFuncUseCtx) => RawModuleRuleUse[])")]
  pub r#use: Option<Either<Vec<RawModuleRuleUse>, ThreadsafeUse>>,
  pub r#type: Option<String>,
  pub layer: Option<String>,
  #[napi(ts_type = "RawConfigValue")]
  pub parser: Option<RawParserOptionObject>,
  #[napi(ts_type = "RawConfigValue")]
  pub generator: Option<RawGeneratorOptionObject>,
  #[napi(ts_type = "RawConfigValue")]
  pub resolve: Option<RawConfigValue>,
  pub issuer: Option<RawRuleSetCondition>,
  pub issuer_layer: Option<RawRuleSetCondition>,
  pub dependency: Option<RawRuleSetCondition>,
  pub phase: Option<RawRuleSetCondition>,
  pub scheme: Option<RawRuleSetCondition>,
  pub mimetype: Option<RawRuleSetCondition>,
  pub one_of: Option<Vec<RawModuleRule>>,
  pub rules: Option<Vec<RawModuleRule>>,
  /// Specifies the category of the loader. No value means normal loader.
  #[napi(ts_type = "'pre' | 'post'")]
  pub enforce: Option<String>,
  /// Whether to extract source maps from the module.
  pub extract_source_map: Option<bool>,
}

// Use tuple to pass arguments separately to JS function
#[derive(Debug)]
#[napi(object)]
pub struct RawCssImportContext {
  pub url: String,
  pub media: Option<String>,
  pub resource_path: String,
  pub supports: Option<String>,
  pub layer: Option<String>,
}

type RawCssImportFn = ThreadsafeFunction<RawCssImportContext, bool>;

fn convert_import_option(import: Option<Either<bool, RawCssImportFn>>) -> Option<CssParserImport> {
  import.map(|i| match i {
    Either::A(b) => CssParserImport::Bool(b),
    Either::B(f) => CssParserImport::Func(Arc::new(move |args: CssParserImportContext| {
      let f = f.clone();
      Box::pin(async move {
        f.call_with_sync(RawCssImportContext {
          url: args.url,
          media: args.media,
          resource_path: args.resource_path,
          supports: args.supports,
          layer: args.layer,
        })
        .await
      })
    })),
  })
}

#[napi(object)]
pub struct RawAssetGeneratorDataUrlFnCtx {
  pub filename: String,
  #[napi(ts_type = "Module")]
  pub module: ModuleObject,
}

impl From<AssetGeneratorDataUrlFnCtx<'_>> for RawAssetGeneratorDataUrlFnCtx {
  fn from(value: AssetGeneratorDataUrlFnCtx) -> Self {
    // AssetGeneratorDataUrlFn may be called during the importModule process,
    // at which point the corresponding Module is not present in compilation.moduleGraph.
    // Therefore, we use a raw pointer to allow JavaScript to access the Module.
    #[allow(clippy::unwrap_used)]
    Self {
      filename: value.filename,
      module: ModuleObject::with_ptr(
        NonNull::new(
          value.module as *const dyn rspack_core::Module as *mut dyn rspack_core::Module,
        )
        .unwrap(),
        value.compilation.compiler_id(),
      ),
    }
  }
}

#[napi]
pub fn get_config_value_fields() -> HashMap<String, Vec<String>> {
  deferred::config_value_fields()
}

#[napi(object, object_from_js = false, object_to_js = false)]
#[derive(Debug)]
pub struct RawModuleOptions {
  pub rules: Vec<RawModuleRule>,
  #[napi(ts_type = "Record<string, RawConfigValue>")]
  pub parser: Option<HashMap<String, RawParserOptions>>,
  #[napi(ts_type = "Record<string, RawConfigValue>")]
  pub generator: Option<HashMap<String, RawGeneratorOptions>>,
  #[napi(
    ts_type = "string | RegExp | ((request: string) => boolean) | (string | RegExp | ((request: string) => boolean))[]"
  )]
  #[debug(skip)]
  pub no_parse: Option<RawModuleNoParseRules>,
}

impl FromNapiValue for RawModuleOptions {
  unsafe fn from_napi_value(env: sys::napi_env, value: sys::napi_value) -> napi::Result<Self> {
    let object = unsafe { Object::from_napi_value(env, value)? };
    let rules = unsafe {
      from_raw_required_field(
        env,
        get_named_property_raw(env, value, c"rules".as_ptr())?,
        "RawModuleOptions",
        "rules",
      )?
    };
    Ok(Self {
      rules,
      parser: object
        .get::<Object>("parser")?
        .map(RawParserOptionObject::capture_module_options)
        .transpose()?,
      generator: object
        .get::<Object>("generator")?
        .map(RawGeneratorOptionObject::capture_module_options)
        .transpose()?,
      no_parse: object.get("noParse")?,
    })
  }
}

impl ValidateNapiValue for RawModuleOptions {}

#[derive(Debug, Clone)]
#[napi(object)]
pub struct RawFuncUseCtx {
  pub resource: Option<String>,
  pub real_resource: Option<String>,
  pub resource_query: String,
  pub resource_fragment: String,
  pub issuer: String,
  pub issuer_layer: String,
}

impl From<FuncUseCtx> for RawFuncUseCtx {
  fn from(value: FuncUseCtx) -> Self {
    Self {
      resource: value.resource,
      real_resource: value.real_resource,
      resource_query: value.resource_query.unwrap_or_default(),
      resource_fragment: value.resource_fragment.unwrap_or_default(),
      issuer: value.issuer.map(|s| s.to_string()).unwrap_or_default(),
      issuer_layer: value.issuer_layer.unwrap_or_default(),
    }
  }
}

impl TryFrom<RawModuleRule> for ModuleRule {
  type Error = rspack_error::Error;

  fn try_from(value: RawModuleRule) -> rspack_error::Result<Self> {
    let uses = value.r#use.map(|raw| match raw {
      Either::A(array) => {
        let uses = array
          .into_iter()
          .map(|rule_use| ModuleRuleUseLoader {
            loader: rule_use.loader,
            options: rule_use.options,
            cache: rule_use.cache,
            options_cache_key: rule_use.options_cache_key,
            filter: rule_use.code_filter.map(into_loader_filter),
          })
          .collect::<Vec<_>>();
        Ok::<ModuleRuleUse, rspack_error::Error>(ModuleRuleUse::Array(uses))
      }
      Either::B(tsfn) => Ok::<ModuleRuleUse, rspack_error::Error>(ModuleRuleUse::Func(Box::new(
        move |ctx: FuncUseCtx| {
          let tsfn = tsfn.clone();
          Box::pin(async move {
            tsfn.call_with_sync(ctx.into()).await.map(|uses| {
              uses
                .into_iter()
                .map(|rule_use| ModuleRuleUseLoader {
                  loader: rule_use.loader,
                  options: rule_use.options,
                  cache: rule_use.cache,
                  options_cache_key: rule_use.options_cache_key,
                  filter: rule_use.code_filter.map(into_loader_filter),
                })
                .collect::<Vec<_>>()
            })
          })
        },
      ))),
    });

    let module_type = value.r#type.map(|t| (&*t).into());

    let one_of = value
      .one_of
      .map(|one_of| {
        one_of
          .into_iter()
          .map(|raw| raw.try_into())
          .collect::<rspack_error::Result<Vec<_>>>()
      })
      .transpose()?;

    let rules = value
      .rules
      .map(|rule| {
        rule
          .into_iter()
          .map(|raw| raw.try_into())
          .collect::<rspack_error::Result<Vec<_>>>()
      })
      .transpose()?;

    let description_data = value
      .description_data
      .map(|data| {
        data
          .into_iter()
          .map(|(k, v)| Ok((k, v.try_into()?)))
          .collect::<rspack_error::Result<DescriptionData>>()
      })
      .transpose()?;

    let with = value
      .with
      .map(|data| {
        data
          .into_iter()
          .map(|(k, v)| Ok((k, v.try_into()?)))
          .collect::<rspack_error::Result<DescriptionData>>()
      })
      .transpose()?;

    let enforce = value
      .enforce
      .map(|enforce| match &*enforce {
        "pre" => Ok(ModuleRuleEnforce::Pre),
        "post" => Ok(ModuleRuleEnforce::Post),
        _ => Err(error!(
          "Unsupported Rule.enforce type, supported: 'pre' | 'post' | undefined"
        )),
      })
      .transpose()?
      .unwrap_or_default();

    Ok(ModuleRule {
      r#as: value.r#as.map(Into::into),
      rspack_resource: value
        .rspack_resource
        .map(|raw| raw.try_into())
        .transpose()?,
      test: value.test.map(|raw| raw.try_into()).transpose()?,
      include: value.include.map(|raw| raw.try_into()).transpose()?,
      exclude: value.exclude.map(|raw| raw.try_into()).transpose()?,
      resource_query: value.resource_query.map(|raw| raw.try_into()).transpose()?,
      resource_fragment: value
        .resource_fragment
        .map(|raw| raw.try_into())
        .transpose()?,
      resource: value.resource.map(|raw| raw.try_into()).transpose()?,
      description_data,
      with,
      issuer: value.issuer.map(|raw| raw.try_into()).transpose()?,
      issuer_layer: value.issuer_layer.map(|raw| raw.try_into()).transpose()?,
      dependency: value.dependency.map(|raw| raw.try_into()).transpose()?,
      phase: value.phase.map(|raw| raw.try_into()).transpose()?,
      scheme: value.scheme.map(|raw| raw.try_into()).transpose()?,
      mimetype: value.mimetype.map(|raw| raw.try_into()).transpose()?,
      one_of,
      rules,
      effect: ModuleRuleEffect {
        r#use: uses.transpose()?.unwrap_or_default(),
        r#type: module_type,
        layer: value.layer,
        deferred_options: RuleOptions::deferred(value.parser, value.generator),
        resolve: value
          .resolve
          .map(|raw| {
            raw.try_into().map_err(|error: rspack_error::Error| {
              rspack_error::error!("Invalid rule.resolve: {error}")
            })
          })
          .transpose()?,
        side_effects: value.side_effects,
        enforce,
        extract_source_map: value.extract_source_map,
        ..Default::default()
      },
      extract_source_map: value.extract_source_map,
    })
  }
}

impl TryFrom<RawModuleOptions> for ModuleOptions {
  type Error = rspack_error::Error;

  fn try_from(value: RawModuleOptions) -> rspack_error::Result<Self> {
    let rules = value
      .rules
      .into_iter()
      .map(|rule| rule.try_into())
      .collect::<rspack_error::Result<Vec<ModuleRule>>>()?;

    let mut module_options = ModuleOptions {
      rules,
      parser: value
        .parser
        .map(RawParserOptionObject::into_module_options)
        .transpose()?,
      generator: value
        .generator
        .map(RawGeneratorOptionObject::into_module_options)
        .transpose()?,
      no_parse: value
        .no_parse
        .map(|x| RawModuleNoParseRulesWrapper(x).into()),
    };
    module_options.assign_rule_ids()?;
    Ok(module_options)
  }
}

type RawModuleNoParseRule = Either3<String, RspackRegex, ThreadsafeFunction<String, Option<bool>>>;
type RawModuleNoParseRules = Either<RawModuleNoParseRule, Vec<RawModuleNoParseRule>>;

struct RawModuleNoParseRuleWrapper(RawModuleNoParseRule);

struct RawModuleNoParseRulesWrapper(RawModuleNoParseRules);

fn js_func_to_no_parse_test_func(
  v: ThreadsafeFunction<String, Option<bool>>,
) -> ModuleNoParseTestFn {
  Box::new(move |s| {
    let v = v.clone();
    Box::pin(async move { v.call_with_sync(s).await.map(|v| v.unwrap_or_default()) })
  })
}

impl From<RawModuleNoParseRuleWrapper> for ModuleNoParseRule {
  fn from(x: RawModuleNoParseRuleWrapper) -> Self {
    match x.0 {
      Either3::A(v) => Self::AbsPathPrefix(v),
      Either3::B(v) => Self::Regexp(v),
      Either3::C(v) => Self::TestFn(js_func_to_no_parse_test_func(v)),
    }
  }
}

impl From<RawModuleNoParseRulesWrapper> for ModuleNoParseRules {
  fn from(x: RawModuleNoParseRulesWrapper) -> Self {
    match x.0 {
      Either::A(v) => Self::Rule(RawModuleNoParseRuleWrapper(v).into()),
      Either::B(v) => Self::Rules(
        v.into_iter()
          .map(|r| RawModuleNoParseRuleWrapper(r).into())
          .collect(),
      ),
    }
  }
}
