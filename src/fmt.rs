//! Formatter: renders a parsed program as canonical source text.
//!
//! The formatter takes source text that lexes and parses cleanly and
//! re-emits it in a deterministic layout: four-space indentation, one
//! statement per line, single spaces around binary operators, and blocks
//! always expanded across lines. Formatting is idempotent — formatting
//! formatted output reproduces it byte for byte.
//!
//! Comments are preserved. The lexer reports comment spans alongside
//! tokens ([`Lexer::tokenize_with_comments`]); the formatter re-inserts
//! each comment either trailing the preceding output line (when it shared
//! a source line with the preceding code) or as a standalone line at the
//! enclosing indentation. A comment written in the middle of a long
//! expression moves to a standalone line ahead of the statement that
//! contains it. Nothing is ever dropped.
//!
//! Literal tokens (integers, strings, identifiers) are reproduced verbatim
//! from their source spans, so escape sequences and numeric spellings are
//! untouched and evaluating formatted output matches evaluating the input
//! exactly.
//!
//! Block-shaped values usually sit where they can be expanded across lines
//! (function declarations, `let` initializers, and similar). In the rare
//! spots where expansion is impossible — a block nested inside a call
//! argument or an operand — the construct is rendered compactly on one
//! line instead.

use crate::diagnostic::DiagnosticSink;
use crate::lexer::{CommentKind, CommentTrivia, Lexer};
use crate::parser::{AstKind, AstNode, Parser};
use crate::source::{SourceFile, Span};

/// The unit of indentation.
const INDENT: &str = "    ";

/// Formats `contents` as canonical source text.
///
/// The input must lex and parse without errors; otherwise the diagnostics
/// are returned and nothing is formatted. The result always ends with
/// exactly one newline, and CRLF line endings are normalized to `\n`.
pub fn format_source(name: &str, contents: &str) -> Result<String, DiagnosticSink> {
    let normalized = contents.replace("\r\n", "\n");
    let source = SourceFile::new(name, normalized);
    let mut sink = DiagnosticSink::new();
    let (tokens, comments) = Lexer::new(&source).tokenize_with_comments(&mut sink);
    if sink.has_errors() {
        return Err(sink);
    }
    let Some(ast) = Parser::new(tokens).parse(&mut sink) else {
        return Err(sink);
    };
    if sink.has_errors() {
        return Err(sink);
    }
    Ok(Formatter::new(&source, &comments).format_program(&ast))
}

/// Renders an AST back to canonical source text.
struct Formatter<'src> {
    /// The output under construction; statements append themselves here.
    out: String,
    /// The source being formatted, used to slice literal spans verbatim.
    source: &'src SourceFile,
    /// All comments in source order, with precomputed line positions.
    comments: Vec<Comment>,
    /// Index into `comments` of the next comment awaiting placement.
    next_comment: usize,
    /// Byte offsets where each source line begins, for line lookups.
    line_starts: Vec<usize>,
}

/// A comment with derived line information.
struct Comment {
    kind: CommentKind,
    span: Span,
    /// Line the comment starts on (zero-based).
    start_line: usize,
    /// Whether the comment's text spans more than one source line.
    multiline: bool,
}

/// Kinds that own a braced body and therefore deserve expanded layout when
/// they appear in a value position the formatter controls.
fn is_block_shaped(kind: &AstKind) -> bool {
    matches!(
        kind,
        AstKind::Block { .. }
            | AstKind::If { .. }
            | AstKind::While { .. }
            | AstKind::For { .. }
            | AstKind::Function { .. }
    )
}

impl<'src> Formatter<'src> {
    fn new(source: &'src SourceFile, trivia: &[CommentTrivia]) -> Self {
        let contents = source.contents();
        let mut line_starts = vec![0usize];
        for (index, byte) in contents.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        let comments = trivia
            .iter()
            .map(|trivia| Comment {
                kind: trivia.kind,
                span: trivia.span,
                start_line: line_of(&line_starts, trivia.span.start),
                multiline: source
                    .slice(trivia.span)
                    .is_some_and(|text| text.contains('\n')),
            })
            .collect();
        Self {
            out: String::new(),
            source,
            comments,
            next_comment: 0,
            line_starts,
        }
    }

