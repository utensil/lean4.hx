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
        }
    }

    pub fn from_plain_goal(stamp: RequestStamp, goal: PlainGoal) -> Self {
        let styled_goals = goal
            .goals
            .iter()
            .map(|goal| {
                goal.lines()
                    .map(|line| {
                        vec![TerminalSpan {
                            text: line.to_owned(),
                            style: TerminalStyle::Plain,
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
            let object = goal.as_object()?;
            let prefix = object
                .get("goalPrefix")
                .and_then(Value::as_str)
                .unwrap_or("⊢ ");
            let mut lines = Vec::new();
            let mut plain_lines = Vec::new();
            let mut target = vec![TerminalSpan {
                text: prefix.to_owned(),
                style: TerminalStyle::Goal,
            }];
            if let Some(value) = object.get("type") {
                if let Ok(tagged) = serde_json::from_value::<TaggedText>(value.clone()) {
                    target.extend(terminal_spans(&tagged));
                } else if let Some(text) = value.as_str() {
                    target.push(TerminalSpan {
                        text: text.to_owned(),
                        style: TerminalStyle::Plain,
                    });
                }
            }
            if let Some(hyps) = object.get("hyps").and_then(Value::as_array) {
                for hyp in hyps {
                    let hyp_object = hyp.as_object()?;
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
                    }];
                    if let Some(value) = hyp_object.get("type") {
                        if let Ok(tagged) = serde_json::from_value::<TaggedText>(value.clone()) {
                            line.extend(terminal_spans(&tagged));
                        } else if let Some(text) = value.as_str() {
                            line.push(TerminalSpan {
                                text: text.to_owned(),
                                style: TerminalStyle::Plain,
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
    next_generation: u64,
}

impl Default for GoalState {
    fn default() -> Self {
        Self {
            active: None,
            snapshot: None,
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
        self.active = Some(stamp.clone());
        stamp
    }

    pub fn accept(&mut self, snapshot: GoalSnapshot) -> bool {
        if self.active.as_ref() != Some(&snapshot.stamp) {
            return false;
        }
        self.snapshot = Some(snapshot);
        true
    }

    pub fn accept_plain_goal(&mut self, generation: u64, goal: PlainGoal) -> bool {
        let Some(stamp) = self.active.clone() else {
            return false;
        };
        if stamp.generation != generation {
            return false;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.stamp == stamp && snapshot.interactive)
        {
            return true;
        }
        self.accept(GoalSnapshot::from_plain_goal(stamp, goal))
    }

    pub fn invalidate(&mut self, reason: impl Into<String>) {
        if let Some(active) = self.active.clone() {
            self.snapshot = Some(GoalSnapshot::unavailable(active, reason));
            self.active = None;
        }
    }

    pub fn invalidate_and_advance(&mut self, reason: impl Into<String>) {
        self.next_generation = self.next_generation.saturating_add(1);
        self.invalidate(reason);
    }

    pub fn close(&mut self) {
        self.active = None;
        self.snapshot = None;
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
}
