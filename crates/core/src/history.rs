//! Deshacer / rehacer mediante instantáneas del timeline.
//!
//! El timeline es pequeño (solo metadatos), así que guardar copias completas es
//! simple, robusto y suficientemente barato.

use crate::Timeline;

const LIMIT: usize = 200;

#[derive(Default)]
pub struct History {
    undo: Vec<Timeline>,
    redo: Vec<Timeline>,
}

impl History {
    /// Guarda el estado *antes* de una edición.
    pub fn checkpoint(&mut self, before: &Timeline) {
        self.undo.push(before.clone());
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: &mut Timeline) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(current, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, current: &mut Timeline) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(current, next));
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}
