use std::{borrow::Borrow, collections::HashMap, hash::Hash};

/// A stack of hash maps representing nested lexical scopes.
///
/// Scope `0` is the outermost (global) scope and is always present.
#[derive(Clone, Debug)]
pub struct ScopedMap<K, V> {
    scopes: Vec<HashMap<K, V>>,
}

impl<K, V> ScopedMap<K, V>
where
    K: Eq + Hash,
{
    #[must_use]
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Try to pop the innermost scope.
    ///
    /// Returns `None` when the caller attempts to pop the global scope.
    pub fn try_pop_scope(&mut self) -> Option<HashMap<K, V>> {
        if self.scopes.len() > 1 {
            self.scopes.pop()
        } else {
            None
        }
    }

    /// Pop the innermost scope.
    ///
    /// Panics when called at global scope.
    pub fn pop_scope(&mut self) -> HashMap<K, V> {
        self.try_pop_scope()
            .expect("attempted to pop global scope in ScopedMap::pop_scope")
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.scopes
            .last_mut()
            .expect("ScopedMap must always contain a global scope")
            .insert(key, value)
    }

    /// Insert only when the key does not exist in the current scope.
    ///
    /// This is useful for "same-scope duplicate definition" checks.
    pub fn insert_unique(&mut self, key: K, value: V) -> Result<(), (K, V)> {
        if self.contains_in_current_scope(&key) {
            Err((key, value))
        } else {
            let _ = self.insert(key, value);
            Ok(())
        }
    }

    /// Push an existing scope map as the new innermost scope.
    ///
    /// This is intended for state rewind/restore workflows.
    pub fn push_scope_map(&mut self, scope: HashMap<K, V>) {
        self.scopes.push(scope);
    }

    /// Remove a key only from the innermost scope.
    pub fn remove_in_current_scope<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.scopes
            .last_mut()
            .expect("ScopedMap must always contain a global scope")
            .remove(key)
    }

    /// Lookup from inner scope to outer scope.
    #[must_use]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(key) {
                return Some(value);
            }
        }
        None
    }

    /// Lookup only in the innermost scope.
    #[must_use]
    pub fn get_in_current_scope<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.scopes.last().and_then(|scope| scope.get(key))
    }

    #[must_use]
    pub fn contains_in_current_scope<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.get_in_current_scope(key).is_some()
    }

    /// Borrow the innermost scope map.
    #[must_use]
    pub fn current_scope(&self) -> &HashMap<K, V> {
        self.scopes
            .last()
            .expect("ScopedMap must always contain a global scope")
    }

    /// Iterate all bindings in the innermost scope.
    pub fn iter_current_scope(&self) -> impl Iterator<Item = (&K, &V)> {
        self.current_scope().iter()
    }

    #[must_use]
    pub fn scope_depth(&self) -> usize {
        self.scopes.len()
    }
}

impl<K, V> Default for ScopedMap<K, V>
where
    K: Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ScopedMap;
    use std::collections::HashMap;

    #[test]
    fn inner_scope_shadows_outer_scope() {
        let mut map = ScopedMap::<String, i32>::default();
        let _ = map.insert("x".to_string(), 1);
        assert_eq!(map.get("x"), Some(&1));

        map.push_scope();
        let _ = map.insert("x".to_string(), 2);
        assert_eq!(map.get("x"), Some(&2));

        let popped = map.try_pop_scope().expect("inner scope should pop");
        assert_eq!(popped.get("x"), Some(&2));
        assert_eq!(map.get("x"), Some(&1));
    }

    #[test]
    fn cannot_pop_global_scope() {
        let mut map = ScopedMap::<String, i32>::default();
        assert!(map.try_pop_scope().is_none());
        assert_eq!(map.scope_depth(), 1);
    }

    #[test]
    #[should_panic(expected = "attempted to pop global scope")]
    fn pop_scope_panics_at_global_scope() {
        let mut map = ScopedMap::<String, i32>::default();
        let _ = map.pop_scope();
    }

    #[test]
    fn insert_unique_checks_current_scope_only() {
        let mut map = ScopedMap::<String, i32>::default();
        assert!(map.insert_unique("x".to_string(), 1).is_ok());
        assert!(map.insert_unique("x".to_string(), 2).is_err());

        map.push_scope();
        assert!(map.insert_unique("x".to_string(), 3).is_ok());
        assert_eq!(map.get("x"), Some(&3));
    }

    #[test]
    fn iter_current_scope_traverses_innermost_bindings() {
        let mut map = ScopedMap::<String, i32>::default();
        let _ = map.insert("global".to_string(), 1);
        map.push_scope();
        let _ = map.insert("local_a".to_string(), 2);
        let _ = map.insert("local_b".to_string(), 3);

        let mut keys = map
            .iter_current_scope()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, vec!["local_a", "local_b"]);
    }

    #[test]
    fn push_scope_map_and_remove_in_current_scope_work() {
        let mut map = ScopedMap::<String, i32>::default();
        let _ = map.insert("x".to_string(), 1);

        let mut restored = HashMap::new();
        restored.insert("y".to_string(), 2);
        map.push_scope_map(restored);

        assert_eq!(map.get("y"), Some(&2));
        assert_eq!(map.remove_in_current_scope("y"), Some(2));
        assert_eq!(map.get("y"), None);
        assert_eq!(map.get("x"), Some(&1));
    }
}