    /// Formats a whole program, ending the output with one newline.
    fn format_program(mut self, program: &AstNode) -> String {
        let AstKind::Program { statements } = &program.kind else {
            unreachable!("the parser only produces a Program root");
        };
        let mut previous_end = None;
        for statement in statements {
            let placed = self.flush_comments_before(statement.span.start, previous_end, 0);
            if placed == 0 {
                self.preserve_blank_line(previous_end, statement.span.start);
            }
            self.emit_statement(statement, 0, previous_end);
            previous_end = Some(statement.span.end);
        }
        // Comments after the last statement still belong in the output.
        self.flush_comments_before(usize::MAX, previous_end, 0);
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    /// Emits one statement, first surfacing any comments written inside it.
    ///
    /// A comment may sit in a non-block position within a statement (an
    /// inline `/* */` between operands, say). Rendering happens into a
    /// scratch region; comments that rendering did not consume are placed
    /// as standalone lines ahead of the rendered statement, so they stay
    /// attached to it rather than drifting to the end of the block.
    fn emit_statement(&mut self, node: &AstNode, indent: usize, previous_end: Option<usize>) {
        let mark = self.out.len();
        self.emit_statement_inner(node, indent);
        if self.has_pending_before(node.span.end) {
            let rendered = self.out.split_off(mark);
            let placed = self.flush_comments_before(node.span.end, previous_end, indent);
            let _ = placed;
            self.out.push_str(&rendered);
        }
    }

    /// Emits one statement: indented, on its own line, semicolon-terminated
    /// unless its value is a braced construct.
    fn emit_statement_inner(&mut self, node: &AstNode, indent: usize) {
        self.out.push_str(&indent_padding(indent));
        match &node.kind {
            AstKind::Let { name, value, .. } => {
                let name = self.verbatim(*name);
                self.out.push_str("let ");
                self.out.push_str(&name);
                self.out.push_str(" = ");
                self.emit_value_suffix(value, indent);
                self.out.push_str(";\n");
            }
            AstKind::Assignment { target, value } => {
                let target = self.expr_text(target, indent);
                self.out.push_str(&target);
                self.out.push_str(" = ");
                self.emit_value_suffix(value, indent);
                self.out.push_str(";\n");
            }
            AstKind::Return { value } => match value {
                Some(value) => {
                    self.out.push_str("return ");
                    self.emit_value_suffix(value, indent);
                    self.out.push_str(";\n");
                }
                None => self.out.push_str("return;\n"),
            },
            AstKind::Break => self.out.push_str("break;\n"),
            AstKind::Continue => self.out.push_str("continue;\n"),
            AstKind::Use { path, alias, .. } => {
                let path = self.verbatim(*path);
                self.out.push_str("use ");
                self.out.push_str(&path);
                if let Some(alias) = alias {
                    let alias = self.verbatim(*alias);
                    self.out.push_str(" as ");
                    self.out.push_str(&alias);
                }
                self.out.push_str(";\n");
            }
            AstKind::Integer
            | AstKind::BooleanLiteral(_)
            | AstKind::StringLiteral
            | AstKind::Identifier
            | AstKind::Member { .. }
            | AstKind::Index { .. }
            | AstKind::List { .. }
            | AstKind::Group { .. }
            | AstKind::Unary { .. }
            | AstKind::Binary { .. }
            | AstKind::Call { .. }
            | AstKind::Program { .. } => {
                let text = self.expr_text(node, indent);
                self.out.push_str(&text);
                self.out.push_str(";\n");
            }
            AstKind::Block { .. }
            | AstKind::If { .. }
            | AstKind::While { .. }
            | AstKind::For { .. }
            | AstKind::Function { .. } => {
                self.emit_block_shaped(node, indent);
                // The parser requires a semicolon between statements even
                // when the first ends in a brace.
                self.out.push_str(";\n");
            }
        }
    }

    /// Emits the value half of `let`/assignment/`return`.
    ///
    /// Block-shaped values expand across lines starting right where the
    /// `=` ended; everything else renders on the current line.
    fn emit_value_suffix(&mut self, value: &AstNode, indent: usize) {
        if is_block_shaped(&value.kind) {
            self.emit_block_shaped(value, indent);
        } else {
            let text = self.expr_text(value, indent);
            self.out.push_str(&text);
        }
    }

    // ------------------------------------------------------------------
    // Braced constructs
    // ------------------------------------------------------------------

    /// Emits a block-shaped construct with its header on the current line.
    fn emit_block_shaped(&mut self, node: &AstNode, indent: usize) {
        match &node.kind {
            AstKind::Block { statements } => {
                self.emit_braced_body(statements, indent, node.span.end);
            }
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.expr_text(condition, indent);
                self.out.push_str("if ");
                self.out.push_str(&condition);
                self.out.push(' ');
                let AstKind::Block { statements } = &then_branch.kind else {
                    unreachable!("an if branch is always a block");
                };
                self.emit_braced_body(statements, indent, then_branch.span.end);
                match else_branch {
                    Some(else_branch) if matches!(else_branch.kind, AstKind::If { .. }) => {
                        self.out.push_str(" else ");
                        self.emit_block_shaped(else_branch, indent);
                    }
                    Some(else_branch) => {
                        let AstKind::Block { statements } = &else_branch.kind else {
                            unreachable!("an else branch is always a block or if");
                        };
                        self.out.push_str(" else ");
                        self.emit_braced_body(statements, indent, else_branch.span.end);
                    }
                    None => {}
                }
            }
            AstKind::While { condition, body } => {
                let condition = self.expr_text(condition, indent);
                self.out.push_str("while ");
                self.out.push_str(&condition);
                self.out.push(' ');
                let AstKind::Block { statements } = &body.kind else {
                    unreachable!("a while body is always a block");
                };
                self.emit_braced_body(statements, indent, body.span.end);
            }
            AstKind::For {
                variable,
                start,
                end,
                body,
            } => {
                let variable = self.verbatim(*variable);
                self.out.push_str("for ");
                self.out.push_str(&variable);
                self.out.push_str(" in ");
                match start {
                    Some(start) => {
                        let start = self.expr_text(start, indent);
                        let end = self.expr_text(end, indent);
                        self.out.push_str(&start);
                        self.out.push_str("..");
                        self.out.push_str(&end);
                    }
                    None => {
                        let iterable = self.expr_text(end, indent);
                        self.out.push_str(&iterable);
                    }
                }
                self.out.push(' ');
                let AstKind::Block { statements } = &body.kind else {
                    unreachable!("a for body is always a block");
                };
                self.emit_braced_body(statements, indent, body.span.end);
            }
            AstKind::Function {
                name,
                parameters,
                body,
                ..
            } => {
                self.out.push_str("fn");
                if let Some(name) = name {
                    let name = self.verbatim(*name);
                    self.out.push(' ');
                    self.out.push_str(&name);
                }
                self.out.push('(');
                let parameters: Vec<String> =
                    parameters.iter().map(|span| self.verbatim(*span)).collect();
                self.out.push_str(&parameters.join(", "));
                self.out.push_str(") ");
                let AstKind::Block { statements } = &body.kind else {
                    unreachable!("a function body is always a block");
                };
                self.emit_braced_body(statements, indent, body.span.end);
            }
            other => {
                unreachable!("non-block-shaped kind reached the block emitter: {other:?}");
            }
        }
    }

