use crate::common::span::SourceSpan;
use crate::frontend::parser::ast::StorageClass;
use crate::frontend::sema::diagnostic::{SemaDiagnostic, SemaDiagnosticCode};
use crate::frontend::sema::types::{TypeArena, TypeId, composite_type};

/// Opaque identifier for a symbol in the symbol arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(pub u32);

/// The kind of a symbol in C's ordinary namespace.
///
/// C has four separate namespaces: ordinary (objects/functions/typedef/enum constants),
/// tag (struct/union/enum), label, and member (struct/union fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    /// Variable or parameter.
    Object,
    /// Function.
    Function,
    /// Typedef name.
    Typedef,
    /// Enum constant (enumerator).
    EnumConst,
}

/// Linkage of a symbol (C99 6.2.2).
///
/// Linkage determines whether multiple declarations refer to the same entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    /// External linkage: visible across translation units.
    External,
    /// Internal linkage: visible only within the current translation unit.
    Internal,
    /// No linkage: local to the current scope.
    None,
}

/// Definition status of a symbol.
///
/// This tracks whether a symbol has been declared, tentatively defined,
/// or fully defined (C99 6.9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionStatus {
    /// Only declared (e.g., `extern int x;` or `int f();`).
    Declared,
    /// Tentative definition (e.g., `int x;` at file scope without initializer).
    Tentative,
    /// Fully defined (e.g., `int x = 0;` or function with body).
    Defined,
}

/// Storage category for object symbols needed by MIR lowering.
///
/// This metadata preserves where an object came from in source:
/// - `FileScope`: any file-scope object declaration/definition
/// - `Extern`: block-scope `extern` object declaration
/// - `Static`: block-scope `static` object
/// - `Auto` / `Register`: block-scope automatic objects
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectStorageClass {
    Auto,
    Register,
    Static,
    Extern,
    FileScope,
}

/// A symbol in the ordinary namespace.
///
/// Symbols are stored in a `SymbolArena` and referenced by `SymbolId`.
/// Fields are private to enforce invariants through accessor methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    name: String,
    kind: SymbolKind,
    ty: TypeId,
    linkage: Linkage,
    status: DefinitionStatus,
    decl_span: SourceSpan,
    object_storage_class: Option<ObjectStorageClass>,
}

impl Symbol {
    /// Creates a new symbol.
    pub fn new(
        name: String,
        kind: SymbolKind,
        ty: TypeId,
        linkage: Linkage,
        status: DefinitionStatus,
        decl_span: SourceSpan,
    ) -> Self {
        Self {
            name,
            kind,
            ty,
            linkage,
            status,
            decl_span,
            object_storage_class: None,
        }
    }

    /// Returns the symbol's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the symbol's kind.
    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    /// Returns the symbol's type.
    pub fn ty(&self) -> TypeId {
        self.ty
    }

    /// Returns the symbol's linkage.
    pub fn linkage(&self) -> Linkage {
        self.linkage
    }

    /// Returns the symbol's definition status.
    pub fn status(&self) -> DefinitionStatus {
        self.status
    }

    /// Returns the source span of the symbol's declaration.
    pub fn decl_span(&self) -> SourceSpan {
        self.decl_span
    }

    /// Returns the object storage category if this is an object symbol.
    pub fn object_storage_class(&self) -> Option<ObjectStorageClass> {
        self.object_storage_class
    }

    /// Updates the symbol's type.
    ///
    /// Used when merging declarations to update to the composite type.
    pub fn set_ty(&mut self, ty: TypeId) {
        self.ty = ty;
    }

    /// Updates the symbol's definition status.
    ///
    /// Used when a tentative definition becomes a full definition.
    pub fn set_status(&mut self, status: DefinitionStatus) {
        self.status = status;
    }

    /// Updates the symbol's declaration span.
    pub fn set_decl_span(&mut self, span: SourceSpan) {
        self.decl_span = span;
    }

    /// Updates the symbol's object storage category metadata.
    pub fn set_object_storage_class(&mut self, storage: Option<ObjectStorageClass>) {
        self.object_storage_class = storage;
    }
}

/// Arena for storing symbols.
///
/// Symbols are allocated in a contiguous vector and referenced by `SymbolId`.
#[derive(Debug, Clone, Default)]
pub struct SymbolArena {
    symbols: Vec<Symbol>,
}

impl SymbolArena {
    /// Inserts a new symbol and returns its ID.
    pub fn insert(&mut self, symbol: Symbol) -> SymbolId {
        let id = SymbolId(self.symbols.len() as u32);
        self.symbols.push(symbol);
        id
    }

