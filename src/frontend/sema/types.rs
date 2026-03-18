use std::collections::HashMap;

/// Opaque identifier for an interned type in the type arena.
///
/// Types are deduplicated: structurally equal types share the same `TypeId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

/// Opaque identifier for a struct or union definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RecordId(pub u32);

/// Opaque identifier for an enum definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

/// Opaque identifier for a struct/union field.
///
/// Note: Field arena is not yet implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FieldId(pub u32);

/// Identifier for a tag namespace entry (struct/union/enum).
///
/// C has separate namespaces for tags and ordinary identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagId {
    Record(RecordId),
    Enum(EnumId),
}

/// C type qualifiers: const, volatile, restrict.
///
/// These can be applied to any type, though their semantics vary.
/// For example, array and function qualifiers are ignored in most contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Qualifiers {
    pub is_const: bool,
    pub is_volatile: bool,
    pub is_restrict: bool,
}

/// A complete C type with qualifiers.
///
/// Types are interned in `TypeArena` for efficient comparison and storage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Type {
    pub kind: TypeKind,
    pub quals: Qualifiers,
}

impl Type {
    /// Creates an error type for error recovery.
    ///
    /// Error types are compatible with all other types to suppress
    /// cascading diagnostics.
    #[must_use]
    pub fn error() -> Self {
        Self {
            kind: TypeKind::Error,
            quals: Qualifiers::default(),
        }
    }
}

/// Array length specification.
///
/// Arrays can have known size, incomplete size (e.g., `int[]`),
/// or be flexible array members (C99 6.7.2.1p16).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayLen {
    /// Known array size (e.g., `int[10]`).
    Known(u64),
    /// Incomplete array type (e.g., `int[]`).
    Incomplete,
    /// Flexible array member (last field of struct).
    FlexibleMember,
}

/// Function declaration style.
///
/// C supports both prototype-style (`int f(int, char)`) and
/// old-style non-prototype declarations (`int f()`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FunctionStyle {
    /// Prototype-style: parameter types are specified.
    Prototype,
    /// Old-style: parameter types are unspecified.
    NonPrototype,
}

/// Function type information.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionType {
    /// Return type.
    pub ret: TypeId,
    /// Parameter types (empty for non-prototype functions).
    pub params: Vec<TypeId>,
    /// Whether the function is variadic (has `...`).
    pub variadic: bool,
    /// Declaration style.
    pub style: FunctionStyle,
}

/// The kind of a C type, without qualifiers.
///
/// This represents the structural part of a type. Qualifiers are
/// stored separately in the `Type` struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// `void` type.
    Void,
    /// `_Bool` type (C99).
    Bool,

    /// Plain `char` (implementation-defined signedness).
    Char,
    /// Explicitly `signed char`.
    SignedChar,
    /// Explicitly `unsigned char`.
    UnsignedChar,

    /// `short` or `short int`.
    Short { signed: bool },
    /// `int`.
    Int { signed: bool },
    /// `long` or `long int`.
    Long { signed: bool },
    /// `long long` or `long long int`.
    LongLong { signed: bool },

    /// `float` type.
    Float,
    /// `double` type.
    Double,

    /// Pointer type.
    Pointer { pointee: TypeId },
    /// Array type.
    Array { elem: TypeId, len: ArrayLen },
    /// Function type.
    Function(FunctionType),

    /// Struct or union type.
    Record(RecordId),
    /// Enum type.
    Enum(EnumId),

    /// Error type for error recovery.
    Error,
}

/// Interned semantic type arena.
///
/// Equal type structures are deduplicated and share one `TypeId`.
/// This enables efficient type comparison (just compare IDs) and
/// reduces memory usage.
#[derive(Debug, Clone, Default)]
pub struct TypeArena {
    types: Vec<Type>,
    interner: HashMap<Type, TypeId>,
}

impl TypeArena {
    /// Creates a new empty type arena.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns a type, returning its unique ID.
    ///
    /// If an equal type already exists, returns the existing ID.
    /// Otherwise, allocates a new ID and stores the type.
    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(id) = self.interner.get(&ty).copied() {
            return id;
        }

        let id = TypeId(self.types.len() as u32);
        self.interner.insert(ty.clone(), id);
        self.types.push(ty);
        id
    }

    /// Retrieves a type by its ID.
    ///
    /// Panics if the ID is invalid (not allocated by this arena).
    pub fn get(&self, id: TypeId) -> &Type {
        self.types
            .get(id.0 as usize)
            .expect("invalid TypeId for TypeArena::get")
    }

    /// Returns the number of unique types in the arena.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// Returns `true` if the arena contains no types.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

/// Arena for struct and union definitions.
#[derive(Debug, Clone, Default)]
pub struct RecordArena {
    records: Vec<RecordDef>,
}

