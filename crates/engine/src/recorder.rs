//! Macro recorder: what the person did, as lines of text the AI assistant can read.
//!
//! Only what the person does at the top level is recorded: a command typed or chosen, each input
//! it received (point, keyword, text, objects picked), and commands run with JSON parameters.
//! Whatever those start inside the engine (a LISP routine's `(command ...)`, a command that runs
//! another) belongs to the one that started it and is not recorded again, so a recording replays
//! the person's actions and not their echoes.
//!
//! A line is `command NAME`, `point X,Y`, `keyword K`, `text T`, `pick H1 H2`, `enter`, `cancel`
//! or `run ID {json}`; a failed one ends with `  ; error: why`.

use crate::Session;
use crate::prompt::Input;

/// The most lines a recording keeps; further actions are counted but not stored.
pub const MAX_LINES: usize = 5000;

/// A recording in progress.
#[derive(Debug, Default)]
pub struct Recorder {
    /// What was recorded so far, oldest first.
    pub lines: Vec<String>,
    /// Actions that did not fit in [`MAX_LINES`].
    pub dropped: usize,
    /// How deep inside an action the engine is: only depth 0 is the person's own.
    depth: u32,
}

impl Recorder {
    fn push(&mut self, line: String) {
        if self.lines.len() < MAX_LINES {
            self.lines.push(line);
        } else {
            self.dropped += 1;
        }
    }
}

/// An input as a recording line.
pub fn describe_input(input: &Input) -> String {
    match input {
        Input::Point(p) => format!("point {},{}", p.x, p.y),
        Input::Deferred(d) => format!("deferred {d:?}"),
        Input::Keyword(k) => format!("keyword {k}"),
        Input::Text(t) => format!("text {t}"),
        Input::Pick(hs) => {
            let list: Vec<String> = hs.iter().map(|h| format!("{h:?}")).collect();
            format!("pick {}", list.join(" "))
        }
        Input::Enter => "enter".to_string(),
        Input::Cancel => "cancel".to_string(),
    }
}

impl Session {
    /// Begin a recording, dropping any earlier one.
    pub fn start_recording(&mut self) {
        self.recorder = Some(Recorder::default());
    }

    /// End the recording and hand back its lines (empty when none was running).
    pub fn stop_recording(&mut self) -> Vec<String> {
        let Some(r) = self.recorder.take() else { return Vec::new() };
        let mut lines = r.lines;
        if r.dropped > 0 {
            lines.push(format!("; {} more actions were not kept", r.dropped));
        }
        lines
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// How many lines the running recording holds.
    pub fn recorded_lines(&self) -> usize {
        self.recorder.as_ref().map_or(0, |r| r.lines.len())
    }

    /// Enter an action: true when it is the person's own (to be recorded when it ends).
    pub(crate) fn rec_enter(&mut self) -> bool {
        let Some(r) = self.recorder.as_mut() else { return false };
        r.depth += 1;
        r.depth == 1
    }

    /// Leave an action; `line` is what to record for it when it was the person's own.
    pub(crate) fn rec_leave(&mut self, top: bool, line: impl FnOnce() -> String, error: Option<String>) {
        let Some(r) = self.recorder.as_mut() else { return };
        r.depth = r.depth.saturating_sub(1);
        if top {
            let mut line = line();
            if let Some(e) = error {
                line.push_str("  ; error: ");
                line.push_str(&e.replace('\n', " "));
            }
            r.push(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Vec2;
    use serde_json::json;

    #[test]
    fn records_the_persons_actions_and_not_what_they_start() {
        let mut s = Session::new();
        assert!(!s.is_recording());
        s.start_recording();
        s.start("line").unwrap();
        s.input(Input::Point(Vec2::new(0.0, 0.0))).unwrap();
        s.input(Input::Point(Vec2::new(10.0, 5.0))).unwrap();
        s.input(Input::Enter).unwrap();
        s.execute("circle", &json!({"center": [1.0, 1.0], "radius": 2.0})).unwrap();
        let lines = s.stop_recording();
        assert!(!s.is_recording());
        assert_eq!(lines[0], "command line");
        assert_eq!(&lines[1..4], ["point 0,0", "point 10,5", "enter"]);
        assert!(lines[4].starts_with("run circle {"), "{lines:?}");
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn a_lisp_routine_is_one_action() {
        let mut s = Session::new();
        s.start_recording();
        s.execute("lisp", &json!({"code": "(command \"LINE\" \"0,0\" \"3,4\" \"\")"})).unwrap();
        let lines = s.stop_recording();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("run lisp "));
    }

    #[test]
    fn failures_are_marked_and_nothing_is_kept_when_not_recording() {
        let mut s = Session::new();
        let _ = s.execute("no-such-command", &json!({}));
        assert!(s.stop_recording().is_empty());
        s.start_recording();
        let _ = s.execute("no-such-command", &json!({}));
        let lines = s.stop_recording();
        assert!(lines[0].contains("; error:"), "{lines:?}");
    }
}
