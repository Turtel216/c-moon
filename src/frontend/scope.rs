//! A lexically scoped name table shared by the frontend's resolution passes.
//!
//! Both the semantic analyzer (which maps names to types) and the renamer
//! (which maps names to unique variable ids) need exactly the same structure:
//! a stack of scopes where the innermost one shadows the ones below it. The
//! only difference is what each stores per name, so the container is generic
//! over that value.

use std::collections::HashMap;

/// A stack of lexical scopes mapping identifiers to `T`.
///
/// The last element of `scopes` is the innermost scope. A fresh table already
/// contains one scope -- the global one -- so callers never have to remember
/// to open it.
#[derive(Debug, Clone)]
pub struct ScopeStack<T> {
    scopes: Vec<HashMap<String, T>>,
}

impl<T> Default for ScopeStack<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ScopeStack<T> {
    /// Creates a table holding a single, empty global scope.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    /// Opens a nested scope. Every declaration made from now on shadows the
    /// enclosing scopes until [`ScopeStack::pop_scope`] is called.
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Closes the innermost scope, discarding its declarations.
    ///
    /// # Panics
    ///
    /// Panics if the global scope has already been popped, which would mean a
    /// pass opened and closed scopes in an unbalanced way -- a compiler bug.
    pub fn pop_scope(&mut self) {
        self.scopes.pop().expect("scope stack underflow");
        debug_assert!(!self.scopes.is_empty(), "global scope must stay open");
    }

    /// Declares `name` in the innermost scope.
    ///
    /// # Returns
    ///
    /// `true` when the name was free, `false` when it is already declared in
    /// *this* scope. Shadowing a name from an enclosing scope succeeds.
    pub fn declare(&mut self, name: &str, value: T) -> bool {
        let scope = self.scopes.last_mut().expect("global scope must stay open");
        if scope.contains_key(name) {
            return false;
        }
        // The key is only allocated once we know the insertion will happen.
        scope.insert(name.to_owned(), value);
        true
    }

    /// Looks `name` up from the innermost scope outwards.
    pub fn lookup(&self, name: &str) -> Option<&T> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_scope_shadows_outer() {
        let mut scopes = ScopeStack::new();
        assert!(scopes.declare("x", 1));

        scopes.push_scope();
        assert!(scopes.declare("x", 2));
        assert_eq!(scopes.lookup("x"), Some(&2));

        scopes.pop_scope();
        assert_eq!(scopes.lookup("x"), Some(&1));
    }

    #[test]
    fn rejects_redeclaration_in_same_scope() {
        let mut scopes = ScopeStack::new();
        assert!(scopes.declare("x", 1));
        assert!(!scopes.declare("x", 2));
        assert_eq!(scopes.lookup("x"), Some(&1));
    }

    #[test]
    fn reports_unknown_names() {
        let scopes: ScopeStack<i32> = ScopeStack::new();
        assert_eq!(scopes.lookup("missing"), None);
    }
}
