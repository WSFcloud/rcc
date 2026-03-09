use crate::common::token::TokenKind;
use crate::frontend::parser::parse::ParseError;
use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, sources};
use chumsky::error::{RichPattern, RichReason};

/// Convert parser tokens into human-readable strings before feeding them to ariadne.
/// Lexing errors keep their own message, normal tokens use `TokenKind::Display``.
fn render_token(token: TokenKind) -> String {
    match token {
        TokenKind::Error(err) => err.to_string(),
        other => other.to_string(),
    }
}

/// We label atom parsers with "expression" in parse.rs. This helper checks for
/// that label inside `Rich::expected()` to identify "missing operand" patterns.
fn is_expression_label(pattern: &RichPattern<'_, TokenKind>) -> bool {
    matches!(pattern, RichPattern::Label(label) if label.as_ref() == "expression")
}

/// Tokens that are valid as the first token of a unary expression.
/// When the parser expects only these + "expression", and encounters something
/// else, the likely root cause is a missing operand.
fn is_unary_expression_starter(pattern: &RichPattern<'_, TokenKind>) -> bool {
    match pattern {
        RichPattern::Token(token) => matches!(
            **token,
            TokenKind::PlusPlus
                | TokenKind::MinusMinus
                | TokenKind::Sizeof
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Bang
                | TokenKind::Tilde
                | TokenKind::Star
                | TokenKind::Amp
        ),
        _ => false,
    }
}

/// Detect a specific high-frequency diagnostic shape:
/// "an operator was parsed, then another token appeared where an operand should be".
/// Example: `4 + / 5`.
fn is_missing_expression_operand(error: &ParseError<'_>) -> bool {
    let mut has_expression_label = false;
    let mut has_unrelated_expected = false;

    for expected in error.expected() {
        if is_expression_label(expected) {
            has_expression_label = true;
            continue;
        }

        if !is_unary_expression_starter(expected) {
            has_unrelated_expected = true;
            break;
        }
    }

    has_expression_label && !has_unrelated_expected
}

/// Produce a concise top-level error summary for known patterns.
/// Falls back to chumsky's default message for all unmatched cases.
fn summarize_error_message(error: &ParseError<'_>) -> String {
    match error.reason() {
        RichReason::ExpectedFound { .. } if is_missing_expression_operand(error) => {
            match error.found() {
                Some(found) => format!(
                    "expected an expression operand, found {}",
                    render_token(found.clone())
                ),
                None => "expected an expression operand, found end of input".to_string(),
            }
        }
        _ => error.to_string(),
    }
}

/// Produce the primary inline label attached to the error span.
fn summarize_primary_label(error: &ParseError<'_>) -> String {
    match error.reason() {
        RichReason::ExpectedFound { .. } if is_missing_expression_operand(error) => {
            "expected an expression operand here".to_string()
        }
        _ => error.reason().to_string(),
    }
}

/// Render parser errors via ariadne with:
/// - one summarized headline message
/// - one primary label at error span
/// - optional parser-context labels from `Rich::contexts()`
pub fn emit_parse_diagnostics<'tokens>(
    filename: &str,
    source: &str,
    errors: Vec<ParseError<'tokens>>,
) -> std::io::Result<()> {
    let file_id = filename.to_string();
    let src = source.to_string();

    for raw_error in errors {
        let report_message = summarize_error_message(&raw_error);
        let primary_label_message = summarize_primary_label(&raw_error);
        let error = raw_error.map_token(render_token);

        Report::build(
            ReportKind::Error,
            (file_id.clone(), error.span().into_range()),
        )
        .with_config(Config::new().with_index_type(IndexType::Byte))
        .with_message(report_message)
        .with_label(
            Label::new((file_id.clone(), error.span().into_range()))
                .with_message(primary_label_message)
                .with_color(Color::Red),
        )
        .with_labels(error.contexts().map(|(label, span)| {
            Label::new((file_id.clone(), span.into_range()))
                .with_message(format!("while parsing this {label}"))
                .with_color(Color::Yellow)
        }))
        .finish()
        .print(sources([(file_id.clone(), src.clone())]))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::lexer_from_source;
    use crate::frontend::parser::parse;
    use chumsky::input::{Input, Stream};

    #[test]
    fn simplifies_missing_operand_message() {
        let src = "4+/5";
        let tokens = lexer_from_source(src);
        let stream = Stream::from_iter(tokens)
            .map((src.len()..src.len()).into(), |(token, span)| (token, span));

        let errors = parse::parse(stream).expect_err("input should fail to parse");
        let message = summarize_error_message(&errors[0]);

        assert!(
            message.contains("expected an expression operand"),
            "actual message: {message}"
        );
        assert!(message.contains("'/'"), "actual message: {message}");
    }
}
