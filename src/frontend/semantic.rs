//! Semantic analysis: scope checking and type checking over the AST.
//!
//! The analyzer walks the translation unit once, keeping a [`ScopeStack`] of
//! variable types and a table of function signatures. Errors are collected per
//! top-level declaration rather than aborting the pass, so one run reports the
//! problems in several functions.
//!
//! Struct definitions are laid out as they are met, which is what C requires
//! anyway: a struct must be defined before an object of it can exist. The
//! resulting [`StructTable`] travels to the middle-end inside the [`TypeMap`],
//! where a member's byte offset becomes the address a load reads from.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use crate::driver::diagnostics::{CompilerError, Diagnostic, codes};
use crate::frontend::ast::{
    BinaryOp, BlockItem, CType, Decl, DeclKind, Expr, ExprKind, Literal, NodeId, ParamDecl, Sign,
    Stmt, StmtKind, UnaryOp,
};
use crate::frontend::layout::StructTable;
use crate::frontend::scope::ScopeStack;
use crate::frontend::span::Span;
use crate::frontend::suggest;

/// Convenient semantic result alias.
pub type SemanticResult<T> = Result<T, SemanticError>;

/// A semantic error produced during name resolution / type checking.
///
/// Every variant carries the spans a diagnostic needs: the one to blame, and
/// where relevant the earlier declaration that explains why it is wrong.
#[derive(Debug, Clone, PartialEq)]
pub enum SemanticError {
    UndeclaredVariable {
        name: String,
        span: Span,
        /// A name in scope that the unknown one is probably a typo of.
        suggestion: Option<String>,
    },
    RedeclaredVariable {
        name: String,
        span: Span,
        /// Where the name was declared the first time.
        previous: Span,
    },
    UndeclaredFunction {
        name: String,
        span: Span,
        suggestion: Option<String>,
    },
    RedeclaredFunction {
        name: String,
        span: Span,
        previous: Span,
    },
    /// A struct tag defined twice.
    RedeclaredStruct {
        name: String,
        span: Span,
        /// Where the tag was defined the first time.
        previous: Span,
    },
    ArgumentCountMismatch {
        name: String,
        expected: usize,
        found: usize,
        span: Span,
        /// The function's signature, which fixes the expected count.
        declared: Span,
    },
    TypeError {
        expected: Type,
        found: Type,
        span: Span,
        /// What imposed the expected type -- the declaration an initializer
        /// must match, say. `None` when the language itself does.
        origin: Option<Span>,
        /// Why that type is required here, read either as a caption for
        /// `origin` or, without one, as a closing note.
        context: &'static str,
    },
    NotIndexable {
        found: Type,
        span: Span,
    },
    NotDereferenceable {
        found: Type,
        span: Span,
    },
    NotAnObject {
        span: Span,
        /// What was attempted, e.g. `assign to`.
        operation: &'static str,
    },
    Unsupported {
        /// What the compiler cannot do yet, e.g. ``the type `float` ``.
        feature: String,
        span: Span,
        /// Where it was attempted, e.g. `in this variable declaration`.
        context: &'static str,
    },
    /// An object of a type whose size is not known: a `struct` tag that was
    /// declared but never defined, or an array of one.
    IncompleteType {
        ty: Type,
        span: Span,
        /// What the size was needed for, e.g. `in this declaration`.
        context: &'static str,
        /// A defined tag the undefined one is probably a typo of.
        suggestion: Option<String>,
    },
    /// `.` or `->` applied to something with no members to read.
    NotAStruct {
        found: Type,
        span: Span,
        /// Whether it was written `->` rather than `.`, which is what decides
        /// the operator the help line proposes instead.
        is_arrow: bool,
    },
    /// `break` or `continue` written where there is no loop for it to act on.
    JumpOutsideLoop {
        /// The keyword as written, `break` or `continue`.
        keyword: &'static str,
        span: Span,
    },
    /// A member the struct does not have.
    NoSuchMember {
        /// The struct the member was looked for in.
        struct_ty: Type,
        member: String,
        span: Span,
        /// A member of that struct the name is probably a typo of.
        suggestion: Option<String>,
        /// Where the struct was defined, so the reader can see what it holds.
        declared: Span,
    },
}

impl fmt::Display for SemanticError {
    /// Writes the headline the compiler prints for this error.
    ///
    /// This is the single source of the wording; the diagnostic's message
    /// defers to it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticError::UndeclaredVariable { name, .. } => {
                write!(f, "cannot find value `{name}` in this scope")
            }
            SemanticError::UndeclaredFunction { name, .. } => {
                write!(f, "cannot find function `{name}` in this scope")
            }
            SemanticError::RedeclaredVariable { name, .. }
            | SemanticError::RedeclaredFunction { name, .. } => {
                write!(f, "the name `{name}` is defined multiple times")
            }
            SemanticError::ArgumentCountMismatch {
                expected, found, ..
            } => write!(
                f,
                "this function takes {} but {} supplied",
                arguments(*expected),
                match found {
                    1 => String::from("1 argument was"),
                    _ => format!("{found} arguments were"),
                }
            ),
            SemanticError::RedeclaredStruct { name, .. } => {
                write!(f, "the struct `{name}` is defined multiple times")
            }
            SemanticError::TypeError { .. } => write!(f, "mismatched types"),
            SemanticError::NotIndexable { found, .. } => {
                write!(f, "cannot index into a value of type `{found}`")
            }
            SemanticError::NotDereferenceable { found, .. } => {
                write!(f, "type `{found}` cannot be dereferenced")
            }
            SemanticError::NotAnObject { operation, .. } => {
                write!(f, "cannot {operation} this expression")
            }
            SemanticError::Unsupported { feature, .. } => {
                write!(f, "{feature} is not supported yet")
            }
            SemanticError::IncompleteType { ty, .. } => {
                write!(f, "the type `{ty}` is incomplete")
            }
            SemanticError::NotAStruct {
                found,
                is_arrow: true,
                ..
            } => write!(f, "type `{found}` is not a pointer to a struct"),
            SemanticError::NotAStruct { found, .. } => {
                write!(f, "type `{found}` is not a struct")
            }
            SemanticError::NoSuchMember {
                struct_ty, member, ..
            } => write!(f, "no member named `{member}` on type `{struct_ty}`"),
            SemanticError::JumpOutsideLoop { keyword, .. } => {
                write!(f, "`{keyword}` outside of a loop")
            }
        }
    }
}

impl std::error::Error for SemanticError {}

