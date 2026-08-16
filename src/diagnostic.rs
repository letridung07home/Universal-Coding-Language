//! Structured diagnostics: errors, warnings, and notes emitted by the pipeline.

use crate::source::Span;

/// The severity of a [`Diagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A fatal problem; compilation/execution cannot proceed.
    Error,
    /// A problem that does not stop the pipeline.
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
    pub span: Option<Span>,
    /// The human-readable message.
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
    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

/// Collects diagnostics during a pipeline run.
///
/// Stages push diagnostics here instead of printing directly, so the CLI can
/// format and render them consistently.
#[derive(Default)]
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticSink {
    /// Creates an empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a diagnostic.
    pub fn emit(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Whether any error-severity diagnostic has been emitted.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Iterates over all recorded diagnostics, in emission order.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter()
    }
}
