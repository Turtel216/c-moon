//! Compiler frontend: everything that turns C source text into a checked AST.
//!
//! The phases run in this order, each consuming the previous one's output:
//!
//! 1. [`lexer`] -- characters to macro-expanded tokens
//! 2. [`parser`] -- tokens to an [`ast`] translation unit
//! 3. [`semantic`] -- scope and type checking
//! 4. [`renamer`] -- resolves every identifier to a unique variable id
//!
//! [`span`] and [`scope`] hold the vocabulary the phases share: source
//! locations, and the lexically scoped name table used by both resolution
//! passes; [`suggest`] turns an unresolved name into a "did you mean";
//! [`layout`] says where the members of a struct sit.

pub mod ast;
pub mod layout;
pub mod lexer;
pub mod parser;
pub mod renamer;
pub mod scope;
pub mod semantic;
pub mod span;
pub mod suggest;
