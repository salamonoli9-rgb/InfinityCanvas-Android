//! Undo / redo as a stack of reversible actions.

use crate::document::Document;
use crate::stroke::Stroke;

#[derive(Clone, Debug)]
pub enum Action {
    Add(Vec<Stroke>),
    Remove(Vec<Stroke>),
}

impl Action {
    fn apply(&self, doc: &mut Document) {
        match self {
            Action::Add(v) => v.iter().for_each(|s| doc.insert(s.clone())),
            Action::Remove(v) => v.iter().for_each(|s| {
                doc.remove(s.id);
            }),
        }
    }

    fn inverse(&self) -> Action {
        match self {
            Action::Add(v) => Action::Remove(v.clone()),
            Action::Remove(v) => Action::Add(v.clone()),
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
}
