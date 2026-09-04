//! FFI cdylib loader — dynamically load a compiled workflow `.so` and invoke it.
//!
//! A saved workflow is codegen'd to Rust, built as a **cdylib**, content-hashed,
//! and stored in the artifact registry (see the compile-on-save pipeline). At
//! run time the daemon resolves the artifact, fetches the `.so` from S3 into a
//! local cache, `dlopen`s it via [`libloading`], and invokes it over a stable
//! **C ABI** — the [`CallableHandle::Ffi`] variant.
//!
//! ## ABI contract
//!
//! The cdylib MUST export two `#[no_mangle] extern "C"` symbols:
//!
//! ```ignore
//! /// Run the workflow with a JSON input string; returns an owned JSON output
//! /// string (NUL-terminated). On workflow error, returns a JSON object
//! /// `{"error": "..."}`. MUST NOT unwind across the boundary — wrap the body
//! /// in `std::panic::catch_unwind`.
//! #[no_mangle]
//! pub extern "C" fn veil_workflow_run(input_json: *const c_char) -> *mut c_char;
//!
//! /// Free a string previously returned by `veil_workflow_run`.
//! #[no_mangle]
//! pub extern "C" fn veil_workflow_free(ptr: *mut c_char);
//! ```
//!
//! Using a JSON-bytes C ABI (rather than sharing a Rust trait object across the
//! seam) makes the boundary **ABI-stable**: the daemon and the cdylib need only
//! agree on the two C symbol names and JSON, not on Rust type layout. This
//! sidesteps the biggest operational risk called out in the spec (trait/type
//! ABI drift between daemon and artifact toolchains).
//!
//! ## Panic isolation
//!
//! The daemon wraps every FFI call in [`std::panic::catch_unwind`] as a second
//! line of defence, so a workflow that panics past its own `catch_unwind`
//! cannot tear down the daemon. A caught panic becomes a [`BoxErr`].
//!
//! ## Lifetime / caching
//!
//! Each loaded library is kept alive by an [`Arc<LoadedWorkflow>`] that owns the
//! [`libloading::Library`]; dropping the last `Arc` `dlclose`s it. The registry
//! holds these in an LRU keyed by content-hash so hot workflows stay resident
//! and are never unloaded while a call is in flight (the `Arc` refcount pins
//! them). See [`FfiLibraryCache`].

use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::BoxErr;

/// A loaded workflow cdylib, owning the `Library` handle and resolved symbols.
///
/// Owning the `Library` keeps the `.so` mapped for as long as any `Arc` to this
/// struct is alive. The raw symbol pointers borrow from `_lib`, so `_lib` MUST
/// outlive them — enforced by keeping them in the same struct and never handing
/// out the pointers.
pub struct LoadedWorkflow {
    /// The dlopen'd library. Field order matters: symbols above are dropped
    /// first, then the library — but since we store raw fn pointers (Copy), the
    /// only real invariant is that `_lib` is not dropped while we call through
    /// the pointers. Holding an `Arc<LoadedWorkflow>` across a call guarantees
    /// that.
    _lib: libloading::Library,
    /// `veil_workflow_run(input_json: *const c_char) -> *mut c_char`
    run: unsafe extern "C" fn(*const c_char) -> *mut c_char,
    /// `veil_workflow_free(ptr: *mut c_char)`
    free: unsafe extern "C" fn(*mut c_char),
    /// Content hash this library was loaded from (for diagnostics).
    content_hash: String,
}

// The raw C fn pointers are `Send + Sync` (plain fn pointers), and `Library` is
// `Send + Sync`. Asserting it lets us store `Arc<LoadedWorkflow>` in the shared
// cache and hand clones to async tasks.
unsafe impl Send for LoadedWorkflow {}
unsafe impl Sync for LoadedWorkflow {}

impl std::fmt::Debug for LoadedWorkflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedWorkflow")
            .field("content_hash", &self.content_hash)
            .finish()
    }
}

impl LoadedWorkflow {
    /// `dlopen` the library at `path` and resolve the workflow ABI symbols.
    ///
    /// # Safety
    /// Loading an arbitrary shared object executes its initializers. Only load
    /// artifacts produced by the trusted compile-on-save pipeline and pinned by
    /// content hash.
    pub fn load(path: &std::path::Path, content_hash: impl Into<String>) -> Result<Self, BoxErr> {
        let content_hash = content_hash.into();
        // SAFETY: path points at a trusted, content-hash-pinned artifact built
        // by our own compile pipeline.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| -> BoxErr { format!("dlopen {path:?}: {e}").into() })?;

        // Resolve the two ABI symbols up front so a malformed artifact fails at
        // load time, not on first invoke.
        let run = unsafe {
            let sym: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_char> = lib
                .get(b"veil_workflow_run\0")
                .map_err(|e| -> BoxErr {
                    format!("symbol veil_workflow_run missing in {path:?}: {e}").into()
                })?;
            *sym
        };
        let free = unsafe {
            let sym: libloading::Symbol<unsafe extern "C" fn(*mut c_char)> = lib
                .get(b"veil_workflow_free\0")
                .map_err(|e| -> BoxErr {
                    format!("symbol veil_workflow_free missing in {path:?}: {e}").into()
                })?;
            *sym
        };

        Ok(Self {
            _lib: lib,
            run,
            free,
            content_hash,
        })
    }

