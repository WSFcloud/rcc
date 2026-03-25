pub mod lexer;
pub mod parser {
    pub mod ast;
    pub mod diagnostic;
    pub mod labels;
    pub mod parse;
    pub mod typedefs;
}

pub mod sema {
    pub mod check {
        pub mod decl;
        pub mod expr;
        pub mod stmt;
    }
    pub mod const_eval;
    pub mod context;
    pub mod diagnostic;
    pub mod init;
    pub mod symbols;
    pub mod typed_ast;
    pub mod types;

    use crate::frontend::parser;
    use crate::frontend::parser::ast::ExternalDecl;
    use crate::frontend::sema::context::SemaContext;
    use crate::frontend::sema::diagnostic::SemaDiagnostic;
    use crate::frontend::sema::symbols::SymbolArena;
    use crate::frontend::sema::typed_ast::{TypedExternalDecl, TypedTranslationUnit};
    use crate::frontend::sema::types::{EnumArena, RecordArena, TypeArena};

    #[derive(Debug)]
    pub struct SemaResult {
        pub typed_tu: TypedTranslationUnit,
        pub types: TypeArena,
        pub symbols: SymbolArena,
        pub records: RecordArena,
        pub enums: EnumArena,
    }

    pub fn analyze(
        file_id: &str,
        source: &str,
        tu: &parser::ast::TranslationUnit,
    ) -> Result<SemaResult, Vec<SemaDiagnostic>> {
        let mut cx = SemaContext::new(file_id, source);

        // Pass 1: file-scope declarations and symbol ground work.
        check::decl::pass1_translation_unit(&mut cx, tu);

        // Pass 2: function body checks and typed AST assembly.
        let mut typed_items = Vec::with_capacity(tu.items.len());
        for item in &tu.items {
            match item {
                ExternalDecl::Declaration(decl) => {
                    let typed_decl = check::decl::lower_external_declaration(&mut cx, decl);
                    typed_items.push(TypedExternalDecl::Declaration(typed_decl));
                }
                ExternalDecl::FunctionDef(func) => {
                    let typed_func = check::stmt::lower_function_definition(&mut cx, func);
                    typed_items.push(TypedExternalDecl::Function(typed_func));
                }
            }
        }

        let tentative_defs = check::decl::finalize_tentative_definitions(&mut cx);
        for decl in tentative_defs {
            typed_items.push(TypedExternalDecl::Declaration(decl));
        }

        if cx.has_errors() {
            return Err(cx.take_diagnostics());
        }

        let (types, symbols, records, enums) = cx.into_arenas();
        Ok(SemaResult {
            typed_tu: TypedTranslationUnit { items: typed_items },
            types,
            symbols,
            records,
            enums,
        })
    }

    #[cfg(test)]
    mod tests;
}