impl RecordArena {
    /// Inserts a new record definition and returns its ID.
    pub fn insert(&mut self, record: RecordDef) -> RecordId {
        let id = RecordId(self.records.len() as u32);
        self.records.push(record);
        id
    }

    /// Retrieves a record definition by its ID.
    ///
    /// Panics if the ID is invalid.
    pub fn get(&self, id: RecordId) -> &RecordDef {
        self.records
            .get(id.0 as usize)
            .expect("invalid RecordId for RecordArena::get")
    }

    /// Retrieves a mutable reference to a record definition by its ID.
    ///
    /// Panics if the ID is invalid.
    pub fn get_mut(&mut self, id: RecordId) -> &mut RecordDef {
        self.records
            .get_mut(id.0 as usize)
            .expect("invalid RecordId for RecordArena::get_mut")
    }
}

/// A struct or union definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordDef {
    /// Optional tag name (e.g., `struct Point`).
    pub tag: Option<String>,
    /// Whether this is a struct or union.
    pub kind: crate::frontend::parser::ast::RecordKind,
    /// Field definitions.
    pub fields: Vec<FieldDef>,
    /// Whether the definition is complete (has a body).
    pub is_complete: bool,
}

/// A field in a struct or union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    /// Field name (None for anonymous fields).
    pub name: Option<String>,
    /// Field type.
    pub ty: TypeId,
    /// Bit-field width (None for regular fields).
    pub bit_width: Option<u16>,
}

/// Arena for enum definitions.
#[derive(Debug, Clone, Default)]
pub struct EnumArena {
    enums: Vec<EnumDef>,
}

impl EnumArena {
    /// Inserts a new enum definition and returns its ID.
    pub fn insert(&mut self, value: EnumDef) -> EnumId {
        let id = EnumId(self.enums.len() as u32);
        self.enums.push(value);
        id
    }

    /// Retrieves an enum definition by its ID.
    ///
    /// Panics if the ID is invalid.
    pub fn get(&self, id: EnumId) -> &EnumDef {
        self.enums
            .get(id.0 as usize)
            .expect("invalid EnumId for EnumArena::get")
    }

    /// Retrieves a mutable reference to an enum definition by its ID.
    ///
    /// Panics if the ID is invalid.
    pub fn get_mut(&mut self, id: EnumId) -> &mut EnumDef {
        self.enums
            .get_mut(id.0 as usize)
            .expect("invalid EnumId for EnumArena::get_mut")
    }
}

/// Alias for `types_compatible` for backward compatibility.
pub fn compatible(a: TypeId, b: TypeId, arena: &TypeArena) -> bool {
    types_compatible(a, b, arena)
}

/// Checks if two types are compatible according to C99 6.2.7.
///
/// Two types are compatible if they are the same type, or if they
/// are structurally equivalent according to C's type compatibility rules.
///
/// # Special cases
///
/// - Error types are compatible with all types (for error recovery).
/// - Array qualifiers are ignored (C99 6.7.3).
/// - Function qualifiers are ignored.
/// - Incomplete arrays are compatible with complete arrays of the same element type.
pub fn types_compatible(a: TypeId, b: TypeId, arena: &TypeArena) -> bool {
    if a == b {
        return true;
    }

    let lhs = arena.get(a);
    let rhs = arena.get(b);

    if matches!(lhs.kind, TypeKind::Error) || matches!(rhs.kind, TypeKind::Error) {
        return true;
    }

    match (&lhs.kind, &rhs.kind) {
        (TypeKind::Void, TypeKind::Void)
        | (TypeKind::Bool, TypeKind::Bool)
        | (TypeKind::Char, TypeKind::Char)
        | (TypeKind::SignedChar, TypeKind::SignedChar)
        | (TypeKind::UnsignedChar, TypeKind::UnsignedChar)
        | (TypeKind::Float, TypeKind::Float)
        | (TypeKind::Double, TypeKind::Double) => lhs.quals == rhs.quals,
        (TypeKind::Short { signed: a }, TypeKind::Short { signed: b })
        | (TypeKind::Int { signed: a }, TypeKind::Int { signed: b })
        | (TypeKind::Long { signed: a }, TypeKind::Long { signed: b })
        | (TypeKind::LongLong { signed: a }, TypeKind::LongLong { signed: b }) => {
            a == b && lhs.quals == rhs.quals
        }
        (TypeKind::Pointer { pointee: a }, TypeKind::Pointer { pointee: b }) => {
            if lhs.quals != rhs.quals {
                return false;
            }
            types_compatible(*a, *b, arena)
        }
        (TypeKind::Array { elem: ea, len: la }, TypeKind::Array { elem: eb, len: lb }) => {
            // Top-level array qualifiers are ignored for compatibility.
            if !types_compatible(*ea, *eb, arena) {
                return false;
            }
            match (la, lb) {
                (ArrayLen::Known(a), ArrayLen::Known(b)) => a == b,
                (ArrayLen::Incomplete, ArrayLen::Known(_))
                | (ArrayLen::Known(_), ArrayLen::Incomplete)
                | (ArrayLen::Incomplete, ArrayLen::Incomplete) => true,
                (ArrayLen::FlexibleMember, ArrayLen::FlexibleMember) => true,
                _ => false,
            }
        }
        (TypeKind::Function(a), TypeKind::Function(b)) => {
            // Function qualifiers are ignored during type compatibility.
            // For declaration merging, prototype and non-prototype are compatible.
            if !types_compatible(a.ret, b.ret, arena) {
                return false;
            }
            match (&a.style, &b.style) {
                (FunctionStyle::Prototype, FunctionStyle::Prototype) => {
                    a.variadic == b.variadic
                        && a.params.len() == b.params.len()
                        && a.params
                            .iter()
                            .zip(&b.params)
                            .all(|(lhs, rhs)| types_compatible(*lhs, *rhs, arena))
                }
                (FunctionStyle::NonPrototype, FunctionStyle::NonPrototype) => true,
                // Prototype and non-prototype are compatible for declaration merging.
                (FunctionStyle::Prototype, FunctionStyle::NonPrototype)
                | (FunctionStyle::NonPrototype, FunctionStyle::Prototype) => true,
            }
        }
        (TypeKind::Record(a), TypeKind::Record(b)) => a == b && lhs.quals == rhs.quals,
        (TypeKind::Enum(a), TypeKind::Enum(b)) => a == b && lhs.quals == rhs.quals,
        _ => false,
    }
}

