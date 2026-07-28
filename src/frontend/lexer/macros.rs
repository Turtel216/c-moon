//! Macro definitions and their expansion.
//!
//! This layer is deliberately pure: it turns a stored replacement list plus a
//! set of arguments into tokens. Reading the arguments out of the token stream
//! is the job of [`super::Lexer`].

use crate::frontend::span::Span;

use super::token::{Token, TokenKind};

/// A `#define`d macro.
///
/// Object-like macros stand for a token sequence; function-like macros take
/// arguments that are substituted into that sequence.
#[derive(Debug, Clone)]
pub enum MacroDef<'a> {
    /// `#define NAME replacement`
    Object { replacement: Vec<Token<'a>> },
    /// `#define NAME(params) replacement`
    Function {
        params: Vec<String>,
        replacement: Vec<Token<'a>>,
    },
}

impl<'a> MacroDef<'a> {
    /// The parameter names; empty for an object-like macro.
    pub fn params(&self) -> &[String] {
        match self {
            MacroDef::Object { .. } => &[],
            MacroDef::Function { params, .. } => params,
        }
    }

    /// The token sequence the macro stands for.
    pub fn replacement(&self) -> &[Token<'a>] {
        match self {
            MacroDef::Object { replacement } => replacement,
            MacroDef::Function { replacement, .. } => replacement,
        }
    }

    /// Whether this macro must be followed by an argument list to expand.
    pub fn is_function_like(&self) -> bool {
        matches!(self, MacroDef::Function { .. })
    }

    /// Expands the macro at `use_span`, substituting `args` for its parameters.
    ///
    /// # Arguments
    ///
    /// * `args` - one token list per parameter, in declaration order; empty for
    ///   an object-like macro
    /// * `use_span` - location of the macro name in the program, which every
    ///   produced token inherits so diagnostics point at the use site rather
    ///   than at the `#define`
    ///
    /// # Returns
    ///
    /// The replacement tokens with parameters substituted. The result is *not*
    /// rescanned for further macro names, so a macro expanding to another
    /// macro's name stops there.
    pub fn expand<'b>(&self, args: &[Vec<Token<'_>>], use_span: Span) -> Vec<Token<'b>> {
        let params = self.params();
        let mut expanded = Vec::with_capacity(self.replacement().len());

        for token in self.replacement() {
            // Only identifiers name parameters, and parameter lists are a
            // handful of names at most: a linear scan beats building a hash map
            // for every single expansion.
            let argument = (token.kind == TokenKind::Identifier)
                .then(|| {
                    params
                        .iter()
                        .position(|param| param.as_str() == token.lexeme.as_ref())
                })
                .flatten()
                .and_then(|index| args.get(index));

            match argument {
                Some(tokens) => expanded.extend(tokens.iter().map(|t| t.to_owned_at(use_span))),
                None => expanded.push(token.to_owned_at(use_span)),
            }
        }

        expanded
    }
}
