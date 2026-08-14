use rspack_cacheable::{cacheable, cacheable_dyn};
use rspack_core::{
  AsContextDependency, AsModuleDependency, ConditionalInitFragment, DependencyCodeGeneration,
  DependencyRange, DependencyTemplate, DependencyTemplateType, DependencyType, InitFragmentExt,
  InitFragmentKey, InitFragmentStage, RuntimeCondition, RuntimeGlobals, SourceInitFragment,
  TemplateContext, TemplateReplaceSource,
  rspack_sources::{ConcatSource, PlaceholderKey, PlaceholderSource, RawStringSource, SourceExt},
};
use rspack_util::json_stringify_str;
const RSTEST_HOIST_PLACEHOLDER_KEY_PREFIX: &str = "rspack:rstest:hoist:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RstestHoistPlaceholderKind {
  Start,
  End,
  Target,
}

impl RstestHoistPlaceholderKind {
  fn as_str(self) -> &'static str {
    match self {
      Self::Start => "start",
      Self::End => "end",
      Self::Target => "target",
    }
  }

  fn from_str(value: &str) -> Option<Self> {
    match value {
      "start" => Some(Self::Start),
      "end" => Some(Self::End),
      "target" => Some(Self::Target),
      _ => None,
    }
  }

  fn fallback_name(self) -> &'static str {
    match self {
      Self::Start => "HOIST_START",
      Self::End => "HOIST_END",
      Self::Target => "PLACEHOLDER",
    }
  }
}

pub(crate) struct RstestHoistPlaceholder<'a> {
  pub kind: RstestHoistPlaceholderKind,
  pub hoist_id: &'a str,
  pub request: &'a str,
}

struct RstestHoistIdentity<'a> {
  hoist_id: &'a str,
  request: &'a str,
}
fn hoist_placeholder_source(
  flag: &str,
  hoist_id: &str,
  request: &str,
  kind: RstestHoistPlaceholderKind,
) -> PlaceholderSource {
  PlaceholderSource::new(
    PlaceholderKey::new(format!(
      "{RSTEST_HOIST_PLACEHOLDER_KEY_PREFIX}{}:{hoist_id}:{request}",
      kind.as_str()
    )),
    format!(
      "/* RSTEST:{flag}:{hoist_id}:{request}:{} */{}",
      kind.fallback_name(),
      if kind == RstestHoistPlaceholderKind::Target {
        ";"
      } else {
        ""
      }
    ),
  )
}
fn hoist_end_source(flag: &str, hoist_id: &str, request: &str) -> ConcatSource {
  let mut source = ConcatSource::default();
  source.add(RawStringSource::from_static("\n"));
  source.add(hoist_placeholder_source(
    flag,
    hoist_id,
    request,
    RstestHoistPlaceholderKind::End,
  ));
  source
}

pub(crate) fn parse_hoist_placeholder(key: &PlaceholderKey) -> Option<RstestHoistPlaceholder<'_>> {
  let rest = key
    .as_str()
    .strip_prefix(RSTEST_HOIST_PLACEHOLDER_KEY_PREFIX)?;
  let (kind, rest) = rest.split_once(':')?;
  let (hoist_id, request) = rest.split_once(':')?;
  Some(RstestHoistPlaceholder {
    kind: RstestHoistPlaceholderKind::from_str(kind)?,
    hoist_id,
    request,
  })
}

#[cacheable]
#[derive(Debug, Clone)]
pub struct MockMethodDependency {
  call_expr_range: DependencyRange,
  callee_range: DependencyRange,
  // Intentionally stored as `DependencyRange` so hoist insertion positions
  // remain cacheable and survive persistent cache restore.
  statement_range: Option<DependencyRange>,
  request: String,
  hoist: bool,
  method: MockMethod,
  /// Byte offset (end of the last call argument's expression, before any
  /// trailing comma) at which to inject the clean `request` literal as the
  /// trailing argument of the emitted `rstest_*` call, e.g. turning
  /// `rstest_mock(id, factory)` into `rstest_mock(id, factory, "request")`.
  /// `None` skips injection — used for `rs.hoisted` (no request) and the 1-arg
  /// auto-mock form (whose request is carried by the synthetic-target
  /// dependency's suffix instead, to avoid colliding at the same offset).
  args_request_end: Option<u32>,
  /// Source order of the ESM import that provides the `rs` or `rstest`
  /// binding. The import must run before hoisted callbacks that use other API
  /// members such as `rs.fn()`.
  test_api_import_source_order: Option<i32>,
}