/// Checks if a value of type `from` can be assigned to a variable of type `to`.
///
/// This is a simplified version that doesn't consider null pointer constants.
/// Use `assignment_compatible_with_const` for full checking.
pub fn assignment_compatible(from: TypeId, to: TypeId, arena: &TypeArena) -> bool {
    assignment_compatible_with_const(from, None, to, arena)
}

/// Checks assignment compatibility with support for null pointer constants.
///
/// This implements C99 6.5.16.1 assignment constraints:
/// - Compatible types can be assigned
/// - Arithmetic types can be converted to other arithmetic types
/// - Null pointer constant (integer 0) can be assigned to any pointer type
/// - Pointers with compatible pointee types can be assigned
///
/// # Parameters
///
/// - `from`: The source type
/// - `from_integer_const`: If the source is an integer constant, its value
/// - `to`: The destination type
/// - `arena`: The type arena
///
/// # Note
///
/// This is a simplified implementation. Full C99 assignment compatibility
/// also requires checking:
/// - Qualifier compatibility for pointers (`int*` -> `const int*` is OK)
/// - `void*` compatibility (any pointer can be assigned to/from `void*`)
pub fn assignment_compatible_with_const(
    from: TypeId,
    from_integer_const: Option<i64>,
    to: TypeId,
    arena: &TypeArena,
) -> bool {
    if types_compatible(from, to, arena) {
        return true;
    }

    let from_kind = &arena.get(from).kind;
    let to_kind = &arena.get(to).kind;

    // Null pointer constant: integer constant 0 can be assigned to any pointer.
    if from_integer_const == Some(0)
        && is_integer(from_kind)
        && matches!(to_kind, TypeKind::Pointer { .. })
    {
        return true;
    }

    // Arithmetic types can be converted to each other.
    if is_arithmetic(from_kind) && is_arithmetic(to_kind) {
        return true;
    }

    // Pointer assignment (simplified: only checks pointee compatibility).
    match (from_kind, to_kind) {
        (TypeKind::Pointer { pointee: from_p }, TypeKind::Pointer { pointee: to_p }) => {
            types_compatible(*from_p, *to_p, arena)
        }
        _ => false,
    }
}

/// Checks if a type is an integer type.
fn is_integer(kind: &TypeKind) -> bool {
    matches!(
        kind,
        TypeKind::Bool
            | TypeKind::Char
            | TypeKind::SignedChar
            | TypeKind::UnsignedChar
            | TypeKind::Short { .. }
            | TypeKind::Int { .. }
            | TypeKind::Long { .. }
            | TypeKind::LongLong { .. }
            | TypeKind::Enum(_)
    )
}