    /// Retrieves a symbol by its ID.
    ///
    /// # Panics
    ///
    /// Panics if the ID is invalid.
    pub fn get(&self, id: SymbolId) -> &Symbol {
        self.symbols
            .get(id.0 as usize)
            .expect("invalid SymbolId for SymbolArena::get")
    }

    /// Retrieves a mutable reference to a symbol by its ID.
    ///
    /// # Panics
    ///
    /// Panics if the ID is invalid.
    pub fn get_mut(&mut self, id: SymbolId) -> &mut Symbol {
        self.symbols
            .get_mut(id.0 as usize)
            .expect("invalid SymbolId for SymbolArena::get_mut")
    }

    /// Returns the number of symbols in the arena.
    pub fn len(&self) -> usize {
        self.symbols.len()
    }
}

/// Scope level for linkage inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeLevel {
    /// File scope (global).
    File,
    /// Block scope (local).
    Block,
}

/// Error type for linkage inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkageError {
    /// Invalid storage class for the current scope.
    InvalidStorageClass(StorageClass),
    /// Conflicting linkage with previous declaration.
    ConflictingLinkage,
}

/// Lightweight declaration view used by redeclaration merge.
///
/// This avoids allocating a full `Symbol` when checking compatibility.
pub struct DeclInfo<'a> {
    pub name: &'a str,
    pub kind: SymbolKind,
    pub ty: TypeId,
    pub linkage: Linkage,
    pub status: DefinitionStatus,
    pub object_storage_class: Option<ObjectStorageClass>,
    pub span: SourceSpan,
}

/// Infers object storage category from scope + storage-class specifier.
///
/// This is independent from linkage. It preserves enough information for
/// MIR lowering to distinguish file-scope objects, block-scope static, and
/// automatic objects.
pub fn infer_object_storage_class(
    scope_level: ScopeLevel,
    storage_class: Option<StorageClass>,
) -> Result<ObjectStorageClass, LinkageError> {
    match scope_level {
        ScopeLevel::File => match storage_class {
            None | Some(StorageClass::Extern | StorageClass::Static) => {
                Ok(ObjectStorageClass::FileScope)
            }
            Some(other) => Err(LinkageError::InvalidStorageClass(other)),
        },
        ScopeLevel::Block => match storage_class {
            None | Some(StorageClass::Auto) => Ok(ObjectStorageClass::Auto),
            Some(StorageClass::Register) => Ok(ObjectStorageClass::Register),
            Some(StorageClass::Static) => Ok(ObjectStorageClass::Static),
            Some(StorageClass::Extern) => Ok(ObjectStorageClass::Extern),
            Some(other) => Err(LinkageError::InvalidStorageClass(other)),
        },
    }
}

/// Infers the linkage of a declaration based on scope and storage class.
///
/// This implements C99 6.2.2 linkage rules:
/// - File scope without storage class: external linkage (or inherit from prior decl)
/// - File scope with `extern`: external linkage (or inherit from prior decl)
/// - File scope with `static`: internal linkage
/// - Block scope without storage class: no linkage
/// - Block scope with `extern`: external linkage (or inherit from prior decl)
///
/// # Errors
///
/// Returns an error if:
/// - The storage class is invalid for the scope
/// - The linkage conflicts with a previous declaration
pub fn infer_linkage(
    kind: SymbolKind,
    scope_level: ScopeLevel,
    storage_class: Option<StorageClass>,
    existing: Option<&Symbol>,
) -> Result<Linkage, LinkageError> {
    match scope_level {
        ScopeLevel::File => match storage_class {
            None => Ok(existing.map_or(Linkage::External, |sym| sym.linkage)),
            Some(StorageClass::Extern) => {
                if let Some(existing) = existing
                    && existing.linkage() == Linkage::Internal
                {
                    return Err(LinkageError::ConflictingLinkage);
                }
                Ok(existing.map_or(Linkage::External, Symbol::linkage))
            }
            Some(StorageClass::Static) => {
                // File-scope `static` entities, including `static inline` functions,
                // always have internal linkage.
                if let Some(existing) = existing
                    && existing.linkage() == Linkage::External
                {
                    return Err(LinkageError::ConflictingLinkage);
                }
                Ok(Linkage::Internal)
            }
            Some(other) => Err(LinkageError::InvalidStorageClass(other)),
        },
        ScopeLevel::Block => match storage_class {
            None => match kind {
                // Block-scope object declarations without storage class have no linkage.
                SymbolKind::Object => Ok(Linkage::None),
                // Block-scope function declarations still denote external/internal entities.
                SymbolKind::Function => Ok(existing.map_or(Linkage::External, Symbol::linkage)),
                SymbolKind::Typedef | SymbolKind::EnumConst => Ok(Linkage::None),
            },
            Some(StorageClass::Extern) => Ok(existing.map_or(Linkage::External, Symbol::linkage)),
            Some(storage @ (StorageClass::Auto | StorageClass::Register)) => match kind {
                SymbolKind::Object => Ok(Linkage::None),
                SymbolKind::Function | SymbolKind::Typedef | SymbolKind::EnumConst => {
                    Err(LinkageError::InvalidStorageClass(storage))
                }
            },
            Some(StorageClass::Static) => match kind {
                // Block-scope `static` objects have static storage duration but no linkage.
                SymbolKind::Object => Ok(Linkage::None),
                SymbolKind::Function | SymbolKind::Typedef | SymbolKind::EnumConst => {
                    Err(LinkageError::InvalidStorageClass(StorageClass::Static))
                }
            },
            Some(other) => Err(LinkageError::InvalidStorageClass(other)),
        },
    }
}