    /// Emits `{ ... }` around a statement list, expanding unless the block
    /// is empty and comment-free.
    ///
    /// Between statements the formatter preserves single blank lines and
    /// places pending comments; comments before the closing brace are
    /// flushed too.
    fn emit_braced_body(&mut self, statements: &[AstNode], indent: usize, close_byte: usize) {
        if statements.is_empty() && !self.has_pending_before(close_byte) {
            self.out.push_str("{}");
            return;
        }
        self.out.push_str("{\n");
        let mut previous_end = None;
        for statement in statements {
            let placed = self.flush_comments_before(statement.span.start, previous_end, indent + 1);
            if placed == 0 {
                self.preserve_blank_line(previous_end, statement.span.start);
            }
            self.emit_statement(statement, indent + 1, previous_end);
            previous_end = Some(statement.span.end);
        }
        self.flush_comments_before(close_byte, previous_end, indent + 1);
        self.out.push_str(&indent_padding(indent));
        self.out.push('}');
    }

    // ------------------------------------------------------------------
    // Single-line expressions
    // ------------------------------------------------------------------

    /// Renders an expression that fits on one line.
    ///
    /// Block-shaped constructs are compacted (see [`Self::compact`]); this
    /// path never places comments — they surface at the enclosing
    /// statement boundary instead.
    fn expr_text(&mut self, node: &AstNode, indent: usize) -> String {
        match &node.kind {
            AstKind::Integer | AstKind::StringLiteral | AstKind::Identifier => {
                self.verbatim(node.span)
            }
            AstKind::BooleanLiteral(value) => {
                if *value {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
            AstKind::Group { expression } => {
                let expression = self.expr_text(expression, indent);
                format!("({expression})")
            }
            AstKind::Unary { operator, operand } => {
                let operand = self.expr_text(operand, indent);
                format!("{operator}{operand}")
            }
            AstKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.expr_text(left, indent);
                let right = self.expr_text(right, indent);
                format!("{left} {operator} {right}")
            }
            AstKind::Call { callee, arguments } => {
                let callee = self.expr_text(callee, indent);
                let arguments: Vec<String> = arguments
                    .iter()
                    .map(|argument| self.expr_text(argument, indent))
                    .collect();
                format!("{callee}({})", arguments.join(", "))
            }
            AstKind::Index { object, index } => {
                let object = self.expr_text(object, indent);
                let index = self.expr_text(index, indent);
                format!("{object}[{index}]")
            }
            AstKind::Member { object, member } => {
                let object = self.expr_text(object, indent);
                let member = self.verbatim(*member);
                format!("{object}.{member}")
            }
            AstKind::List { elements } => {
                // A list that spanned source lines stays expanded, one
                // element per line with trailing commas; short lists stay
                // inline.
                let multiline = self
                    .source
                    .slice(node.span)
                    .is_some_and(|text| text.contains('\n'));
                if !multiline || elements.is_empty() {
                    let items: Vec<String> = elements
                        .iter()
                        .map(|element| self.expr_text(element, indent))
                        .collect();
                    format!("[{}]", items.join(", "))
                } else {
                    let mut text = String::from("[\n");
                    for element in elements {
                        let item = self.expr_text(element, indent + 1);
                        text.push_str(&indent_padding(indent + 1));
                        text.push_str(&item);
                        text.push_str(",\n");
                    }
                    text.push_str(&indent_padding(indent));
                    text.push(']');
                    text
                }
            }
            other => self.compact(other, indent),
        }
    }

    /// Renders any construct compactly on a single line.
    ///
    /// Statements inside compacted bodies keep their semicolons; braced
    /// groups stay inline. This is the deterministic fallback for
    /// block-shaped values in positions that cannot expand.
    fn compact(&mut self, kind: &AstKind, indent: usize) -> String {
        match kind {
            AstKind::Block { statements } => {
                if statements.is_empty() {
                    return "{}".to_owned();
                }
                let inner: Vec<String> = statements
                    .iter()
                    .map(|statement| self.compact_statement(statement, indent))
                    .collect();
                format!("{{ {} }}", inner.join(" "))
            }
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.compact_condition(condition, indent);
                let then_branch = self.compact_branch(then_branch, indent);
                match else_branch {
                    Some(else_branch) if matches!(else_branch.kind, AstKind::If { .. }) => {
                        let else_branch = self.compact(&else_branch.kind, indent);
                        format!("if {condition} {then_branch} else {else_branch}")
                    }
                    Some(else_branch) => {
                        let else_branch = self.compact_branch(else_branch, indent);
                        format!("if {condition} {then_branch} else {else_branch}")
                    }
                    None => format!("if {condition} {then_branch}"),
                }
            }
            AstKind::While { condition, body } => {
                let condition = self.compact_condition(condition, indent);
                let body = self.compact_branch(body, indent);
                format!("while {condition} {body}")
            }
            AstKind::For {
                variable,
                start,
                end,
                body,
            } => {
                let variable = self.verbatim(*variable);
                let header = match start {
                    Some(start) => {
                        let start = self.expr_text(start, indent);
                        let end = self.expr_text(end, indent);
                        format!("{variable} in {start}..{end}")
                    }
                    None => {
                        let iterable = self.expr_text(end, indent);
                        format!("{variable} in {iterable}")
                    }
                };
                let body = self.compact_branch(body, indent);
                format!("for {header} {body}")
            }
            AstKind::Function {
                name,
                parameters,
                body,
                ..
            } => {
                let parameters: Vec<String> =
                    parameters.iter().map(|span| self.verbatim(*span)).collect();
                let name = name
                    .map(|name| format!(" {}", self.verbatim(name)))
                    .unwrap_or_default();
                let body = self.compact_branch(body, indent);
                format!("fn{name}({}) {body}", parameters.join(", "))
            }
            other => {
                unreachable!(
                    "kinds with dedicated single-line rendering never reach `compact`: {other:?}"
                )
            }
        }
    }

