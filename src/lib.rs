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

use crate::{
    correspondence::{first_location, same_document},
    goals::GoalState,
    protocol::{Extras, PlainGoal, Position},
};

pub mod actions;
pub mod correspondence;
pub mod goals;
pub mod protocol;
pub mod unicode;

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
    document_version: AtomicUsize,
    request_inflight: AtomicBool,
    focused: AtomicBool,
    scroll: AtomicUsize,
    selected_goal: AtomicUsize,
    last_request: Mutex<String>,
    last_callback: Mutex<String>,
    goal: Mutex<String>,
    navigation: Mutex<String>,
    navigation_target: Mutex<Option<crate::protocol::Location>>,
    current_uri: Mutex<String>,
    goal_state: Mutex<GoalState>,
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
            document_version: AtomicUsize::new(1),
            request_inflight: AtomicBool::new(false),
            focused: AtomicBool::new(false),
            scroll: AtomicUsize::new(0),
            selected_goal: AtomicUsize::new(0),
            last_request: Mutex::new("none".to_owned()),
            last_callback: Mutex::new("none".to_owned()),
            goal: Mutex::new("Lean goal unavailable".to_owned()),
            navigation: Mutex::new("Navigation unavailable".to_owned()),
            navigation_target: Mutex::new(None),
            current_uri: Mutex::new(String::new()),
            goal_state: Mutex::new(GoalState::default()),
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
        self.request_inflight.store(false, Ordering::Release);
        self.focused.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.selected_goal.store(0, Ordering::Release);
        self.goal_state.lock().expect("goal state poisoned").close();
        *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
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
            self.record_document_change();
        }
    }

    fn record_document_change(&self) {
        if !self.is_active() {
            return;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.document_version.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.selected_goal.store(0, Ordering::Release);
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .invalidate_and_advance("document changed");
        *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
        self.inserts.fetch_add(1, Ordering::Relaxed);
        eprintln!(
            "LEAN4_HX_DOCUMENT_CHANGED version={}",
            self.document_version.load(Ordering::Acquire)
        );
    }

    fn record_opened(&self) {
        if self.is_active() {
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.document_version.store(1, Ordering::Release);
            self.request_inflight.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.selected_goal.store(0, Ordering::Release);
            self.goal_state.lock().expect("goal state poisoned").close();
            *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
            *self.navigation.lock().expect("navigation state poisoned") =
                "Navigation unavailable".to_owned();
            *self
                .navigation_target
                .lock()
                .expect("navigation state poisoned") = None;
            self.opened.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_OPEN");
        }
    }

    fn record_closed(&self) {
        if self.is_active() {
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.request_inflight.store(false, Ordering::Release);
            self.focused.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.selected_goal.store(0, Ordering::Release);
            self.goal_state.lock().expect("goal state poisoned").close();
            *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
            *self.navigation.lock().expect("navigation state poisoned") =
                "Navigation unavailable".to_owned();
            *self
                .navigation_target
                .lock()
                .expect("navigation state poisoned") = None;
            self.closed.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_CLOSE");
        }
    }

    fn begin_request(&self, path: String, line: usize, character: usize) -> usize {
        if !self.is_active()
            || self
                .request_inflight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return 0;
        }
        let position = Position {
            line: line as u32,
            character: character as u32,
            extra: Extras::new(),
        };
        let uri = file_uri(&path);
        *self.current_uri.lock().expect("navigation state poisoned") = uri.clone();
        let stamp = self.goal_state.lock().expect("goal state poisoned").begin(
            uri,
            self.document_version.load(Ordering::Acquire) as i32,
            position,
        );
        self.generation
            .store(stamp.generation as usize, Ordering::Release);
        stamp.generation as usize
    }

    fn cancel_request(&self) {
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .invalidate_and_advance("selection changed");
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
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
                self.goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .invalidate("Lean returned no goal");
                "Lean goal unavailable".to_owned()
            } else {
                let accepted = serde_json::from_str::<Value>(&result)
                    .ok()
                    .and_then(|value| Self::plain_goal(&value))
                    .map(|plain| {
                        self.goal_state
                            .lock()
                            .expect("goal state poisoned")
                            .accept_plain_goal(generation as u64, plain)
                    })
                    .unwrap_or(false);
                if !accepted {
                    eprintln!("LEAN4_HX_CALLBACK result=stale");
                    return;
                }
                self.goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .display_text()
            };
            self.request_inflight.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.selected_goal.store(0, Ordering::Release);
            *self.goal.lock().expect("goal state poisoned") = goal.clone();
            eprintln!("LEAN4_HX_CALLBACK result=reply");
            eprintln!("LEAN4_HX_GOAL generation={generation} text={goal}");
            if goal != "Lean goal unavailable" && !goal.is_empty() {
                let lines = self
                    .goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .snapshot()
                    .map(|snapshot| snapshot.display_lines().len())
                    .unwrap_or(0);
                eprintln!("LEAN4_HX_GOAL_RENDER_READY lines={lines} available=true");
                eprintln!("LEAN4_HX_GOAL_AVAILABLE");
            }
        } else {
            eprintln!("LEAN4_HX_CALLBACK result=stale");
        }
    }

    fn record_navigation(&self, generation: usize, result: String) {
        if !self.is_active() || self.generation.load(Ordering::Acquire) != generation {
            eprintln!("LEAN4_HX_NAVIGATION stale");
            return;
        }
        let current_uri = self
            .current_uri
            .lock()
            .expect("navigation state poisoned")
            .clone();
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
        let navigation = serde_json::from_str::<Value>(&result)
            .ok()
            .and_then(|value| first_location(&value))
            .map(|location| {
                *self
                    .navigation_target
                    .lock()
                    .expect("navigation state poisoned") = Some(location.clone());
                let relation = if same_document(&current_uri, &location) {
                    "same-file"
                } else {
                    "cross-file"
                };
                eprintln!("LEAN4_HX_NAVIGATION {relation} uri={}", location.uri);
                format!("navigation {relation}")
            })
            .unwrap_or_else(|| {
                eprintln!("LEAN4_HX_NAVIGATION unavailable");
                "navigation unavailable".to_owned()
            });
        *self.navigation.lock().expect("navigation state poisoned") = navigation;
    }

    fn navigation_uri(&self) -> String {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.uri.clone())
            .unwrap_or_default()
    }

    fn navigation_path(&self) -> String {
        local_file_path(&self.navigation_uri()).unwrap_or_default()
    }

    fn navigation_target_json(&self) -> String {
        let target = self
            .navigation_target
            .lock()
            .expect("navigation state poisoned");
        let Some(location) = target.as_ref() else {
            return "{}".to_owned();
        };
        let Some(path) = local_file_path(&location.uri) else {
            return "{}".to_owned();
        };
        serde_json::json!({
            "path": path,
            "start-line": location.range.start.line,
            "start-character": location.range.start.character,
            "end-line": location.range.end.line,
            "end-character": location.range.end.character,
        })
        .to_string()
    }

    fn navigation_start_line(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.start.line as usize)
            .unwrap_or(0)
    }

    fn navigation_start_character(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.start.character as usize)
            .unwrap_or(0)
    }

    fn navigation_end_line(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.end.line as usize)
            .unwrap_or(0)
    }

    fn navigation_end_character(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.end.character as usize)
            .unwrap_or(0)
    }

    fn record_rpc(&self, generation: usize, result: String) {
        if self.is_active() && self.generation.load(Ordering::Acquire) == generation {
            if result.contains("sessionId") {
                eprintln!("LEAN4_HX_RPC action=connect");
            } else {
                eprintln!("LEAN4_HX_RPC action=reply");
            }
        } else {
            eprintln!("LEAN4_HX_RPC stale");
        }
    }

    fn plain_goal(value: &Value) -> Option<PlainGoal> {
        if let Some(result) = value.get("result") {
            return Self::plain_goal(result);
        }
        serde_json::from_value(value.clone()).ok()
    }

    fn label(&self) -> String {
        let renders = self.renders.fetch_add(1, Ordering::Relaxed) + 1;
        let goal = self.goal.lock().expect("goal state poisoned").clone();
        let navigation = self
            .navigation
            .lock()
            .expect("navigation state poisoned")
            .clone();
        eprintln!("LEAN4_HX_RENDER {goal}");
        format!("lean4.hx goal {renders}: {goal} [{navigation}]")
    }

    fn lines(&self) -> Vec<String> {
        let snapshot = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .cloned();
        let count = snapshot.as_ref().map_or(0, |snapshot| snapshot.goals.len());
        let selected = self
            .selected_goal
            .load(Ordering::Acquire)
            .min(count.saturating_sub(1));
        let mut lines = vec![format!(
            "Lean goals [{}] goal {}/{}",
            if self.focused() { "focused" } else { "source" },
            if count == 0 { 0 } else { selected + 1 },
            count
        )];
        let body = snapshot
            .map(|snapshot| {
                if snapshot.unavailable.is_none() && !snapshot.goals.is_empty() {
                    snapshot.goals[selected]
                        .lines()
                        .map(str::to_owned)
                        .collect()
                } else {
                    snapshot.display_lines()
                }
            })
            .unwrap_or_else(|| vec!["Lean goal unavailable".to_owned()]);
        let offset = self
            .scroll
            .load(Ordering::Acquire)
            .min(body.len().saturating_sub(1));
        self.scroll.store(offset, Ordering::Release);
        lines.extend(body.into_iter().skip(offset));
        lines
    }

    fn focused(&self) -> bool {
        self.focused.load(Ordering::Acquire)
    }

    fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Release);
        eprintln!("LEAN4_HX_GOAL_FOCUS focused={focused}");
    }

    fn scroll(&self, amount: isize) {
        let current = self.scroll.load(Ordering::Acquire) as isize;
        self.scroll.store(
            current.saturating_add(amount).max(0) as usize,
            Ordering::Release,
        );
    }

    fn next_goal(&self) {
        self.scroll.store(0, Ordering::Release);
        let count = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .map_or(0, |snapshot| snapshot.goals.len());
        if count > 0 {
            self.selected_goal
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |index| {
                    Some((index + 1).min(count - 1))
                })
                .ok();
        }
        eprintln!(
            "LEAN4_HX_GOAL_SELECTED index={}",
            self.selected_goal.load(Ordering::Acquire)
        );
    }

    fn previous_goal(&self) {
        self.scroll.store(0, Ordering::Release);
        self.selected_goal
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |index| {
                Some(index.saturating_sub(1))
            })
            .ok();
        eprintln!(
            "LEAN4_HX_GOAL_SELECTED index={}",
            self.selected_goal.load(Ordering::Acquire)
        );
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

    fn unicode_start(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.start_chars)
            .unwrap_or(0)
    }

    fn unicode_end(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.end_chars)
            .unwrap_or(0)
    }

    fn unicode_replacement(&self, line: String, cursor_utf16: usize) -> String {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.replacement)
            .unwrap_or_default()
    }

    fn unicode_cursor(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.cursor_chars)
            .unwrap_or(0)
    }

    fn utf16_to_chars(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::utf16_to_chars(&line, cursor_utf16).unwrap_or(0)
    }
}