impl CompilerError for SemanticError {
    fn into_diagnostic(self) -> Diagnostic {
        // The headline comes from `Display`, so the two never drift apart.
        let message = self.to_string();

        match self {
            SemanticError::UndeclaredVariable {
                span, suggestion, ..
            } => Diagnostic::error(codes::UNDECLARED_VARIABLE, message, span)
                .with_label("not found in this scope")
                .with_optional_help(
                    suggestion
                        .map(|name| format!("a variable with a similar name exists: `{name}`")),
                ),

            SemanticError::UndeclaredFunction {
                span, suggestion, ..
            } => Diagnostic::error(codes::UNDECLARED_FUNCTION, message, span)
                .with_label("not found in this scope")
                .with_optional_help(
                    suggestion
                        .map(|name| format!("a function with a similar name exists: `{name}`")),
                ),

            SemanticError::RedeclaredVariable {
                name,
                span,
                previous,
            } => Diagnostic::error(codes::DUPLICATE_DEFINITION, message, span)
                .with_label(format!("`{name}` redeclared here"))
                .with_secondary(previous, format!("previous declaration of `{name}` here"))
                .with_note(format!(
                    "`{name}` must be declared only once in the same scope"
                )),

            SemanticError::RedeclaredFunction {
                name,
                span,
                previous,
            } => Diagnostic::error(codes::DUPLICATE_DEFINITION, message, span)
                .with_label(format!("`{name}` redefined here"))
                .with_secondary(previous, format!("previous definition of `{name}` here"))
                .with_note(format!("`{name}` must be defined only once")),

            SemanticError::RedeclaredStruct {
                name,
                span,
                previous,
            } => Diagnostic::error(codes::DUPLICATE_DEFINITION, message, span)
                .with_label(format!("`{name}` redefined here"))
                .with_secondary(
                    previous,
                    format!("previous definition of `struct {name}` here"),
                )
                .with_note(format!("`struct {name}` must be defined only once")),

            SemanticError::ArgumentCountMismatch {
                name,
                expected,
                found,
                span,
                declared,
            } => Diagnostic::error(codes::WRONG_ARGUMENT_COUNT, message, span)
                .with_label(format!("expected {}, found {found}", arguments(expected)))
                .with_secondary(declared, format!("function `{name}` defined here")),

            SemanticError::TypeError {
                expected,
                found,
                span,
                origin,
                context,
            } => {
                let diagnostic = Diagnostic::error(codes::MISMATCHED_TYPES, message, span)
                    .with_label(format!("expected `{expected}`, found `{found}`"));
                match origin {
                    Some(origin) => diagnostic.with_secondary(origin, context),
                    None => diagnostic.with_note(context),
                }
            }

            SemanticError::NotIndexable { span, .. } => {
                Diagnostic::error(codes::MISMATCHED_TYPES, message, span)
                    .with_label("not an array")
                    .with_note("only a value of array type can be indexed")
            }

            SemanticError::NotDereferenceable { span, .. } => {
                Diagnostic::error(codes::MISMATCHED_TYPES, message, span)
                    .with_label("not a pointer")
            }

            SemanticError::NotAnObject { span, .. } => {
                Diagnostic::error(codes::INVALID_ASSIGNMENT, message, span)
                    .with_label("not an lvalue")
                    .with_note("only variables, array elements and dereferences are lvalues")
            }

            SemanticError::Unsupported { span, context, .. } => {
                Diagnostic::error(codes::UNSUPPORTED, message, span)
                    .with_label(context)
                    .with_note("this compiler implements the subset of C listed in its README")
            }

            SemanticError::IncompleteType {
                ty,
                span,
                context,
                suggestion,
            } => Diagnostic::error(codes::INCOMPLETE_TYPE, message, span)
                .with_label(context)
                .with_note(format!(
                    "`{ty}` is not defined at this point, so neither its size nor its members \
                     are known"
                ))
                .with_optional_help(
                    suggestion.map(|tag| format!("a struct with a similar name exists: `{tag}`")),
                ),

            SemanticError::NotAStruct {
                found,
                span,
                is_arrow,
            } => {
                // Reaching for the wrong one of the pair is the usual mistake,
                // so the help says which one this operand wants.
                let help = match (is_arrow, &found) {
                    (true, Type::Struct(_)) => Some("`.` reads a member of a struct value"),
                    (false, Type::Pointer(pointee)) if pointee.is_struct() => {
                        Some("`->` reads a member through a pointer to a struct")
                    }
                    _ => None,
                };
                Diagnostic::error(codes::MISMATCHED_TYPES, message, span)
                    .with_label(match is_arrow {
                        true => "not a pointer to a struct",
                        false => "not a struct",
                    })
                    .with_optional_help(help.map(str::to_owned))
            }

            SemanticError::JumpOutsideLoop { keyword, span } => {
                Diagnostic::error(codes::JUMP_OUTSIDE_LOOP, message, span)
                    .with_label(format!("cannot `{keyword}` outside of a loop"))
                    .with_note(format!(
                        "`{keyword}` acts on the innermost enclosing `while` or `for`, and there \
                         is none here"
                    ))
            }

            SemanticError::NoSuchMember {
                span,
                suggestion,
                declared,
                struct_ty,
                ..
            } => Diagnostic::error(codes::UNKNOWN_MEMBER, message, span)
                .with_label("unknown member")
                .with_secondary(declared, format!("`{struct_ty}` defined here"))
                .with_optional_help(
                    suggestion.map(|name| format!("a member with a similar name exists: `{name}`")),
                ),
        }
    }
}

/// Spells out an argument count, e.g. `1 argument` or `3 arguments`.
fn arguments(count: usize) -> String {
    match count {
        1 => String::from("1 argument"),
        _ => format!("{count} arguments"),
    }
}

/// Internal semantic type model for the current compiler stage.
///
/// This is deliberately smaller than [`CType`]: it only models what the rest of
/// the pipeline can compile, so unsupported syntax is rejected here rather than
/// crashing a later phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// `char`: an 8-bit integer.
    ///
    /// Plain `char` is *signed* here, which is what the System V x86-64 ABI
    /// says it is. That is a per-ABI choice rather than a rule of the
    /// language, and it is the reason widening a plain `char` sign-extends
    /// where widening an `unsigned char` fills with zeroes: see
    /// [`Type::promoted`] and the `Convert` lowering in the backend.
    Char(Sign),
    /// `int`: a 32-bit integer, as on every ABI this compiler targets.
    Int(Sign),
    /// `long int`: a 64-bit integer.
    Long(Sign),
    Void,
    /// A fixed-size array of a primitive type, e.g. `int arr[3]` -> `Array(Int, 3)`.
    Array(Box<Type>, usize),
    /// A pointer to another type, e.g. `int *p` -> `Pointer(Int)`.
    Pointer(Box<Type>),
    /// `struct Tag`.
    ///
    /// The tag is the whole type: two struct types are the same type exactly
    /// when they name the same tag, and what the tag *holds* is recorded once
    /// in the program's [`StructTable`] rather than in every copy of the type.
    /// A tag with no entry there is an incomplete type -- named, but not yet
    /// defined -- which is why the table is consulted before any object of a
    /// struct type is created.
    Struct(String),
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Char(sign) => write!(f, "{}char", sign.prefix()),
            Type::Int(sign) => write!(f, "{}int", sign.prefix()),
            Type::Long(sign) => write!(f, "{}long int", sign.prefix()),
            Type::Void => write!(f, "void"),
            Type::Array(elem, size) => write!(f, "{elem}[{size}]"),
            Type::Pointer(inner) => write!(f, "{inner}*"),
            Type::Struct(tag) => write!(f, "struct {tag}"),
        }
    }
}

