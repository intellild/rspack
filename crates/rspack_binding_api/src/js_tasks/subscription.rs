use std::{
  cell::RefCell,
  sync::{Arc, Mutex, Weak},
};

use dashmap::DashMap;
use napi::{Env, sys};
use tokio_util::sync::CancellationToken;

use super::{
  dispatcher::JS_TASK_DISPATCHER,
  tsfn::{Delivery, JsTaskTsfn},
};

pub(crate) static MAIN_TASK_SUBSCRIPTION: Mutex<Weak<JsTaskSubscription>> = Mutex::new(Weak::new());

thread_local! {
  // A replacement cannot consume the permanent channel until the old loop exits.
  static PREVIOUS_LOOP: RefCell<Option<CancellationToken>> = const { RefCell::new(None) };
}

pub struct JsTaskSubscription {
  pub(crate) ready: CancellationToken,
  cancel: CancellationToken,
  pub(crate) worker_subscriptions: DashMap<u32, Arc<JsTaskSubscription>>,
}

impl JsTaskSubscription {
  pub(super) fn new(
    env: &Env,
    callback: sys::napi_value,
    worker: Option<u32>,
  ) -> napi::Result<Arc<Self>> {
    rspack_napi::runtime::ensure_runtime(env)?;
    let tsfn = Arc::new(JsTaskTsfn::new(env, callback)?);
    let cancel = CancellationToken::new();
    let cleanup_cancel = cancel.clone();
    let cleanup_tsfn = Arc::downgrade(&tsfn);
    env.add_env_cleanup_hook((), move |_| {
      cleanup_cancel.cancel();
      if let Some(tsfn) = cleanup_tsfn.upgrade() {
        tsfn.abort();
      }
    })?;
    let ready = CancellationToken::new();
    let exited = CancellationToken::new();
    let previous = PREVIOUS_LOOP.with(|slot| slot.replace(Some(exited.clone())));
    let subscription = Arc::new(Self {
      ready: ready.clone(),
      cancel: cancel.clone(),
      worker_subscriptions: DashMap::new(),
    });
    let receiver = if worker.is_some() {
      JS_TASK_DISPATCHER.worker_receiver.clone()
    } else {
      JS_TASK_DISPATCHER.main_receiver.clone()
    };
    // Never capture subscription: its last owner's Drop must stop this loop.
    let exit = exited.drop_guard();
    rspack_napi::runtime::spawn(async move {
      let _exit = exit;
      if let Some(previous) = previous {
        previous.cancelled().await;
      }
      ready.cancel();
      loop {
        let task = tokio::select! {
          biased;
          _ = cancel.cancelled() => break,
          task = receiver.recv() => match task {
            Ok(task) => task,
            Err(_) => break,
          },
        };
        if cancel.is_cancelled() {
          task.fail("JS task subscription has closed");
          break;
        }
        tsfn.post(Delivery {
          task,
          worker,
          cancel: cancel.clone(),
        });
      }
      if worker.is_none() {
        while let Ok(task) = receiver.try_recv() {
          task.fail("Main JS task subscription has closed");
        }
      }
    });
    Ok(subscription)
  }

  pub(crate) fn cancel(&self) {
    self.cancel.cancel();
    for worker in &self.worker_subscriptions {
      worker.cancel();
    }
  }
}

impl Drop for JsTaskSubscription {
  fn drop(&mut self) {
    self.cancel();
  }
}
