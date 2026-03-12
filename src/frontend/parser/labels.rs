/// Parser label identities shared by parsing and diagnostics.
///
/// Chumsky labels are stored as strings internally, so we convert through
/// `as_str()` when assigning labels and `from_str()` when matching them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserLabel {
    Expr,
    Declaration,
    DeclarationSpecifier,
    IdentifierDeclarator,
    ExpressionStatement,
    BlockItem,
    CompoundStatement,
    ReturnStatement,
    IfStatement,
    WhileStatement,
    DoWhileStatement,
    ForStatement,
    BreakStatement,
    ContinueStatement,
    Statement,
}

impl ParserLabel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Expr => "expression",
            Self::Declaration => "declaration",
            Self::DeclarationSpecifier => "declaration specifier",
            Self::IdentifierDeclarator => "identifier declarator",
            Self::ExpressionStatement => "expression statement",
            Self::BlockItem => "block item",
            Self::CompoundStatement => "compound statement",
            Self::ReturnStatement => "return statement",
            Self::IfStatement => "if statement",
            Self::WhileStatement => "while statement",
            Self::DoWhileStatement => "do-while statement",
            Self::ForStatement => "for statement",
            Self::BreakStatement => "break statement",
            Self::ContinueStatement => "continue statement",
            Self::Statement => "statement",
        }
    }

    pub fn from_str(label: &str) -> Option<Self> {
        match label {
            "expression" => Some(Self::Expr),
            "declaration" => Some(Self::Declaration),
            "declaration specifier" => Some(Self::DeclarationSpecifier),
            "identifier declarator" => Some(Self::IdentifierDeclarator),
            "expression statement" => Some(Self::ExpressionStatement),
            "block item" => Some(Self::BlockItem),
            "compound statement" => Some(Self::CompoundStatement),
            "return statement" => Some(Self::ReturnStatement),
            "if statement" => Some(Self::IfStatement),
            "while statement" => Some(Self::WhileStatement),
            "do-while statement" => Some(Self::DoWhileStatement),
            "for statement" => Some(Self::ForStatement),
            "break statement" => Some(Self::BreakStatement),
            "continue statement" => Some(Self::ContinueStatement),
            "statement" => Some(Self::Statement),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ParserLabel;

    #[test]
    fn parser_label_round_trip() {
        let all = [
            ParserLabel::Expr,
            ParserLabel::Declaration,
            ParserLabel::DeclarationSpecifier,
            ParserLabel::IdentifierDeclarator,
            ParserLabel::ExpressionStatement,
            ParserLabel::BlockItem,
            ParserLabel::CompoundStatement,
            ParserLabel::ReturnStatement,
            ParserLabel::IfStatement,
            ParserLabel::WhileStatement,
            ParserLabel::DoWhileStatement,
            ParserLabel::ForStatement,
            ParserLabel::BreakStatement,
            ParserLabel::ContinueStatement,
            ParserLabel::Statement,
        ];

        for label in all {
            assert_eq!(ParserLabel::from_str(label.as_str()), Some(label));
        }
    }
}