impl Type {
    /// Translates a syntactic type into the semantic model.
    ///
    /// # Arguments
    ///
    /// * `ty` - the type as written in the source
    /// * `span` - location to blame if the type is not supported
    /// * `context` - where the type was written, e.g. `in this declaration`
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Unsupported`] for types the compiler cannot
    /// lower yet: `float`, `double`, and arrays of unknown size.
    fn from_ctype(ty: &CType, span: Span, context: &'static str) -> SemanticResult<Self> {
        match ty {
            CType::Char(sign) => Ok(Type::Char(*sign)),
            CType::Int(sign) => Ok(Type::Int(*sign)),
            CType::Long(sign) => Ok(Type::Long(*sign)),
            CType::Void => Ok(Type::Void),
            CType::Array(elem, Some(size)) => Ok(Type::Array(
                Box::new(Type::from_ctype(elem, span, context)?),
                *size,
            )),
            CType::Pointer(inner) => Ok(Type::Pointer(Box::new(Type::from_ctype(
                inner, span, context,
            )?))),
            // Whether the tag has been defined is not asked here: a pointer to
            // an undefined struct is a perfectly good type, and it is what
            // lets `struct Node *next;` appear inside `struct Node`. The
            // question is asked where an *object* is created instead, by
            // `SemanticAnalyzer::expect_complete`.
            CType::Struct(tag) => Ok(Type::Struct(tag.clone())),
            CType::Float | CType::Double | CType::Array(_, None) => {
                Err(SemanticError::Unsupported {
                    feature: format!("the type `{ty}`"),
                    span,
                    context,
                })
            }
        }
    }

    /// The plain signed `int`.
    ///
    /// It is what every promotion lands on, what a comparison answers with,
    /// and what a diagnostic names when what it really wants is "an integer",
    /// so it is worth a name of its own.
    pub const INT: Type = Type::Int(Sign::Signed);

    /// How many bytes an integer type occupies and how its top bit reads.
    ///
    /// # Returns
    ///
    /// `None` for everything that is not an integer type, which is what
    /// [`Type::is_integer`] asks.
    fn integer(&self) -> Option<(u8, Sign)> {
        Some(match *self {
            Type::Char(sign) => (1, sign),
            Type::Int(sign) => (4, sign),
            Type::Long(sign) => (8, sign),
            Type::Void | Type::Array(_, _) | Type::Pointer(_) | Type::Struct(_) => return None,
        })
    }

    /// The integer type that is `bytes` wide and reads as `sign`.
    ///
    /// # Panics
    ///
    /// Panics unless `bytes` is the size of one of the integer types. Callers
    /// take it from [`Type::integer`], so anything else is a compiler bug.
    fn integer_of(bytes: u8, sign: Sign) -> Type {
        match bytes {
            1 => Type::Char(sign),
            4 => Type::Int(sign),
            8 => Type::Long(sign),
            other => panic!("Compiler Bug: no integer type is {other} bytes wide"),
        }
    }

    /// Whether this is one of the integer types, which are the only ones
    /// arithmetic, comparisons and conditions accept.
    pub fn is_integer(&self) -> bool {
        self.integer().is_some()
    }

    /// Whether this is a struct type.
    pub fn is_struct(&self) -> bool {
        matches!(self, Type::Struct(_))
    }

    /// Whether a value of this type lives in memory rather than in a register.
    ///
    /// An array and a struct are aggregates: several values behind one name,
    /// which no single register can hold. That is what gives them storage of
    /// their own in the frame, keeps them out of SSA form -- see
    /// [`promote`](crate::middle::ssa::promote) -- and makes every read of one
    /// of their parts a memory access.
    pub fn is_aggregate(&self) -> bool {
        matches!(self, Type::Array(_, _) | Type::Struct(_))
    }

    /// This type after the integer promotions.
    ///
    /// C promotes anything narrower than an `int` to an `int` before an
    /// operator ever sees it, so `c1 + c2` adds two `int`s and has type `int`
    /// even though neither operand was one. The promotion is to the *signed*
    /// `int` even from an `unsigned char`, because an `int` can represent
    /// every value eight bits can hold whichever way they read. Nothing else
    /// is affected: the types that are already at least as wide as an `int`
    /// promote to themselves.
    pub fn promoted(&self) -> Type {
        match self {
            Type::Char(_) => Type::Int(Sign::Signed),
            other => other.clone(),
        }
    }

    /// The type both operands of an arithmetic or relational operator are
    /// converted to before it is applied.
    ///
    /// C calls this the usual arithmetic conversions: the operands are
    /// promoted first, and then the wider type wins -- and where they are
    /// equally wide, the unsigned one does. That is the standard's rule
    /// spelled out for the types here. A `long int` can represent every value
    /// an `unsigned int` has, so the wider signed type is enough for that
    /// pair; `int` and `unsigned int` have no such type between them, and meet
    /// as `unsigned int`.
    ///
    /// # Panics
    ///
    /// Panics unless both types are integers, which the caller checks first.
    pub fn common(lhs: &Type, rhs: &Type) -> Type {
        let (lhs, rhs) = (lhs.promoted(), rhs.promoted());
        let ((lhs_bytes, lhs_sign), (rhs_bytes, rhs_sign)) = match (lhs.integer(), rhs.integer()) {
            (Some(left), Some(right)) => (left, right),
            _ => panic!("Compiler Bug: the usual arithmetic conversions apply to integers only"),
        };

        let sign = match lhs_bytes.cmp(&rhs_bytes) {
            Ordering::Greater => lhs_sign,
            Ordering::Less => rhs_sign,
            Ordering::Equal if lhs_sign == rhs_sign => lhs_sign,
            Ordering::Equal => Sign::Unsigned,
        };
        Type::integer_of(lhs_bytes.max(rhs_bytes), sign)
    }
}

/// One parameter of a declared function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSig {
    pub ty: Type,
    /// The parameter as written, so an argument of the wrong type can be
    /// blamed on the parameter that rejects it.
    pub span: Span,
}

/// A function's declared interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSig {
    pub return_ty: Type,
    pub params: Vec<ParamSig>,
    /// The signature as written, quoted when a call does not match it.
    pub span: Span,
}

/// A variable's declared type and where it was declared.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VarInfo {
    ty: Type,
    /// The declared name, quoted when a second declaration collides with it.
    span: Span,
}

/// What semantic analysis worked out about every typed node of the tree.
///
/// The middle-end lowers with it: an `int` is 32 bits wide and a `long int` is
/// 64, so the type an expression was given here decides the width of the
/// instruction it becomes and where a conversion has to be inserted. Types are
/// recorded in a side table keyed by [`NodeId`], like the renamer's
/// resolution map, so the AST itself stays untouched.
#[derive(Debug, Default)]
pub struct TypeMap {
    /// The type of every expression that was checked.
    exprs: HashMap<NodeId, Type>,
    /// The declared type of every variable and parameter declaration.
    decls: HashMap<NodeId, Type>,
    /// Every function's signature, by name.
    functions: HashMap<String, FunctionSig>,
    /// Where the members of every defined struct sit, which is what turns
    /// `s.x` into a load at a known offset.
    structs: StructTable,
}

impl TypeMap {
    /// The type of the expression `id` names.
    ///
    /// # Panics
    ///
    /// Panics if the expression was never checked, which means the middle-end
    /// is lowering a tree semantic analysis did not accept.
    pub fn expr(&self, id: NodeId) -> &Type {
        self.exprs
            .get(&id)
            .expect("Compiler Bug: expression was never type checked")
    }

    /// The declared type of the declaration `id` names.
    ///
    /// # Panics
    ///
    /// Panics if the declaration was never checked.
    pub fn decl(&self, id: NodeId) -> &Type {
        self.decls
            .get(&id)
            .expect("Compiler Bug: declaration was never type checked")
    }

    /// The signature of the function called `name`.
    ///
    /// # Panics
    ///
    /// Panics if no such function was declared.
    pub fn function(&self, name: &str) -> &FunctionSig {
        self.functions
            .get(name)
            .expect("Compiler Bug: call to a function that was never declared")
    }

    /// The layout of every struct the translation unit defines.
    ///
    /// The middle-end lowers a member access with it: a member's offset is
    /// what the address of `s.x` is formed from, and a type's size is what
    /// reserves the frame storage the object needs.
    pub fn structs(&self) -> &StructTable {
        &self.structs
    }
}

/// Semantic analyzer for declarations/statements/expressions.
#[derive(Debug, Default)]
pub struct SemanticAnalyzer {
    /// Variable types, innermost scope last.
    symbols: ScopeStack<VarInfo>,
    /// The types every checked node was given, handed to the middle-end once
    /// the whole unit has been accepted.
    types: TypeMap,
    /// Return type of the function whose body is being checked, and the
    /// signature that declared it.
    current_function: Option<(Type, Span)>,
    /// How many loops enclose the statement being checked, which is what says
    /// whether a `break` or a `continue` has anything to act on.
    loop_depth: usize,
    errors: Vec<SemanticError>,
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The types of every node, once analysis has reported no errors.
    pub fn into_types(self) -> TypeMap {
        self.types
    }

    /// Analyze a translation unit (top-level declarations).
    ///
    /// # Returns
    ///
    /// Every error found. Analysis of one declaration stops at its first error
    /// but the remaining declarations are still checked.
    pub fn analyze_program(&mut self, decls: &[Decl]) -> Vec<SemanticError> {
        if let Err(error) = self.register_function_signatures(decls) {
            self.errors.push(error);
        }

        for decl in decls {
            if let Err(error) = self.analyze_decl(decl) {
                self.errors.push(error);
            }
        }

        std::mem::take(&mut self.errors)
    }

    /// Records every function's signature before any body is checked.
    fn register_function_signatures(&mut self, decls: &[Decl]) -> SemanticResult<()> {
        for decl in decls {
            let DeclKind::Function {
                return_ty,
                name,
                params,
                ..
            } = &decl.kind
            else {
                continue;
            };

            let return_ty = Type::from_ctype(return_ty, decl.span, "in this return type")?;
            Self::expect_passable(&return_ty, decl.span, "returning", "in this return type")?;

            let sig = FunctionSig {
                return_ty,
                params: self.param_signatures(params)?,
                span: decl.span,
            };

            if let Some(previous) = self.types.functions.insert(name.clone(), sig) {
                return Err(SemanticError::RedeclaredFunction {
                    name: name.clone(),
                    span: decl.name_span,
                    previous: previous.span,
                });
            }
        }

        Ok(())
    }

    /// Translates a parameter list into semantic types, recording the type of
    /// each parameter declaration as it goes.
    fn param_signatures(&mut self, params: &[ParamDecl]) -> SemanticResult<Vec<ParamSig>> {
        params
            .iter()
            .map(|param| {
                let ty = Type::from_ctype(&param.ty, param.span, "in this parameter")?;
                Self::expect_passable(&ty, param.span, "passing", "in this parameter")?;
                self.types.decls.insert(param.id, ty.clone());
                Ok(ParamSig {
                    ty,
                    span: param.span,
                })
            })
            .collect()
    }

    fn analyze_decl(&mut self, decl: &Decl) -> SemanticResult<()> {
        match &decl.kind {
            DeclKind::Variable {
                ty,
                name,
                initializer,
            } => self.analyze_variable_decl(decl, ty, name, initializer.as_ref()),

            DeclKind::Function {
                return_ty,
                params,
                body,
                ..
            } => self.analyze_function_decl(decl, return_ty, params, body.as_ref()),

            DeclKind::Struct { name, members } => {
                self.analyze_struct_decl(decl, name.as_deref(), members.as_deref())
            }
        }
    }

    /// Checks a struct declaration and, if it is a definition, lays it out.
    ///
    /// A definition is what gives the tag a layout, so it is also what makes
    /// objects of that type possible. Members are laid out in the order they
    /// are written; see [`StructTable::lay_out`].
    ///
    /// # Arguments
    ///
    /// * `decl` - the declaration, whose `name_span` is what a later
    ///   diagnostic quotes as "defined here"
    /// * `name` - the tag, absent for an anonymous `struct { ... };`
    /// * `members` - the body, absent for the forward declaration `struct P;`
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::RedeclaredStruct`] for a tag defined twice,
    /// [`SemanticError::RedeclaredVariable`] for a member name used twice, and
    /// [`SemanticError::IncompleteType`] for a member whose size is unknown --
    /// which is what rejects a struct that contains itself.
    fn analyze_struct_decl(
        &mut self,
        decl: &Decl,
        name: Option<&str>,
        members: Option<&[Decl]>,
    ) -> SemanticResult<()> {
        // A forward declaration says only that the tag exists. An anonymous
        // definition names no type that anything could be declared with, so
        // neither one has a layout to record.
        let (Some(members), Some(tag)) = (members, name) else {
            return Ok(());
        };

        let mut laid_out = Vec::with_capacity(members.len());
        // Where each member name was first written, so the second use of one
        // can point at it.
        let mut declared: HashMap<&str, Span> = HashMap::with_capacity(members.len());

        for member in members {
            let DeclKind::Variable {
                ty,
                name: member_name,
                ..
            } = &member.kind
            else {
                panic!("Compiler Bug: a struct member is always a variable declaration");
            };

            if let Some(&previous) = declared.get(member_name.as_str()) {
                return Err(SemanticError::RedeclaredVariable {
                    name: member_name.clone(),
                    span: member.name_span,
                    previous,
                });
            }
            declared.insert(member_name, member.name_span);

            let member_ty = Type::from_ctype(ty, member.span, "in this member")?;
            // The member needs a size, which is what rejects `struct S { struct
            // S inner; };`: `S` is not defined until this loop has finished.
            self.expect_complete(&member_ty, member.span, "in this member")?;
            laid_out.push((member_name.clone(), member_ty, member.name_span));
        }

        let layout = self.types.structs.lay_out(laid_out, decl.name_span);
        match self.types.structs.define(tag.to_owned(), layout) {
            Some(previous) => Err(SemanticError::RedeclaredStruct {
                name: tag.to_owned(),
                span: decl.name_span,
                previous: previous.span(),
            }),
            None => Ok(()),
        }
    }

    fn analyze_variable_decl(
        &mut self,
        decl: &Decl,
        ty: &CType,
        name: &str,
        initializer: Option<&Expr>,
    ) -> SemanticResult<()> {
        let var_ty = Type::from_ctype(ty, decl.span, "in this declaration")?;
        if var_ty == Type::Void {
            return Err(SemanticError::TypeError {
                expected: Type::INT,
                found: Type::Void,
                span: decl.span,
                origin: None,
                context: "a variable cannot have type `void`",
            });
        }

        self.expect_complete(&var_ty, decl.span, "in this declaration")?;
        self.types.decls.insert(decl.id, var_ty.clone());

        let info = VarInfo {
            ty: var_ty.clone(),
            span: decl.name_span,
        };
        if !self.symbols.declare(name, info) {
            let previous = self
                .symbols
                .lookup_local(name)
                .expect("`declare` only fails on a name already in this scope");
            return Err(SemanticError::RedeclaredVariable {
                name: name.to_owned(),
                span: decl.name_span,
                previous: previous.span,
            });
        }

        let Some(init) = initializer else {
            return Ok(());
        };

        // Array declarations have no scalar initializer.
        if matches!(var_ty, Type::Array(_, _)) {
            return Err(SemanticError::Unsupported {
                feature: String::from("array initializer lists"),
                span: decl.span,
                context: "in this declaration",
            });
        }

        let init_ty = self.analyze_expr(init)?;
        // The declaration is blamed through its name rather than its whole
        // text: a span that contained the initializer would underline the very
        // expression the error is about.
        Self::expect_type(
            &var_ty,
            &init_ty,
            init.span,
            Some(decl.name_span),
            "expected because of this declaration",
        )
    }

    fn analyze_function_decl(
        &mut self,
        decl: &Decl,
        return_ty: &CType,
        params: &[ParamDecl],
        body: Option<&Stmt>,
    ) -> SemanticResult<()> {
        let ret_ty = Type::from_ctype(return_ty, decl.span, "in this return type")?;

        // A prototype declares no scope of its own; only a definition is checked.
        let Some(body) = body else {
            return Ok(());
        };

        self.symbols.push_scope();

        for param in params {
            let info = VarInfo {
                ty: Type::from_ctype(&param.ty, param.span, "in this parameter")?,
                span: param.span,
            };
            // An unnamed parameter still occupies a slot; `_` cannot collide
            // with a real identifier because it would be a valid C name --
            // a name clash here is reported like any other redeclaration.
            let param_name = param.name.as_deref().unwrap_or("_");
            if !self.symbols.declare(param_name, info) {
                let previous = self
                    .symbols
                    .lookup_local(param_name)
                    .expect("`declare` only fails on a name already in this scope");
                return Err(SemanticError::RedeclaredVariable {
                    name: param_name.to_owned(),
                    span: param.span,
                    previous: previous.span,
                });
            }
        }

        let enclosing = self.current_function.replace((ret_ty, decl.span));
        let result = self.analyze_stmt(body);
        self.current_function = enclosing;
        self.symbols.pop_scope();

        result
    }

    fn analyze_stmt(&mut self, stmt: &Stmt) -> SemanticResult<()> {
        match &stmt.kind {
            StmtKind::Empty => Ok(()),

            StmtKind::Expr(expr) => self.analyze_expr(expr).map(|_| ()),

            StmtKind::Return(value) => {
                // A `return` outside any function cannot be parsed, so the
                // enclosing signature is always known here.
                let (expected, signature) = self
                    .current_function
                    .clone()
                    .unwrap_or((Type::Void, stmt.span));
                let found = match value {
                    Some(expr) => self.analyze_expr(expr)?,
                    None => Type::Void,
                };
                // Blame the returned value when there is one; a bare `return`
                // has only the statement itself to point at.
                let span = value.as_ref().map_or(stmt.span, |expr| expr.span);
                Self::expect_type(
                    &expected,
                    &found,
                    span,
                    Some(signature),
                    "expected because of this return type",
                )
            }

            StmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.analyze_condition(condition, "an `if` condition must be an integer")?;
                self.analyze_stmt(then_branch)?;
                match else_branch {
                    Some(branch) => self.analyze_stmt(branch),
                    None => Ok(()),
                }
            }

            StmtKind::While { condition, body } => {
                self.analyze_condition(condition, "a `while` condition must be an integer")?;
                self.in_loop(|analyzer| analyzer.analyze_stmt(body))
            }

            StmtKind::For {
                init,
                condition,
                step,
                body,
            } => {
                // The init clause may declare a variable, which is scoped to
                // the loop.
                self.in_new_scope(|analyzer| {
                    if let Some(init) = init {
                        analyzer.analyze_for_init(init)?;
                    }
                    if let Some(condition) = condition {
                        analyzer
                            .analyze_condition(condition, "a `for` condition must be an integer")?;
                    }
                    if let Some(step) = step {
                        analyzer.analyze_expr(step)?;
                    }
                    analyzer.in_loop(|analyzer| analyzer.analyze_stmt(body))
                })
            }

            StmtKind::Break => self.check_inside_loop("break", stmt.span),
            StmtKind::Continue => self.check_inside_loop("continue", stmt.span),

            StmtKind::Block(items) => {
                self.in_new_scope(|analyzer| analyzer.analyze_block_items(items))
            }
        }
    }

    /// Rejects a `break` or a `continue` that has no loop to act on.
    ///
    /// # Arguments
    ///
    /// * `keyword` - the jump as written, `break` or `continue`
    /// * `span` - the statement, which is what the diagnostic underlines
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::JumpOutsideLoop`] when no loop encloses it.
    fn check_inside_loop(&self, keyword: &'static str, span: Span) -> SemanticResult<()> {
        match self.loop_depth {
            0 => Err(SemanticError::JumpOutsideLoop { keyword, span }),
            _ => Ok(()),
        }
    }

    /// Checks the items of a block in the scope that is already open.
    ///
    /// A failing item is recorded and the block carries on with the next one,
    /// so a single run reports every bad statement in a function rather than
    /// just the first. Recovery is safe here because a declaration binds its
    /// name before its initializer is checked: a later use of that name does
    /// not produce a second, spurious error.
    fn analyze_block_items(&mut self, items: &[BlockItem]) -> SemanticResult<()> {
        for item in items {
            let checked = match item {
                BlockItem::Stmt(stmt) => self.analyze_stmt(stmt),
                BlockItem::Decl(decl) => self.analyze_decl(decl),
            };
            if let Err(error) = checked {
                self.errors.push(error);
            }
        }
        Ok(())
    }

    /// Checks the init clause of a `for`, which belongs to the loop's scope.
    ///
    /// The parser wraps a declaration in the init clause in a block so that it
    /// fits where a statement is expected. That wrapper must not get a scope of
    /// its own, or the loop variable would go out of scope before the condition
    /// is reached: in `for (int i = 0; i < n; i = i + 1)`, `i` has to stay
    /// visible to the condition, the step and the body.
    fn analyze_for_init(&mut self, init: &Stmt) -> SemanticResult<()> {
        match &init.kind {
            StmtKind::Block(items) => self.analyze_block_items(items),
            _ => self.analyze_stmt(init),
        }
    }

    /// Runs `analysis` with one more enclosing loop counted, so that a `break`
    /// or a `continue` inside it is accepted.
    ///
    /// The count is restored even when the analysis fails, so that an error in
    /// a loop body does not leave later statements looking as if they were
    /// still inside it.
    fn in_loop<T>(
        &mut self,
        analysis: impl FnOnce(&mut Self) -> SemanticResult<T>,
    ) -> SemanticResult<T> {
        self.loop_depth += 1;
        let result = analysis(self);
        self.loop_depth -= 1;
        result
    }

    /// Runs `analysis` in a nested scope, closing it even when the analysis fails.
    fn in_new_scope<T>(
        &mut self,
        analysis: impl FnOnce(&mut Self) -> SemanticResult<T>,
    ) -> SemanticResult<T> {
        self.symbols.push_scope();
        let result = analysis(self);
        self.symbols.pop_scope();
        result
    }

    /// Checks a controlling expression, which C requires to be a scalar.
    fn analyze_condition(&mut self, condition: &Expr, context: &'static str) -> SemanticResult<()> {
        let cond_ty = self.analyze_expr(condition)?;
        Self::expect_integer(&cond_ty, condition.span, context)
    }

    /// Checks one expression and records the type it was given.
    ///
    /// Every expression goes through here exactly once, so the recorded types
    /// cover the whole tree the middle-end will lower.
    fn analyze_expr(&mut self, expr: &Expr) -> SemanticResult<Type> {
        let ty = self.check_expr(expr)?;
        self.types.exprs.insert(expr.id, ty.clone());
        Ok(ty)
    }

    fn check_expr(&mut self, expr: &Expr) -> SemanticResult<Type> {
        match &expr.kind {
            ExprKind::Literal(literal) => Self::literal_type(literal, expr.span),

            ExprKind::Identifier(name) => match self.symbols.lookup(name) {
                Some(info) => Ok(info.ty.clone()),
                None => Err(SemanticError::UndeclaredVariable {
                    name: name.clone(),
                    span: expr.span,
                    suggestion: suggest::nearest(name, self.symbols.names()).map(str::to_owned),
                }),
            },

            ExprKind::Binary(op, lhs, rhs) => self.analyze_binary(*op, lhs, rhs),

            ExprKind::Unary(op, operand) => self.analyze_unary(*op, operand, expr.span),

            ExprKind::Cast(to_ty, operand) => {
                self.analyze_expr(operand)?;
                let target = Type::from_ctype(to_ty, expr.span, "in this cast")?;
                // A cast converts one value into another, and an aggregate is
                // not one value: C has no cast to a struct or an array type.
                match target.is_aggregate() {
                    false => Ok(target),
                    true => Err(SemanticError::TypeError {
                        expected: Type::INT,
                        found: target,
                        span: expr.span,
                        origin: None,
                        context: "a cast converts to an integer or a pointer type",
                    }),
                }
            }

            ExprKind::Call { callee, args } => self.analyze_call(callee, args, expr.span),

            ExprKind::Index { array, index } => self.analyze_index(array, index),

            ExprKind::MemberAccess {
                base,
                member,
                is_arrow,
            } => self.analyze_member_access(base, member, *is_arrow, expr.span),

            // Out of current language scope: reject with a clear unsupported
            // diagnostic rather than crashing a later phase.
            ExprKind::SizeOf(_) => Err(SemanticError::Unsupported {
                feature: String::from("`sizeof`"),
                span: expr.span,
                context: "in this expression",
            }),
        }
    }

    /// The type of a literal. Only integer literals are supported so far.
    ///
    /// C gives an unsuffixed integer literal the first type of `int`, `long
    /// int`, ... that can represent it, so a literal too large for 32 bits is
    /// already a `long int` where it is written. One too large for a signed
    /// 64-bit type has only `unsigned long int` left, which is the type GCC
    /// gives it too.
    ///
    /// A character constant is an integer literal too: `'a'` has type `int` in
    /// C and not `char`, which is why `sizeof('a')` is 4 rather than 1. Only
    /// the *value* comes from the character.
    fn literal_type(literal: &Literal, span: Span) -> SemanticResult<Type> {
        match literal {
            Literal::Int(value) => Ok(match value {
                _ if i32::try_from(*value).is_ok() => Type::INT,
                _ if i64::try_from(*value).is_ok() => Type::Long(Sign::Signed),
                _ => Type::Long(Sign::Unsigned),
            }),
            Literal::Char(_) => Ok(Type::INT),
            Literal::Float(_) => Err(SemanticError::unsupported_literal("floating-point", span)),
            Literal::String(_) => Err(SemanticError::unsupported_literal("string", span)),
        }
    }

    /// Checks `array[index]`, which requires an array and an integer index.
    fn analyze_index(&mut self, array: &Expr, index: &Expr) -> SemanticResult<Type> {
        let base_ty = self.analyze_expr(array)?;
        let Type::Array(elem_ty, _) = base_ty else {
            return Err(SemanticError::NotIndexable {
                found: base_ty,
                span: array.span,
            });
        };

        let index_ty = self.analyze_expr(index)?;
        Self::expect_integer(&index_ty, index.span, "an array index must be an integer")?;
        Ok(*elem_ty)
    }

    /// Checks `base.member` or `base->member`.
    ///
    /// The two differ only in what they read the member out of: `.` reads it
    /// from a struct, `->` from a pointer to one. Everything after that -- the
    /// tag must be defined, and it must have such a member -- is the same.
    ///
    /// # Arguments
    ///
    /// * `base` - what the member is read from
    /// * `member` - the name written after the operator
    /// * `is_arrow` - whether it was written `->` rather than `.`
    /// * `span` - the whole access, which is what a bad member is blamed on
    ///
    /// # Returns
    ///
    /// The member's declared type.
    fn analyze_member_access(
        &mut self,
        base: &Expr,
        member: &str,
        is_arrow: bool,
        span: Span,
    ) -> SemanticResult<Type> {
        let base_ty = self.analyze_expr(base)?;

        let struct_ty = match (is_arrow, &base_ty) {
            (false, Type::Struct(_)) => base_ty.clone(),
            (true, Type::Pointer(pointee)) if pointee.is_struct() => (**pointee).clone(),
            _ => {
                return Err(SemanticError::NotAStruct {
                    found: base_ty,
                    span: base.span,
                    is_arrow,
                });
            }
        };

        // A tag that was only forward-declared has no members to read: the
        // pointer to it is fine, following it is not.
        self.expect_complete(&struct_ty, base.span, "in this member access")?;

        let Type::Struct(tag) = &struct_ty else {
            unreachable!("the match above accepted only a struct type");
        };
        let layout = self
            .types
            .structs
            .layout(tag)
            .expect("`expect_complete` accepted the tag, so it is defined");

        match layout.member(member) {
            Some(found) => Ok(found.ty.clone()),
            None => Err(SemanticError::NoSuchMember {
                struct_ty: struct_ty.clone(),
                member: member.to_owned(),
                span,
                suggestion: suggest::nearest(member, layout.member_names()).map(str::to_owned),
                declared: layout.span(),
            }),
        }
    }

    fn analyze_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> SemanticResult<Type> {
        let ExprKind::Identifier(name) = &callee.kind else {
            // Calls through function pointers are not supported yet.
            return Err(SemanticError::Unsupported {
                feature: String::from("calling anything but a named function"),
                span: callee.span,
                context: "in this call",
            });
        };

        let Some(sig) = self.types.functions.get(name).cloned() else {
            return Err(SemanticError::UndeclaredFunction {
                name: name.clone(),
                span: callee.span,
                suggestion: suggest::nearest(name, self.types.functions.keys().map(String::as_str))
                    .map(str::to_owned),
            });
        };

        if args.len() != sig.params.len() {
            return Err(SemanticError::ArgumentCountMismatch {
                name: name.clone(),
                expected: sig.params.len(),
                found: args.len(),
                span,
                declared: sig.span,
            });
        }

        for (arg, param) in args.iter().zip(&sig.params) {
            let found = self.analyze_expr(arg)?;
            Self::expect_type(
                &param.ty,
                &found,
                arg.span,
                Some(param.span),
                "expected because of this parameter",
            )?;
        }

        Ok(sig.return_ty)
    }

    fn analyze_binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> SemanticResult<Type> {
        if op == BinaryOp::Assign {
            if !Self::is_lvalue(lhs) {
                return Err(SemanticError::NotAnObject {
                    span: lhs.span,
                    operation: "assign to",
                });
            }

            let target_ty = self.analyze_expr(lhs)?;
            let value_ty = self.analyze_expr(rhs)?;
            Self::expect_type(
                &target_ty,
                &value_ty,
                rhs.span,
                Some(lhs.span),
                "expected because of this assignment target",
            )?;
            return Ok(target_ty);
        }

        // For the current subset every arithmetic, comparison, logical,
        // bitwise and shift operator takes two integers...
        let lhs_ty = self.analyze_expr(lhs)?;
        let rhs_ty = self.analyze_expr(rhs)?;
        Self::expect_integer(&lhs_ty, lhs.span, OPERANDS_MUST_BE_INTEGERS)?;
        Self::expect_integer(&rhs_ty, rhs.span, OPERANDS_MUST_BE_INTEGERS)?;

        // ... and yields the type they are both converted to, except that a
        // comparison or a logical operator yields the 0 or 1 of an `int`, and
        // that a shift takes its type from its left operand alone.
        Ok(match op {
            BinaryOp::Eq
            | BinaryOp::Neq
            | BinaryOp::Lt
            | BinaryOp::Lte
            | BinaryOp::Gt
            | BinaryOp::Gte
            | BinaryOp::LogicalAnd
            | BinaryOp::LogicalOr => Type::INT,
            // A shift does not pair its operands off against each other the
            // way the arithmetic operators do: each is promoted on its own,
            // and the answer has the type of the left one. `1L << c` is a
            // `long int` however wide `c` is, and `1 << c` an `int` even when
            // `c` is a `long int`.
            BinaryOp::Shl | BinaryOp::Shr => lhs_ty.promoted(),
            _ => Type::common(&lhs_ty, &rhs_ty),
        })
    }

    fn analyze_unary(&mut self, op: UnaryOp, operand: &Expr, span: Span) -> SemanticResult<Type> {
        let operand_ty = self.analyze_expr(operand)?;

        match op {
            UnaryOp::Neg
            | UnaryOp::Not
            | UnaryOp::BitNot
            | UnaryOp::PreInc
            | UnaryOp::PreDec
            | UnaryOp::PostInc
            | UnaryOp::PostDec => {
                Self::expect_integer(&operand_ty, operand.span, OPERANDS_MUST_BE_INTEGERS)?;
                // `!x` answers a question and so is an `int`; `++x` and `x--`
                // read and write one object and keep its type; the arithmetic
                // ones promote their operand, so `-c` on a `char` is an `int`.
                match op {
                    UnaryOp::Not => Ok(Type::INT),
                    UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec => {
                        Ok(operand_ty)
                    }
                    _ => Ok(operand_ty.promoted()),
                }
            }
            UnaryOp::AddressOf => {
                if !Self::is_lvalue(operand) {
                    return Err(SemanticError::NotAnObject {
                        span: operand.span,
                        operation: "take the address of",
                    });
                }
                Ok(Type::Pointer(Box::new(operand_ty)))
            }
            UnaryOp::Deref => match operand_ty {
                Type::Pointer(pointee) => Ok(*pointee),
                _ => Err(SemanticError::NotDereferenceable {
                    found: operand_ty,
                    span,
                }),
            },
        }
    }

    /// Whether an expression denotes an object: something that can be assigned
    /// to and whose address can be taken.
    fn is_lvalue(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(_)
            | ExprKind::Index { .. }
            | ExprKind::Unary(UnaryOp::Deref, _) => true,
            // `p->m` names an object wherever `p` points; `s.m` names one only
            // if `s` itself does.
            ExprKind::MemberAccess { base, is_arrow, .. } => *is_arrow || Self::is_lvalue(base),
            _ => false,
        }
    }

    /// Checks that a value of type `ty` can cross a call boundary.
    ///
    /// Neither aggregate can. A struct passed or returned by value is split
    /// across argument registers or copied into the caller's frame according
    /// to the System V classification rules; an array parameter is converted
    /// to a pointer to its first element. Neither conversion is implemented,
    /// so both travel through a pointer written out by hand.
    ///
    /// # Arguments
    ///
    /// * `ty` - the declared parameter or return type
    /// * `span` - what to blame
    /// * `action` - `passing` or `returning`, which the headline reads with
    /// * `context` - `in this parameter` or `in this return type`
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Unsupported`] for an array or a struct type.
    fn expect_passable(
        ty: &Type,
        span: Span,
        action: &'static str,
        context: &'static str,
    ) -> SemanticResult<()> {
        if !ty.is_aggregate() {
            return Ok(());
        }
        // "By value" is what distinguishes the unsupported struct case from
        // the supported one; an array has no by-value form to contrast with.
        let feature = match ty {
            Type::Array(_, _) => format!("{action} a value of type `{ty}`"),
            _ => format!("{action} `{ty}` by value"),
        };
        Err(SemanticError::Unsupported {
            feature,
            span,
            context,
        })
    }

    /// Checks that an object of type `ty` can exist here.
    ///
    /// It cannot when the size is unknown, which for this language means a
    /// struct tag that was declared but never defined, or an array of one.
    ///
    /// # Arguments
    ///
    /// * `ty` - the type an object is about to be given
    /// * `span` - what to blame
    /// * `context` - where the size was needed, e.g. `in this declaration`
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::IncompleteType`], offering the closest defined
    /// tag when the undefined one looks like a typo of it.
    fn expect_complete(&self, ty: &Type, span: Span, context: &'static str) -> SemanticResult<()> {
        if self.types.structs.is_complete(ty) {
            return Ok(());
        }

        // An array is incomplete exactly when its element type is, so the tag
        // worth suggesting a spelling for is the one at the bottom of it.
        let mut element = ty;
        while let Type::Array(inner, _) = element {
            element = inner;
        }
        let suggestion = match element {
            Type::Struct(tag) => {
                suggest::nearest(tag, self.types.structs.tags()).map(str::to_owned)
            }
            _ => None,
        };

        Err(SemanticError::IncompleteType {
            ty: ty.clone(),
            span,
            context,
            suggestion,
        })
    }

    /// Checks that `found` is one of the integer types.
    ///
    /// # Arguments
    ///
    /// * `found` - the type of the expression written there
    /// * `span` - the expression to blame
    /// * `context` - why an integer is required
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::TypeError`] blaming `span`, reporting `int` as
    /// the expected type: it is the narrower of the two that would do.
    fn expect_integer(found: &Type, span: Span, context: &'static str) -> SemanticResult<()> {
        match found.is_integer() {
            true => Ok(()),
            false => Err(SemanticError::TypeError {
                expected: Type::INT,
                found: found.clone(),
                span,
                origin: None,
                context,
            }),
        }
    }

    /// Checks that a value of type `found` may be used where `expected` is
    /// required, implicitly converting it if C says so.
    ///
    /// # Arguments
    ///
    /// * `expected` - the type this position requires
    /// * `found` - the type of the expression written there
    /// * `span` - the expression to blame
    /// * `origin` - the declaration that imposed `expected`, when there is one
    /// * `context` - why `expected` is required; see
    ///   [`SemanticError::TypeError`]
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::TypeError`] blaming `span`.
    fn expect_type(
        expected: &Type,
        found: &Type,
        span: Span,
        origin: Option<Span>,
        context: &'static str,
    ) -> SemanticResult<()> {
        if expected == found {
            return Ok(());
        }
        // An integer converts to any other integer type: `long int x = i;`
        // widens, `int i = x;` truncates, and both are silent in C.
        if expected.is_integer() && found.is_integer() {
            return Ok(());
        }
        // Allow assigning integer literal 0 (NULL) to any pointer type.
        if matches!(expected, Type::Pointer(_)) && found.is_integer() {
            return Ok(());
        }
        Err(SemanticError::TypeError {
            expected: expected.clone(),
            found: found.clone(),
            span,
            origin,
            context,
        })
    }
}