    /// Renders one statement for inclusion in a compacted body.
    fn compact_statement(&mut self, node: &AstNode, indent: usize) -> String {
        match &node.kind {
            AstKind::Let { name, value, .. } => {
                let name = self.verbatim(*name);
                let value = self.value_in_narrow_position(value, indent);
                format!("let {name} = {value};")
            }
            AstKind::Assignment { target, value } => {
                let target = self.expr_text(target, indent);
                let value = self.value_in_narrow_position(value, indent);
                format!("{target} = {value};")
            }
            AstKind::Return { value } => match value {
                Some(value) => {
                    let value = self.value_in_narrow_position(value, indent);
                    format!("return {value};")
                }
                None => "return;".to_owned(),
            },
            AstKind::Break => "break;".to_owned(),
            AstKind::Continue => "continue;".to_owned(),
            AstKind::Use { path, alias, .. } => {
                let path = self.verbatim(*path);
                match alias {
                    Some(alias) => {
                        let alias = self.verbatim(*alias);
                        format!("use {path} as {alias};")
                    }
                    None => format!("use {path};"),
                }
            }
            AstKind::Integer
            | AstKind::BooleanLiteral(_)
            | AstKind::StringLiteral
            | AstKind::Identifier
            | AstKind::Member { .. }
            | AstKind::Index { .. }
            | AstKind::List { .. }
            | AstKind::Group { .. }
            | AstKind::Unary { .. }
            | AstKind::Binary { .. }
            | AstKind::Call { .. }
            | AstKind::Program { .. } => {
                let text = self.expr_text(node, indent);
                format!("{text};")
            }
            AstKind::Block { .. }
            | AstKind::If { .. }
            | AstKind::While { .. }
            | AstKind::For { .. }
            | AstKind::Function { .. } => self.compact(&node.kind, indent),
        }
    }

