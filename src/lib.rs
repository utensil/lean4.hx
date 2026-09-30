//! Rust-owned state for the Steel/Helix host integration.
//!
//! The editor and language-server lifetimes remain owned by Helix. Scheme is
//! only the thin adapter that registers Helix hooks and builds LSP values;
//! counters and lifecycle state live here so reloads cannot silently replace
//! the state used by the native component.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};

use serde_json::Value;

use steel::{
    rvals::Custom,
    steel_vm::ffi::{FFIModule, RegisterFFIFn},
};

pub mod actions;
pub mod correspondence;
pub mod goals;
pub mod protocol;

#[derive(Debug)]
struct NativeState {
    active: AtomicBool,
    renders: AtomicUsize,
    installs: AtomicUsize,
    removals: AtomicUsize,
    selections: AtomicUsize,
    inserts: AtomicUsize,
    opened: AtomicUsize,
    closed: AtomicUsize,
    requests: AtomicUsize,
    callbacks: AtomicUsize,
    generation: AtomicUsize,
    last_request: Mutex<String>,
    last_callback: Mutex<String>,
    goal: Mutex<String>,
}

impl Custom for NativeState {}

impl NativeState {
    fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            renders: AtomicUsize::new(0),
            installs: AtomicUsize::new(0),
            removals: AtomicUsize::new(0),
            selections: AtomicUsize::new(0),
            inserts: AtomicUsize::new(0),
            opened: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            callbacks: AtomicUsize::new(0),
            generation: AtomicUsize::new(0),
            last_request: Mutex::new("none".to_owned()),
            last_callback: Mutex::new("none".to_owned()),
            goal: Mutex::new("Lean goal unavailable".to_owned()),
        }
    }

    fn activate(&self) {
        self.active.store(true, Ordering::Release);
        self.installs.fetch_add(1, Ordering::Relaxed);
        eprintln!("LEAN4_HX_INSTALL active=true");
    }

    fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
        *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
        self.removals.fetch_add(1, Ordering::Relaxed);
        eprintln!("LEAN4_HX_REMOVE active=false");
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn record_selection(&self) {
        if self.is_active() {
            self.selections.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_SELECTION");
        }
    }

    fn record_insert(&self) {
        if self.is_active() {
            self.inserts.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_INSERT");
        }
    }

    fn record_opened(&self) {
        if self.is_active() {
            self.opened.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_OPEN");
        }
    }

    fn record_closed(&self) {
        if self.is_active() {
            self.generation.fetch_add(1, Ordering::AcqRel);
            *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
            self.closed.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_CLOSE");
        }
    }

    fn begin_request(&self) -> usize {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn record_request(&self, path: String, line: usize, character: usize) {
        if self.is_active() {
            self.requests.fetch_add(1, Ordering::Relaxed);
            let mut last = self.last_request.lock().expect("request state poisoned");
            *last = format!("line={line},character={character},uri={}", file_uri(&path));
            eprintln!(
                "LEAN4_HX_REQUEST line={line} character={character} uri={}",
                file_uri(&path)
            );
        }
    }

    fn record_callback(&self, generation: usize, result: String) {
        if self.is_active() && self.generation.load(Ordering::Acquire) == generation {
            self.callbacks.fetch_add(1, Ordering::Relaxed);
            let mut last = self.last_callback.lock().expect("callback state poisoned");
            *last = "reply".to_owned();
            let goal = if result == "null" {
                "Lean goal unavailable".to_owned()
            } else {
                serde_json::from_str::<Value>(&result)
                    .ok()
                    .and_then(|value| Self::goal_text(&value))
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| result.clone())
            };
            *self.goal.lock().expect("goal state poisoned") = goal.clone();
            eprintln!("LEAN4_HX_CALLBACK result=reply");
            eprintln!("LEAN4_HX_GOAL generation={generation} text={goal}");
            if goal != "Lean goal unavailable" && !goal.is_empty() {
                eprintln!("LEAN4_HX_GOAL_AVAILABLE");
            }
        } else {
            eprintln!("LEAN4_HX_CALLBACK result=stale");
        }
    }

    fn goal_text(value: &Value) -> Option<String> {
        if let Some(rendered) = value.get("rendered").and_then(Value::as_str) {
            return Some(rendered.to_owned());
        }
        if let Some(result) = value.get("result") {
            if let Some(text) = Self::goal_text(result) {
                return Some(text);
            }
        }
        value.get("goals").and_then(Value::as_array).map(|goals| {
            goals
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    fn label(&self) -> String {
        let renders = self.renders.fetch_add(1, Ordering::Relaxed) + 1;
        let goal = self.goal.lock().expect("goal state poisoned").clone();
        eprintln!("LEAN4_HX_RENDER {goal}");
        format!("lean4.hx goal {renders}: {goal}")
    }

    fn summary(&self) -> String {
        let request = self.last_request.lock().expect("request state poisoned");
        let callback = self.last_callback.lock().expect("callback state poisoned");
        format!(
            "active={} installs={} removals={} renders={} selection={} insert={} open={} close={} requests={} callbacks={} last_request={} last_callback={}",
            self.is_active(),
            self.installs.load(Ordering::Relaxed),
            self.removals.load(Ordering::Relaxed),
            self.renders.load(Ordering::Relaxed),
            self.selections.load(Ordering::Relaxed),
            self.inserts.load(Ordering::Relaxed),
            self.opened.load(Ordering::Relaxed),
            self.closed.load(Ordering::Relaxed),
            self.requests.load(Ordering::Relaxed),
            self.callbacks.load(Ordering::Relaxed),
            &*request,
            &*callback,
        )
    }
}

/// Convert an absolute editor path into the URI required by Lean's LSP.
/// Percent-encoding is implemented locally so the host adapter does not add
/// another dependency solely for this boundary conversion.
fn file_uri(path: &str) -> String {
    let absolute = if Path::new(path).is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_owned())
    };
    let mut uri = String::from("file://");
    for byte in absolute.as_bytes() {
        let safe = matches!(byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' |
            b'-' | b'.' | b'_' | b'~' | b'/');
        if safe {
            uri.push(*byte as char);
        } else {
            uri.push('%');
            uri.push(
                char::from_digit((byte >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            uri.push(
                char::from_digit((byte & 0x0f) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    uri
}

/// Construct the Steel FFI module loaded by `#%require-dylib`.
pub fn build_module() -> FFIModule {
    let mut module = FFIModule::new("dylib/lean4-hx");
    module
        .register_fn("native-state", NativeState::new)
        .register_fn("native-activate!", NativeState::activate)
        .register_fn("native-deactivate!", NativeState::deactivate)
        .register_fn("native-active?", NativeState::is_active)
        .register_fn("native-record-selection!", NativeState::record_selection)
        .register_fn("native-record-insert!", NativeState::record_insert)
        .register_fn("native-record-open!", NativeState::record_opened)
        .register_fn("native-record-close!", NativeState::record_closed)
        .register_fn("native-record-request!", NativeState::record_request)
        .register_fn("native-begin-request!", NativeState::begin_request)
        .register_fn("native-record-callback!", NativeState::record_callback)
        .register_fn("native-label", NativeState::label)
        .register_fn("native-summary", NativeState::summary)
        .register_fn("native-file-uri", file_uri);
    module
}

steel::declare_module!(build_module);