/// Decode local file URIs without interpreting shell syntax or accepting remote hosts.
fn local_file_path(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://")?;
    let path = path
        .strip_prefix("localhost/")
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| path.to_owned());
    if !path.starts_with('/') || path.contains(['?', '#']) {
        return None;
    }
    let mut bytes = Vec::new();
    let mut input = path.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = (input.next()? as char).to_digit(16)?;
            let low = (input.next()? as char).to_digit(16)?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    if bytes.contains(&0) {
        return None;
    }
    let path = String::from_utf8(bytes).ok()?;
    // Avoid Helix's open command creating a scratch buffer for a missing target.
    Path::new(&path).is_file().then_some(path)
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
        .register_fn(
            "native-record-document-change!",
            NativeState::record_document_change,
        )
        .register_fn("native-record-open!", NativeState::record_opened)
        .register_fn("native-record-close!", NativeState::record_closed)
        .register_fn("native-record-request!", NativeState::record_request)
        .register_fn("native-begin-request!", NativeState::begin_request)
        .register_fn("native-cancel-request!", NativeState::cancel_request)
        .register_fn("native-record-callback!", NativeState::record_callback)
        .register_fn("native-record-navigation!", NativeState::record_navigation)
        .register_fn("native-record-rpc!", NativeState::record_rpc)
        .register_fn("native-label", NativeState::label)
        .register_fn("native-lines", NativeState::lines)
        .register_fn("native-focused?", NativeState::focused)
        .register_fn("native-set-focused!", NativeState::set_focused)
        .register_fn("native-scroll!", NativeState::scroll)
        .register_fn("native-next-goal!", NativeState::next_goal)
        .register_fn("native-previous-goal!", NativeState::previous_goal)
        .register_fn("native-summary", NativeState::summary)
        .register_fn("native-unicode-start", NativeState::unicode_start)
        .register_fn("native-unicode-end", NativeState::unicode_end)
        .register_fn(
            "native-unicode-replacement",
            NativeState::unicode_replacement,
        )
        .register_fn("native-unicode-cursor", NativeState::unicode_cursor)
        .register_fn("native-utf16-to-chars", NativeState::utf16_to_chars)
        .register_fn(
            "native-navigation-target",
            NativeState::navigation_target_json,
        )
        .register_fn("native-navigation-path", NativeState::navigation_path)
        .register_fn("native-navigation-uri", NativeState::navigation_uri)
        .register_fn(
            "native-navigation-start-line",
            NativeState::navigation_start_line,
        )
        .register_fn(
            "native-navigation-start-character",
            NativeState::navigation_start_character,
        )
        .register_fn(
            "native-navigation-end-line",
            NativeState::navigation_end_line,
        )
        .register_fn(
            "native-navigation-end-character",
            NativeState::navigation_end_character,
        )
        .register_fn("native-file-uri", file_uri);
    module
}

steel::declare_module!(build_module);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_state_rejects_stale_goal_after_cancel() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        assert_ne!(generation, 0);
        assert_eq!(state.begin_request("/tmp/Main.lean".into(), 1, 0), 0);
        state.cancel_request();
        state.record_callback(
            generation,
            r#"{"goals":["n : Nat\n⊢ n = n"],"rendered":"goal"}"#.into(),
        );
        assert_eq!(
            *state.goal.lock().expect("goal state poisoned"),
            "Lean goal unavailable"
        );
    }

    #[test]
    fn document_change_invalidates_callback_and_advances_version() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        assert_ne!(generation, 0);
        state.record_document_change();
        state.record_callback(
            generation,
            r#"{"goals":["stale"],"rendered":"stale"}"#.into(),
        );
        assert_eq!(
            *state.goal.lock().expect("goal state poisoned"),
            "Lean goal unavailable"
        );
        let reopened = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        assert!(reopened > generation);
    }
}
