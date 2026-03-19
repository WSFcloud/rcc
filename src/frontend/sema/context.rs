use crate::common::span::SourceSpan;
use crate::common::symbol_table::SymbolTable;
use crate::frontend::sema::diagnostic::SemaDiagnostic;
use crate::frontend::sema::symbols::{ScopeLevel, Symbol, SymbolArena, SymbolId};
use crate::frontend::sema::typed_ast::LabelId;
use crate::frontend::sema::types::{EnumArena, RecordArena, TagId, Type, TypeArena, TypeId};
use std::collections::HashMap;

/// Shared semantic-analysis session state.
///
/// This struct holds all mutable state for a single semantic analysis pass:
/// - Type arena for interned types
/// - Symbol arena for all declared symbols
/// - Record and enum arenas for aggregate type definitions
/// - Symbol table for name resolution (ordinary, tag, label namespaces)
/// - Diagnostic accumulator
///
/// Rule-heavy language logic should live in check/type/symbol modules,
/// not in this context struct.
pub struct SemaContext<'a> {
    /// Source file identifier.
    pub file_id: &'a str,
    /// Source code text.
    pub source: &'a str,
    /// Accumulated diagnostics.
    diagnostics: Vec<SemaDiagnostic>,

    /// Type arena for interned types.
    pub types: TypeArena,
    /// Symbol arena for all symbols.
    pub symbols: SymbolArena,
    /// Record (struct/union) definitions.
    pub records: RecordArena,
    /// Enum definitions.
    pub enums: EnumArena,

    /// Symbol table for name resolution (ordinary, tag, label namespaces).
    symbol_table: SymbolTable<SymbolId, TagId, LabelId>,
    /// Enum constant values (separate from symbols for efficient lookup).
    enum_const_values: HashMap<SymbolId, i64>,
    /// Current scope depth (0 = file scope, >0 = block scope).
    scope_depth: usize,
    /// Cached error type ID for error recovery.
    error_type: TypeId,
}

impl<'a> SemaContext<'a> {
    /// Creates a new semantic analysis context.
    #[must_use]
    pub fn new(file_id: &'a str, source: &'a str) -> Self {
        let mut types = TypeArena::new();
        let error_type = types.intern(Type::error());

        Self {
            file_id,
            source,
            diagnostics: Vec::new(),
            types,
            symbols: SymbolArena::default(),
            records: RecordArena::default(),
            enums: EnumArena::default(),
            symbol_table: SymbolTable::new(),
            enum_const_values: HashMap::new(),
            scope_depth: 0,
            error_type,
        }
    }

    /// Returns the error type ID for error recovery.
    pub fn error_type(&self) -> TypeId {
        self.error_type
    }

    /// Emits a diagnostic message.
    pub fn emit(&mut self, diagnostic: SemaDiagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Returns `true` if any diagnostics have been emitted.
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }

    /// Returns all accumulated diagnostics without consuming them.
    pub fn diagnostics(&self) -> &[SemaDiagnostic] {
        &self.diagnostics
    }

    /// Takes all accumulated diagnostics, leaving the vector empty.
    pub fn take_diagnostics(&mut self) -> Vec<SemaDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// Retrieves a symbol by its ID.
    pub fn symbol(&self, id: SymbolId) -> &Symbol {
        self.symbols.get(id)
    }

    /// Retrieves a mutable reference to a symbol by its ID.
    pub fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        self.symbols.get_mut(id)
    }

    /// Inserts a new symbol into the arena.
    pub fn insert_symbol(&mut self, symbol: Symbol) -> SymbolId {
        self.symbols.insert(symbol)
    }

    /// Inserts a symbol into the ordinary namespace.
    ///
    /// # Errors
    ///
    /// Returns an error if a symbol with the same name already exists
    /// in the current scope.
    pub fn insert_ordinary(
        &mut self,
        name: String,
        id: SymbolId,
    ) -> Result<(), (String, SymbolId)> {
        self.symbol_table.insert_ordinary(name, id)
    }

    /// Looks up a symbol in the ordinary namespace.
    ///
    /// Returns the most recently declared symbol with the given name,
    /// searching from the current scope outward.
    pub fn lookup_ordinary(&self, name: &str) -> Option<SymbolId> {
        self.symbol_table.get_ordinary(name).copied()
    }

    /// Looks up an ordinary symbol only in the current scope.
    pub fn lookup_ordinary_in_current_scope(&self, name: &str) -> Option<SymbolId> {
        self.symbol_table
            .get_ordinary_in_current_scope(name)
            .copied()
    }

    /// Resolves a symbol with declaration-before-use checking.
    ///
    /// Returns `None` if the symbol is declared after the use site.
    /// This implements C's "declaration before use" rule.
    pub fn resolve_ordinary(&self, name: &str, use_span: SourceSpan) -> Option<SymbolId> {
        let sym_id = self.lookup_ordinary(name)?;
        let symbol = self.symbol(sym_id);
        if symbol.decl_span().start <= use_span.start {
            Some(sym_id)
        } else {
            None
        }
    }

    /// Inserts a tag into the tag namespace.
    ///
    /// # Errors
    ///
    /// Returns an error if a tag with the same name already exists
    /// in the current scope.
    pub fn insert_tag(&mut self, name: String, id: TagId) -> Result<(), (String, TagId)> {
        self.symbol_table.insert_tag(name, id)
    }

    /// Looks up a tag in the tag namespace.
    pub fn lookup_tag(&self, name: &str) -> Option<TagId> {
        self.symbol_table.get_tag(name).copied()
    }

    /// Looks up a tag only in the current scope (no outer scope lookup).
    pub fn lookup_tag_in_current_scope(&self, name: &str) -> Option<TagId> {
        self.symbol_table.get_tag_in_current_scope(name).copied()
    }

    /// Associates an enum constant symbol with its integer value.
    pub fn set_enum_const_value(&mut self, id: SymbolId, value: i64) {
        self.enum_const_values.insert(id, value);
    }

    /// Retrieves the value of an enum constant.
    pub fn lookup_enum_const_value(&self, id: SymbolId) -> Option<i64> {
        self.enum_const_values.get(&id).copied()
    }

    /// Enters a new block scope.
    pub fn enter_scope(&mut self) {
        self.symbol_table.enter_scope();
        self.scope_depth += 1;
    }

    /// Leaves the current block scope.
    pub fn leave_scope(&mut self) {
        if self.scope_depth > 0 {
            let _ = self.symbol_table.leave_scope();
            self.scope_depth -= 1;
        }
    }

    /// Returns the current scope level.
    pub fn scope_level(&self) -> ScopeLevel {
        if self.scope_depth == 0 {
            ScopeLevel::File
        } else {
            ScopeLevel::Block
        }
    }

    /// Consumes the context and returns the arenas.
    ///
    /// This is used to extract the final semantic analysis results.
    pub fn into_arenas(self) -> (TypeArena, SymbolArena, RecordArena, EnumArena) {
        (self.types, self.symbols, self.records, self.enums)
    }
}
