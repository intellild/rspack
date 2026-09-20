//! Native loader work is moved between the scheduler and JS isolates. No N-API
//! value is constructed until a worker takes a task's context.
use std::sync::{
  Arc, LazyLock, Mutex,
  atomic::{AtomicBool, AtomicU32, Ordering},
};

use napi::bindgen_prelude::*;
use napi_derive::napi;
use rspack_collections::Identifiable;
use rspack_core::{CompilerId, LoaderContext, ModuleIdentifier, RunnerContext};
use rustc_hash::FxHashMap;
use tokio::sync::oneshot;

use super::{
  JsLoaderContext,
  bridge::MainObjectHandle,
  scheduler::{merge_loader_context, merge_loader_hooks},
};
use crate::module::{ModuleObject, WorkerModuleScope};

type TaskResult = (
  Box<LoaderContext<RunnerContext>>,
  Vec<String>,
  rspack_error::Result<()>,
);

pub(crate) struct LoaderTask {
  permit: Option<crate::js_tasks::TaskPermit>,
  module_scope: Option<Arc<WorkerModuleScope>>,
  context: Option<Box<LoaderContext<RunnerContext>>>,
  pub bridge_handle: Option<MainObjectHandle>,
  pub main_object_handle: u32,
  pub start: u32,
  pub end: u32,
  result: Option<oneshot::Sender<TaskResult>>,
}

impl LoaderTask {
  pub(crate) fn new(
    context: Box<LoaderContext<RunnerContext>>,
    main_object_handle: u32,
    start: u32,
    end: u32,
    result: oneshot::Sender<TaskResult>,
  ) -> Self {
    Self {
      permit: None,
      module_scope: None,
      context: Some(context),
      bridge_handle: None,
      main_object_handle,
      start,
      end,
      result: Some(result),
    }
  }

  pub(crate) fn set_permit(&mut self, permit: Option<crate::js_tasks::TaskPermit>) {
    self.permit = permit;
  }

  pub(crate) fn context(&self) -> &LoaderContext<RunnerContext> {
    self
      .context
      .as_deref()
      .expect("unfinished loader task owns its context")
  }

  fn context_mut(&mut self) -> &mut LoaderContext<RunnerContext> {
    self
      .context
      .as_deref_mut()
      .expect("unfinished loader task owns its context")
  }

  pub(crate) fn finish(mut self, pitches: Vec<String>, result: rspack_error::Result<()>) {
    if let Some(scope) = &self.module_scope {
      scope.revoke();
    }
    if let (Some(context), Some(sender)) = (self.context.take(), self.result.take()) {
      let _ = sender.send((context, pitches, result));
    }
  }
}

impl Drop for LoaderTask {
  fn drop(&mut self) {
    if let Some(scope) = &self.module_scope {
      scope.revoke();
    }
    // Cancellation can drop a future before enqueue or N-API delivery. Always
    // restore the owned state instead of only cancelling the result channel.
    if let (Some(context), Some(sender)) = (self.context.take(), self.result.take()) {
      let _ = sender.send((
        context,
        Vec::new(),
        Err(rspack_error::error!("Loader task was cancelled")),
      ));
    }
  }
}

#[derive(Default)]
struct WorkerSlot {
  stopped: AtomicBool,
  tasks: Mutex<FxHashMap<u32, TaskCell>>,
}

static NEXT_TASK: AtomicU32 = AtomicU32::new(1);
pub(crate) type TaskCell = Arc<Mutex<Option<LoaderTask>>>;
static TASKS: LazyLock<Mutex<FxHashMap<u32, TaskCell>>> = LazyLock::new(Default::default);
static WORKERS: LazyLock<Mutex<FxHashMap<u32, Arc<WorkerSlot>>>> = LazyLock::new(Default::default);

