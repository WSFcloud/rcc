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
        // In this implementation, enum underlying type is `int`.
        (TypeKind::Enum(_), TypeKind::Int { signed: true })
        | (TypeKind::Int { signed: true }, TypeKind::Enum(_)) => lhs.quals == rhs.quals,
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
/// Pointer assignment handling includes:
/// - Qualifier directionality (`T* -> const T*` allowed, reverse rejected)
/// - `void*` and object-pointer interoperability
/// - Nested-pointer qualifier safety checks (rejecting the classic `T** -> const T**` hole)
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

    // Pointer assignment: qualifier-aware checks including `void*` and nested pointers.
    match (from_kind, to_kind) {
        (TypeKind::Pointer { pointee: from_p }, TypeKind::Pointer { pointee: to_p }) => {
            pointer_assignment_compatible(*from_p, *to_p, arena, 0)
        }
        _ => false,
    }
}

/// Checks whether two pointer types are comparable with `==`/`!=` or relational operators.
///
/// - When `allow_void_object` is `true`, `void*` is considered compatible with any object pointer.
/// - Qualifier differences are ignored for compatibility checking in comparisons.
pub fn pointer_comparison_compatible(
    lhs: TypeId,
    rhs: TypeId,
    arena: &TypeArena,
    allow_void_object: bool,
) -> bool {
    let (
        TypeKind::Pointer {
            pointee: lhs_pointee,
        },
        TypeKind::Pointer {
            pointee: rhs_pointee,
        },
    ) = (&arena.get(lhs).kind, &arena.get(rhs).kind)
    else {
        return false;
    };

    if types_compatible_ignoring_quals(*lhs_pointee, *rhs_pointee, arena) {
        return true;
    }

    if !allow_void_object {
        return false;
    }

    (is_void_type(*lhs_pointee, arena) && is_object_type(*rhs_pointee, arena))
        || (is_void_type(*rhs_pointee, arena) && is_object_type(*lhs_pointee, arena))
}