    /// Invoke the workflow with JSON args, isolating panics that cross the seam.
    ///
    /// The input is serialized to a NUL-terminated JSON C string; the returned
    /// pointer is copied into an owned Rust string and then freed via the
    /// cdylib's own allocator (`veil_workflow_free`) to avoid a cross-allocator
    /// free. The whole FFI call is wrapped in `catch_unwind`.
    pub fn invoke(&self, args: &Value) -> Result<Value, BoxErr> {
        let input = serde_json::to_string(args)
            .map_err(|e| -> BoxErr { format!("serialize workflow input: {e}").into() })?;
        let c_input = CString::new(input)
            .map_err(|e| -> BoxErr { format!("input contains NUL byte: {e}").into() })?;

        let run = self.run;
        let free = self.free;

        // Second line of defence: catch a panic that escapes the cdylib's own
        // catch_unwind so one bad workflow cannot crash the daemon.
        let out_ptr = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: `run` is a resolved symbol of the loaded library, kept
            // mapped for the duration of this call (self borrow -> _lib alive).
            unsafe { run(c_input.as_ptr()) }
        }))
        .map_err(|_| -> BoxErr {
            format!(
                "workflow (hash {}) panicked across the FFI boundary",
                self.content_hash
            )
            .into()
        })?;

        if out_ptr.is_null() {
            return Err(format!(
                "workflow (hash {}) returned a null result pointer",
                self.content_hash
            )
            .into());
        }

        // Copy the C string out, then free it with the cdylib's allocator.
        let out_json = unsafe {
            let s = CStr::from_ptr(out_ptr).to_string_lossy().into_owned();
            free(out_ptr);
            s
        };

        let value: Value = serde_json::from_str(&out_json)
            .map_err(|e| -> BoxErr { format!("workflow returned non-JSON: {e}: {out_json}").into() })?;

        // Convention: an object with a top-level "error" string is a workflow
        // error surfaced as Err so callers fail closed.
        if let Some(err) = value.get("error").and_then(|e| e.as_str()) {
            return Err(format!("workflow error: {err}").into());
        }
        Ok(value)
    }

    /// The content hash this library was loaded from.
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }
}

/// A bounded LRU cache of loaded workflow libraries keyed by content hash.
///
/// Keeping an `Arc<LoadedWorkflow>` resident means a hot workflow is loaded once
/// and reused across invokes (sub-ms warm calls, no dlopen per request). An
/// entry is only unloaded (`dlclose`d) when it is evicted AND no in-flight call
/// holds an `Arc` to it — the `Arc` refcount guarantees a library is never
/// unmapped mid-call.
#[derive(Clone)]
pub struct FfiLibraryCache {
    inner: Arc<Mutex<LruInner>>,
}

struct LruInner {
    /// hash → loaded library.
    map: HashMap<String, Arc<LoadedWorkflow>>,
    /// Most-recently-used order, front = most recent.
    order: Vec<String>,
    /// Max resident libraries.
    capacity: usize,
}

impl FfiLibraryCache {
    /// Create a cache holding at most `capacity` resident libraries.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LruInner {
                map: HashMap::new(),
                order: Vec::new(),
                capacity: capacity.max(1),
            })),
        }
    }

    /// Get a resident library by content hash, if loaded.
    pub fn get(&self, hash: &str) -> Option<Arc<LoadedWorkflow>> {
        let mut inner = self.inner.lock().expect("ffi cache poisoned");
        if let Some(lib) = inner.map.get(hash).cloned() {
            inner.touch(hash);
            Some(lib)
        } else {
            None
        }
    }

    /// Insert a loaded library, evicting the least-recently-used entry if over
    /// capacity. Returns the shared handle.
    pub fn insert(&self, hash: String, lib: Arc<LoadedWorkflow>) -> Arc<LoadedWorkflow> {
        let mut inner = self.inner.lock().expect("ffi cache poisoned");
        inner.map.insert(hash.clone(), lib.clone());
        inner.touch(&hash);
        inner.evict_if_needed();
        lib
    }

    /// Number of resident libraries.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("ffi cache poisoned").map.len()
    }

    /// Whether the cache is empty.
    // Retained: collection API completeness alongside `len()`.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl LruInner {
    fn touch(&mut self, hash: &str) {
        self.order.retain(|h| h != hash);
        self.order.insert(0, hash.to_string());
    }

    fn evict_if_needed(&mut self) {
        while self.order.len() > self.capacity {
            if let Some(victim) = self.order.pop() {
                // Dropping the Arc here only unloads if no in-flight call holds
                // a clone; otherwise the library stays mapped until the last
                // Arc is dropped.
                self.map.remove(&victim);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A dummy LoadedWorkflow is hard to fabricate without a real .so, so cache
    // behaviour is tested via a lightweight fake wrapped in Arc through the
    // public API using a real (tiny) shared object is out of scope for unit
    // tests. Here we test the LRU bookkeeping through insert/get/evict using
    // real loaded libraries only in integration tests. Instead, exercise the
    // ordering logic directly.

    #[test]
    fn lru_touch_moves_to_front() {
        let mut inner = LruInner {
            map: HashMap::new(),
            order: Vec::new(),
            capacity: 2,
        };
        inner.touch("a");
        inner.touch("b");
        assert_eq!(inner.order, vec!["b".to_string(), "a".to_string()]);
        inner.touch("a");
        assert_eq!(inner.order, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut inner = LruInner {
            map: HashMap::new(),
            order: Vec::new(),
            capacity: 2,
        };
        // Simulate three inserts with capacity 2; oldest (a) should evict.
        for h in ["a", "b", "c"] {
            inner.order.retain(|x| x != h);
            inner.order.insert(0, h.to_string());
            inner.evict_if_needed();
        }
        assert_eq!(inner.order.len(), 2);
        assert_eq!(inner.order, vec!["c".to_string(), "b".to_string()]);
    }

    #[test]
    fn cache_capacity_floor_is_one() {
        let cache = FfiLibraryCache::new(0);
        assert!(cache.is_empty());
        assert_eq!(cache.inner.lock().unwrap().capacity, 1);
    }
}