    /// Renders a value in a compacted context: block-shaped values
    /// compact; everything else stays single-line.
    fn value_in_narrow_position(&mut self, value: &AstNode, indent: usize) -> String {
        if is_block_shaped(&value.kind) {
            self.compact(&value.kind, indent)
        } else {
            self.expr_text(value, indent)
        }
    }

    /// Compacts a condition, wrapping block-shaped conditions in
    /// parentheses so the header stays unambiguous.
    fn compact_condition(&mut self, condition: &AstNode, indent: usize) -> String {
        let text = self.value_in_narrow_position(condition, indent);
        if is_block_shaped(&condition.kind) {
            format!("({text})")
        } else {
            text
        }
    }

    /// Compacts a branch that is always a block.
    fn compact_branch(&mut self, branch: &AstNode, indent: usize) -> String {
        let AstKind::Block { statements } = &branch.kind else {
            unreachable!("branches and loop bodies are always blocks");
        };
        if statements.is_empty() {
            return "{}".to_owned();
        }
        let inner: Vec<String> = statements
            .iter()
            .map(|statement| self.compact_statement(statement, indent))
            .collect();
        format!("{{ {} }}", inner.join(" "))
    }

    // ------------------------------------------------------------------
    // Comments and layout details
    // ------------------------------------------------------------------

    /// Whether a not-yet-placed comment ends at or before `byte`.
    fn has_pending_before(&self, byte: usize) -> bool {
        self.next_comment < self.comments.len() && self.comments[self.next_comment].span.end <= byte
    }

