//! Small, versioned goal snapshots shared by the native component and tests.

use crate::protocol::{PlainGoal, PlainTermGoal, Position, Range};

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
            term_goal: None,
            term_range: None,
            unavailable: Some(reason.into()),
        }
    }

    pub fn from_plain_goal(stamp: RequestStamp, goal: PlainGoal) -> Self {
        Self {
            stamp,
            rendered: goal.rendered,
            goals: goal.goals,
            term_goal: None,
            term_range: None,
            unavailable: None,
        }
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
}

#[derive(Clone, Debug, Default)]
pub struct GoalState {
    active: Option<RequestStamp>,
    snapshot: Option<GoalSnapshot>,
}

impl GoalState {
    pub fn begin(
        &mut self,
        uri: impl Into<String>,
        version: i32,
        position: Position,
    ) -> RequestStamp {
        let generation = self
            .active
            .as_ref()
            .map_or(1, |stamp| stamp.generation.saturating_add(1));
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

    pub fn invalidate(&mut self, reason: impl Into<String>) {
        if let Some(active) = self.active.clone() {
            self.snapshot = Some(GoalSnapshot::unavailable(active, reason));
        }
    }

    pub fn close(&mut self) {
        self.active = None;
        self.snapshot = None;
    }

    pub fn snapshot(&self) -> Option<&GoalSnapshot> {
        self.snapshot.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Extras, PlainGoal, PlainTermGoal};

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
            stamp,
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
        state.close();
        assert!(state.snapshot().is_none());
    }
}
