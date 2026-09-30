//! Rust support for the Lean-aware Helix integration.
//!
//! The host owns editor state and event/language-server lifetimes. This cdylib
//! deliberately exports only an opaque, native value used by the Steel dynamic
//! component. The editor-facing hooks and callback are in `host/extension.scm`, where
//! they use the pinned Helix extension API.

use std::sync::atomic::{AtomicUsize, Ordering};

use steel::{
    rvals::Custom,
    steel_vm::ffi::{FFIModule, RegisterFFIFn},
};

pub mod protocol;

#[derive(Debug)]
struct NativeState {
    renders: AtomicUsize,
}

impl Custom for NativeState {}

impl NativeState {
    fn new() -> Self {
        Self {
            renders: AtomicUsize::new(0),
        }
    }

    fn label(&self) -> String {
        let renders = self.renders.fetch_add(1, Ordering::Relaxed) + 1;
        format!("lean4.hx native component (render {renders})")
    }
}

/// Construct the Steel FFI module loaded by `#%require-dylib`.
///
/// The module intentionally has no editor or LSP ABI of its own. Those calls
/// stay on Helix's built-in Steel API, which is the ABI the pinned host owns.
pub fn build_module() -> FFIModule {
    let mut module = FFIModule::new("dylib/lean4-hx");
    module
        .register_fn("native-state", NativeState::new)
        .register_fn("native-label", NativeState::label);
    module
}

steel::declare_module!(build_module);
