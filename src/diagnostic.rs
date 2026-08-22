//! Structured diagnostics: errors, warnings, and notes emitted by the pipeline.

use std::fmt;

use crate::source::Span;

/// The severity of a [`Diagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A fatal problem; compilation/execution cannot proceed.
    Error,
    /// A problem that does not stop the pipeline but should be addressed.
    Warning,
    /// Additional context attached to another diagnostic.
    Note,
}

/// A single diagnostic message, optionally anchored to a source [`Span`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// How serious this diagnostic is.
    pub severity: Severity,
    /// The source span this diagnostic refers to, if any.
    ///
    /// A `None` span indicates a diagnostic that is not tied to a specific
    /// location in the source (e.g., a general information message).
    pub span: Option<Span>,
    /// The human-readable message describing the diagnostic.
    pub message: String,
}

impl Diagnostic {
    /// Creates an error diagnostic with no span.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, None, message)
    }

    /// Creates a diagnostic with the given severity, span, and message.
    pub fn new(severity: Severity, span: Option<Span>, message: impl Into<String>) -> Self {
        Self {
            severity,
            span,
            message: message.into(),
        }
    }

    /// Attaches a source span to this diagnostic, consuming it.
    ///
    /// This is a fluent builder method that allows chaining:
    /// ```ignore
    /// Diagnostic::error("message").at(span)
    /// ```
    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

impl fmt::Display for Diagnostic {
    /// Formats the diagnostic as "severity: message".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.severity, self.message)
    }
}

impl fmt::Display for Severity {
    /// Formats the severity level as a lowercase string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Note => write!(f, "note"),
        }
    }
}

/// Collects diagnostics during a pipeline run.
///
/// Stages push diagnostics here instead of printing directly, so the CLI can
/// format and render them consistently.
#[derive(Default)]
pub struct DiagnosticSink {
    /// The collected diagnostics, in emission order.
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSink {
    /// Creates an empty sink with no diagnostics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a diagnostic in the sink.
    ///
    /// Diagnostics are collected in emission order and can be retrieved
    /// via [`iter`](Self::iter).
    pub fn emit(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Returns true if any error-severity diagnostic has been emitted.
    ///
    /// This is used by the CLI to determine the exit code.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Returns the number of recorded diagnostics.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Returns true if no diagnostics have been recorded.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Returns an iterator over all recorded diagnostics, in emission order.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter()
    }
}