/// Why an arithmetic operand must be an integer; shared by the unary and
/// binary checks so the two never word it differently.
const OPERANDS_MUST_BE_INTEGERS: &str =
    "arithmetic, comparison and bitwise operators only apply to integers";

impl SemanticError {
    /// Error for a literal whose type the compiler cannot represent yet.
    fn unsupported_literal(kind: &str, span: Span) -> Self {
        SemanticError::Unsupported {
            feature: format!("{kind} literals"),
            span,
            context: "in this expression",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::Lexer;
    use crate::frontend::parser::Parser;

    /// Analyzes `src` and returns the errors reported.
    fn analyze(src: &str) -> Vec<SemanticError> {
        let mut parser = Parser::from_lexer(Lexer::new(src)).expect("lexing should succeed");
        let (decls, parse_errors) = parser.parse_translation_unit();
        assert!(
            parse_errors.is_empty(),
            "unexpected parse errors: {parse_errors:?}"
        );

        SemanticAnalyzer::new().analyze_program(&decls)
    }

    /// Analyzes `src`, asserting that it is accepted.
    fn analyze_ok(src: &str) {
        let errors = analyze(src);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
    }

    /// Analyzes `src`, asserting that exactly one error is reported.
    fn analyze_err(src: &str) -> SemanticError {
        let mut errors = analyze(src);
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one error, got {errors:?}"
        );
        errors.remove(0)
    }

    /// Analyzes `src`, asserting that it is accepted, and returns the types.
    fn types_of(src: &str) -> TypeMap {
        let mut parser = Parser::from_lexer(Lexer::new(src)).expect("lexing should succeed");
        let (decls, parse_errors) = parser.parse_translation_unit();
        assert!(
            parse_errors.is_empty(),
            "unexpected parse errors: {parse_errors:?}"
        );

        let mut analyzer = SemanticAnalyzer::new();
        let errors = analyzer.analyze_program(&decls);
        assert!(errors.is_empty(), "expected no errors, got {errors:?}");
        analyzer.into_types()
    }

    #[test]
    fn accepts_a_struct_and_records_where_its_members_sit() {
        // Arrange / Act
        let types = types_of(
            "struct Point { char tag; int x; int y; };
             int main() { struct Point p; p.x = 1; return p.y; }",
        );

        // Assert: the layout travels to the middle-end inside the type map,
        // with the padding C's alignment rules put in.
        let layout = types
            .structs()
            .layout("Point")
            .expect("`Point` was defined");
        assert_eq!(layout.member("tag").expect("declared").offset, 0);
        assert_eq!(layout.member("x").expect("declared").offset, 4);
        assert_eq!(layout.member("y").expect("declared").offset, 8);
        assert_eq!(layout.size(), 12);
    }

    #[test]
    fn a_member_has_the_type_it_was_declared_with() {
        analyze_ok(
            "struct Point { long int x; };
             int main() { struct Point p; long int n = p.x; return (int)n; }",
        );
    }

    #[test]
    fn reads_a_member_through_a_pointer_with_arrow() {
        analyze_ok(
            "struct Point { int x; };
             int get(struct Point *p) { return p->x; }
             int main() { struct Point p; p.x = 1; return get(&p); }",
        );
    }

    #[test]
    fn a_struct_may_hold_a_pointer_to_itself() {
        // A pointer is a machine word whatever it points at, so it needs no
        // layout for the tag -- which is what makes a linked list expressible.
        analyze_ok("struct Node { int value; struct Node *next; };");
    }

    #[test]
    fn fails_on_a_struct_that_contains_itself() {
        // The tag has no layout until its own definition is finished, so a
        // member of that type has no size.
        match analyze_err("struct Node { struct Node inner; };") {
            SemanticError::IncompleteType { ty, .. } => {
                assert_eq!(ty, Type::Struct("Node".to_string()));
            }
            other => panic!("expected IncompleteType, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_an_object_of_a_forward_declared_struct() {
        match analyze_err("struct Point; int main() { struct Point p; return 0; }") {
            SemanticError::IncompleteType { context, .. } => {
                assert_eq!(context, "in this declaration");
            }
            other => panic!("expected IncompleteType, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_a_member_the_struct_does_not_have() {
        match analyze_err(
            "struct Point { int counter; };
             int main() { struct Point p; return p.countr; }",
        ) {
            SemanticError::NoSuchMember {
                member, suggestion, ..
            } => {
                assert_eq!(member, "countr");
                assert_eq!(suggestion.as_deref(), Some("counter"));
            }
            other => panic!("expected NoSuchMember, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_the_wrong_member_operator() {
        // `.` on a pointer and `->` on a value are each other's mistake.
        match analyze_err(
            "struct Point { int x; };
             int main() { struct Point p; struct Point *q = &p; return q.x; }",
        ) {
            SemanticError::NotAStruct { is_arrow, .. } => assert!(!is_arrow),
            other => panic!("expected NotAStruct, got {other:?}"),
        }
        match analyze_err(
            "struct Point { int x; };
             int main() { struct Point p; return p->x; }",
        ) {
            SemanticError::NotAStruct { is_arrow, .. } => assert!(is_arrow),
            other => panic!("expected NotAStruct, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_a_member_read_from_a_non_struct() {
        match analyze_err("int main() { int n = 1; return n.x; }") {
            SemanticError::NotAStruct { found, .. } => assert_eq!(found, Type::INT),
            other => panic!("expected NotAStruct, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_a_struct_defined_twice() {
        match analyze_err("struct Point { int x; }; struct Point { int y; };") {
            SemanticError::RedeclaredStruct { name, .. } => assert_eq!(name, "Point"),
            other => panic!("expected RedeclaredStruct, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_a_member_name_used_twice() {
        match analyze_err("struct Point { int x; int x; };") {
            SemanticError::RedeclaredVariable { name, .. } => assert_eq!(name, "x"),
            other => panic!("expected RedeclaredVariable, got {other:?}"),
        }
    }

    #[test]
    fn a_struct_only_crosses_a_call_boundary_through_a_pointer() {
        // Splitting a struct across argument registers is the System V
        // classification, which this compiler does not implement.
        analyze_ok(
            "struct Point { int x; };
             int take(struct Point *p) { return p->x; }
             int main() { struct Point p; p.x = 1; return take(&p); }",
        );
        assert!(matches!(
            analyze_err("struct Point { int x; }; int take(struct Point p) { return p.x; }"),
            SemanticError::Unsupported { .. }
        ));
        assert!(matches!(
            analyze_err(
                "struct Point { int x; }; struct Point make() { struct Point p; return p; }"
            ),
            SemanticError::Unsupported { .. }
        ));
        // An array parameter is C's other aggregate at a call boundary, and
        // needs the same decay this compiler does not do yet.
        assert!(matches!(
            analyze_err("int first(int a[3]) { return a[0]; }"),
            SemanticError::Unsupported { .. }
        ));
    }

    #[test]
    fn assigns_only_between_the_same_struct_type() {
        analyze_ok(
            "struct Point { int x; };
             int main() { struct Point a; struct Point b; a = b; return a.x; }",
        );
        assert!(matches!(
            analyze_err(
                "struct A { int x; }; struct B { int x; };
                 int main() { struct A a; struct B b; a = b; return 0; }"
            ),
            SemanticError::TypeError { .. }
        ));
    }

    #[test]
    fn a_member_is_an_object_that_can_be_assigned_and_addressed() {
        analyze_ok(
            "struct Point { int x; };
             int main() { struct Point p; p.x = 1; int *q = &p.x; return *q; }",
        );
    }

    #[test]
    fn fails_on_a_struct_used_as_a_number() {
        match analyze_err("struct A { int x; }; int main() { struct A a; return a + 1; }") {
            SemanticError::TypeError { found, .. } => {
                assert_eq!(found, Type::Struct("A".to_string()));
            }
            other => panic!("expected TypeError, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_a_cast_to_a_struct() {
        // There is no such cast in C: a cast converts one value, and a struct
        // is not one value.
        assert!(matches!(
            analyze_err("struct A { int x; }; int main() { int n = 1; return ((struct A)n).x; }"),
            SemanticError::TypeError { .. }
        ));
    }

    #[test]
    fn valid_integer_declaration_and_assignment() {
        analyze_ok("int main() { int x = 5; x = x + 1; return x; }");
    }

    #[test]
    fn fails_on_undeclared_variable() {
        match analyze_err("int main() { y = 10; return 0; }") {
            SemanticError::UndeclaredVariable { name, .. } => assert_eq!(name, "y"),
            other => panic!("expected UndeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn fails_on_redeclaration_in_same_scope() {
        match analyze_err("int main() { int x; int x; return 0; }") {
            SemanticError::RedeclaredVariable { name, .. } => assert_eq!(name, "x"),
            other => panic!("expected RedeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn allows_shadowing_in_a_nested_block() {
        analyze_ok("int main() { int x = 1; { int x = 2; x = x + 1; } return x; }");
    }

    #[test]
    fn accepts_a_variable_declared_in_a_for_init_clause() {
        analyze_ok(
            "int main() { int s = 0; for (int i = 0; i < 3; i = i + 1) { s = s + i; } return s; }",
        );
    }

    #[test]
    fn for_loop_variable_does_not_outlive_the_loop() {
        match analyze_err("int main() { for (int i = 0; i < 3; i = i + 1) { } return i; }") {
            SemanticError::UndeclaredVariable { name, .. } => assert_eq!(name, "i"),
            other => panic!("expected UndeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn accepts_empty_statements() {
        analyze_ok("int main() { ; if (1) ; else ; while (0) ; return 0; }");
    }

    #[test]
    fn accepts_a_jump_inside_the_loop_it_acts_on() {
        // However deeply the jump is nested inside the body, and whichever
        // kind of loop encloses it.
        analyze_ok("int main() { while (1) { if (1) { break; } continue; } return 0; }");
        analyze_ok("int main() { for (int i = 0; i < 1; i = i + 1) { continue; } return 0; }");
    }

    #[test]
    fn rejects_a_break_with_no_loop_to_leave() {
        match analyze_err("int main() { break; }") {
            SemanticError::JumpOutsideLoop { keyword, .. } => assert_eq!(keyword, "break"),
            other => panic!("expected JumpOutsideLoop, got: {other:?}"),
        }
    }

    #[test]
    fn rejects_a_continue_after_the_loop_has_been_left() {
        // The loop encloses the jump in the source text only up to its closing
        // brace: what follows is outside it again.
        match analyze_err("int main() { while (1) { break; } continue; }") {
            SemanticError::JumpOutsideLoop { keyword, .. } => assert_eq!(keyword, "continue"),
            other => panic!("expected JumpOutsideLoop, got: {other:?}"),
        }
    }

    #[test]
    fn a_loop_in_one_function_does_not_reach_into_the_next() {
        // The depth is restored on the way out of every loop, so a `break` in
        // a later function is still outside one.
        match analyze_err("int loop() { while (1) { break; } return 0; } int main() { break; }") {
            SemanticError::JumpOutsideLoop { keyword, .. } => assert_eq!(keyword, "break"),
            other => panic!("expected JumpOutsideLoop, got: {other:?}"),
        }
    }

    #[test]
    fn fails_on_argument_count_mismatch() {
        match analyze_err("int add(int a, int b) { return a + b; } int main() { return add(1); }") {
            SemanticError::ArgumentCountMismatch {
                name,
                expected,
                found,
                ..
            } => {
                assert_eq!(name, "add");
                assert_eq!((expected, found), (2, 1));
            }
            other => panic!("expected ArgumentCountMismatch, got: {other:?}"),
        }
    }

    #[test]
    fn fails_on_assignment_to_non_lvalue() {
        let error = analyze_err("int main() { 1 = 2; return 0; }");
        assert!(matches!(error, SemanticError::NotAnObject { .. }));
    }

    #[test]
    fn checks_pointer_dereference_and_address_of() {
        analyze_ok("int main() { int x = 1; int *p = &x; *p = 2; return *p; }");
    }

    #[test]
    fn rejects_dereference_of_non_pointer() {
        let error = analyze_err("int main() { int x = 1; return *x; }");
        assert!(matches!(error, SemanticError::NotDereferenceable { .. }));
    }

    #[test]
    fn accepts_long_declarations_and_the_conversions_between_the_two() {
        analyze_ok(
            "int main() { long int wide = 5; int narrow = wide; wide = narrow; return narrow; }",
        );
    }

    #[test]
    fn arithmetic_on_a_long_and_an_int_yields_a_long() {
        // A `long int` may only be initialized from something that converts to
        // one, so accepting this is the analyzer saying the sum is 64 bits.
        analyze_ok("int main() { int a = 1; long int b = 2; long int c = a + b; return c; }");
    }

    #[test]
    fn a_comparison_of_longs_is_an_int() {
        analyze_ok("int main() { long int a = 1; long int b = 2; int less = a < b; return less; }");
    }

    /// The signed and unsigned forms of the type `bytes` bytes wide.
    fn pair(bytes: u8) -> (Type, Type) {
        (
            Type::integer_of(bytes, Sign::Signed),
            Type::integer_of(bytes, Sign::Unsigned),
        )
    }

    #[test]
    fn a_char_promotes_to_an_int_before_any_operator_sees_it() {
        // Arrange
        let (char_, uchar) = pair(1);
        let (int, uint) = pair(4);
        let (long, ulong) = pair(8);

        // Act / Assert: the integer promotions, which are why `c1 + c2` has
        // type `int` rather than `char`. Both eight-bit types promote to the
        // *signed* `int`, which can represent every value either of them has.
        assert_eq!(char_.promoted(), int);
        assert_eq!(uchar.promoted(), int);
        assert_eq!(int.promoted(), int);
        assert_eq!(uint.promoted(), uint);
        assert_eq!(long.promoted(), long);
        assert_eq!(ulong.promoted(), ulong);

        assert_eq!(Type::common(&char_, &uchar), int);
        assert_eq!(Type::common(&char_, &int), int);
        assert_eq!(Type::common(&char_, &long), long);
    }

    #[test]
    fn the_usual_arithmetic_conversions_favour_width_then_unsignedness() {
        // Arrange
        let (int, uint) = pair(4);
        let (long, ulong) = pair(8);

        // Act / Assert: equally wide, the unsigned type wins, because no type
        // here can represent every value of both ...
        assert_eq!(Type::common(&int, &uint), uint);
        assert_eq!(Type::common(&uint, &int), uint);
        assert_eq!(Type::common(&long, &ulong), ulong);

        // ... but a wider signed type can represent every value of a narrower
        // unsigned one, so width decides first.
        assert_eq!(Type::common(&uint, &long), long);
        assert_eq!(Type::common(&long, &uint), long);
        assert_eq!(Type::common(&int, &ulong), ulong);
    }

    #[test]
    fn accepts_the_unsigned_types_and_the_conversions_between_them() {
        analyze_ok(
            "unsigned int scale(unsigned char c) { return c * 2; } \
             int main() { unsigned int u = 4294967295; unsigned long int w = u; \
             unsigned char b = u; int i = u; u = i; return scale(b) + w + u; }",
        );
    }

    #[test]
    fn unsigned_is_a_type_specifier_on_its_own() {
        // `unsigned` with nothing after it means `unsigned int`.
        analyze_ok("int main() { unsigned u = 1; unsigned int v = u; return v; }");
    }

    #[test]
    fn a_character_constant_has_type_int() {
        // Arrange / Act / Assert: `'a'` is an `int` in C, which is why
        // `sizeof('a')` is 4 and not 1.
        let span = Span::new(1, 1, 0, 3);
        assert_eq!(
            SemanticAnalyzer::literal_type(&Literal::Char(b'a'), span),
            Ok(Type::INT)
        );
    }

    #[test]
    fn accepts_char_declarations_and_the_conversions_around_them() {
        analyze_ok(
            "char twice(char c) { return c + c; } \
             int main() { char c = 'A'; int wide = c; c = wide; return twice((char) wide); }",
        );
    }

    #[test]
    fn accepts_a_char_wherever_an_integer_is_required() {
        analyze_ok(
            "int main() { char c = 'a'; char a[2]; a[c - 'a'] = c; \
             if (c) { while (c > 'A') { c = c - 1; } } return !c; }",
        );
    }

    #[test]
    fn rejects_a_pointer_where_an_integer_is_required() {
        // The two integer types convert to each other, but a pointer is not
        // one of them.
        let error = analyze_err("long int main() { int x = 1; long int y = &x; return y; }");
        match error {
            SemanticError::TypeError {
                expected, found, ..
            } => {
                assert_eq!(expected, Type::Long(Sign::Signed));
                assert_eq!(found, Type::Pointer(Box::new(Type::INT)));
            }
            other => panic!("expected TypeError, got: {other:?}"),
        }
    }

    #[test]
    fn an_integer_literal_too_large_for_an_int_is_a_long() {
        // Arrange / Act / Assert
        let span = Span::new(1, 1, 0, 1);
        assert_eq!(
            SemanticAnalyzer::literal_type(&Literal::Int(2147483647), span),
            Ok(Type::INT)
        );
        assert_eq!(
            SemanticAnalyzer::literal_type(&Literal::Int(2147483648), span),
            Ok(Type::Long(Sign::Signed))
        );
    }

    #[test]
    fn rejects_unsupported_types() {
        let error = analyze_err("int main() { float f; return 0; }");
        assert!(matches!(error, SemanticError::Unsupported { .. }));
    }

    #[test]
    fn reports_errors_from_several_functions() {
        let errors = analyze("int f() { return a; } int g() { return b; }");
        assert_eq!(errors.len(), 2, "got {errors:?}");
    }

    #[test]
    fn headline_is_the_diagnostic_message() {
        let error = analyze_err("int main() { return missing(); }");
        let headline = error.to_string();

        assert_eq!(headline, "cannot find function `missing` in this scope");
        assert_eq!(error.into_diagnostic().message, headline);
    }

    #[test]
    fn suggests_a_similarly_spelled_variable() {
        match analyze_err("int main() { int value = 1; return valu; }") {
            SemanticError::UndeclaredVariable { suggestion, .. } => {
                assert_eq!(suggestion.as_deref(), Some("value"));
            }
            other => panic!("expected UndeclaredVariable, got: {other:?}"),
        }
    }

    #[test]
    fn points_a_redeclaration_at_the_first_one() {
        match analyze_err("int main() { int x; int x; return 0; }") {
            SemanticError::RedeclaredVariable { span, previous, .. } => {
                assert_eq!((previous.line, previous.column), (1, 18));
                assert_eq!((span.line, span.column), (1, 25));
            }
            other => panic!("expected RedeclaredVariable, got: {other:?}"),
        }
    }
}