/// Merges a new declaration with an existing symbol.
///
/// This implements C99 6.2.7 compatible type rules for redeclarations:
/// - The kinds must match (both objects, both functions, etc.)
/// - The linkages must match
/// - The types must be compatible, and are merged into a composite type
/// - The definition status is updated (Defined > Tentative > Declared)
///
/// # Errors
///
/// Returns an error if the declarations are incompatible.
pub fn merge_declarations(
    existing: &mut Symbol,
    new_decl: &DeclInfo<'_>,
    type_arena: &mut TypeArena,
) -> Result<TypeId, SemaDiagnostic> {
    if existing.kind() != new_decl.kind {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::RedeclarationConflict,
            format!(
                "redeclaration kind mismatch for '{}': {:?} vs {:?}",
                new_decl.name,
                existing.kind(),
                new_decl.kind
            ),
            new_decl.span,
        )
        .with_secondary(existing.decl_span(), "previous declaration is here"));
    }

    if existing.linkage() != new_decl.linkage {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::InvalidLinkageMerge,
            format!(
                "linkage mismatch for '{}': {:?} vs {:?}",
                new_decl.name,
                existing.linkage(),
                new_decl.linkage
            ),
            new_decl.span,
        )
        .with_secondary(
            existing.decl_span(),
            "previous declaration uses different linkage",
        ));
    }

    // C99 6.9p5: at most one definition per external-linkage identifier.
    if existing.status() == DefinitionStatus::Defined
        && new_decl.status == DefinitionStatus::Defined
    {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::RedeclarationConflict,
            format!("redefinition of '{}'", new_decl.name),
            new_decl.span,
        )
        .with_secondary(existing.decl_span(), "previous definition is here"));
    }

    let Some(merged_ty) = composite_type(existing.ty(), new_decl.ty, type_arena) else {
        return Err(SemaDiagnostic::new(
            SemaDiagnosticCode::IncompatibleTypes,
            format!("incompatible redeclaration type for '{}'", new_decl.name),
            new_decl.span,
        )
        .with_secondary(existing.decl_span(), "previous declaration type is here"));
    };

    existing.set_ty(merged_ty);
    existing.set_status(merge_definition_status(existing.status(), new_decl.status));
    existing.set_decl_span(new_decl.span);
    if existing.kind() == SymbolKind::Object {
        let merged_storage = merge_object_storage_class(
            existing.object_storage_class(),
            new_decl.object_storage_class,
        );
        existing.set_object_storage_class(merged_storage);
    }

    Ok(merged_ty)
}

/// Merges definition statuses, taking the "most defined" one.
///
/// Priority: Defined > Tentative > Declared
fn merge_definition_status(
    existing: DefinitionStatus,
    incoming: DefinitionStatus,
) -> DefinitionStatus {
    match (existing, incoming) {
        (DefinitionStatus::Defined, _) | (_, DefinitionStatus::Defined) => {
            DefinitionStatus::Defined
        }
        (DefinitionStatus::Tentative, _) | (_, DefinitionStatus::Tentative) => {
            DefinitionStatus::Tentative
        }
        _ => DefinitionStatus::Declared,
    }
}

