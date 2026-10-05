//! Small, versioned goal snapshots shared by the native component and tests.

use crate::{
    correspondence::{terminal_spans, TerminalSpan, TerminalStyle},
    protocol::{PlainGoal, PlainTermGoal, Position, Range, TaggedText},
};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct RequestStamp {
    pub uri: String,
    pub version: i32,
    pub position: Position,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GoalSnapshot {
    pub stamp: RequestStamp,
    pub rendered: String,
    pub goals: Vec<String>,
    /// Per-goal, per-line spans produced from Lean's TaggedText payload.
    /// Plain goal notifications populate this with unstyled fallback spans.
    pub styled_goals: Vec<Vec<Vec<TerminalSpan>>>,
    pub interactive: bool,
    pub term_goal: Option<String>,
    pub term_range: Option<Range>,
    pub unavailable: Option<String>,
    /// Lean returns an empty goal list after a proof is complete. Preserve
    /// that state as content instead of confusing it with unavailable data.
    pub completed: bool,
}

impl GoalSnapshot {
    pub fn unavailable(stamp: RequestStamp, reason: impl Into<String>) -> Self {
        Self {
            stamp,
            rendered: String::new(),
            goals: Vec::new(),
            styled_goals: Vec::new(),
            interactive: false,
            term_goal: None,
            term_range: None,
            unavailable: Some(reason.into()),
            completed: false,
        }
    }

    pub fn from_plain_goal(stamp: RequestStamp, goal: PlainGoal) -> Self {
        let completed = goal.goals.is_empty();
        let styled_goals = goal
            .goals
            .iter()
            .map(|goal| {
                goal.lines()
                    .map(|line| {
                        vec![TerminalSpan {
                            text: line.to_owned(),
                            style: TerminalStyle::Plain,
                            tags: Vec::new(),
                        }]
                    })
                    .collect()
            })
            .collect();
        Self {
            stamp,
            rendered: goal.rendered,
            goals: goal.goals,
            styled_goals,
            interactive: false,
            term_goal: None,
            term_range: None,
            unavailable: None,
            completed,
        }
    }

    /// Parse the open-ended result of Lean.Widget.getInteractiveGoals. The
    /// surrounding RPC schema is intentionally kept as JSON, while each
    /// tagged type is decoded losslessly for terminal rendering.
    pub fn from_interactive_goals(stamp: RequestStamp, value: &Value) -> Option<Self> {
        let value = value.get("result").unwrap_or(value);
        let goals = value.get("goals")?.as_array()?;
        let mut rendered_goals = Vec::with_capacity(goals.len());
        let mut styled_goals = Vec::with_capacity(goals.len());
        for goal in goals {
            let Some(object) = goal.as_object() else {
                let text = "Lean goal unavailable: malformed interactive goal".to_owned();
                rendered_goals.push(text.clone());
                styled_goals.push(vec![vec![TerminalSpan {
                    text,
                    style: TerminalStyle::Plain,
                    tags: Vec::new(),
                }]]);
                continue;
            };
            let prefix = object
                .get("goalPrefix")
                .and_then(Value::as_str)
                .unwrap_or("⊢ ");
            let mut lines = Vec::new();
            let mut plain_lines = Vec::new();
            let mut target = vec![TerminalSpan {
                text: prefix.to_owned(),
                style: TerminalStyle::Goal,
                tags: Vec::new(),
            }];
            if let Some(value) = object.get("type") {
                if let Ok(tagged) = serde_json::from_value::<TaggedText>(value.clone()) {
                    target.extend(terminal_spans(&tagged));
                } else {
                    let text = value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string());
                    target.push(TerminalSpan {
                        text,
                        style: TerminalStyle::Plain,
                        tags: Vec::new(),
                    });
                }
            }
            if let Some(hyps) = object.get("hyps").and_then(Value::as_array) {
                for hyp in hyps {
                    let Some(hyp_object) = hyp.as_object() else {
                        let text = "[malformed hypothesis]".to_owned();
                        plain_lines.push(text.clone());
                        lines.push(vec![TerminalSpan {
                            text,
                            style: TerminalStyle::Plain,
                            tags: Vec::new(),
                        }]);
                        continue;
                    };
                    let names = hyp_object
                        .get("names")
                        .and_then(Value::as_array)
                        .map(|names| {
                            names
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    let mut line = vec![TerminalSpan {
                        text: format!("{names} : "),
                        style: TerminalStyle::Plain,
                        tags: Vec::new(),
                    }];
                    if let Some(value) = hyp_object.get("type") {
                        if let Ok(tagged) = serde_json::from_value::<TaggedText>(value.clone()) {
                            line.extend(terminal_spans(&tagged));
                        } else {
                            let text = value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string());
                            line.push(TerminalSpan {
                                text,
                                style: TerminalStyle::Plain,
                                tags: Vec::new(),
                            });
                        }
                    }
                    plain_lines.push(spans_text(&line));
                    lines.push(line);
                }
            }
            plain_lines.push(spans_text(&target));
            lines.push(target);
            rendered_goals.push(plain_lines.join("\n"));
            styled_goals.push(lines);
        }
        let rendered = rendered_goals.join("\n\n");
        Some(Self {
            stamp,
            rendered,
            goals: rendered_goals,
            styled_goals,
            interactive: true,
            term_goal: None,
            term_range: None,
            unavailable: None,
            completed: goals.is_empty(),
        })
    }

    pub fn with_term_goal(mut self, goal: PlainTermGoal) -> Self {
        self.term_goal = Some(goal.goal);
        self.term_range = Some(goal.range);
        self
    }

    pub fn display_text(&self) -> String {
        if let Some(reason) = &self.unavailable {
            return format!("Lean unavailable: {reason}");
        }
        if self.completed {
            return "🎉 Goal complete".to_owned();
        }
        let mut text = self.rendered.clone();
        if let Some(term) = &self.term_goal {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("term: ");
            text.push_str(term);
        }
        text
    }

    /// Lines for the native component. Keep layout out of Scheme so the same
    /// snapshot is rendered by the host and any future fallback transport.
    pub fn display_lines(&self) -> Vec<String> {
        if let Some(reason) = &self.unavailable {
            return vec![format!("Lean goal unavailable: {reason}")];
        }
        let mut lines = Vec::new();
        if self.completed {
            lines.push("🎉 Goal complete".to_owned());
        }
        for line in self.rendered.lines() {
            lines.push(line.to_owned());
        }
        if lines.is_empty() {
            lines.push("Lean goal has no rendered text".to_owned());
        }
        if let Some(term) = &self.term_goal {
            lines.push(format!("term: {term}"));
        }
        lines
    }

    pub fn styled_lines(&self, selected: usize) -> Vec<Vec<TerminalSpan>> {
        if self.completed {
            return vec![vec![TerminalSpan {
                text: "🎉 Goal complete".to_owned(),
                style: TerminalStyle::Goal,
                tags: Vec::new(),
            }]];
        }
        self.styled_goals
            .get(selected.min(self.styled_goals.len().saturating_sub(1)))
            .cloned()
            .unwrap_or_else(|| {
                self.display_lines()
                    .into_iter()
                    .map(|text| {
                        vec![TerminalSpan {
                            text,
                            style: TerminalStyle::Plain,
                            tags: Vec::new(),
                        }]
                    })
                    .collect()
            })
    }
}

fn spans_text(spans: &[TerminalSpan]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

#[derive(Clone, Debug)]
pub struct GoalState {
    active: Option<RequestStamp>,
    snapshot: Option<GoalSnapshot>,
    /// An empty goal response only proves that the goal at the exact cursor
    /// position is complete. Keep it pending until a later cursor position
    /// also has no goal; this prevents a solved bullet from masquerading as a
    /// completed theorem while sibling goals remain.
    completion_candidate: Option<RequestStamp>,
    /// A verified empty-goal response may be followed by one cursor refresh
    /// that reports no goal again. Keep the celebration through that refresh
    /// so it is visible instead of being replaced by an unavailable message.
    completed_stamp: Option<RequestStamp>,
    next_generation: u64,
}

impl Default for GoalState {
    fn default() -> Self {
        Self {
            active: None,
            snapshot: None,
            completion_candidate: None,
            completed_stamp: None,
            next_generation: 0,
        }
    }
}

impl GoalState {
    pub fn begin(
        &mut self,
        uri: impl Into<String>,
        version: i32,
        position: Position,
    ) -> RequestStamp {
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        let stamp = RequestStamp {
            uri: uri.into(),
            version,
            position,
            generation,
        };
        if self.completion_candidate.as_ref().is_some_and(|candidate| {
            candidate.uri != stamp.uri
                || candidate.version != stamp.version
                || !position_after(&stamp.position, &candidate.position)
        }) {
            self.completion_candidate = None;
        }
        if self.completed_stamp.as_ref().is_some_and(|completed| {
            completed.uri != stamp.uri
                || completed.version != stamp.version
                || !position_at_or_after(&stamp.position, &completed.position)
        }) {
            self.completed_stamp = None;
        }
        self.active = Some(stamp.clone());
        stamp
    }

    pub fn accept(&mut self, snapshot: GoalSnapshot) -> bool {
        if self.active.as_ref() != Some(&snapshot.stamp) {
            return false;
        }
        if snapshot.completed {
            if self.completion_candidate.as_ref().is_some_and(|candidate| {
                position_after(&snapshot.stamp.position, &candidate.position)
            }) {
                self.completion_candidate = None;
                self.completed_stamp = Some(snapshot.stamp.clone());
                self.snapshot = Some(snapshot);
            } else {
                self.completion_candidate = Some(snapshot.stamp.clone());
                self.snapshot = Some(GoalSnapshot::unavailable(snapshot.stamp, "no current goal"));
            }
            return true;
        }
        self.completion_candidate = None;
        self.completed_stamp = None;
        self.snapshot = Some(snapshot);
        true
    }

    pub fn accept_plain_goal(&mut self, generation: u64, goal: PlainGoal) -> bool {
        self.accept_plain_goal_with_completion(generation, goal, true)
    }

    pub fn accept_plain_goal_with_completion(
        &mut self,
        generation: u64,
        goal: PlainGoal,
        allow_completion: bool,
    ) -> bool {
        let Some(stamp) = self.active.clone() else {
            return false;
        };
        if stamp.generation != generation {
            return false;
        }
        if !allow_completion && goal.goals.is_empty() {
            self.completion_candidate = None;
            return self.accept_unavailable(generation, "Lean reported an error");
        }
        let snapshot = GoalSnapshot::from_plain_goal(stamp, goal);
        if allow_completion && snapshot.completed {
            return self.accept_completed(snapshot);
        }
        self.accept(snapshot)
    }

    pub fn invalidate(&mut self, reason: impl Into<String>) {
        if let Some(active) = self.active.clone() {
            self.snapshot = Some(GoalSnapshot::unavailable(active, reason));
            self.active = None;
        }
        self.completion_candidate = None;
        self.completed_stamp = None;
    }

    /// Record a successful request with no goal while keeping its stamp alive
    /// for the other cursor-scoped LSP replies (hover, signature, and inlay).
    /// A null plain-goal response is absence of goal content, not cancellation
    /// of the cursor request itself.
    pub fn accept_unavailable(&mut self, generation: u64, reason: impl Into<String>) -> bool {
        let Some(stamp) = self.active.clone() else {
            return false;
        };
        if stamp.generation != generation {
            return false;
        }
        self.snapshot = Some(GoalSnapshot::unavailable(stamp, reason));
        self.completed_stamp = None;
        true
    }

    pub fn accept_completed(&mut self, snapshot: GoalSnapshot) -> bool {
        if !snapshot.completed || self.active.as_ref() != Some(&snapshot.stamp) {
            return false;
        }
        self.completion_candidate = None;
        self.completed_stamp = Some(snapshot.stamp.clone());
        self.snapshot = Some(snapshot);
        true
    }

    pub fn clear_completion_candidate(&mut self) {
        self.completion_candidate = None;
    }

    /// A null plain-goal reply normally means that the cursor is outside a
    /// proof. Treat it as a completed-proof transition only after a preceding
    /// empty response was followed by a later cursor position with no goal.
    /// This avoids celebrating a solved bullet while sibling goals remain.
    pub fn accept_no_goal(&mut self, generation: u64, reason: impl Into<String>) -> bool {
        self.accept_no_goal_with_completion(generation, reason, true)
    }

    pub fn accept_no_goal_with_completion(
        &mut self,
        generation: u64,
        reason: impl Into<String>,
        allow_completion: bool,
    ) -> bool {
        let Some(stamp) = self.active.clone() else {
            return false;
        };
        if stamp.generation != generation {
            return false;
        }
        if !allow_completion {
            self.completion_candidate = None;
            return self.accept_unavailable(generation, reason);
        }
        if self.completed_stamp.as_ref().is_some_and(|completed| {
            completed.uri == stamp.uri
                && completed.version == stamp.version
                && position_at_or_after(&stamp.position, &completed.position)
        }) {
            self.completed_stamp = None;
            let accepted = self.accept_completed(GoalSnapshot::from_plain_goal(
                stamp,
                PlainGoal {
                    rendered: String::new(),
                    goals: Vec::new(),
                    extra: Default::default(),
                },
            ));
            self.completed_stamp = None;
            return accepted;
        }
        let completed = self.completion_candidate.as_ref().is_some_and(|candidate| {
            candidate.uri == stamp.uri
                && candidate.version == stamp.version
                && position_after(&stamp.position, &candidate.position)
        });
        if completed {
            self.accept(GoalSnapshot::from_plain_goal(
                stamp,
                PlainGoal {
                    rendered: String::new(),
                    goals: Vec::new(),
                    extra: Default::default(),
                },
            ))
        } else {
            self.accept_unavailable(generation, reason)
        }
    }

    pub fn invalidate_and_advance(&mut self, reason: impl Into<String>) {
        self.next_generation = self.next_generation.saturating_add(1);
        self.invalidate(reason);
    }

    pub fn close(&mut self) {
        self.active = None;
        self.snapshot = None;
        self.completion_candidate = None;
        self.completed_stamp = None;
    }

    pub fn snapshot(&self) -> Option<&GoalSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn active_stamp(&self) -> Option<&RequestStamp> {
        self.active.as_ref()
    }

    pub fn display_text(&self) -> String {
        self.snapshot.as_ref().map_or_else(
            || "Lean goal unavailable".to_owned(),
            GoalSnapshot::display_text,
        )
    }
}

fn position_after(left: &Position, right: &Position) -> bool {
    left.line > right.line || (left.line == right.line && left.character > right.character)
}

fn position_at_or_after(left: &Position, right: &Position) -> bool {
    left == right || position_after(left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Extras, PlainGoal, PlainTermGoal};
    use serde_json::json;

    fn position() -> Position {
        Position {
            line: 2,
            character: 4,
            extra: Extras::new(),
        }
    }

    #[test]
    fn stale_goal_is_rejected_after_cursor_or_edit_generation_changes() {
        let mut state = GoalState::default();
        let first = state.begin("file:///Main.lean", 1, position());
        let second = state.begin("file:///Main.lean", 2, position());
        let goal = GoalSnapshot::from_plain_goal(
            first,
            PlainGoal {
                rendered: "old".into(),
                goals: vec!["old".into()],
                extra: Extras::new(),
            },
        );
        assert!(!state.accept(goal));
        assert!(state.accept(GoalSnapshot::from_plain_goal(
            second,
            PlainGoal {
                rendered: "new".into(),
                goals: vec!["new".into()],
                extra: Extras::new()
            },
        )));
        assert_eq!(state.snapshot().unwrap().display_text(), "new");
    }

    #[test]
    fn term_goal_and_unavailable_state_render_without_losing_range() {
        let mut state = GoalState::default();
        let stamp = state.begin("file:///Main.lean", 1, position());
        let term = PlainTermGoal {
            goal: "Nat".into(),
            range: Range {
                start: position(),
                end: position(),
                extra: Extras::new(),
            },
            extra: Extras::new(),
        };
        let snapshot = GoalSnapshot::from_plain_goal(
            stamp.clone(),
            PlainGoal {
                rendered: "⊢ n = n".into(),
                goals: vec!["n = n".into()],
                extra: Extras::new(),
            },
        )
        .with_term_goal(term.clone());
        assert!(state.accept(snapshot));
        assert_eq!(state.snapshot().unwrap().term_range, Some(term.range));
        assert!(state
            .snapshot()
            .unwrap()
            .display_text()
            .contains("term: Nat"));
        state.invalidate("server restarting");
        assert_eq!(
            state.snapshot().unwrap().display_text(),
            "Lean unavailable: server restarting"
        );
        assert!(!state.accept(GoalSnapshot::from_plain_goal(
            stamp.clone(),
            PlainGoal {
                rendered: "late".into(),
                goals: vec!["late".into()],
                extra: Extras::new(),
            },
        )));
        state.close();
        assert!(state.snapshot().is_none());
        let reopened = state.begin("file:///Main.lean", 1, position());
        assert!(reopened.generation > stamp.generation);
    }

    #[test]
    fn interactive_goals_preserve_hypotheses_and_tag_styles() {
        let stamp = RequestStamp {
            uri: "file:///Main.lean".into(),
            version: 1,
            position: position(),
            generation: 1,
        };
        let value = json!({"goals":[{
            "goalPrefix":"⊢ ",
            "hyps":[{"names":["x"],"type":{"tag":[{"info":{"p":"0"},"subexprPos":"/"},{"text":"Nat"}]}}],
            "type":{"tag":[{"info":{"p":"1"},"subexprPos":"/0"},{"text":"x = x"}]}
        }]});
        let snapshot = GoalSnapshot::from_interactive_goals(stamp, &value).unwrap();
        assert_eq!(snapshot.goals.len(), 1);
        assert!(snapshot.goals[0].contains("x : Nat"));
        let lines = snapshot.styled_lines(0);
        assert_eq!(lines.len(), 2);
        assert!(lines[1]
            .iter()
            .any(|span| span.style == TerminalStyle::Goal));
        assert!(lines[0]
            .iter()
            .any(|span| span.style == TerminalStyle::Type));
    }

    #[test]
    fn completed_goal_renders_a_single_celebration_marker() {
        let stamp = RequestStamp {
            uri: "file:///Main.lean".into(),
            version: 1,
            position: position(),
            generation: 1,
        };
        let snapshot = GoalSnapshot::from_plain_goal(
            stamp,
            PlainGoal {
                rendered: String::new(),
                goals: Vec::new(),
                extra: Extras::new(),
            },
        );
        assert!(snapshot.completed);
        assert_eq!(snapshot.display_lines(), vec!["🎉 Goal complete"]);
        let lines = snapshot.styled_lines(0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0][0].text, "🎉 Goal complete");
        assert_eq!(lines[0][0].style, TerminalStyle::Goal);
    }

    #[test]
    fn solved_bullet_does_not_mark_theorem_complete() {
        let mut state = GoalState::default();
        let first = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 0,
                extra: Extras::new(),
            },
        );
        assert!(state.accept(GoalSnapshot::from_plain_goal(
            first,
            PlainGoal {
                rendered: "⊢ n = n".into(),
                goals: vec!["⊢ n = n".into()],
                extra: Extras::new(),
            },
        )));
        let second = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 3,
                extra: Extras::new(),
            },
        );
        assert!(state.accept_no_goal(second.generation, "Lean returned no goal"));
        assert!(!state.snapshot().unwrap().completed);
        assert_eq!(
            state.display_text(),
            "Lean unavailable: Lean returned no goal"
        );
    }

    #[test]
    fn diagnostic_rejects_empty_goal_completion() {
        let mut state = GoalState::default();
        let first = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 0,
                extra: Extras::new(),
            },
        );
        assert!(state.accept_plain_goal_with_completion(
            first.generation,
            PlainGoal {
                rendered: "⊢ p".into(),
                goals: vec!["⊢ p".into()],
                extra: Extras::new(),
            },
            true,
        ));
        let second = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 3,
                extra: Extras::new(),
            },
        );
        assert!(state.accept_plain_goal_with_completion(
            second.generation,
            PlainGoal {
                rendered: String::new(),
                goals: Vec::new(),
                extra: Extras::new(),
            },
            false,
        ));
        assert!(!state.display_text().contains("Goal complete"));
        assert!(state.display_text().contains("Lean unavailable"));
    }

    #[test]
    fn verified_completion_survives_followup_no_goal_refresh() {
        let mut state = GoalState::default();
        let solved = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 3,
                extra: Extras::new(),
            },
        );
        assert!(state.accept_plain_goal_with_completion(
            solved.generation,
            PlainGoal {
                rendered: String::new(),
                goals: Vec::new(),
                extra: Extras::new(),
            },
            true,
        ));
        assert_eq!(state.display_text(), "🎉 Goal complete");

        let refresh = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 3,
                extra: Extras::new(),
            },
        );
        assert!(state.accept_no_goal(refresh.generation, "Lean returned no goal"));
        assert_eq!(state.display_text(), "🎉 Goal complete");
    }

    #[test]
    fn later_no_goal_confirms_completion_after_empty_bullet() {
        let mut state = GoalState::default();
        let first = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 0,
                extra: Extras::new(),
            },
        );
        assert!(state.accept(GoalSnapshot::from_plain_goal(
            first,
            PlainGoal {
                rendered: "⊢ p".into(),
                goals: vec!["⊢ p".into()],
                extra: Extras::new(),
            },
        )));
        let solved = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 1,
                character: 3,
                extra: Extras::new(),
            },
        );
        assert!(state.accept(GoalSnapshot::from_plain_goal(
            solved,
            PlainGoal {
                rendered: "no goals".into(),
                goals: Vec::new(),
                extra: Extras::new(),
            },
        )));
        assert!(!state.snapshot().unwrap().completed);
        let after = state.begin(
            "file:///Main.lean",
            1,
            Position {
                line: 2,
                character: 0,
                extra: Extras::new(),
            },
        );
        assert!(state.accept_no_goal(after.generation, "Lean returned no goal"));
        assert!(state.snapshot().unwrap().completed);
        assert_eq!(state.display_text(), "🎉 Goal complete");
    }

    #[test]
    fn malformed_interactive_items_leave_other_goals_renderable() {
        let stamp = RequestStamp {
            uri: "file:///Main.lean".into(),
            version: 1,
            position: position(),
            generation: 1,
        };
        let value = json!({"goals":[
            7,
            {"goalPrefix":"⊢ ","hyps":[null],"type":{"tag":["bad"]}}
        ]});
        let snapshot = GoalSnapshot::from_interactive_goals(stamp, &value).unwrap();
        assert_eq!(snapshot.goals.len(), 2);
        assert!(snapshot.goals[0].contains("malformed interactive goal"));
        assert!(snapshot.goals[1].contains("malformed hypothesis"));
        assert!(snapshot.goals[1].contains("bad"));
    }
}
