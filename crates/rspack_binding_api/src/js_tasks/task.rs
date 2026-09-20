use crate::plugins::js_loader::worker::LoaderTask;

/// Native work stays owned by the queue until its destination env consumes it.
/// Other JS work can add a variant without changing the subscription protocol.
pub(crate) enum JsTask {
  Loader(LoaderTask),
}

impl JsTask {
  pub(crate) fn fail(self, message: &str) {
    match self {
      Self::Loader(task) => task.finish(Vec::new(), Err(rspack_error::error!("{message}"))),
    }
  }
}
