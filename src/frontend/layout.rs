//! Struct layout: how much a struct occupies and where each member sits in it.
//!
//! C does not say where a member goes; the ABI does. The System V AMD64 ABI
//! lays a struct out the way almost every C implementation does: members in
//! declaration order, each one pushed up to the next multiple of its own
//! alignment, and the whole object rounded up to the alignment of its widest
//! member so that an array of them keeps every element aligned.
//!
//! That single rule is what the rest of the compiler needs from a struct. Once
//! a member has a byte offset, reading `s.x` is a load from an address the
//! compiler already knows how to form, which is why nothing downstream of here
//! has a notion of "struct" at all -- only of objects, offsets and widths.
//!
//! # Padding
//!
//! ```text
//! struct Mixed { char c; int n; };
//!
//!   byte 0   1   2   3   4   5   6   7
//!        c  ---padding---  n   n   n   n     size 8, alignment 4
//! ```
//!
//! `n` cannot start at byte 1 because an `int` is four-byte aligned, so three
//! bytes go unused -- which is why `sizeof` a struct is not the sum of its
//! members' sizes.

use std::collections::HashMap;

use crate::frontend::semantic::Type;
use crate::frontend::span::Span;

/// One member of a struct: what it is called, what it holds, and where it sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub ty: Type,
    /// Bytes from the start of the enclosing object to this member.
    pub offset: usize,
    /// Where the member was declared, quoted when a use of it is rejected.
    pub span: Span,
}

/// What one struct type occupies and where each of its members sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayout {
    /// The members in declaration order, which is the order a diagnostic
    /// lists them in.
    members: Vec<Member>,
    /// Position of each member in `members`, so a lookup by name is constant
    /// time however many members the struct has.
    index: HashMap<String, usize>,
    size: usize,
    align: usize,
    /// Where the struct was defined, quoted when a use of it is rejected.
    span: Span,
}

impl StructLayout {
    /// The member called `name`, if the struct has one.
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.index.get(name).map(|&at| &self.members[at])
    }

    /// Every member's name, in declaration order.
    ///
    /// This is what a "no such member" diagnostic searches for a near miss.
    pub fn member_names(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|member| member.name.as_str())
    }

    /// Bytes an object of this type occupies, padding included.
    pub fn size(&self) -> usize {
        self.size
    }

    /// The alignment an object of this type requires, in bytes.
    pub fn align(&self) -> usize {
        self.align
    }

    /// Where the struct was defined.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// Every struct type the translation unit defines, by tag.
///
/// A tag that was only forward-declared is absent: it names a type whose
/// layout nothing knows yet, and C calls such a type incomplete. Objects of an
/// incomplete type cannot exist, which is exactly what
/// [`StructTable::is_complete`] is asked before one is created.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StructTable {
    layouts: HashMap<String, StructLayout>,
}

impl StructTable {
    /// The layout of `struct tag`, or `None` if the tag is incomplete.
    pub fn layout(&self, tag: &str) -> Option<&StructLayout> {
        self.layouts.get(tag)
    }

    /// Every defined tag, in no particular order.
    ///
    /// Used to suggest a near miss for a tag that does not exist.
    pub fn tags(&self) -> impl Iterator<Item = &str> {
        self.layouts.keys().map(String::as_str)
    }

    /// Records `layout` under `tag`.
    ///
    /// # Returns
    ///
    /// The layout `tag` already had, if it was defined before. A struct may be
    /// defined only once, so a caller that gets one back reports it.
    pub fn define(&mut self, tag: String, layout: StructLayout) -> Option<StructLayout> {
        self.layouts.insert(tag, layout)
    }

    /// Lays `members` out in declaration order.
    ///
    /// Every member is placed at the next offset its own alignment allows, and
    /// the object as a whole is rounded up to the widest alignment among them
    /// -- so that `struct Point p[2]` keeps `p[1]` as well aligned as `p[0]`.
    ///
    /// # Arguments
    ///
    /// * `members` - each member's name, type and declaration site, in order
    /// * `span` - where the struct is defined
    ///
    /// # Panics
    ///
    /// Panics if any member's type is incomplete or `void`; the caller checks
    /// both first so that the user sees a diagnostic rather than a crash.
    pub fn lay_out(&self, members: Vec<(String, Type, Span)>, span: Span) -> StructLayout {
        let mut laid_out = Vec::with_capacity(members.len());
        let mut index = HashMap::with_capacity(members.len());
        let mut offset = 0;
        // An empty struct still has to be aligned somewhere, and 1 is the
        // alignment that constrains nothing.
        let mut align = 1;

        for (name, ty, member_span) in members {
            let (size, member_align) = (self.size_of(&ty), self.align_of(&ty));

            // The member starts at the next offset its own alignment allows;
            // the padding that skips to it is what makes a struct larger than
            // the sum of its members.
            offset = align_up(offset, member_align);
            align = align.max(member_align);

            index.insert(name.clone(), laid_out.len());
            laid_out.push(Member {
                name,
                ty,
                offset,
                span: member_span,
            });
            offset += size;
        }

        StructLayout {
            members: laid_out,
            index,
            size: align_up(offset, align),
            align,
            span,
        }
    }