    /// Places every comment that ends at or before `byte`, returning how
    /// many were placed.
    ///
    /// A comment sharing a source line with the end of the previously
    /// emitted statement becomes a trailing comment on that output line;
    /// multi-line block comments always stand alone. Everything else is
    /// emitted as its own indented line (or lines, for spanning block
    /// comments, whose interior is preserved verbatim).
    fn flush_comments_before(
        &mut self,
        byte: usize,
        previous_end: Option<usize>,
        indent: usize,
    ) -> usize {
        let mut placed = 0;
        while self.next_comment < self.comments.len() {
            let comment = &self.comments[self.next_comment];
            if comment.span.end > byte {
                break;
            }
            let kind = comment.kind;
            let multiline = comment.multiline;
            let start_line = comment.start_line;
            let text = self.verbatim(comment.span);
            self.next_comment += 1;
            placed += 1;

            let trailing = !multiline
                && previous_end.is_some_and(|end| line_of(&self.line_starts, end) == start_line);
            if trailing {
                attach_trailing(&mut self.out, &text);
            } else {
                let pad = indent_padding(indent);
                for (index, line) in text.split('\n').enumerate() {
                    if index == 0 {
                        self.out.push_str(&pad);
                        self.out.push_str(line);
                    } else if kind == CommentKind::Block {
                        // Interior lines of a spanning block comment are
                        // kept exactly as written.
                        self.out.push_str(line);
                    } else {
                        self.out.push_str(&pad);
                        self.out.push_str(line);
                    }
                    self.out.push('\n');
                }
            }
        }
        placed
    }

    /// Appends one empty output line when at least one blank source line when at least one blank source line
    /// separates the two byte positions.
    fn preserve_blank_line(&mut self, previous_end: Option<usize>, next_start: usize) {
        let Some(previous_end) = previous_end else {
            return;
        };
        let contents = self.source.contents();
        let separated = previous_end <= next_start
            && contents
                .get(previous_end..next_start)
                .is_some_and(|gap| gap.matches('\n').count() >= 2);
        if separated && !self.out.ends_with("\n\n") && !self.out.is_empty() {
            self.out.push('\n');
        }
    }

    /// The exact source text covered by `span`.
    fn verbatim(&self, span: Span) -> String {
        self.source
            .slice(span)
            .expect("AST spans always fall inside their source")
            .to_owned()
    }
}

/// The indentation prefix for a nesting level.
fn indent_padding(indent: usize) -> String {
    INDENT.repeat(indent)
}

/// The zero-based line containing byte offset `pos`.
fn line_of(line_starts: &[usize], pos: usize) -> usize {
    line_starts.partition_point(|&start| start <= pos) - 1
}