/// Computes the composite type of two compatible types (C99 6.2.7).
///
/// The composite type is used when merging declarations. For example:
/// - `int[]` + `int[10]` -> `int[10]`
/// - `int f()` + `int f(int)` -> `int f(int)` (prototype wins)
///
/// Returns `None` if the types are not compatible.
pub fn composite_type(a: TypeId, b: TypeId, arena: &mut TypeArena) -> Option<TypeId> {
    if !types_compatible(a, b, arena) {
        return None;
    }

    if a == b {
        return Some(a);
    }

    let lhs = arena.get(a).clone();
    let rhs = arena.get(b).clone();

    let quals = lhs.quals;
    let kind = match (lhs.kind, rhs.kind) {
        (TypeKind::Array { elem: ea, len: la }, TypeKind::Array { elem: eb, len: lb }) => {
            let elem = composite_type(ea, eb, arena)?;
            let len = match (la, lb) {
                (ArrayLen::Known(a), ArrayLen::Known(b)) if a == b => ArrayLen::Known(a),
                (ArrayLen::Incomplete, ArrayLen::Known(n))
                | (ArrayLen::Known(n), ArrayLen::Incomplete) => ArrayLen::Known(n),
                (ArrayLen::Incomplete, ArrayLen::Incomplete) => ArrayLen::Incomplete,
                (ArrayLen::FlexibleMember, ArrayLen::FlexibleMember) => ArrayLen::FlexibleMember,
                _ => return None,
            };
            TypeKind::Array { elem, len }
        }
        (TypeKind::Pointer { pointee: ea }, TypeKind::Pointer { pointee: eb }) => {
            let pointee = composite_type(ea, eb, arena)?;
            TypeKind::Pointer { pointee }
        }
        (TypeKind::Function(a_fn), TypeKind::Function(b_fn)) => {
            let ret = composite_type(a_fn.ret, b_fn.ret, arena)?;
            // Prototype style takes precedence over non-prototype.
            let style = if matches!(a_fn.style, FunctionStyle::Prototype)
                || matches!(b_fn.style, FunctionStyle::Prototype)
            {
                FunctionStyle::Prototype
            } else {
                FunctionStyle::NonPrototype
            };

            let (params, variadic) = match (&a_fn.style, &b_fn.style) {
                (FunctionStyle::Prototype, FunctionStyle::Prototype) => {
                    if a_fn.params.len() != b_fn.params.len() {
                        return None;
                    }
                    let mut params = Vec::with_capacity(a_fn.params.len());
                    for (lhs, rhs) in a_fn.params.iter().zip(&b_fn.params) {
                        params.push(composite_type(*lhs, *rhs, arena)?);
                    }
                    (params, a_fn.variadic || b_fn.variadic)
                }
                (FunctionStyle::Prototype, FunctionStyle::NonPrototype) => {
                    (a_fn.params.clone(), a_fn.variadic)
                }
                (FunctionStyle::NonPrototype, FunctionStyle::Prototype) => {
                    (b_fn.params.clone(), b_fn.variadic)
                }
                (FunctionStyle::NonPrototype, FunctionStyle::NonPrototype) => (Vec::new(), false),
            };

            TypeKind::Function(FunctionType {
                ret,
                params,
                variadic,
                style,
            })
        }
        (same, _) => same,
    };

    Some(arena.intern(Type { kind, quals }))
}

/// Checks if a type is an arithmetic type (integer or floating-point).
fn is_arithmetic(kind: &TypeKind) -> bool {
    matches!(
        kind,
        TypeKind::Bool
            | TypeKind::Char
            | TypeKind::SignedChar
            | TypeKind::UnsignedChar
            | TypeKind::Short { .. }
            | TypeKind::Int { .. }
            | TypeKind::Long { .. }
            | TypeKind::LongLong { .. }
            | TypeKind::Float
            | TypeKind::Double
            | TypeKind::Enum(_)
    )
}

/// An enum definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDef {
    /// Optional tag name (e.g., `enum Color`).
    pub tag: Option<String>,
    /// Underlying integer type (always `int` in C99).
    pub underlying_ty: TypeId,
    /// Enum constant definitions.
    pub constants: Vec<EnumConstant>,
}

/// An enum constant (enumerator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumConstant {
    /// Constant name.
    pub name: String,
    /// Constant value.
    pub value: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_arena_interns_structurally_equal_types() {
        let mut arena = TypeArena::new();

        let first = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let second = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });

        assert_eq!(first, second);
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn array_top_level_qualifiers_do_not_break_compatibility() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let plain_array = arena.intern(Type {
            kind: TypeKind::Array {
                elem: int_ty,
                len: ArrayLen::Known(4),
            },
            quals: Qualifiers::default(),
        });
        let const_array = arena.intern(Type {
            kind: TypeKind::Array {
                elem: int_ty,
                len: ArrayLen::Known(4),
            },
            quals: Qualifiers {
                is_const: true,
                is_volatile: false,
                is_restrict: false,
            },
        });

        assert!(types_compatible(plain_array, const_array, &arena));
    }

    #[test]
    fn null_pointer_constant_assignment_is_allowed() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let ptr_ty = arena.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });

        assert!(assignment_compatible_with_const(
            int_ty,
            Some(0),
            ptr_ty,
            &arena
        ));
    }
}