    /// The bytes an object of type `ty` occupies.
    ///
    /// # Panics
    ///
    /// Panics for `void` and for an incomplete struct type. Neither can be the
    /// type of an object, and semantic analysis rejects both before one is
    /// created, so reaching this is a compiler bug.
    pub fn size_of(&self, ty: &Type) -> usize {
        match ty {
            Type::Char(_) => 1,
            Type::Int(_) => 4,
            // Every pointer is a machine word, whatever it points at -- which
            // is why a pointer to an incomplete struct is perfectly sized.
            Type::Long(_) | Type::Pointer(_) => 8,
            Type::Array(element, count) => self.size_of(element) * count,
            Type::Struct(tag) => self.complete(tag).size(),
            Type::Void => panic!("Compiler Bug: `void` has no size"),
        }
    }

    /// The alignment an object of type `ty` requires, in bytes.
    ///
    /// Every scalar type is aligned to its own size, an array to its element's
    /// alignment, and a struct to the widest alignment among its members.
    ///
    /// # Panics
    ///
    /// Panics under the same conditions as [`StructTable::size_of`].
    pub fn align_of(&self, ty: &Type) -> usize {
        match ty {
            Type::Char(_) => 1,
            Type::Int(_) => 4,
            Type::Long(_) | Type::Pointer(_) => 8,
            // An array is as aligned as one element: the elements are packed,
            // so aligning the first aligns them all.
            Type::Array(element, _) => self.align_of(element),
            Type::Struct(tag) => self.complete(tag).align(),
            Type::Void => panic!("Compiler Bug: `void` has no alignment"),
        }
    }

    /// Whether an object of type `ty` can exist.
    ///
    /// It cannot if the type is `void`, a struct that was never defined, or an
    /// array of either: none of the three has a size, so nothing can reserve
    /// storage for one. A *pointer* to any of them is complete -- a pointer is
    /// a machine word regardless -- which is what lets `struct Node *next;`
    /// appear inside `struct Node` itself.
    pub fn is_complete(&self, ty: &Type) -> bool {
        match ty {
            Type::Char(_) | Type::Int(_) | Type::Long(_) | Type::Pointer(_) => true,
            Type::Array(element, _) => self.is_complete(element),
            Type::Struct(tag) => self.layouts.contains_key(tag),
            Type::Void => false,
        }
    }

    /// The layout of `struct tag`, which the caller has established exists.
    ///
    /// # Panics
    ///
    /// Panics if the tag is incomplete.
    fn complete(&self, tag: &str) -> &StructLayout {
        self.layouts
            .get(tag)
            .expect("Compiler Bug: an object of an incomplete struct type was created")
    }
}

