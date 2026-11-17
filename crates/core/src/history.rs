//! Pile annuler/refaire générique, avec regroupement de la frappe continue.

/// Nature d'une modification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Frappe dans le texte : les frappes rapprochées forment une seule étape.
    Typing,
    /// Toute autre modification (catégorie, statut, champ…) : une étape chacune.
    Discrete,
}

/// Historique d'états `T` (des instantanés complets).
#[derive(Clone, Debug)]
pub struct History<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    last: Option<(EditKind, u64)>,
    capacity: usize,
    group_ms: u64,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self::new(500, 1000)
    }
}

impl<T> History<T> {
    /// `capacity` : nombre maximal d'étapes conservées ; `group_ms` : délai
    /// maximal entre deux frappes d'une même étape.
    pub fn new(capacity: usize, group_ms: u64) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            last: None,
            capacity: capacity.max(1),
            group_ms,
        }
    }

    /// À appeler **avant** d'appliquer une modification, avec l'état d'avant.
    /// `now_ms` est une horloge monotone quelconque en millisecondes.
    pub fn record(&mut self, before: T, kind: EditKind, now_ms: u64) {
        self.redo.clear();
        let coalesce = matches!(
            (kind, self.last),
            (EditKind::Typing, Some((EditKind::Typing, t))) if now_ms.saturating_sub(t) <= self.group_ms
        );
        self.last = Some((kind, now_ms));
        if coalesce {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > self.capacity {
            let excess = self.undo.len() - self.capacity;
            self.undo.drain(..excess);
        }
    }

    /// Termine l'étape en cours : la prochaine frappe ouvrira une nouvelle étape.
    pub fn break_group(&mut self) {
        self.last = None;
    }

    /// Renvoie l'état à restaurer, et mémorise `current` pour refaire.
    pub fn undo(&mut self, current: T) -> Option<T> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        self.last = None;
        Some(prev)
    }

    /// Renvoie l'état à restaurer, et mémorise `current` pour annuler.
    pub fn redo(&mut self, current: T) -> Option<T> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        self.last = None;
        Some(next)
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

    #[test]
    fn typing_is_grouped_within_delay() {
        let mut h = History::new(100, 1000);
        h.record("", EditKind::Typing, 0);
        h.record("a", EditKind::Typing, 300);
        h.record("ab", EditKind::Typing, 900);
        // Pause de plus d'une seconde : nouvelle étape.
        h.record("abc", EditKind::Typing, 3000);
        assert_eq!(h.undo("abcd"), Some("abc"));
        assert_eq!(h.undo("abc"), Some(""));
        assert_eq!(h.undo(""), None);
    }

    #[test]
    fn discrete_edits_are_separate_steps() {
        let mut h = History::new(100, 1000);
        h.record(1, EditKind::Discrete, 0);
        h.record(2, EditKind::Discrete, 10);
        assert_eq!(h.undo(3), Some(2));
        assert_eq!(h.undo(2), Some(1));
    }

    #[test]
    fn redo_restores_and_is_cleared_by_new_edit() {
        let mut h = History::new(100, 1000);
        h.record(1, EditKind::Discrete, 0);
        assert_eq!(h.undo(2), Some(1));
        assert!(h.can_redo());
        assert_eq!(h.redo(1), Some(2));
        assert_eq!(h.undo(2), Some(1));
        h.record(1, EditKind::Discrete, 50);
        assert!(!h.can_redo());
    }

    #[test]
    fn undo_breaks_typing_group() {
        let mut h = History::new(100, 1000);
        h.record("", EditKind::Typing, 0);
        assert_eq!(h.undo("a"), Some(""));
        h.record("", EditKind::Typing, 10);
        assert!(h.can_undo());
    }

    #[test]
    fn capacity_drops_oldest() {
        let mut h = History::new(2, 0);
        for i in 0..5 {
            h.record(i, EditKind::Discrete, i as u64);
        }
        assert_eq!(h.undo(5), Some(4));
        assert_eq!(h.undo(4), Some(3));
        assert_eq!(h.undo(3), None);
    }
}
