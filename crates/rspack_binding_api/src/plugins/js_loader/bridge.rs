use std::sync::{Arc, LazyLock, Mutex, Weak};

use napi::{
  Env, Status,
  threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
};
use napi_derive::napi;
use rustc_hash::FxHashMap;

type ReleaseCallback = ThreadsafeFunction<u32, (), u32, Status, false, true, 0>;

// The env owns its route until cleanup. Handles retain that exact route, so an
// old handle can never release the same numeric ID in a replacement env.
static RELEASE: Mutex<Weak<MainObjectRelease>> = Mutex::new(Weak::new());

struct MainObjectRelease {
  callback: Mutex<Option<ReleaseCallback>>,
}

struct MainObjectHandleInner {
  handle: u32,
  release: Arc<MainObjectRelease>,
}

impl Drop for MainObjectHandleInner {
  fn drop(&mut self) {
    if let Some(callback) = self
      .release
      .callback
      .lock()
      .expect("main object release lock poisoned")
      .as_ref()
    {
      callback.call(self.handle, ThreadsafeFunctionCallMode::NonBlocking);
    }
  }
}

/// A cloneable handle for a JavaScript value owned by the main thread.
///
/// The last clone releases the handle through a threadsafe main-thread callback.
#[derive(Clone)]
pub struct MainObjectHandle {
  inner: Arc<MainObjectHandleInner>,
}

impl MainObjectHandle {
  pub fn new(handle: u32) -> Self {
    Self {
      inner: Arc::new(MainObjectHandleInner {
        handle,
        release: RELEASE
          .lock()
          .expect("main object release lock poisoned")
          .upgrade()
          .expect("main-thread JS value release callback should be registered"),
      }),
    }
  }

  pub fn handle(&self) -> u32 {
    self.inner.handle
  }
}

static REFERENCES: LazyLock<Mutex<FxHashMap<u32, FxHashMap<String, bool>>>> =
  LazyLock::new(Default::default);

#[napi(ts_args_type = "release: (value: number) => void")]
pub fn register_main_object_release(env: Env, release: ReleaseCallback) -> napi::Result<()> {
  let mut global = RELEASE.lock().expect("main object release lock poisoned");
  if let Some(current) = global.upgrade()
    && current
      .callback
      .lock()
      .expect("main object release lock poisoned")
      .is_some()
  {
    return Err(napi::Error::from_reason(
      "Main object release callback already belongs to a live JS env",
    ));
  }
  let route = Arc::new(MainObjectRelease {
    callback: Mutex::new(Some(release)),
  });
  env.add_env_cleanup_hook(route.clone(), |route| {
    // Serialize with handle Drop before releasing the TSFN. The route remains
    // inert even if Rust-owned handles outlive this JS env.
    let callback = route
      .callback
      .lock()
      .expect("main object release lock poisoned")
      .take();
    let mut global = RELEASE.lock().expect("main object release lock poisoned");
    if global.ptr_eq(&Arc::downgrade(&route)) {
      *global = Weak::new();
      REFERENCES
        .lock()
        .expect("loader references lock poisoned")
        .clear();
    }
    drop(global);
    drop(callback);
  })?;
  *global = Arc::downgrade(&route);
  Ok(())
}

#[napi]
pub fn register_loader_reference(owner: u32, ident: String, parallel: bool) -> bool {
  REFERENCES
    .lock()
    .expect("loader references lock poisoned")
    .entry(owner)
    .or_default()
    .insert(ident, parallel);
  parallel && !cfg!(target_family = "wasm")
}

#[napi]
pub fn release_loader_references(owner: u32) {
  REFERENCES
    .lock()
    .expect("loader references lock poisoned")
    .remove(&owner);
}

pub(super) fn is_parallel(owner: u32, query: Option<&str>) -> bool {
  let Some(ident) = query.and_then(|q| q.strip_prefix("??")) else {
    return false;
  };
  REFERENCES
    .lock()
    .expect("loader references lock poisoned")
    .get(&owner)
    .and_then(|references| references.get(ident))
    .copied()
    .unwrap_or(false)
}
