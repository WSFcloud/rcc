use crate::common::scope::ScopedMap;
use std::collections::HashMap;

/// Generic semantic symbol-table container with separated namespaces.
///
/// Language-specific rules (linkage, compatibility, visibility by source order)
/// are implemented in sema modules. This type only manages scoped storage.
#[derive(Debug, Clone)]
pub struct SymbolTable<O, T, L> {
    ordinary: ScopedMap<String, O>,
    tags: ScopedMap<String, T>,
    labels: Option<HashMap<String, L>>,
}

impl<O, T, L> SymbolTable<O, T, L> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            ordinary: ScopedMap::new(),
            tags: ScopedMap::new(),
            labels: None,
        }
    }

    /// Enter a block scope for ordinary/tag namespaces.
    pub fn enter_scope(&mut self) {
        self.ordinary.push_scope();
        self.tags.push_scope();
    }

    /// Leave a block scope for ordinary/tag namespaces.
    ///
    /// Returns `false` when called on global scope.
    pub fn leave_scope(&mut self) -> bool {
        let ordinary_ok = self.ordinary.try_pop_scope().is_some();
        let tags_ok = self.tags.try_pop_scope().is_some();
        debug_assert_eq!(
            ordinary_ok, tags_ok,
            "namespace scopes should move together"
        );
        ordinary_ok && tags_ok
    }

    pub fn insert_ordinary(&mut self, name: String, id: O) -> Result<(), (String, O)> {
        self.ordinary.insert_unique(name, id)
    }

    #[must_use]
    pub fn get_ordinary(&self, name: &str) -> Option<&O> {
        self.ordinary.get(name)
    }

    pub fn insert_tag(&mut self, name: String, id: T) -> Result<(), (String, T)> {
        self.tags.insert_unique(name, id)
    }

    #[must_use]
    pub fn get_tag(&self, name: &str) -> Option<&T> {
        self.tags.get(name)
    }

    /// Start a fresh function-level label namespace.
    pub fn begin_function_labels(&mut self) {
        self.labels = Some(HashMap::new());
    }

    pub fn insert_label(&mut self, name: String, id: L) -> Result<(), (String, L)> {
        let labels = self.labels.get_or_insert_with(HashMap::new);
        if labels.contains_key(&name) {
            Err((name, id))
        } else {
            labels.insert(name, id);
            Ok(())
        }
    }

    #[must_use]
    pub fn get_label(&self, name: &str) -> Option<&L> {
        self.labels.as_ref().and_then(|labels| labels.get(name))
    }

    /// Finish label tracking for one function and return collected labels.
    ///
    /// Returns an empty map when labels were never started.
    pub fn end_function_labels(&mut self) -> HashMap<String, L> {
        self.labels.take().unwrap_or_default()
    }
}

impl<O, T, L> Default for SymbolTable<O, T, L> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::SymbolTable;

    #[test]
    fn ordinary_namespace_is_scoped() {
        let mut table = SymbolTable::<u32, u32, u32>::new();
        assert!(table.insert_ordinary("x".to_string(), 1).is_ok());
        assert_eq!(table.get_ordinary("x"), Some(&1));

        table.enter_scope();
        assert!(table.insert_ordinary("x".to_string(), 2).is_ok());
        assert_eq!(table.get_ordinary("x"), Some(&2));

        assert!(table.leave_scope());
        assert_eq!(table.get_ordinary("x"), Some(&1));
    }

    #[test]
    fn duplicate_in_same_scope_is_rejected() {
        let mut table = SymbolTable::<u32, u32, u32>::new();
        assert!(table.insert_ordinary("value".to_string(), 1).is_ok());
        assert!(table.insert_ordinary("value".to_string(), 2).is_err());
    }

    #[test]
    fn tags_use_independent_namespace() {
        let mut table = SymbolTable::<u32, u32, u32>::new();
        assert!(table.insert_ordinary("Node".to_string(), 1).is_ok());
        assert!(table.insert_tag("Node".to_string(), 9).is_ok());

        assert_eq!(table.get_ordinary("Node"), Some(&1));
        assert_eq!(table.get_tag("Node"), Some(&9));
    }

    #[test]
    fn labels_are_function_scoped() {
        let mut table = SymbolTable::<u32, u32, u32>::new();
        table.begin_function_labels();

        assert!(table.insert_label("entry".to_string(), 1).is_ok());
        assert!(table.insert_label("entry".to_string(), 2).is_err());
        assert_eq!(table.get_label("entry"), Some(&1));

        let labels = table.end_function_labels();
        assert_eq!(labels.len(), 1);
        assert!(table.get_label("entry").is_none());
    }

    #[test]
    fn leaving_global_scope_returns_false() {
        let mut table = SymbolTable::<u32, u32, u32>::new();
        assert!(!table.leave_scope());
    }
}
