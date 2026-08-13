#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NodeSummary {
  pub(crate) bytes: usize,
  pub(crate) generated_line: u32,
  pub(crate) generated_column: u32,
  pub(crate) is_ascii: bool,
}

impl NodeSummary {
  pub(crate) fn combine(left: Self, right: Self) -> Self {
    let (generated_line, generated_column) = if right.generated_line > 1 {
      (
        left.generated_line + right.generated_line - 1,
        right.generated_column,
      )
    } else {
      (
        left.generated_line,
        left.generated_column + right.generated_column,
      )
    };
    Self {
      bytes: left.bytes + right.bytes,
      generated_line,
      generated_column,
      is_ascii: left.is_ascii && right.is_ascii,
    }
  }
}
