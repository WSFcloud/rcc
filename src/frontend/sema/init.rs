use crate::frontend::parser::ast::{Initializer, InitializerKind};
use crate::frontend::sema::check::expr;
use crate::frontend::sema::context::SemaContext;
use crate::frontend::sema::typed_ast::TypedInitializer;
use crate::frontend::sema::types::{FieldId, TypeId};

/// An element in the initialization path.
///
/// This represents one step in the path to a subobject being initialized.
/// For example, in `struct S s = { .a[5].b = 42 }`, the path is:
/// - `StructField(a)`
/// - `ArrayIndex(5)`
/// - `StructField(b)`
pub enum InitPathElem {
    /// Index into an array element.
    ArrayIndex(usize),
    /// Access to a struct field.
    StructField(FieldId),
    /// Access to a union field.
    UnionField(FieldId),
}

/// Cursor for tracking the current position during initialization.
///
/// This structure maintains the state while walking through an initializer,
/// tracking which subobject is currently being initialized.
pub struct InitCursor {
    /// The type of the object being initialized.
    pub object_ty: TypeId,
    /// The path from the root object to the current subobject.
    pub path: Vec<InitPathElem>,
    /// The type of the current subobject.
    pub current_subobject_ty: TypeId,
    /// The next implicit array index (for brace elision).
    pub next_implicit_index: usize,
}

/// Entry point for initializer semantic checking.
///
/// This function type-checks an initializer and produces a typed initializer.
/// In the framework stage, only simple expression initializers are supported.
///
/// # TODO
/// - Implement aggregate initialization with designators
/// - Implement brace elision and implicit zero-initialization
/// - Validate initializer compatibility with target type
pub fn lower_initializer(
    cx: &mut SemaContext<'_>,
    target_ty: TypeId,
    init: &Initializer,
) -> TypedInitializer {
    match &init.kind {
        InitializerKind::Expr(expr_node) => TypedInitializer::Expr(expr::lower_expr(cx, expr_node)),
        InitializerKind::Aggregate(_) => {
            let _ = target_ty;
            todo!("designator path walk and aggregate completion in init::lower_initializer")
        }
    }
}