pub(super) async fn dispatch(task: LoaderTask) {
  let workers = WORKERS.lock().expect("loader workers lock poisoned");
  if workers.is_empty() {
    task.finish(
      Vec::new(),
      Err(rspack_error::error!(
        "No parallel loader workers are available"
      )),
    );
    return;
  }
  // The unbounded channel never waits for capacity. Keep registration and
  // enqueue atomic with respect to retiring the last worker.
  if let Err(error) = crate::js_tasks::JS_TASK_DISPATCHER
    .worker_sender
    .try_send(crate::js_tasks::JsTask::Loader(task))
  {
    error.into_inner().fail("Parallel loader queue is closed");
  }
}

#[napi]
pub fn register_loader_worker(id: u32) {
  WORKERS
    .lock()
    .expect("loader workers lock poisoned")
    .entry(id)
    .or_default();
}

/// Called only after Node reports worker exit: no JS can still access its context.
#[napi]
pub fn stop_loader_worker(id: u32) {
  let mut workers = WORKERS.lock().expect("loader workers lock poisoned");
  let slot = workers.remove(&id);
  // Dispatch and replacement registration use the same lock. Drain before a
  // newly registered consumer can enqueue work into the permanent channel.
  if workers.is_empty() {
    while let Ok(task) = crate::js_tasks::JS_TASK_DISPATCHER
      .worker_receiver
      .try_recv()
    {
      task.fail("No parallel loader workers are available");
    }
  }
  drop(workers);
  if let Some(slot) = slot {
    slot.stopped.store(true, Ordering::Release);
    let tasks = std::mem::take(&mut *slot.tasks.lock().expect("loader tasks lock poisoned"));
    for (id, cell) in tasks {
      TASKS
        .lock()
        .expect("loader tasks lock poisoned")
        .remove(&id);
      if let Some(task) = cell.lock().expect("loader task lock poisoned").take() {
        task.finish(
          Vec::new(),
          Err(rspack_error::error!(
            "Parallel loader worker exited before completing its task"
          )),
        );
      }
    }
  }
}

#[napi]
pub struct JsLoaderTask {
  slot: Arc<WorkerSlot>,
  id: u32,
  task: TaskCell,
  context_taken: bool,
  worker: bool,
  module: Option<(CompilerId, ModuleIdentifier)>,
  finished: bool,
}

#[napi]
impl JsLoaderTask {
  #[napi(getter)]
  pub fn id(&self) -> u32 {
    self.id
  }
  #[napi(getter)]
  pub fn main_object_handle(&self) -> napi::Result<u32> {
    let guard = self.task.lock().expect("loader task lock poisoned");
    Ok(
      guard
        .as_ref()
        .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?
        .main_object_handle,
    )
  }

  #[napi]
  pub fn take_context(&mut self) -> napi::Result<JsLoaderContext> {
    if self.context_taken {
      return Err(napi::Error::from_reason(
        "Loader task context has already been taken",
      ));
    }
    let mut guard = self.task.lock().expect("loader task lock poisoned");
    let task = guard
      .as_mut()
      .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?;
    let mut context = JsLoaderContext::try_from(task.context_mut())
      .map_err(|error| napi::Error::from_reason(error.to_string()))?;
    if self.worker {
      let native = &task.context().context;
      let scope = WorkerModuleScope::new(native.options.context.clone());
      self.module = Some((native.compiler_id, native.module.identifier()));
      context.module = context
        .module
        .map(|module| module.with_worker_scope(scope.clone()));
      task.module_scope = Some(scope);
    }
    context.loader_chain_start = task.start;
    context.loader_chain_end = task.end;
    context.bridge_handle = task.bridge_handle.as_ref().map(MainObjectHandle::handle);
    self.context_taken = true;
    Ok(context)
  }

  #[napi]
  pub fn complete(&mut self, context: JsLoaderContext) -> napi::Result<()> {
    let mut task = self
      .task
      .lock()
      .expect("loader task lock poisoned")
      .take()
      .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?;
    let pitches = context
      .loader_items
      .iter()
      .zip(task.context().loader_items.iter())
      .filter(|(js, _)| js.no_pitch)
      .map(|(_, native)| native.path().to_string())
      .collect();
    let result = merge_loader_context(task.context_mut(), context);
    self.finished = true;
    self.unregister();
    task.finish(pitches, result);
    Ok(())
  }