/// Appends `text` to the most recently written output line.
///
/// The output always grows line by line, so the last complete line ends at
/// the final newline; the comment is spliced onto it. With no completed
/// line yet the comment joins whatever partial content exists.
fn attach_trailing(out: &mut String, text: &str) {
    let trimmed = text.replace('\n', " ");
    match out.rfind('\n') {
        Some(position) => {
            out.truncate(position);
            out.push(' ');
            out.push_str(trimmed.trim_start());
            out.push('\n');
        }
        None => {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(trimmed.trim_start());
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::format_source;

    fn fmt(source: &str) -> String {
        match format_source("test.ucl", source) {
            Ok(text) => text,
            Err(sink) => panic!("test source does not format: {} diagnostics", sink.len()),
        }
    }

    #[test]
    fn normalizes_spacing_and_indentation() {
        // Braced statements keep their required terminators.
        assert_eq!(
            fmt("let   x=1+2;\n  if x>2 { y=3;} else { y = 4 ; }"),
            "let x = 1 + 2;\nif x > 2 {\n    y = 3;\n} else {\n    y = 4;\n};\n"
        );
    }

    #[test]
    fn is_idempotent() {
        let once =
            fmt("fn  add(a,b){\n a+b;};\nlet xs=[1,  2,\n 3,];\nfor i in 0..3 { len(xs); };");
        assert_eq!(
            once,
            "fn add(a, b) {\n    a + b;\n};\nlet xs = [\n    1,\n    2,\n    3,\n];\nfor i in 0..3 {\n    len(xs);\n};\n"
        );
        assert_eq!(fmt(&once), once);
    }

    #[test]
    fn preserves_line_comments_in_place() {
        let out = fmt("let x = 1; // trailing note\n// standalone\nlet y = 2;");
        assert_eq!(
            out,
            "let x = 1; // trailing note\n// standalone\nlet y = 2;\n"
        );
    }

    #[test]
    fn preserves_block_comments_verbatim() {
        // An inline comment inside an expression surfaces above its
        // statement; spanning block comments keep their interior bytes.
        let out = fmt("/* header\n * spans lines\n */\nlet x = /* inline */ 5;");
        assert_eq!(
            out,
            "/* header\n * spans lines\n */\n/* inline */\nlet x = 5;\n"
        );
    }
}

#[cfg(test)]
mod construct_tests {
    use super::format_source;

    fn fmt(source: &str) -> String {
        match format_source("test.ucl", source) {
            Ok(text) => text,
            Err(sink) => panic!("source does not format: {} diagnostics", sink.len()),
        }
    }

    #[test]
    fn formats_all_operator_and_postfix_forms() {
        assert_eq!(
            fmt("let a = -x^2 % 3; let b = !a & a | p.q; let c = xs[i + 1]; f(1, g(2));"),
            "let a = -x ^ 2 % 3;\nlet b = !a & a | p.q;\nlet c = xs[i + 1];\nf(1, g(2));\n"
        );
    }

    #[test]
    fn keeps_string_escapes_verbatim() {
        assert_eq!(
            fmt(r#"let s = "a\n\t\"b\\";"#),
            "let s = \"a\\n\\t\\\"b\\\\\";\n"
        );
    }

    #[test]
    fn chains_else_if_without_nesting() {
        assert_eq!(
            fmt("if a { 1; } else if b { 2; } else { 3; }"),
            "if a {\n    1;\n} else if b {\n    2;\n} else {\n    3;\n};\n"
        );
    }

    #[test]
    fn nests_constructs_with_growing_indentation() {
        assert_eq!(
            fmt("fn outer() { fn inner() { return 1; }; }"),
            "fn outer() {\n    fn inner() {\n        return 1;\n    };\n};\n"
        );
    }

    #[test]
    fn renders_for_headers_in_both_forms() {
        assert_eq!(
            fmt("for i in 0..10 { 1; };\nfor item in xs { 2; };"),
            "for i in 0..10 {\n    1;\n};\nfor item in xs {\n    2;\n};\n"
        );
    }

    #[test]
    fn expands_multiline_lists_and_keeps_short_ones_inline() {
        assert_eq!(
            fmt("let a = [1,\n     2,\n];\nlet b = [1, 2];"),
            "let a = [\n    1,\n    2,\n];\nlet b = [1, 2];\n"
        );
    }

    #[test]
    fn compacts_blocks_in_narrow_positions() {
        // A block-valued expression inside a call argument cannot expand.
        assert_eq!(
            fmt("f(if true { 1; } else { 2; });"),
            "f(if true { 1; } else { 2; });\n"
        );
    }

    #[test]
    fn collapses_blank_line_runs_to_one() {
        assert_eq!(
            fmt("let a = 1;\n\n\n\nlet b = 2;"),
            "let a = 1;\n\nlet b = 2;\n"
        );
    }

    #[test]
    fn normalizes_crlf_and_final_newline() {
        assert_eq!(fmt("let a = 1;\r\nlet b = 2;"), "let a = 1;\nlet b = 2;\n");
        assert_eq!(fmt(""), "");
    }

    #[test]
    fn indents_comments_inside_blocks() {
        assert_eq!(
            fmt("fn f() {\n// about the body\nreturn 1;\n}"),
            "fn f() {\n    // about the body\n    return 1;\n};\n"
        );
    }

    #[test]
    fn keeps_comment_before_closing_brace() {
        assert_eq!(
            fmt("fn f() {\nreturn 1;\n// end of f\n}"),
            "fn f() {\n    return 1;\n    // end of f\n};\n"
        );
    }

    #[test]
    fn attaches_trailing_comments_at_depth() {
        assert_eq!(
            fmt("fn f() {\nlet x = 1; // one\nlet y = 2; // two\n}"),
            "fn f() {\n    let x = 1; // one\n    let y = 2; // two\n};\n"
        );
    }

    #[test]
    fn preserves_use_aliases_and_bare_returns() {
        assert_eq!(
            fmt("use \"math.ucl\" as m;\nuse \"util.ucl\";\nfn f() { return; }"),
            "use \"math.ucl\" as m;\nuse \"util.ucl\";\nfn f() {\n    return;\n};\n"
        );
    }

    #[test]
    fn reports_diagnostics_instead_of_formatting_broken_sources() {
        assert!(format_source("t.ucl", "let = ;").is_err());
        assert!(format_source("t.ucl", "/* unterminated").is_err());
    }
}
