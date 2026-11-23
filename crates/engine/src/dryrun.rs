//! Trial runs on a copy: a checkpoint of the active drawing that can be put back, so a
//! procedure the assistant proposed can be run, looked at and taken back with nothing left behind.
//!
//! The drawing is shared copy-on-write, so a checkpoint is cheap. What it restores is the
//! drawing, the selection and the undo and redo steps; drawing-wide settings a procedure changed
//! (system variables, the view) are not part of it.

use crate::doc_state::Snapshot;
use crate::{Arc, Drawing, EngineError, Handle, Result, Session};

/// The active drawing as it was when [`Session::checkpoint`] was taken.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    uid: u64,
    doc: Arc<Drawing>,
    selection: Vec<Handle>,
    previous_selection: Vec<Handle>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    entities: usize,
}

impl Checkpoint {
    /// How many objects the drawing had then.
    pub fn entities(&self) -> usize {
        self.entities
    }
}

impl Session {
    /// Remember the active drawing. The Block Editor holds a drawing of its own, so there it is refused.
    pub fn checkpoint(&self) -> Result<Checkpoint> {
        let st = self.state()?;
        if st.block_edit.is_some() {
            return Err(EngineError::Other("close the Block Editor before a trial run".into()));
        }
        Ok(Checkpoint {
            uid: st.uid,
            doc: st.doc.clone(),
            selection: st.selection.clone(),
            previous_selection: st.previous_selection.clone(),
            undo: st.undo.clone(),
            redo: st.redo.clone(),
            entities: st.doc.entity_count(),
        })
    }

    /// Put the drawing back as the checkpoint had it. False when that drawing is no longer open.
    pub fn restore(&mut self, c: &Checkpoint) -> bool {
        if self.running.is_some() {
            self.cancel();
        }
        let Some(st) = self.docs.iter_mut().find(|d| d.uid == c.uid) else { return false };
        st.doc = c.doc.clone();
        st.selection = c.selection.clone();
        st.previous_selection = c.previous_selection.clone();
        st.undo = c.undo.clone();
        st.redo = c.redo.clone();
        st.revision += 1;
        true
    }

    /// How many objects the active drawing has now.
    pub fn entity_count(&self) -> usize {
        self.doc().map_or(0, |d| d.entity_count())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn restore_takes_back_what_ran_after_the_checkpoint() {
        let mut s = Session::new();
        s.execute("circle", &json!({"center": [0.0, 0.0], "radius": 1.0})).unwrap();
        let c = s.checkpoint().unwrap();
        assert_eq!(c.entities(), 1);
        let undo_len = s.state().unwrap().undo.len();
        s.execute("circle", &json!({"center": [5.0, 5.0], "radius": 2.0})).unwrap();
        s.execute("circle", &json!({"center": [9.0, 9.0], "radius": 2.0})).unwrap();
        assert_eq!(s.entity_count(), 3);
        assert!(s.restore(&c));
        assert_eq!(s.entity_count(), 1);
        assert_eq!(s.state().unwrap().undo.len(), undo_len);
    }

    #[test]
    fn restore_cancels_a_command_in_progress() {
        let mut s = Session::new();
        let c = s.checkpoint().unwrap();
        s.start("line").unwrap();
        assert!(s.running.is_some());
        assert!(s.restore(&c));
        assert!(s.running.is_none());
    }
}