#[cacheable]
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MockMethod {
  Mock,
  DoMock,
  MockRequire,
  DoMockRequire,
  Unmock,
  DoUnmock,
  Hoisted,
}

impl MockMethodDependency {
  pub fn new(
    call_expr_range: DependencyRange,
    callee_range: DependencyRange,
    request: String,
    hoist: bool,
    method: MockMethod,
  ) -> Self {
    Self {
      call_expr_range,
      callee_range,
      statement_range: None,
      request,
      hoist,
      method,
      args_request_end: None,
      test_api_import_source_order: None,
    }
  }

  pub fn new_with_statement_range(
    call_expr_range: DependencyRange,
    callee_range: DependencyRange,
    statement_range: DependencyRange,
    request: String,
    hoist: bool,
    method: MockMethod,
  ) -> Self {
    Self {
      call_expr_range,
      callee_range,
      statement_range: Some(statement_range),
      request,
      hoist,
      method,
      args_request_end: None,
      test_api_import_source_order: None,
    }
  }

  /// Set the request-injection offset. See [`Self::args_request_end`].
  pub fn with_request_arg_end(mut self, end: Option<u32>) -> Self {
    self.args_request_end = end;
    self
  }

  pub fn with_test_api_import_source_order(mut self, source_order: Option<i32>) -> Self {
    self.test_api_import_source_order = source_order;
    self
  }
}

#[cacheable_dyn]
impl DependencyCodeGeneration for MockMethodDependency {
  fn dependency_template(&self) -> Option<DependencyTemplateType> {
    Some(MockMethodDependencyTemplate::template_type())
  }
}

impl AsModuleDependency for MockMethodDependency {}
impl AsContextDependency for MockMethodDependency {}

#[cacheable]
#[derive(Debug, Clone, Default)]
pub struct MockMethodDependencyTemplate;

impl MockMethodDependencyTemplate {
  pub fn template_type() -> DependencyTemplateType {
    DependencyTemplateType::Dependency(DependencyType::RstestHoistMock)
  }
}

impl DependencyTemplate for MockMethodDependencyTemplate {
  fn render(
    &self,
    dep: &dyn DependencyCodeGeneration,
    source: &mut TemplateReplaceSource,
    code_generatable_context: &mut TemplateContext,
  ) {
    let TemplateContext {
      init_fragments,
      runtime_template,
      ..
    } = code_generatable_context;
    let dep = dep
      .as_any()
      .downcast_ref::<MockMethodDependency>()
      .expect("MockMethodDependencyTemplate can only be applied to MockMethodDependency");

    let request = &dep.request;
    let require_name = runtime_template.render_runtime_globals(&RuntimeGlobals::REQUIRE);
    let hoist_id = dep.hoist_id();

    let hoist_flag = Self::get_hoist_flag(&dep.method);
    let mock_method = Self::get_mock_method(&dep.method);

    if dep.hoist
      && let Some(flag) = hoist_flag
    {
      // Step 1: Add placeholder init fragment for hoistable methods
      Self::add_placeholder_fragment(init_fragments, flag, &hoist_id, request);

      // Step 2: Hoist the import that provides the test API before all hoisted code
      Self::hoist_test_api_import(init_fragments, dep.test_api_import_source_order);
    }

    // Step 3: Transform the source code
    Self::transform_source(
      source,
      dep,
      &require_name,
      mock_method,
      hoist_flag,
      &hoist_id,
      request,
    );

    // Inject the request as the call's trailing arg (before any trailing comma,
    // valid for `rs.mock('x', f,)`) so a dynamic `import(request)` resolves to the
    // mock by request. See `args_request_end` for the `None` cases.
    if let Some(end) = dep.args_request_end {
      source.replace(end, end, format!(", {}", json_stringify_str(request)), None);
    }
  }
}

impl MockMethodDependency {
  fn hoist_id(&self) -> String {
    format!(
      "{}-{}",
      self.call_expr_range.start, self.call_expr_range.end
    )
  }
}