fn merge_object_storage_class(
    existing: Option<ObjectStorageClass>,
    incoming: Option<ObjectStorageClass>,
) -> Option<ObjectStorageClass> {
    match (existing, incoming) {
        (Some(ObjectStorageClass::FileScope), _) | (_, Some(ObjectStorageClass::FileScope)) => {
            Some(ObjectStorageClass::FileScope)
        }
        (Some(current), _) => Some(current),
        (None, next) => next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::sema::types::{ArrayLen, Qualifiers, Type, TypeKind};

    #[test]
    fn merge_declarations_builds_composite_array_type() {
        let mut type_arena = TypeArena::new();
        let int_ty = type_arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let incomplete_array = type_arena.intern(Type {
            kind: TypeKind::Array {
                elem: int_ty,
                len: ArrayLen::Incomplete,
            },
            quals: Qualifiers::default(),
        });
        let known_array = type_arena.intern(Type {
            kind: TypeKind::Array {
                elem: int_ty,
                len: ArrayLen::Known(4),
            },
            quals: Qualifiers::default(),
        });

        let mut existing = Symbol::new(
            "arr".to_string(),
            SymbolKind::Object,
            incomplete_array,
            Linkage::External,
            DefinitionStatus::Tentative,
            SourceSpan::new(0, 3),
        );

        let incoming = DeclInfo {
            name: "arr",
            kind: SymbolKind::Object,
            ty: known_array,
            linkage: Linkage::External,
            status: DefinitionStatus::Tentative,
            object_storage_class: Some(ObjectStorageClass::FileScope),
            span: SourceSpan::new(4, 7),
        };

        let merged_ty = merge_declarations(&mut existing, &incoming, &mut type_arena)
            .expect("array redeclaration should merge");

        let merged = type_arena.get(merged_ty);
        assert_eq!(
            merged.kind,
            TypeKind::Array {
                elem: int_ty,
                len: ArrayLen::Known(4)
            }
        );
        assert_eq!(existing.ty(), merged_ty);
    }

    #[test]
    fn file_scope_extern_conflicts_with_existing_internal_linkage() {
        let existing = Symbol::new(
            "x".to_string(),
            SymbolKind::Object,
            TypeId(0),
            Linkage::Internal,
            DefinitionStatus::Declared,
            SourceSpan::new(0, 1),
        );

        let result = infer_linkage(
            SymbolKind::Object,
            ScopeLevel::File,
            Some(StorageClass::Extern),
            Some(&existing),
        );
        assert_eq!(result, Err(LinkageError::ConflictingLinkage));
    }

    #[test]
    fn merge_declarations_updates_decl_span_to_latest_declaration() {
        let mut type_arena = TypeArena::new();
        let int_ty = type_arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });

        let mut existing = Symbol::new(
            "x".to_string(),
            SymbolKind::Object,
            int_ty,
            Linkage::External,
            DefinitionStatus::Declared,
            SourceSpan::new(1, 2),
        );

        let incoming = DeclInfo {
            name: "x",
            kind: SymbolKind::Object,
            ty: int_ty,
            linkage: Linkage::External,
            status: DefinitionStatus::Defined,
            object_storage_class: Some(ObjectStorageClass::FileScope),
            span: SourceSpan::new(20, 25),
        };

        let _ = merge_declarations(&mut existing, &incoming, &mut type_arena)
            .expect("compatible redeclaration should merge");
        assert_eq!(existing.decl_span(), incoming.span);
    }

    #[test]
    fn infer_object_storage_class_distinguishes_file_and_block_cases() {
        assert_eq!(
            infer_object_storage_class(ScopeLevel::File, None),
            Ok(ObjectStorageClass::FileScope)
        );
        assert_eq!(
            infer_object_storage_class(ScopeLevel::Block, None),
            Ok(ObjectStorageClass::Auto)
        );
        assert_eq!(
            infer_object_storage_class(ScopeLevel::Block, Some(StorageClass::Register)),
            Ok(ObjectStorageClass::Register)
        );
        assert_eq!(
            infer_object_storage_class(ScopeLevel::Block, Some(StorageClass::Static)),
            Ok(ObjectStorageClass::Static)
        );
        assert_eq!(
            infer_object_storage_class(ScopeLevel::Block, Some(StorageClass::Extern)),
            Ok(ObjectStorageClass::Extern)
        );
    }

    #[test]
    fn merge_object_storage_class_prefers_filescope_and_keeps_existing_non_filescope() {
        assert_eq!(
            merge_object_storage_class(Some(ObjectStorageClass::FileScope), None),
            Some(ObjectStorageClass::FileScope)
        );
        assert_eq!(
            merge_object_storage_class(
                Some(ObjectStorageClass::Extern),
                Some(ObjectStorageClass::FileScope)
            ),
            Some(ObjectStorageClass::FileScope)
        );
        assert_eq!(
            merge_object_storage_class(None, Some(ObjectStorageClass::Static)),
            Some(ObjectStorageClass::Static)
        );
        assert_eq!(
            merge_object_storage_class(
                Some(ObjectStorageClass::Register),
                Some(ObjectStorageClass::Auto)
            ),
            Some(ObjectStorageClass::Register)
        );
        assert_eq!(merge_object_storage_class(None, None), None);
    }
}
