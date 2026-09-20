use std::sync::LazyLock;

use async_channel::{Receiver, Sender};

use super::task::JsTask;

pub(crate) static JS_TASK_DISPATCHER: LazyLock<JsTaskDispatcher> =
  LazyLock::new(JsTaskDispatcher::new);

pub(crate) struct JsTaskDispatcher {
  pub(crate) main_sender: Sender<JsTask>,
  pub(crate) main_receiver: Receiver<JsTask>,
  pub(crate) worker_sender: Sender<JsTask>,
  pub(crate) worker_receiver: Receiver<JsTask>,
}

impl JsTaskDispatcher {
  fn new() -> Self {
    let (main_sender, main_receiver) = async_channel::unbounded();
    let (worker_sender, worker_receiver) = async_channel::unbounded();
    Self {
      main_sender,
      main_receiver,
      worker_sender,
      worker_receiver,
    }
  }
}

/// The caller's running compiler owns the main subscription. Keep a temporary
/// reference until enqueue so the last owner cannot retire between both steps.
pub(crate) async fn dispatch_main(task: JsTask) {
  let subscription = super::MAIN_TASK_SUBSCRIPTION
    .lock()
    .expect("main subscription lock poisoned")
    .upgrade();
  let Some(subscription) = subscription else {
    task.fail("Main JS task subscription is unavailable");
    return;
  };
  subscription.ready.cancelled().await;
  if let Err(error) = JS_TASK_DISPATCHER.main_sender.try_send(task) {
    error.into_inner().fail("Main JS task queue is closed");
  }
}