/// Rounds `value` up to the next multiple of `alignment`.
fn align_up(value: usize, alignment: usize) -> usize {
    debug_assert!(
        alignment > 0 && alignment & (alignment - 1) == 0,
        "an alignment is a positive power of two"
    );
    // Adding `alignment - 1` carries into the next multiple; the mask clears
    // whatever is below it.
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::frontend::ast::Sign;

    const SPAN: Span = Span::new(1, 1, 0, 1);

    /// The layout of a struct whose members are `(name, type)` in order.
    fn lay_out(table: &StructTable, members: &[(&str, Type)]) -> StructLayout {
        let members = members
            .iter()
            .map(|(name, ty)| ((*name).to_string(), ty.clone(), SPAN))
            .collect();
        table.lay_out(members, SPAN)
    }

    /// A table holding one struct definition, under the given tag.
    fn table_with(tag: &str, members: &[(&str, Type)]) -> StructTable {
        let mut table = StructTable::default();
        let layout = lay_out(&table, members);
        table.define(tag.to_string(), layout);
        table
    }

    #[test]
    fn members_sit_at_the_offsets_declaration_order_gives_them() {
        // Arrange / Act: two `int`s, which need no padding between them.
        let layout = lay_out(
            &StructTable::default(),
            &[("x", Type::INT), ("y", Type::INT)],
        );

        // Assert
        assert_eq!(layout.member("x").expect("`x` was declared").offset, 0);
        assert_eq!(layout.member("y").expect("`y` was declared").offset, 4);
        assert_eq!(layout.size(), 8);
        assert_eq!(layout.align(), 4);
    }

    #[test]
    fn a_member_is_padded_up_to_its_own_alignment() {
        // Arrange / Act: an `int` cannot start at byte 1, so three bytes go
        // unused -- see the diagram in the module documentation.
        let layout = lay_out(
            &StructTable::default(),
            &[("c", Type::Char(Sign::Signed)), ("n", Type::INT)],
        );

        // Assert
        assert_eq!(layout.member("c").expect("`c` was declared").offset, 0);
        assert_eq!(layout.member("n").expect("`n` was declared").offset, 4);
        assert_eq!(layout.size(), 8);
    }

    #[test]
    fn the_object_is_rounded_up_to_its_widest_members_alignment() {
        // Arrange / Act: eight bytes of `long int` then one of `char` is nine
        // bytes of members ...
        let layout = lay_out(
            &StructTable::default(),
            &[
                ("n", Type::Long(Sign::Signed)),
                ("c", Type::Char(Sign::Signed)),
            ],
        );

        // Assert: ... rounded up to sixteen, so that the next element of an
        // array of these is still eight-byte aligned.
        assert_eq!(layout.size(), 16);
        assert_eq!(layout.align(), 8);
    }

    #[test]
    fn a_nested_struct_carries_its_own_size_and_alignment() {
        // Arrange: `struct Inner { int a; int b; }`, eight bytes aligned to
        // four.
        let table = table_with("Inner", &[("a", Type::INT), ("b", Type::INT)]);

        // Act: a `char` followed by that struct.
        let layout = lay_out(
            &table,
            &[
                ("c", Type::Char(Sign::Signed)),
                ("inner", Type::Struct("Inner".to_string())),
            ],
        );

        // Assert: the nested struct is aligned to four, not to one.
        assert_eq!(layout.member("inner").expect("declared").offset, 4);
        assert_eq!(layout.size(), 12);
        assert_eq!(layout.align(), 4);
    }

    #[test]
    fn an_array_member_is_as_long_as_its_elements_make_it() {
        // Arrange / Act
        let layout = lay_out(
            &StructTable::default(),
            &[
                ("data", Type::Array(Box::new(Type::INT), 3)),
                ("n", Type::INT),
            ],
        );

        // Assert: twelve bytes of array, then the `int` right after it.
        assert_eq!(layout.member("n").expect("declared").offset, 12);
        assert_eq!(layout.size(), 16);
    }

    #[test]
    fn an_array_of_structs_is_as_long_as_the_padded_element() {
        // Arrange: `struct Odd { int n; char c; }` occupies eight bytes, three
        // of which are padding.
        let table = table_with("Odd", &[("n", Type::INT), ("c", Type::Char(Sign::Signed))]);
        let element = Type::Struct("Odd".to_string());

        // Act / Assert: the padding is part of every element, which is the
        // whole reason the object is rounded up.
        assert_eq!(table.size_of(&element), 8);
        assert_eq!(table.size_of(&Type::Array(Box::new(element), 3)), 24);
    }

    #[test]
    fn only_a_defined_tag_can_be_the_type_of_an_object() {
        // Arrange
        let table = table_with("Point", &[("x", Type::INT)]);
        let defined = Type::Struct("Point".to_string());
        let never_defined = Type::Struct("Ghost".to_string());

        // Act / Assert: a pointer to an undefined struct is still a machine
        // word, which is what lets a struct hold a pointer to itself.
        assert!(table.is_complete(&defined));
        assert!(!table.is_complete(&never_defined));
        assert!(table.is_complete(&Type::Pointer(Box::new(never_defined.clone()))));
        assert!(!table.is_complete(&Type::Array(Box::new(never_defined), 2)));
        assert!(!table.is_complete(&Type::Void));
    }

    #[test]
    fn an_empty_struct_occupies_nothing() {
        // Arrange / Act: C has no empty struct, but GCC accepts one as an
        // extension and gives it no storage at all.
        let layout = lay_out(&StructTable::default(), &[]);

        // Assert
        assert_eq!(layout.size(), 0);
        assert_eq!(layout.align(), 1);
        assert_eq!(layout.member("anything"), None);
    }
}
