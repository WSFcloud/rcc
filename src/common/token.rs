use std::{fmt, num::ParseIntError};

use logos::Logos;

#[derive(Debug, PartialEq, Clone, Default)]
pub enum LexingErrorType {
    InvalidInteger(String),
    InvalidFloat(String),
    NonAsciiCharacter(char),
    #[default]
    Other,
}
impl From<std::num::ParseIntError> for LexingErrorType {
    fn from(err: ParseIntError) -> Self {
        use std::num::IntErrorKind::*;
        match err.kind() {
            PosOverflow | NegOverflow => {
                LexingErrorType::InvalidInteger("overflow error".to_string())
            }
            _ => LexingErrorType::InvalidInteger("other error".to_string()),
        }
    }
}
impl From<std::num::ParseFloatError> for LexingErrorType {
    fn from(_: std::num::ParseFloatError) -> Self {
        LexingErrorType::InvalidFloat("InvalidFloat".to_string())
    }
}

impl fmt::Display for LexingErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexingErrorType::InvalidInteger(s) => write!(f, "invalid integer literal: '{}'", s),
            LexingErrorType::InvalidFloat(s) => write!(f, "invalid float literal: '{}'", s),
            LexingErrorType::NonAsciiCharacter(c) => write!(f, "non-ASCII character: '{}'", c),
            LexingErrorType::Other => write!(f, "unknown lexing error"),
        }
    }
}

impl LexingErrorType {
    fn from_lexer(lex: &mut logos::Lexer<'_, TokenKind>) -> Self {
        LexingErrorType::NonAsciiCharacter(lex.slice().chars().next().unwrap())
    }
}

/// Strip integer suffix characters (u/U/l/L) from the end, parse the numeric
/// part (decimal / hex / octal), then return the appropriate typed result.
///
/// `unsigned` / `long` / `long_long` describe which suffix was matched so that
/// logos can route to the correct variant — the actual value bits are the same.
fn parse_int_literal(
    s: &str,
    unsigned: bool,
    long: bool,
    long_long: bool,
) -> Result<u64, LexingErrorType> {
    // Strip trailing suffix characters
    let digits = s.trim_end_matches(|c: char| matches!(c, 'u' | 'U' | 'l' | 'L'));
    let value: u64 = if digits.starts_with("0x") || digits.starts_with("0X") {
        u64::from_str_radix(&digits[2..], 16)
    } else if digits.starts_with('0') && digits.len() > 1 {
        u64::from_str_radix(&digits[1..], 8)
    } else {
        digits.parse()
    }
    .map_err(|_| LexingErrorType::InvalidInteger(s.to_string()))?;
    let _ = (unsigned, long, long_long); // routing info used only by logos variant selection
    Ok(value)
}

fn parse_float_f64(s: &str) -> Result<f64, LexingErrorType> {
    s.parse::<f64>()
        .map_err(|_| LexingErrorType::InvalidFloat(s.to_string()))
}