impl MockMethodDependencyTemplate {
  /// Get the hoist flag string for methods that need hoisting
  fn get_hoist_flag(method: &MockMethod) -> Option<&'static str> {
    match method {
      MockMethod::Mock => Some("MOCK"),
      MockMethod::MockRequire => Some("MOCKREQUIRE"),
      MockMethod::Unmock => Some("UNMOCK"),
      MockMethod::Hoisted => Some("HOISTED"),
      MockMethod::DoMock | MockMethod::DoMockRequire | MockMethod::DoUnmock => None,
    }
  }

  /// Get the runtime method name
  fn get_mock_method(method: &MockMethod) -> &'static str {
    match method {
      MockMethod::Mock => "rstest_mock",
      MockMethod::DoMock => "rstest_do_mock",
      MockMethod::MockRequire => "rstest_mock_require",
      MockMethod::DoMockRequire => "rstest_do_mock_require",
      MockMethod::Unmock => "rstest_unmock",
      MockMethod::Hoisted => "rstest_hoisted",
      MockMethod::DoUnmock => "rstest_do_unmock",
    }
  }

  /// Add a placeholder init fragment that marks where hoisted code should be inserted.
  ///
  /// `StageESMImports` ordering contract (position → fragment):
  /// - `-3`: the hoisted test API import (see [`Self::hoist_test_api_import`])
  /// - `-2`: this placeholder — mock registrations run before any user module is
  ///   evaluated, so an `importActual` subgraph still sees mocked transitive deps
  /// - `-1`: `importActual` imports (see `esm_import_dependency.rs`) — evaluated
  ///   after mocks are registered (registration does not run mock factories) but
  ///   before normal imports, so factories that spread an `importActual` binding
  ///   observe an initialized value once they lazily run
  /// - `>= 1`: normal ESM imports (`source_order` starts at 1)
  fn add_placeholder_fragment(
    init_fragments: &mut Vec<Box<dyn rspack_core::InitFragment<rspack_core::GenerateContext<'_>>>>,
    flag: &str,
    hoist_id: &str,
    request: &str,
  ) {
    let init = SourceInitFragment::new(
      hoist_placeholder_source(flag, hoist_id, request, RstestHoistPlaceholderKind::Target).boxed(),
      InitFragmentStage::StageESMImports,
      -2,
      InitFragmentKey::Const(format!("rstest mock_hoist {hoist_id}")),
      None,
    );
    init_fragments.push(init.boxed());
  }

  /// Hoist the ESM import that provides the test API to the very top of the module.
  ///
  /// This ensures that `rs.fn()` and other utilities are available inside
  /// `rs.hoisted()` callbacks regardless of which module re-exports the API.
  ///
  /// We achieve this by inserting a higher-priority fragment with the same key.
  /// Since ESMImport's merge logic returns the first fragment when its runtime_condition is true,
  /// our new fragment will take precedence and the original will be ignored.
  fn hoist_test_api_import(
    init_fragments: &mut Vec<Box<dyn rspack_core::InitFragment<rspack_core::GenerateContext<'_>>>>,
    source_order: Option<i32>,
  ) {
    let Some(source_order) = source_order else {
      return;
    };

    let Some(fragment) = init_fragments.iter().find(|fragment| {
      fragment.stage() == InitFragmentStage::StageESMImports
        && fragment.position() == source_order
        && matches!(fragment.key(), InitFragmentKey::ESMImport(_))
    }) else {
      return;
    };
    let target_key = fragment.key().clone();

    // Clone and downcast to get the content
    let cloned: Box<dyn rspack_core::InitFragment<_>> = fragment.clone();
    let Ok(conditional_fragment) = cloned.into_any().downcast::<ConditionalInitFragment>() else {
      return;
    };

    // Create a new fragment with higher priority (position=-3, before the mock
    // placeholder at -2 and importActual imports at -1) and insert at the beginning
    let content = conditional_fragment.content().to_string();
    let test_api_import = ConditionalInitFragment::new(
      content,
      InitFragmentStage::StageESMImports,
      -3,
      target_key,
      None,
      RuntimeCondition::Boolean(true),
    );
    init_fragments.insert(0, test_api_import.boxed());
  }

  /// Transform the source code by:
  /// 1. Adding hoist markers (HOIST_START/HOIST_END) around the code to be hoisted
  /// 2. Replacing the original callee with the runtime method
  fn transform_source(
    source: &mut TemplateReplaceSource,
    dep: &MockMethodDependency,
    require_name: &str,
    mock_method: &str,
    hoist_flag: Option<&str>,
    hoist_id: &str,
    request: &str,
  ) {
    let Some(flag) = hoist_flag.filter(|_| dep.hoist) else {
      // No hoisting needed (e.g., `rs.doMock(...)`).
      Self::transform_without_hoist(source, require_name, mock_method, &dep.callee_range);
      return;
    };

    let hoist = RstestHoistIdentity { hoist_id, request };

    if dep.statement_range.is_some() {
      // Variable declaration with hoisting (e.g., `const mocks = rs.hoisted(...)`).
      Self::transform_with_statement_hoist(
        source,
        dep,
        require_name,
        mock_method,
        flag,
        &hoist,
        &dep.callee_range,
      );
    } else {
      // Standalone call with hoisting (e.g., `rs.mock(...)`).
      Self::transform_with_call_hoist(
        source,
        dep,
        require_name,
        mock_method,
        flag,
        &hoist,
        &dep.callee_range,
      );
    }
  }

  /// Transform for variable declarations that need hoisting.
  /// Example: `const mocks = rs.hoisted(() => {...})`
  fn transform_with_statement_hoist(
    source: &mut TemplateReplaceSource,
    dep: &MockMethodDependency,
    require_name: &str,
    mock_method: &str,
    flag: &str,
    hoist: &RstestHoistIdentity<'_>,
    callee_range: &DependencyRange,
  ) {
    let stmt_range = dep
      .statement_range
      .expect("statement_range should be Some when transform_with_statement_hoist is called");

    source.replace_source(
      stmt_range.start,
      stmt_range.start,
      hoist_placeholder_source(
        flag,
        hoist.hoist_id,
        hoist.request,
        RstestHoistPlaceholderKind::Start,
      ),
      None,
    );
    source.replace_source(
      stmt_range.end,
      stmt_range.end,
      hoist_end_source(flag, hoist.hoist_id, hoist.request),
      None,
    );

    // Comment out original callee and replace with runtime method.
    source.replace_static(callee_range.start, callee_range.start, "/* ", None);
    source.replace(
      callee_range.end,
      callee_range.end,
      format!(" */ {require_name}.{mock_method}"),
      None,
    );
  }

  /// Transform for standalone calls that need hoisting.
  /// Example: `rs.mock('./foo', () => {...})`
  fn transform_with_call_hoist(
    source: &mut TemplateReplaceSource,
    dep: &MockMethodDependency,
    require_name: &str,
    mock_method: &str,
    flag: &str,
    hoist: &RstestHoistIdentity<'_>,
    callee_range: &DependencyRange,
  ) {
    source.replace_static(callee_range.start, callee_range.start, "/* ", None);

    let mut callee = ConcatSource::default();
    callee.add(RawStringSource::from_static(" */ "));
    callee.add(hoist_placeholder_source(
      flag,
      hoist.hoist_id,
      hoist.request,
      RstestHoistPlaceholderKind::Start,
    ));
    callee.add(RawStringSource::from(format!(
      "{require_name}.{mock_method}"
    )));
    source.replace_source(callee_range.end, callee_range.end, callee, None);

    source.replace_source(
      dep.call_expr_range.end,
      dep.call_expr_range.end,
      hoist_end_source(flag, hoist.hoist_id, hoist.request),
      None,
    );
  }

  /// Transform for calls without hoisting.
  /// Example: `rs.doMock('./foo', () => {...})`
  /// Result: `/* rs.doMock */ __rspack_require.rstest_do_mock('./foo', () => {...})`
  fn transform_without_hoist(
    source: &mut TemplateReplaceSource,
    require_name: &str,
    mock_method: &str,
    callee_range: &DependencyRange,
  ) {
    source.replace_static(callee_range.start, callee_range.start, "/* ", None);
    source.replace(
      callee_range.end,
      callee_range.end,
      format!(" */ {require_name}.{mock_method}"),
      None,
    );
  }
}