  #[napi]
  pub fn fail(&mut self, error: String) -> napi::Result<()> {
    let task = self
      .task
      .lock()
      .expect("loader task lock poisoned")
      .take()
      .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?;
    self.finished = true;
    self.unregister();
    task.finish(Vec::new(), Err(rspack_error::error!(error)));
    Ok(())
  }
}

impl JsLoaderTask {
  pub(crate) fn from_task(task: LoaderTask, worker: Option<u32>) -> Option<Self> {
    let slot = match worker {
      Some(id) => WORKERS
        .lock()
        .expect("loader workers lock poisoned")
        .get(&id)
        .cloned(),
      None => Some(Arc::default()),
    };
    let Some(slot) = slot else {
      task.finish(
        Vec::new(),
        Err(rspack_error::error!("Loader worker is unavailable")),
      );
      return None;
    };
    let mut active = slot.tasks.lock().expect("loader tasks lock poisoned");
    if slot.stopped.load(Ordering::Acquire) {
      task.finish(
        Vec::new(),
        Err(rspack_error::error!("Loader worker has exited")),
      );
      return None;
    }
    let id = NEXT_TASK.fetch_add(1, Ordering::Relaxed);
    let task = Arc::new(Mutex::new(Some(task)));
    active.insert(id, task.clone());
    TASKS
      .lock()
      .expect("loader tasks lock poisoned")
      .insert(id, task.clone());
    drop(active);
    Some(Self {
      slot,
      id,
      task,
      context_taken: false,
      worker: worker.is_some(),
      module: None,
      finished: false,
    })
  }

  pub(crate) fn failure_handle(&self) -> TaskCell {
    self.task.clone()
  }

  pub(crate) fn fail_handle(cell: TaskCell, error: String) {
    if let Some(task) = cell.lock().expect("loader task lock poisoned").take() {
      task.finish(Vec::new(), Err(rspack_error::error!(error)));
    }
  }

  fn unregister(&mut self) {
    if let Some((compiler_id, module_id)) = self.module.take() {
      ModuleObject::cleanup_by_module_identifiers(&compiler_id, &[module_id]);
    }
    self
      .slot
      .tasks
      .lock()
      .expect("loader tasks lock poisoned")
      .remove(&self.id);
    TASKS
      .lock()
      .expect("loader tasks lock poisoned")
      .remove(&self.id);
  }
}

impl Drop for JsLoaderTask {
  fn drop(&mut self) {
    self.unregister();
    if !self.finished
      && let Some(task) = self.task.lock().expect("loader task lock poisoned").take()
    {
      task.finish(
        Vec::new(),
        Err(rspack_error::error!("Parallel loader task was abandoned")),
      );
    }
  }
}

/// Main-isolate hook view. Called through RPC after a worker receives native work.
#[napi]
pub fn prepare_loader_task(id: u32) -> napi::Result<JsLoaderContext> {
  let cell = TASKS
    .lock()
    .expect("loader tasks lock poisoned")
    .get(&id)
    .cloned()
    .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?;
  let mut guard = cell.lock().expect("loader task lock poisoned");
  let task = guard
    .as_mut()
    .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?;
  let mut context = JsLoaderContext::from_native(task.context_mut(), true)
    .map_err(|error| napi::Error::from_reason(error.to_string()))?;
  context.loader_chain_start = task.start;
  context.loader_chain_end = task.end;
  Ok(context)
}

#[napi]
pub fn complete_loader_task_hooks(id: u32, context: JsLoaderContext) -> napi::Result<()> {
  let handle = MainObjectHandle::new(
    context
      .bridge_handle
      .ok_or_else(|| napi::Error::from_reason("Loader hooks did not register their context"))?,
  );
  let cell = TASKS
    .lock()
    .expect("loader tasks lock poisoned")
    .get(&id)
    .cloned()
    .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?;
  let mut guard = cell.lock().expect("loader task lock poisoned");
  let task = guard
    .as_mut()
    .ok_or_else(|| napi::Error::from_reason("Loader task has finished"))?;
  merge_loader_hooks(task.context_mut(), context);
  task.bridge_handle = Some(handle);
  Ok(())
}
