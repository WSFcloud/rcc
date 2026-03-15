use crate::common::token::TokenKind;
use logos::Logos;

/// Create a token iterator from a source string reference.
///
/// Returns an iterator of (Token, Span) tuples where errors are converted to `Token::Error`.
pub fn lexer_from_source(
    src: &str,
) -> impl Iterator<Item = (TokenKind, chumsky::span::SimpleSpan)> + '_ {
    TokenKind::lexer(src)
        .spanned()
        // Convert logos errors into tokens.
        // We want parsing to be recoverable and not fail at the lexing stage.
        // `Token::Error` variant represents a token error that was previously encountered.
        .map(|(token, span)| match token {
            // Turn the `Range<usize>` spans logos gives us into chumsky's `SimpleSpan` via `Into`.
            Ok(tok) => (tok, span.into()),
            Err(e) => (TokenKind::Error(e), span.into()),
        })
}

/// Display all tokens from a source string.
pub fn print_tokens(src: &str) {
    let tokens = lexer_from_source(src);

    println!("Source:{}", src);
    println!("Tokens:");
    for (token, span) in tokens {
        println!("\t{:?} @ {:?}", token, span);
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::token::LexingErrorType;

    fn lex_kinds(src: &str) -> Vec<TokenKind> {
        lexer_from_source(src).map(|(token, _)| token).collect()
    }

    #[test]
    fn lexes_char_and_string_literals_with_escapes() {
        let tokens = lex_kinds(r#"char c = '\n'; char *s = "hi\t\"x\"";"#);
        assert_eq!(
            tokens,
            vec![
                TokenKind::Char,
                TokenKind::Identifier("c".to_string()),
                TokenKind::Assign,
                TokenKind::CharLiteral('\n'),
                TokenKind::Semicolon,
                TokenKind::Char,
                TokenKind::Star,
                TokenKind::Identifier("s".to_string()),
                TokenKind::Assign,
                TokenKind::StringLiteral("hi\t\"x\"".to_string()),
                TokenKind::Semicolon,
            ]
        );
    }

    #[test]
    fn rejects_invalid_char_literal() {
        let tokens = lex_kinds("char c = 'ab';");
        assert!(tokens.iter().any(|token| matches!(
            token,
            TokenKind::Error(LexingErrorType::InvalidCharLiteral(_))
        )));
    }

    #[test]
    fn rejects_wide_string_literal_prefix() {
        let tokens = lex_kinds(r#"char *s = L"wide";"#);
        assert!(tokens.iter().any(|token| matches!(
            token,
            TokenKind::Error(LexingErrorType::UnsupportedLiteralPrefix(_))
        )));
    }

    #[test]
    #[ignore]
    fn test_lexer() {
        let src = "
            int main() {
                double a = 1.5e-2;
                return 0;
            }
		";

        print_tokens(src);
    }
}