/// Checks if a type is an integer type.
pub fn is_integer(kind: &TypeKind) -> bool {
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

/// Checks if a semantic type id is a pointer.
pub fn is_pointer(ty: TypeId, arena: &TypeArena) -> bool {
    matches!(arena.get(ty).kind, TypeKind::Pointer { .. })
}

/// Checks if a semantic type id is a scalar type (arithmetic or pointer).
pub fn is_scalar(ty: TypeId, arena: &TypeArena) -> bool {
    let kind = &arena.get(ty).kind;
    is_arithmetic(kind) || matches!(kind, TypeKind::Pointer { .. })
}

/// Checks whether a type is exactly `void*` (ignoring top-level qualifiers on the pointer).
pub fn is_void_pointer(ty: TypeId, arena: &TypeArena) -> bool {
    match &arena.get(ty).kind {
        TypeKind::Pointer { pointee } => matches!(arena.get(*pointee).kind, TypeKind::Void),
        _ => false,
    }
}

/// Drops top-level qualifiers from a type.
pub fn unqualified(ty: TypeId, arena: &mut TypeArena) -> TypeId {
    let mut cloned = arena.get(ty).clone();
    cloned.quals = Qualifiers::default();
    arena.intern(cloned)
}

/// Applies integer promotions (C99 6.3.1.1, simplified LP64 model).
pub fn integer_promotion(ty: TypeId, arena: &mut TypeArena) -> TypeId {
    let kind = arena.get(ty).kind.clone();
    let promoted_kind = match kind {
        TypeKind::Bool
        | TypeKind::Char
        | TypeKind::SignedChar
        | TypeKind::UnsignedChar
        | TypeKind::Short { .. }
        | TypeKind::Enum(_) => TypeKind::Int { signed: true },
        other => other,
    };

    arena.intern(Type {
        kind: promoted_kind,
        quals: Qualifiers::default(),
    })
}

/// Applies the usual arithmetic conversions and returns the common type.
pub fn usual_arithmetic_conversions(a: TypeId, b: TypeId, arena: &mut TypeArena) -> TypeId {
    let lhs = integer_promotion(a, arena);
    let rhs = integer_promotion(b, arena);

    let lhs_kind = arena.get(lhs).kind.clone();
    let rhs_kind = arena.get(rhs).kind.clone();

    // Floating-point conversions first.
    if matches!(lhs_kind, TypeKind::Double) || matches!(rhs_kind, TypeKind::Double) {
        return arena.intern(Type {
            kind: TypeKind::Double,
            quals: Qualifiers::default(),
        });
    }
    if matches!(lhs_kind, TypeKind::Float) || matches!(rhs_kind, TypeKind::Float) {
        return arena.intern(Type {
            kind: TypeKind::Float,
            quals: Qualifiers::default(),
        });
    }

    if lhs_kind == rhs_kind {
        return arena.intern(Type {
            kind: lhs_kind,
            quals: Qualifiers::default(),
        });
    }

    let lhs_signed = integer_signedness(&lhs_kind).unwrap_or(true);
    let rhs_signed = integer_signedness(&rhs_kind).unwrap_or(true);
    let lhs_rank = integer_rank(&lhs_kind);
    let rhs_rank = integer_rank(&rhs_kind);
    let lhs_bits = integer_bits(&lhs_kind);
    let rhs_bits = integer_bits(&rhs_kind);

    let common_kind = if lhs_signed == rhs_signed {
        if lhs_rank >= rhs_rank {
            lhs_kind
        } else {
            rhs_kind
        }
    } else {
        let (signed_kind, signed_rank, signed_bits, unsigned_kind, unsigned_rank, unsigned_bits) =
            if lhs_signed {
                (lhs_kind, lhs_rank, lhs_bits, rhs_kind, rhs_rank, rhs_bits)
            } else {
                (rhs_kind, rhs_rank, rhs_bits, lhs_kind, lhs_rank, lhs_bits)
            };

        if unsigned_rank >= signed_rank {
            unsigned_kind
        } else if signed_bits > unsigned_bits {
            signed_kind
        } else {
            unsigned_variant_of(&signed_kind)
        }
    };

    arena.intern(Type {
        kind: common_kind,
        quals: Qualifiers::default(),
    })
}

/// Computes `sizeof` in bytes for semantic types under the simplified LP64
/// data model used by sema.
///
/// Struct layout is modeled as field-size summation without padding; union
/// layout is modeled as max-field-size.
pub fn type_size_of(ty: TypeId, types: &TypeArena, records: &RecordArena) -> Option<u64> {
    let t = types.get(ty);
    match &t.kind {
        TypeKind::Bool | TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar => Some(1),
        TypeKind::Short { .. } => Some(2),
        TypeKind::Int { .. } | TypeKind::Enum(_) | TypeKind::Float => Some(4),
        TypeKind::Long { .. } | TypeKind::LongLong { .. } | TypeKind::Double => Some(8),
        TypeKind::Pointer { .. } => Some(8),
        TypeKind::Array { elem, len } => match len {
            ArrayLen::Known(n) => type_size_of(*elem, types, records)?.checked_mul(*n),
            _ => None,
        },
        TypeKind::Record(record_id) => {
            let record = records.get(*record_id);
            if !record.is_complete {
                return None;
            }
            match record.kind {
                crate::frontend::parser::ast::RecordKind::Struct => {
                    let mut total = 0u64;
                    for field in &record.fields {
                        total = total.checked_add(type_size_of(field.ty, types, records)?)?;
                    }
                    Some(total)
                }
                crate::frontend::parser::ast::RecordKind::Union => {
                    let mut max_size = 0u64;
                    for field in &record.fields {
                        max_size = max_size.max(type_size_of(field.ty, types, records)?);
                    }
                    Some(max_size)
                }
            }
        }
        _ => None,
    }
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
        (TypeKind::Enum(_), TypeKind::Int { signed: true })
        | (TypeKind::Int { signed: true }, TypeKind::Enum(_)) => TypeKind::Int { signed: true },
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
                    if a_fn.variadic != b_fn.variadic {
                        return None;
                    }
                    let mut params = Vec::with_capacity(a_fn.params.len());
                    for (lhs, rhs) in a_fn.params.iter().zip(&b_fn.params) {
                        params.push(composite_type(*lhs, *rhs, arena)?);
                    }
                    (params, a_fn.variadic)
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
pub fn is_arithmetic(kind: &TypeKind) -> bool {
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

fn integer_rank(kind: &TypeKind) -> u8 {
    match kind {
        TypeKind::Bool => 1,
        TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar => 2,
        TypeKind::Short { .. } => 3,
        TypeKind::Int { .. } | TypeKind::Enum(_) => 4,
        TypeKind::Long { .. } => 5,
        TypeKind::LongLong { .. } => 6,
        _ => 0,
    }
}

fn integer_bits(kind: &TypeKind) -> u8 {
    match kind {
        TypeKind::Bool => 1,
        TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar => 8,
        TypeKind::Short { .. } => 16,
        TypeKind::Int { .. } | TypeKind::Enum(_) => 32,
        TypeKind::Long { .. } | TypeKind::LongLong { .. } => 64,
        _ => 0,
    }
}

fn integer_signedness(kind: &TypeKind) -> Option<bool> {
    match kind {
        TypeKind::Bool => Some(false),
        TypeKind::Char | TypeKind::SignedChar => Some(true),
        TypeKind::UnsignedChar => Some(false),
        TypeKind::Short { signed }
        | TypeKind::Int { signed }
        | TypeKind::Long { signed }
        | TypeKind::LongLong { signed } => Some(*signed),
        TypeKind::Enum(_) => Some(true),
        _ => None,
    }
}

fn unsigned_variant_of(kind: &TypeKind) -> TypeKind {
    match kind {
        TypeKind::Bool => TypeKind::Bool,
        TypeKind::Char | TypeKind::SignedChar | TypeKind::UnsignedChar => TypeKind::UnsignedChar,
        TypeKind::Short { .. } => TypeKind::Short { signed: false },
        TypeKind::Int { .. } | TypeKind::Enum(_) => TypeKind::Int { signed: false },
        TypeKind::Long { .. } => TypeKind::Long { signed: false },
        TypeKind::LongLong { .. } => TypeKind::LongLong { signed: false },
        other => other.clone(),
    }
}

fn pointer_assignment_compatible(
    from_pointee: TypeId,
    to_pointee: TypeId,
    arena: &TypeArena,
    depth: usize,
) -> bool {
    let from_ty = arena.get(from_pointee);
    let to_ty = arena.get(to_pointee);

    if matches!(from_ty.kind, TypeKind::Error) || matches!(to_ty.kind, TypeKind::Error) {
        return true;
    }

    // C99 6.3.2.3p1: `void*` can convert to/from any object pointer.
    if (is_void_type(from_pointee, arena) && is_object_type(to_pointee, arena))
        || (is_void_type(to_pointee, arena) && is_object_type(from_pointee, arena))
    {
        return qualifiers_contain(to_ty.quals, from_ty.quals);
    }

    // Base compatibility ignores qualifiers, then we enforce qualifier directionality.
    if !types_compatible_ignoring_quals(from_pointee, to_pointee, arena) {
        return false;
    }

    // LHS (destination) pointee qualifiers must include RHS qualifiers.
    if !qualifiers_contain(to_ty.quals, from_ty.quals) {
        return false;
    }

    // Prevent the classic `T** -> const T**` hole by requiring deep qualifier equality.
    if depth > 0 && from_ty.quals != to_ty.quals {
        return false;
    }

    match (&from_ty.kind, &to_ty.kind) {
        (
            TypeKind::Pointer {
                pointee: from_inner,
            },
            TypeKind::Pointer { pointee: to_inner },
        ) => pointer_assignment_compatible(*from_inner, *to_inner, arena, depth + 1),
        _ => true,
    }
}

fn qualifiers_contain(superset: Qualifiers, subset: Qualifiers) -> bool {
    (!subset.is_const || superset.is_const)
        && (!subset.is_volatile || superset.is_volatile)
        && (!subset.is_restrict || superset.is_restrict)
}

fn is_void_type(ty: TypeId, arena: &TypeArena) -> bool {
    matches!(arena.get(ty).kind, TypeKind::Void)
}

fn is_object_type(ty: TypeId, arena: &TypeArena) -> bool {
    !matches!(
        arena.get(ty).kind,
        TypeKind::Void | TypeKind::Function(_) | TypeKind::Error
    )
}

fn types_compatible_ignoring_quals(a: TypeId, b: TypeId, arena: &TypeArena) -> bool {
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
        | (TypeKind::Double, TypeKind::Double) => true,
        (TypeKind::Short { signed: a }, TypeKind::Short { signed: b })
        | (TypeKind::Int { signed: a }, TypeKind::Int { signed: b })
        | (TypeKind::Long { signed: a }, TypeKind::Long { signed: b })
        | (TypeKind::LongLong { signed: a }, TypeKind::LongLong { signed: b }) => a == b,
        (TypeKind::Enum(_), TypeKind::Int { signed: true })
        | (TypeKind::Int { signed: true }, TypeKind::Enum(_)) => true,
        (TypeKind::Pointer { pointee: a }, TypeKind::Pointer { pointee: b }) => {
            types_compatible_ignoring_quals(*a, *b, arena)
        }
        (TypeKind::Array { elem: ea, len: la }, TypeKind::Array { elem: eb, len: lb }) => {
            if !types_compatible_ignoring_quals(*ea, *eb, arena) {
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
            if !types_compatible_ignoring_quals(a.ret, b.ret, arena) {
                return false;
            }
            match (&a.style, &b.style) {
                (FunctionStyle::Prototype, FunctionStyle::Prototype) => {
                    a.variadic == b.variadic
                        && a.params.len() == b.params.len()
                        && a.params
                            .iter()
                            .zip(&b.params)
                            .all(|(lhs, rhs)| types_compatible_ignoring_quals(*lhs, *rhs, arena))
                }
                (FunctionStyle::NonPrototype, FunctionStyle::NonPrototype) => true,
                (FunctionStyle::Prototype, FunctionStyle::NonPrototype)
                | (FunctionStyle::NonPrototype, FunctionStyle::Prototype) => true,
            }
        }
        (TypeKind::Record(a), TypeKind::Record(b)) => a == b,
        (TypeKind::Enum(a), TypeKind::Enum(b)) => a == b,
        _ => false,
    }
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

    #[test]
    fn integer_promotion_promotes_unsigned_char_to_int() {
        let mut arena = TypeArena::new();
        let uchar_ty = arena.intern(Type {
            kind: TypeKind::UnsignedChar,
            quals: Qualifiers::default(),
        });

        let promoted = integer_promotion(uchar_ty, &mut arena);
        assert!(matches!(
            arena.get(promoted).kind,
            TypeKind::Int { signed: true }
        ));
    }

    #[test]
    fn usual_arithmetic_conversions_choose_unsigned_long() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let ulong_ty = arena.intern(Type {
            kind: TypeKind::Long { signed: false },
            quals: Qualifiers::default(),
        });

        let common = usual_arithmetic_conversions(int_ty, ulong_ty, &mut arena);
        assert!(matches!(
            arena.get(common).kind,
            TypeKind::Long { signed: false }
        ));
    }

    #[test]
    fn composite_type_rejects_variadic_mismatch_between_prototypes() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let proto_non_variadic = arena.intern(Type {
            kind: TypeKind::Function(FunctionType {
                ret: int_ty,
                params: vec![int_ty],
                variadic: false,
                style: FunctionStyle::Prototype,
            }),
            quals: Qualifiers::default(),
        });
        let proto_variadic = arena.intern(Type {
            kind: TypeKind::Function(FunctionType {
                ret: int_ty,
                params: vec![int_ty],
                variadic: true,
                style: FunctionStyle::Prototype,
            }),
            quals: Qualifiers::default(),
        });

        assert!(composite_type(proto_non_variadic, proto_variadic, &mut arena).is_none());
    }

    #[test]
    fn unqualified_removes_top_level_quals() {
        let mut arena = TypeArena::new();
        let const_int = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers {
                is_const: true,
                is_volatile: false,
                is_restrict: false,
            },
        });

        let plain = unqualified(const_int, &mut arena);
        assert_eq!(arena.get(plain).quals, Qualifiers::default());
    }

    #[test]
    fn pointer_assignment_respects_qualifier_direction() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let const_int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers {
                is_const: true,
                is_volatile: false,
                is_restrict: false,
            },
        });
        let int_ptr = arena.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });
        let const_int_ptr = arena.intern(Type {
            kind: TypeKind::Pointer {
                pointee: const_int_ty,
            },
            quals: Qualifiers::default(),
        });

        assert!(assignment_compatible_with_const(
            int_ptr,
            None,
            const_int_ptr,
            &arena
        ));
        assert!(!assignment_compatible_with_const(
            const_int_ptr,
            None,
            int_ptr,
            &arena
        ));
    }

    #[test]
    fn pointer_assignment_rejects_const_hole_through_double_pointer() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let const_int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers {
                is_const: true,
                is_volatile: false,
                is_restrict: false,
            },
        });
        let int_ptr = arena.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });
        let const_int_ptr = arena.intern(Type {
            kind: TypeKind::Pointer {
                pointee: const_int_ty,
            },
            quals: Qualifiers::default(),
        });
        let int_ptr_ptr = arena.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ptr },
            quals: Qualifiers::default(),
        });
        let const_int_ptr_ptr = arena.intern(Type {
            kind: TypeKind::Pointer {
                pointee: const_int_ptr,
            },
            quals: Qualifiers::default(),
        });

        assert!(!assignment_compatible_with_const(
            int_ptr_ptr,
            None,
            const_int_ptr_ptr,
            &arena
        ));
    }

    #[test]
    fn pointer_assignment_allows_void_ptr_object_roundtrip() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let void_ty = arena.intern(Type {
            kind: TypeKind::Void,
            quals: Qualifiers::default(),
        });
        let int_ptr = arena.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });
        let void_ptr = arena.intern(Type {
            kind: TypeKind::Pointer { pointee: void_ty },
            quals: Qualifiers::default(),
        });

        assert!(assignment_compatible_with_const(
            int_ptr, None, void_ptr, &arena
        ));
        assert!(assignment_compatible_with_const(
            void_ptr, None, int_ptr, &arena
        ));
    }

    #[test]
    fn pointer_comparison_ignores_qualifier_differences() {
        let mut arena = TypeArena::new();
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let const_int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers {
                is_const: true,
                is_volatile: false,
                is_restrict: false,
            },
        });
        let int_ptr = arena.intern(Type {
            kind: TypeKind::Pointer { pointee: int_ty },
            quals: Qualifiers::default(),
        });
        let const_int_ptr = arena.intern(Type {
            kind: TypeKind::Pointer {
                pointee: const_int_ty,
            },
            quals: Qualifiers::default(),
        });

        assert!(pointer_comparison_compatible(
            int_ptr,
            const_int_ptr,
            &arena,
            false
        ));
    }

    #[test]
    fn char_signed_char_unsigned_char_are_pairwise_incompatible() {
        let mut arena = TypeArena::new();
        let char_ty = arena.intern(Type {
            kind: TypeKind::Char,
            quals: Qualifiers::default(),
        });
        let schar_ty = arena.intern(Type {
            kind: TypeKind::SignedChar,
            quals: Qualifiers::default(),
        });
        let uchar_ty = arena.intern(Type {
            kind: TypeKind::UnsignedChar,
            quals: Qualifiers::default(),
        });

        assert!(!types_compatible(char_ty, schar_ty, &arena));
        assert!(!types_compatible(char_ty, uchar_ty, &arena));
        assert!(!types_compatible(schar_ty, uchar_ty, &arena));
    }

    #[test]
    fn enum_type_is_compatible_with_signed_int() {
        let mut arena = TypeArena::new();
        let enum_ty = arena.intern(Type {
            kind: TypeKind::Enum(EnumId(0)),
            quals: Qualifiers::default(),
        });
        let int_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: true },
            quals: Qualifiers::default(),
        });
        let uint_ty = arena.intern(Type {
            kind: TypeKind::Int { signed: false },
            quals: Qualifiers::default(),
        });

        assert!(types_compatible(enum_ty, int_ty, &arena));
        assert!(types_compatible(int_ty, enum_ty, &arena));
        assert!(!types_compatible(enum_ty, uint_ty, &arena));
    }
}
