use crate::common::scope::ScopedMap;
use chumsky::{
    input::{Checkpoint, Cursor, Input},
    inspector::Inspector,
};
use std::collections::HashMap;

/// Binding category used by parser-time name disambiguation.
///
/// In C, `typedef` names and ordinary identifiers share the same namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    Typedef,
    Ordinary,
}

/// Debug/event log for scope transitions and declarations.
///
/// This is primarily kept for parser tests and inspection.
#[derive(Clone, Debug)]
pub enum ScopeEntry {
    ScopeStart,
    ScopeEnd,
    Bind { name: String, kind: BindingKind },
}

/// Reverse operation log used to rewind `ScopedMap` without full state cloning.
#[derive(Clone, Debug)]
enum UndoEntry {
    PopScope,
    PushScope(HashMap<String, BindingKind>),
    RestoreBinding {
        name: String,
        previous: Option<BindingKind>,
    },
}

/// Lightweight snapshot marker for parser backtracking.
///
/// `on_save` stores only log lengths (O(1)); `on_rewind` replays undo entries.
#[derive(Clone, Copy, Debug)]
pub struct TypedefCheckpoint {
    undo_len: usize,
    entry_len: usize,
}

/// Parser-side typedef visibility state.
///
/// Maintains:
/// - `scopes`: fast visible-name lookup from innermost scope outward
/// - `entries`: human-readable event history
/// - `undos`: reversible operations for cheap rewind
#[derive(Clone, Default, Debug)]
pub struct Typedefs {
    scopes: ScopedMap<String, BindingKind>,
    entries: Vec<ScopeEntry>,
    undos: Vec<UndoEntry>,
}

impl Typedefs {
    /// Enter a new nested scope.
    pub fn push_scope(&mut self) {
        self.scopes.push_scope();
        self.entries.push(ScopeEntry::ScopeStart);
        self.undos.push(UndoEntry::PopScope);
    }

    /// Exit the innermost scope.
    ///
    /// Panics if called at global scope (compiler internal logic error).
    pub fn pop_scope(&mut self) {
        let popped = self.scopes.pop_scope();
        self.entries.push(ScopeEntry::ScopeEnd);
        self.undos.push(UndoEntry::PushScope(popped));
    }

    /// Bind `name` in the current scope.
    ///
    /// Returns `true` when a new binding was inserted. If the name already exists
    /// in the current scope, the original binding is kept and this returns `false`.
    pub fn bind(&mut self, name: String, kind: BindingKind) -> bool {
        if self.scopes.insert_unique(name.clone(), kind).is_err() {
            return false;
        }

        self.entries.push(ScopeEntry::Bind {
            name: name.clone(),
            kind,
        });
        self.undos.push(UndoEntry::RestoreBinding {
            name,
            previous: None,
        });
        true
    }

    /// Check whether a visible binding of `name` is currently a typedef.
    pub fn is_typedef_name(&self, name: &str) -> bool {
        matches!(
            self.resolve_visible_binding(name),
            Some(BindingKind::Typedef)
        )
    }

    /// Check whether `name` is a typedef in the current (innermost) scope.
    pub fn is_typedef_name_in_current_scope(&self, name: &str) -> bool {
        matches!(
            self.scopes.get_in_current_scope(name),
            Some(BindingKind::Typedef)
        )
    }

    /// Get the binding kind of `name` in the current (innermost) scope.
    pub fn binding_in_current_scope(&self, name: &str) -> Option<BindingKind> {
        self.scopes.get_in_current_scope(name).copied()
    }

    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[ScopeEntry] {
        &self.entries
    }

    fn resolve_visible_binding(&self, name: &str) -> Option<BindingKind> {
        self.scopes.get(name).copied()
    }

    /// Create an O(1) checkpoint for parser save points.
    fn checkpoint(&self) -> TypedefCheckpoint {
        TypedefCheckpoint {
            undo_len: self.undos.len(),
            entry_len: self.entries.len(),
        }
    }

    /// Roll state back to a previously captured checkpoint.
    fn rewind_to_checkpoint(&mut self, target: TypedefCheckpoint) {
        while self.undos.len() > target.undo_len {
            let undo = self
                .undos
                .pop()
                .expect("undo log length should be >= checkpoint");
            self.apply_undo(undo);
        }
        self.entries.truncate(target.entry_len);
    }

    /// Apply one reverse operation from the undo log.
    fn apply_undo(&mut self, undo: UndoEntry) {
        match undo {
            UndoEntry::PopScope => {
                let _ = self.scopes.pop_scope();
            }
            UndoEntry::PushScope(scope) => {
                self.scopes.push_scope_map(scope);
            }
            UndoEntry::RestoreBinding { name, previous } => {
                if let Some(kind) = previous {
                    let _ = self.scopes.insert(name, kind);
                } else {
                    let _ = self.scopes.remove_in_current_scope(&name);
                }
            }
        }
    }
}

impl<'src, I> Inspector<'src, I> for Typedefs
where
    I: Input<'src>,
{
    /// Save-point payload used by chumsky for backtracking.
    type Checkpoint = TypedefCheckpoint;

    fn on_token(&mut self, _: &I::Token) {}

    /// Save only checkpoint markers (O(1)).
    fn on_save<'parse>(&self, _: &Cursor<'src, 'parse, I>) -> Self::Checkpoint {
        self.checkpoint()
    }

    /// Rewind by undo replay and entry truncation.
    fn on_rewind<'parse>(&mut self, checkpoint: &Checkpoint<'src, 'parse, I, Self::Checkpoint>) {
        self.rewind_to_checkpoint(*checkpoint.inspector());
    }
}

#[cfg(test)]
mod tests {
    use super::{BindingKind, Typedefs};

    #[test]
    fn nearest_binding_wins_across_scopes() {
        let mut state = Typedefs::default();
        state.bind("T".to_string(), BindingKind::Typedef);
        assert!(state.is_typedef_name("T"));

        state.push_scope();
        state.bind("T".to_string(), BindingKind::Ordinary);
        assert!(!state.is_typedef_name("T"));

        state.pop_scope();
        assert!(state.is_typedef_name("T"));
    }

    #[test]
    fn rewind_restores_bindings_without_full_clone() {
        let mut state = Typedefs::default();
        state.bind("T".to_string(), BindingKind::Typedef);
        let checkpoint = state.checkpoint();

        state.push_scope();
        state.bind("T".to_string(), BindingKind::Ordinary);
        state.bind("X".to_string(), BindingKind::Typedef);
        assert!(!state.is_typedef_name("T"));
        assert!(state.is_typedef_name("X"));

        state.rewind_to_checkpoint(checkpoint);

        assert!(state.is_typedef_name("T"));
        assert!(!state.is_typedef_name("X"));
    }

    #[test]
    fn same_scope_duplicate_bind_keeps_original_binding() {
        let mut state = Typedefs::default();
        assert!(state.bind("T".to_string(), BindingKind::Typedef));
        assert!(!state.bind("T".to_string(), BindingKind::Ordinary));
        assert!(state.is_typedef_name("T"));
    }
}
