//! Undo / redo as a stack of reversible actions.

use crate::document::Document;
use crate::stroke::Stroke;

#[derive(Clone, Debug)]
pub enum Action {
    Add(Vec<Stroke>),
    Remove(Vec<Stroke>),
    /// Strokes replaced by others (e.g. cut by the eraser).
    Replace { removed: Vec<Stroke>, added: Vec<Stroke> },
}

impl Action {
    fn apply(&self, doc: &mut Document) {
        match self {
            Action::Add(v) => v.iter().for_each(|s| doc.insert(s.clone())),
            Action::Remove(v) => v.iter().for_each(|s| {
                doc.remove(s.id);
            }),
            Action::Replace { removed, added } => {
                removed.iter().for_each(|s| {
                    doc.remove(s.id);
                });
                added.iter().for_each(|s| doc.insert(s.clone()));
            }
        }
    }

    fn inverse(&self) -> Action {
        match self {
            Action::Add(v) => Action::Remove(v.clone()),
            Action::Remove(v) => Action::Add(v.clone()),
            Action::Replace { removed, added } => Action::Replace { removed: added.clone(), added: removed.clone() },
        }
    }
}

const MAX_UNDO: usize = 1000;

#[derive(Default, Debug)]
pub struct History {
    undo: Vec<Action>,
    redo: Vec<Action>,
}

impl History {
    /// Applies `a` to the document and records it.
    pub fn apply(&mut self, doc: &mut Document, a: Action) {
        a.apply(doc);
        self.record(a);
    }

    /// Records an action that has already been applied.
    pub fn record(&mut self, a: Action) {
        self.undo.push(a);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, doc: &mut Document) -> bool {
        let Some(a) = self.undo.pop() else { return false };
        a.inverse().apply(doc);
        self.redo.push(a);
        true
    }

    pub fn redo(&mut self, doc: &mut Document) -> bool {
        let Some(a) = self.redo.pop() else { return false };
        a.apply(doc);
        self.undo.push(a);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::StrokePoint;
    use num_bigint::BigInt;

    fn s(id: u64) -> Stroke {
        let mut s = Stroke::new(id, 0, [BigInt::from(0), BigInt::from(0)], [0; 4], 1.0);
        s.push(StrokePoint { x: 0.0, y: 0.0, pressure: 1.0 });
        s
    }

    #[test]
    fn undo_redo() {
        let mut d = Document::new();
        let mut h = History::default();
        h.apply(&mut d, Action::Add(vec![s(1)]));
        h.apply(&mut d, Action::Add(vec![s(2)]));
        h.apply(&mut d, Action::Remove(vec![s(1)]));
        assert_eq!(d.len(), 1);
        assert!(h.undo(&mut d));
        assert_eq!(d.len(), 2);
        assert!(h.undo(&mut d));
        assert!(h.undo(&mut d));
        assert!(!h.undo(&mut d));
        assert!(d.is_empty());
        assert!(h.redo(&mut d) && h.redo(&mut d));
        assert_eq!(d.len(), 2);
        h.apply(&mut d, Action::Add(vec![s(3)]));
        assert!(!h.can_redo());
    }

    #[test]
    fn replace_roundtrip() {
        let mut d = Document::new();
        let mut h = History::default();
        h.apply(&mut d, Action::Add(vec![s(1)]));
        h.apply(&mut d, Action::Replace { removed: vec![s(1)], added: vec![s(2), s(3)] });
        assert!(d.get(1).is_none() && d.get(2).is_some() && d.get(3).is_some());
        assert!(h.undo(&mut d));
        assert!(d.get(1).is_some() && d.get(2).is_none() && d.get(3).is_none());
        assert!(h.redo(&mut d));
        assert_eq!(d.len(), 2);
    }
}