fn parse_float_f32(s: &str) -> Result<f64, LexingErrorType> {
    // Strip trailing f/F, parse as f32 for precision check, store as f64
    let digits = s.trim_end_matches(|c: char| matches!(c, 'f' | 'F'));
    digits
        .parse::<f32>()
        .map(|v| v as f64)
        .map_err(|_| LexingErrorType::InvalidFloat(s.to_string()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncludeDirective {
    pub filename: String,
    pub is_system: bool,
}

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(error(LexingErrorType, LexingErrorType::from_lexer))]
pub enum TokenKind {
    // Literals
    // Integer: decimal / hex (0x...) / octal (0...), with optional suffix
    // Negative numbers are parsed in the parse phase
    // Longest-match order: ull > ul > ll > l > u > (none)
    #[regex(r"(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)[uU][lL][lL]", |lex| parse_int_literal(lex.slice(), true,  true,  true))]
    #[regex(r"(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)[lL][lL][uU]", |lex| parse_int_literal(lex.slice(), true,  true,  true))]
    ULongLongLiteral(u64), // ull/ULL suffix (unsigned long long, always 64-bit)
    #[regex(r"(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)[lL][lL]",     |lex| parse_int_literal(lex.slice(), false, true,  true))]
    LongLongLiteral(u64), // ll/LL suffix (signedness validated in semantic analysis)
    #[regex(r"(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)[uU][lL]",     |lex| parse_int_literal(lex.slice(), true,  true,  false))]
    #[regex(r"(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)[lL][uU]",     |lex| parse_int_literal(lex.slice(), true,  true,  false))]
    ULongLiteral(u64), // ul/UL suffix (unsigned long)
    #[regex(r"(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)[lL]",         |lex| parse_int_literal(lex.slice(), false, true,  false))]
    LongLiteral(u64), // l/L suffix (signedness validated in semantic analysis)
    #[regex(r"(0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*)[uU]",         |lex| parse_int_literal(lex.slice(), true,  false, false))]
    UIntLiteral(u64), // u/U suffix
    #[regex(r"0[xX][0-9a-fA-F]+|0[0-7]*|[1-9][0-9]*",               |lex| parse_int_literal(lex.slice(), false, false, false))]
    IntLiteral(u64), // no suffix

    // Float: decimal with optional fractional / exponent parts, optional f/F suffix
    // Patterns cover: 1.0  1.  .5  1e3  1.5e-2  etc.
    #[regex(r"([0-9]+\.[0-9]*|[0-9]*\.[0-9]+)([eE][+-]?[0-9]+)?[fF]", |lex| parse_float_f32(lex.slice()))]
    #[regex(r"[0-9]+[eE][+-]?[0-9]+[fF]",                              |lex| parse_float_f32(lex.slice()))]
    FloatLiteralF32(f64), // f/F suffix (float, 32-bit)
    #[regex(r"([0-9]+\.[0-9]*|[0-9]*\.[0-9]+)([eE][+-]?[0-9]+)?",     |lex| parse_float_f64(lex.slice()))]
    #[regex(r"[0-9]+[eE][+-]?[0-9]+",                                  |lex| parse_float_f64(lex.slice()))]
    FloatLiteral(f64), // no suffix (double)
    /// Long double literal (l/L suffix). Stores (f64_approx, f128_bytes).
    /// f128_bytes is IEEE 754 binary128 format with full 112-bit mantissa precision.
    // FloatLiteralLongDouble(f128),
    StringLiteral(String),
    /// Wide string literal (L"..."), stores content as Rust chars (each becomes wchar_t = i32)
    WideStringLiteral(String),
    /// char16_t string literal (u"..."), stores content as Rust chars (each becomes char16_t = u16)
    Char16StringLiteral(String),
    CharLiteral(char),

    // Identifier
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Identifier(String),

    // Keywords
    #[token("break")]
    Break,
    #[token("case")]
    Case,
    #[token("char")]
    Char,
    #[token("const")]
    Const,
    #[token("continue")]
    Continue,
    #[token("default")]
    Default,
    #[token("do")]
    Do,
    #[token("double")]
    Double,
    #[token("else")]
    Else,
    #[token("enum")]
    Enum,
    #[token("extern")]
    Extern,
    #[token("float")]
    Float,
    #[token("for")]
    For,
    #[token("goto")]
    Goto,
    #[token("if")]
    If,
    #[token("inline")]
    Inline,
    #[token("int")]
    Int,
    #[token("long")]
    Long,
    #[token("register")]
    Register,
    #[token("restrict")]
    Restrict,
    #[token("return")]
    Return,
    #[token("short")]
    Short,
    #[token("signed")]
    Signed,
    #[token("sizeof")]
    Sizeof,
    #[token("static")]
    Static,
    #[token("struct")]
    Struct,
    #[token("switch")]
    Switch,
    #[token("typedef")]
    Typedef,
    #[token("union")]
    Union,
    #[token("unsigned")]
    Unsigned,
    #[token("void")]
    Void,
    #[token("volatile")]
    Volatile,
    #[token("while")]
    While,

    // Punctuation
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(";")]
    Semicolon,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token("->")]
    Arrow,
    #[token("...")]
    Ellipsis,

    // Operators
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
    #[token("~")]
    Tilde,
    #[token("!")]
    Bang,
    #[token("=")]
    Assign,
    #[token("<")]
    Less,
    #[token(">")]
    Greater,
    #[token("?")]
    Question,
    #[token(":")]
    Colon,

    // Compound operators
    #[token("++")]
    PlusPlus,
    #[token("--")]
    MinusMinus,
    #[token("+=")]
    PlusAssign,
    #[token("-=")]
    MinusAssign,
    #[token("*=")]
    StarAssign,
    #[token("/=")]
    SlashAssign,
    #[token("%=")]
    PercentAssign,
    #[token("&=")]
    AmpAssign,
    #[token("|=")]
    PipeAssign,
    #[token("^=")]
    CaretAssign,
    #[token("<<")]
    LessLess,
    #[token(">>")]
    GreaterGreater,
    #[token("<<=")]
    LessLessAssign,
    #[token(">>=")]
    GreaterGreaterAssign,
    #[token("==")]
    EqualEqual,
    #[token("!=")]
    BangEqual,
    #[token("<=")]
    LessEqual,
    #[token(">=")]
    GreaterEqual,
    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,
    #[token("#")]
    Hash,
    #[token("##")]
    HashHash,

    // Special
    #[regex(r"[ \t\f\n]+", logos::skip)]
    Whitespace,
    Error(LexingErrorType),
    Eof,
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenKind::IntLiteral(_)
            | TokenKind::UIntLiteral(_)
            | TokenKind::LongLiteral(_)
            | TokenKind::ULongLiteral(_)
            | TokenKind::LongLongLiteral(_)
            | TokenKind::ULongLongLiteral(_) => write!(f, "integer constant"),
            TokenKind::FloatLiteral(_) | TokenKind::FloatLiteralF32(_) => {
                write!(f, "floating constant")
            }
            TokenKind::StringLiteral(_) => write!(f, "string literal"),
            TokenKind::WideStringLiteral(_) => write!(f, "wide string literal"),
            TokenKind::Char16StringLiteral(_) => write!(f, "char16_t string literal"),
            TokenKind::CharLiteral(_) => write!(f, "character constant"),
            TokenKind::Identifier(name) => write!(f, "'{}'", name),
            TokenKind::Break => write!(f, "'break'"),
            TokenKind::Case => write!(f, "'case'"),
            TokenKind::Char => write!(f, "'char'"),
            TokenKind::Const => write!(f, "'const'"),
            TokenKind::Continue => write!(f, "'continue'"),
            TokenKind::Default => write!(f, "'default'"),
            TokenKind::Do => write!(f, "'do'"),
            TokenKind::Double => write!(f, "'double'"),
            TokenKind::Else => write!(f, "'else'"),
            TokenKind::Enum => write!(f, "'enum'"),
            TokenKind::Extern => write!(f, "'extern'"),
            TokenKind::Float => write!(f, "'float'"),
            TokenKind::For => write!(f, "'for'"),
            TokenKind::Goto => write!(f, "'goto'"),
            TokenKind::If => write!(f, "'if'"),
            TokenKind::Inline => write!(f, "'inline'"),
            TokenKind::Int => write!(f, "'int'"),
            TokenKind::Long => write!(f, "'long'"),
            TokenKind::Register => write!(f, "'register'"),
            TokenKind::Restrict => write!(f, "'restrict'"),
            TokenKind::Return => write!(f, "'return'"),
            TokenKind::Short => write!(f, "'short'"),
            TokenKind::Signed => write!(f, "'signed'"),
            TokenKind::Sizeof => write!(f, "'sizeof'"),
            TokenKind::Static => write!(f, "'static'"),
            TokenKind::Struct => write!(f, "'struct'"),
            TokenKind::Switch => write!(f, "'switch'"),
            TokenKind::Typedef => write!(f, "'typedef'"),
            TokenKind::Union => write!(f, "'union'"),
            TokenKind::Unsigned => write!(f, "'unsigned'"),
            TokenKind::Void => write!(f, "'void'"),
            TokenKind::Volatile => write!(f, "'volatile'"),
            TokenKind::While => write!(f, "'while'"),
            TokenKind::LParen => write!(f, "'('"),
            TokenKind::RParen => write!(f, "')'"),
            TokenKind::LBrace => write!(f, "'{{'"),
            TokenKind::RBrace => write!(f, "'}}'"),
            TokenKind::LBracket => write!(f, "'['"),
            TokenKind::RBracket => write!(f, "']'"),
            TokenKind::Semicolon => write!(f, "';'"),
            TokenKind::Comma => write!(f, "','"),
            TokenKind::Dot => write!(f, "'.'"),
            TokenKind::Arrow => write!(f, "'->'"),
            TokenKind::Ellipsis => write!(f, "'...'"),
            TokenKind::Plus => write!(f, "'+'"),
            TokenKind::Minus => write!(f, "'-'"),
            TokenKind::Star => write!(f, "'*'"),
            TokenKind::Slash => write!(f, "'/'"),
            TokenKind::Percent => write!(f, "'%'"),
            TokenKind::Amp => write!(f, "'&'"),
            TokenKind::Pipe => write!(f, "'|'"),
            TokenKind::Caret => write!(f, "'^'"),
            TokenKind::Tilde => write!(f, "'~'"),
            TokenKind::Bang => write!(f, "'!'"),
            TokenKind::Assign => write!(f, "'='"),
            TokenKind::Less => write!(f, "'<'"),
            TokenKind::Greater => write!(f, "'>'"),
            TokenKind::Question => write!(f, "'?'"),
            TokenKind::Colon => write!(f, "':'"),
            TokenKind::PlusPlus => write!(f, "'++'"),
            TokenKind::MinusMinus => write!(f, "'--'"),
            TokenKind::PlusAssign => write!(f, "'+='"),
            TokenKind::MinusAssign => write!(f, "'-='"),
            TokenKind::StarAssign => write!(f, "'*='"),
            TokenKind::SlashAssign => write!(f, "'/='"),
            TokenKind::PercentAssign => write!(f, "'%='"),
            TokenKind::AmpAssign => write!(f, "'&='"),
            TokenKind::PipeAssign => write!(f, "'|='"),
            TokenKind::CaretAssign => write!(f, "'^='"),
            TokenKind::LessLess => write!(f, "'<<'"),
            TokenKind::GreaterGreater => write!(f, "'>>'"),
            TokenKind::LessLessAssign => write!(f, "'<<='"),
            TokenKind::GreaterGreaterAssign => write!(f, "'>>='"),
            TokenKind::EqualEqual => write!(f, "'=='"),
            TokenKind::BangEqual => write!(f, "'!='"),
            TokenKind::LessEqual => write!(f, "'<='"),
            TokenKind::GreaterEqual => write!(f, "'>='"),
            TokenKind::AmpAmp => write!(f, "'&&'"),
            TokenKind::PipePipe => write!(f, "'||'"),
            TokenKind::Hash => write!(f, "'#'"),
            TokenKind::HashHash => write!(f, "'##'"),
            TokenKind::Whitespace => write!(f, "<whitespace>"),
            TokenKind::Error(_) => write!(f, "{:?}", self),
            TokenKind::Eof => write!(f, "<EOF>"),
        }
    }
}
