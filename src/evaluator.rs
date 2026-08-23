//! Evaluator: executes an abstract syntax tree.
//!
//! The evaluator walks the AST produced by the [`crate::parser::Parser`]
//! and computes a [`Value`]. The [`SourceFile`] is needed alongside the AST
//! because nodes carry [`Span`]s rather than their text, so identifier names
//! and integer literals are read back out of the source.
//!
//! Operator semantics, until the specification is written:
//!
//! - Integers: `+`, `-`, `*`, `/`, `%`, and `^` (exponentiation).
//! - Comparison: `<` and `>` on integers, producing booleans.
//! - Booleans: unary `!`, and infix `&` (and) / `|` (or).
//!
//! ## Error Handling
//!
//! The evaluator follows a "collect all errors" strategy:
//! - Errors are emitted to the `DiagnosticSink` but do not stop execution
//! - Evaluation continues to report multiple errors in a single pass
//! - [`Evaluator::evaluate`] returns `None` when any runtime error occurred,
//!   and `Some(value)` with the program's value otherwise
//!
//! This design allows users to see all errors in their program at once,
//! similar to how compilers like rustc operate.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::diagnostic::{Diagnostic, DiagnosticSink, Severity};
use crate::lexer::unescape_string;
use crate::module::ModuleState;
use crate::parser::{AstKind, AstNode, BinaryOperator};
use crate::source::{SourceFile, Span};

/// The maximum evaluation-nesting depth allowed before an error is reported.
///
/// This guards against deeply nested ASTs constructed through the public API
/// that would otherwise overflow the call stack during recursive evaluation.
/// Binary operator chains are flattened into loops (see `eval`), so recursion
/// depth tracks expression nesting: any program the parser accepts at its
/// limit of 256 nested expressions stays well below this bound, and a
/// program the parser accepts must never be rejected here.
const MAX_EVAL_DEPTH: usize = 512;

/// The maximum number of iterations a single `while` loop may run.
///
/// Without a bound, `while true { }` would hang the interpreter forever.
/// The cap also keeps fuzzing and long-lived embedding processes safe from
/// runaway loops; legitimate programs rarely approach it.
const MAX_LOOP_ITERATIONS: u64 = 100_000;

/// The maximum size of a single string value, measured in UTF-8 bytes.
///
/// This keeps repeated concatenation from exhausting host memory. The limit
/// applies to string literals as well as concatenation results so every string
/// value returned by the evaluator has the same deterministic bound.
const MAX_STRING_BYTES: usize = 8 * 1024 * 1024;

/// The maximum number of active UCL function calls.
///
/// This prevents recursive programs from exhausting the host call stack.
/// Each call costs a bounded number of evaluator recursion levels, so this
/// stays safely below [`MAX_EVAL_DEPTH`].
const MAX_CALL_DEPTH: usize = 128;

/// A built-in callable supplied by the UCL prelude.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinFunction {
    /// Returns the number of Unicode scalar values in a string.
    Len,
    /// Returns the result-echo text form of any value.
    Str,
    /// Returns the name of a value's type.
    Type,
    /// Converts a string to upper case.
    Upper,
    /// Converts a string to lower case.
    Lower,
    /// Reports whether one string contains another as a substring.
    Contains,
}

impl BuiltinFunction {
    /// Returns the source-level name used to look up this built-in.
    fn name(self) -> &'static str {
        match self {
            Self::Len => "len",
            Self::Str => "str",
            Self::Type => "type",
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::Contains => "contains",
        }
    }

    /// Iterates over every built-in, in prelude registration order.
    fn all() -> impl Iterator<Item = Self> {
        [
            Self::Len,
            Self::Str,
            Self::Type,
            Self::Upper,
            Self::Lower,
            Self::Contains,
        ]
        .into_iter()
    }
}

/// A read-only collection of exported module bindings.
///
/// Namespace members are created by `use "path" as alias;` and are resolved
/// with `alias.member` expressions. The export map is kept private so UCL code
/// cannot mutate a module namespace after it has been evaluated.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleValue {
    exports: BTreeMap<String, Value>,
}

impl ModuleValue {
    /// Creates a module namespace from its completed top-level exports.
    pub(crate) fn new(exports: BTreeMap<String, Value>) -> Self {
        Self { exports }
    }

    /// Looks up an exported binding by name.
    pub(crate) fn get(&self, name: &str) -> Option<&Value> {
        self.exports.get(name)
    }

    /// Iterates over the namespace's exported bindings in deterministic order.
    pub(crate) fn exports(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.exports.iter()
    }
}

/// A runtime value produced by evaluating a program.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// No value (unit).
    ///
    /// This is the result of declarations and other statements that don't
    /// produce a value.
    Unit,
    /// A signed 64-bit integer.
    Integer(i64),
    /// A boolean value (`true` or `false`).
    Boolean(bool),
    /// A string of Unicode text.
    Str(String),
    /// A named UCL function that can be called with positional arguments.
    Function(FunctionValue),
    /// A callable supplied by the built-in prelude.
    Builtin(BuiltinFunction),
    /// A read-only namespace produced by an aliased module import.
    Module(ModuleValue),
}

/// A callable UCL function value.
///
/// A function executes with the global scope, the bindings it *captured*
/// when it was created, and a fresh parameter scope. Globals are resolved
/// dynamically at call time — which is what lets top-level functions
/// recurse by looking themselves up — while non-global bindings visible at
/// creation are captured by value: later changes to those bindings do not
/// affect an already-created function.
#[derive(Clone, Debug)]
pub struct FunctionValue {
    parameters: Vec<String>,
    body: AstNode,
    captured: HashMap<String, Value>,
    /// The source the function was defined in. Function bodies are evaluated
    /// against their own text so that a function value remains valid when
    /// called from a different source — most importantly from a later line
    /// of an interactive session.
    source: std::sync::Arc<SourceFile>,
}

/// Two function values are equal when they were created from equal code,
/// parameters, and captures; the defining source's identity is irrelevant.
impl PartialEq for FunctionValue {
    fn eq(&self, other: &Self) -> bool {
        self.parameters == other.parameters
            && self.body == other.body
            && self.captured == other.captured
    }
}

impl Value {
    /// Returns the value's short type name, used in diagnostics and by
    /// the `type` built-in.
    fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "unit",
            Value::Integer(_) => "integer",
            Value::Boolean(_) => "boolean",
            Value::Str(_) => "string",
            Value::Function(_) | Value::Builtin(_) => "function",
            Value::Module(_) => "module",
        }
    }

    /// Renders the value as text, exactly as the CLI and REPL echo results.
    ///
    /// Unit renders as `"unit"`, matching its type name; interactive front
    /// ends omit unit results instead of printing this text. Strings render
    /// their raw contents, so `str` of a string is that string.
    pub fn display_text(&self) -> String {
        match self {
            Value::Unit => self.type_name().to_owned(),
            Value::Integer(integer) => format!("{integer}"),
            Value::Boolean(boolean) => format!("{boolean}"),
            Value::Str(string) => string.clone(),
            Value::Function(_) | Value::Builtin(_) => "<function>".to_owned(),
            Value::Module(_) => "<module>".to_owned(),
        }
    }
}

/// A stack of lexical scopes, each mapping names to values.
///
/// Lookups walk the stack from the innermost scope outward, so a block can
/// shadow an outer binding while leaving the outer binding intact.
///
/// The first scope is the *global* scope: it persists for the lifetime of the
/// environment and is what function bodies resolve dynamically. Interactive
/// front ends such as a REPL keep one environment alive across evaluations so
/// bindings survive between inputs.
pub struct Environment {
    /// The stack of user scopes, with the innermost (most recent) scope at the
    /// end. The first scope is the mutable program global scope.
    scopes: Vec<HashMap<String, Value>>,
    /// Read-only built-ins consulted after every user scope. A user declaration
    /// may shadow a built-in, but assignment never mutates the prelude.
    builtins: HashMap<String, Value>,
    /// Module loading bookkeeping: what has been evaluated and what is
    /// currently in progress. Owned logic lives in [`crate::module`].
    pub(crate) modules: ModuleState,
}

impl Environment {
    /// Creates an empty environment containing only the global scope.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            builtins: BuiltinFunction::all()
                .map(|builtin| (builtin.name().to_owned(), Value::Builtin(builtin)))
                .collect(),
            modules: ModuleState::default(),
        }
    }

    /// Saves the scope stack and installs a fresh one holding only an empty
    /// global scope, isolating subsequent evaluation from existing bindings.
    pub(crate) fn isolate_globals(&mut self) -> Vec<HashMap<String, Value>> {
        std::mem::replace(&mut self.scopes, vec![HashMap::new()])
    }

    /// Appends a directory to the session's module search path.
    ///
    /// `use` statements consult these directories, in insertion order, when a
    /// module cannot be found next to the importing file. Adding paths after
    /// modules have already been evaluated is allowed but only affects later
    /// lookups; completed exports stay cached for the session.
    pub fn add_search_path(&mut self, path: impl Into<std::path::PathBuf>) {
        self.modules.push_search_path(path.into());
    }

    /// Restores a scope stack saved by [`Environment::isolate_globals`] and
    /// returns the bindings of the (replaced) global scope.
    pub(crate) fn restore_globals(
        &mut self,
        saved: Vec<HashMap<String, Value>>,
    ) -> HashMap<String, Value> {
        std::mem::replace(&mut self.scopes, saved)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// Binds `name` in the global scope. Returns false when the name is
    /// already bound there, leaving the existing binding untouched.
    pub(crate) fn define_global(&mut self, name: String, value: Value) -> bool {
        let global = &mut self.scopes[0];
        if global.contains_key(&name) {
            return false;
        }
        global.insert(name, value);
        true
    }

    /// Returns whether `name` is already bound in the program global scope.
    pub(crate) fn has_global(&self, name: &str) -> bool {
        self.scopes[0].contains_key(name)
    }

    /// Pushes a new, empty scope onto the stack.
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pops the innermost scope from the stack.
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Binds `name` to `value` in the innermost scope.
    ///
    /// If a binding with the same name already exists in the innermost scope,
    /// it is shadowed (replaced) by the new binding.
    fn define(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("the environment always has at least one scope")
            .insert(name.to_owned(), value);
    }

    /// Looks up `name`, searching scopes from innermost outward.
    ///
    /// Returns the first binding found, or `None` if no binding exists.
    fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.builtins.get(name))
    }

    /// Snapshots the non-global bindings visible from innermost to outermost
    /// scope, used as a function literal's capture.
    ///
    /// Inner scopes shadow outer ones; the first binding seen walking outward
    /// wins. Globals are excluded: they resolve dynamically at call time.
    fn capture_non_globals(&self) -> HashMap<String, Value> {
        let mut captured = HashMap::new();
        for scope in self.scopes.iter().skip(1).rev() {
            for (name, value) in scope {
                captured
                    .entry(name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        captured
    }

    /// Begins a function call with the global scope, the function's captured
    /// bindings, and a fresh parameter scope on top, returning the caller's
    /// scopes for restoration when the call finishes.
    fn begin_call(&mut self, captured: &HashMap<String, Value>) -> Vec<HashMap<String, Value>> {
        let mut caller_scopes = std::mem::take(&mut self.scopes);
        let global = caller_scopes.remove(0);
        self.scopes.push(global);
        let captured = captured
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.scopes.push(captured);
        self.push_scope();
        caller_scopes
    }

    /// Restores caller scopes after a function call while preserving changes
    /// the function made to global bindings.
    fn end_call(&mut self, mut caller_scopes: Vec<HashMap<String, Value>>) {
        let global = self.scopes.remove(0);
        caller_scopes.insert(0, global);
        self.scopes = caller_scopes;
    }

    /// Reassigns an existing binding. Returns `false` if `name` is unbound.
    ///
    /// The assignment searches scopes from innermost to outermost and updates
    /// the first binding found. This allows shadowed bindings to be updated
    /// correctly.
    fn assign(&mut self, name: &str, value: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = value;
                return true;
            }
        }
        false
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolves a module path from a `use` statement against the importing
/// source. Paths are relative to the importing file's directory; sources
/// without a directory (such as the REPL's `<repl>`) resolve relative to the
/// current working directory.
/// Walks an [`AstNode`] and computes a [`Value`].
///
/// The evaluator implements the semantic rules of the UCL language,
/// executing the abstract syntax tree to produce runtime values.
/// A control-flow signal produced by `return`, `break`, or `continue`,
/// waiting to be consumed at its matching boundary.
#[derive(Clone, Debug)]
enum Flow {
    /// A function is returning with this value.
    Return(Value),
    /// The innermost enclosing `while` loop should exit.
    Break,
    /// The innermost enclosing `while` loop should skip to its next check.
    Continue,
}

/// Evaluates an abstract syntax tree against an environment.
pub struct Evaluator {
    /// Number of UCL calls currently active during evaluation.
    call_depth: Cell<usize>,
    /// Whether evaluation has exhausted a deterministic resource limit.
    ///
    /// Once set, remaining work in the current evaluation is skipped so one
    /// resource error cannot cascade into unrelated type errors or more work.
    resource_exhausted: Cell<bool>,
    /// A control-flow signal executed by the innermost active construct,
    /// waiting to be consumed at that construct's boundary.
    pending_flow: RefCell<Option<Flow>>,
}

impl Evaluator {
    /// Creates a new evaluator instance.
    ///
    /// The evaluator may be reused to evaluate multiple ASTs; each evaluation
    /// resets its active function-call counter, resource state, and any pending control-flow signal.
    pub fn new() -> Self {
        Self {
            call_depth: Cell::new(0),
            resource_exhausted: Cell::new(false),
            pending_flow: RefCell::new(None),
        }
    }

    /// Evaluates the given AST to produce a value.
    ///
    /// `source` provides the text that the AST's spans point into. On
    /// success, the result is the value of the program's last statement, or
    /// [`Value::Unit`] if the program is empty.
    ///
    /// # Error Handling
    ///
    /// Returns `None` if any runtime error was emitted to `sink` during this
    /// call (diagnostics already present in the sink before the call are not
    /// considered), and `Some(value)` otherwise. This makes failure explicit:
    /// callers never need to guess whether a [`Value::Unit`] result means a
    /// real unit value or an error.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let source = SourceFile::new("example.ucl", "2 + 3 * 4");
    /// let mut sink = DiagnosticSink::new();
    /// let ast = /* parse the source */;
    /// match Evaluator::new().evaluate(&ast, &source, &mut sink) {
    ///     Some(value) => println!("{value:?}"),
    ///     None => { /* handle runtime errors */ }
    /// }
    /// ```
    pub fn evaluate(
        &self,
        root: &AstNode,
        source: &SourceFile,
        sink: &mut DiagnosticSink,
    ) -> Option<Value> {
        let mut environment = Environment::new();
        self.evaluate_in(&mut environment, root, source, sink)
    }

    /// Evaluates the given AST inside an existing [`Environment`].
    ///
    /// This is the stateful counterpart to [`Evaluator::evaluate`]: bindings
    /// created in the environment's global scope survive across calls, which
    /// lets an interactive front end build on earlier inputs. The global
    /// scope must be present; block scopes pushed during evaluation are
    /// popped again before returning.
    ///
    /// Failure semantics match [`Evaluator::evaluate`], including the
    /// baseline diagnostic accounting.
    pub fn evaluate_in(
        &self,
        environment: &mut Environment,
        root: &AstNode,
        source: &SourceFile,
        sink: &mut DiagnosticSink,
    ) -> Option<Value> {
        self.call_depth.set(0);
        self.resource_exhausted.set(false);
        self.pending_flow.borrow_mut().take();
        let baseline = sink.len();
        let value = self.eval(root, source, environment, sink, 0);
        // A control-flow signal that reaches the program boundary never ran
        // inside its matching construct, which is an error in itself.
        match self.pending_flow.borrow_mut().take() {
            Some(Flow::Return(_)) => {
                sink.emit(Diagnostic::error("`return` outside of a function").at(root.span));
            }
            Some(Flow::Break) => {
                sink.emit(Diagnostic::error("`break` outside of a loop").at(root.span));
            }
            Some(Flow::Continue) => {
                sink.emit(Diagnostic::error("`continue` outside of a loop").at(root.span));
            }
            None => {}
        }
        let failed = sink
            .iter()
            .skip(baseline)
            .any(|d| d.severity == Severity::Error);
        if failed { None } else { Some(value) }
    }

    pub(crate) fn has_pending_flow(&self) -> bool {
        self.pending_flow.borrow().is_some()
    }

    /// Consumes a pending loop signal after a body iteration.
    ///
    /// `Break` is consumed and reports that the loop must exit; `Continue`
    /// is consumed and lets iteration proceed to the next condition check.
    /// Anything else — no pending signal or a `return` waiting for its
    /// function boundary — leaves iteration to the caller's other checks.
    fn take_loop_signal(&self) -> bool {
        let mut flow = self.pending_flow.borrow_mut();
        match &*flow {
            Some(Flow::Break) => {
                *flow = None;
                true
            }
            Some(Flow::Continue) => {
                *flow = None;
                false
            }
            _ => false,
        }
    }

    /// Internal evaluation function that walks the AST.
    ///
    /// This recursively evaluates nodes and returns their values.
    /// Errors are emitted to the sink but do not stop evaluation.
    pub(crate) fn eval(
        &self,
        node: &AstNode,
        source: &SourceFile,
        environment: &mut Environment,
        sink: &mut DiagnosticSink,
        depth: usize,
    ) -> Value {
        if self.resource_exhausted.get() || sink.is_full() {
            return Value::Unit;
        }
        if depth >= MAX_EVAL_DEPTH {
            sink.emit(Diagnostic::error("evaluation nesting is too deep").at(node.span));
            return Value::Unit;
        }

        match &node.kind {
            AstKind::Program { statements } => {
                let mut last = Value::Unit;
                for statement in statements {
                    last = self.eval(statement, source, environment, sink, depth + 1);
                    if self.has_pending_flow() || self.resource_exhausted.get() || sink.is_full() {
                        break;
                    }
                }
                last
            }
            AstKind::Block { statements } => {
                environment.push_scope();
                let mut last = Value::Unit;
                for statement in statements {
                    last = self.eval(statement, source, environment, sink, depth + 1);
                    if self.has_pending_flow() || self.resource_exhausted.get() || sink.is_full() {
                        break;
                    }
                }
                environment.pop_scope();
                last
            }
            AstKind::Let { name, value, .. } => {
                // Spans come from the parser and are valid there, but ASTs
                // can also be constructed through the public API with
                // arbitrary spans; report those instead of panicking.
                let Some(name) = source.slice(*name) else {
                    sink.emit(
                        Diagnostic::error("declaration has an invalid name span").at(node.span),
                    );
                    return Value::Unit;
                };
                let name = name.to_owned();
                let value = self.eval(value, source, environment, sink, depth + 1);
                environment.define(&name, value);
                Value::Unit
            }
            AstKind::Identifier => {
                let name = match source.slice(node.span) {
                    Some(name) => name,
                    None => {
                        sink.emit(Diagnostic::error("invalid identifier span").at(node.span));
                        return Value::Unit;
                    }
                };
                match environment.lookup(name) {
                    Some(value) => value.clone(),
                    None => {
                        sink.emit(
                            Diagnostic::error(format!("undefined variable `{name}`")).at(node.span),
                        );
                        Value::Unit
                    }
                }
            }
            AstKind::Member { object, member } => {
                let object = self.eval(object, source, environment, sink, depth + 1);
                let member_span = *member;
                let Some(member) = source.slice(member_span) else {
                    sink.emit(Diagnostic::error("invalid module member span").at(node.span));
                    return Value::Unit;
                };
                match object {
                    Value::Module(module) => match module.get(member) {
                        Some(value) => value.clone(),
                        None => {
                            sink.emit(
                                Diagnostic::error(format!(
                                    "module has no exported member `{member}`"
                                ))
                                .at(member_span),
                            );
                            Value::Unit
                        }
                    },
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "cannot access member `{member}` on value of type `{}`",
                                other.type_name()
                            ))
                            .at(node.span),
                        );
                        Value::Unit
                    }
                }
            }
            AstKind::Integer => {
                let text = match source.slice(node.span) {
                    Some(text) => text,
                    None => {
                        sink.emit(Diagnostic::error("invalid integer literal span").at(node.span));
                        return Value::Unit;
                    }
                };
                let text = text.to_owned();
                match text.parse::<i64>() {
                    Ok(value) => Value::Integer(value),
                    Err(_) => {
                        sink.emit(
                            Diagnostic::error(format!("integer literal `{text}` is out of range"))
                                .at(node.span),
                        );
                        Value::Integer(0)
                    }
                }
            }
            AstKind::BooleanLiteral(value) => Value::Boolean(*value),
            AstKind::StringLiteral => {
                let text = match source.slice(node.span) {
                    Some(text) => text,
                    None => {
                        sink.emit(Diagnostic::error("invalid string literal span").at(node.span));
                        return Value::Unit;
                    }
                };
                let value = unescape_string(text);
                if self.check_string_size(value.len(), node.span, sink) {
                    Value::Str(value)
                } else {
                    Value::Unit
                }
            }
            AstKind::Group { expression } => {
                self.eval(expression, source, environment, sink, depth + 1)
            }
            AstKind::Unary { operator, operand } => {
                let operand = self.eval(operand, source, environment, sink, depth + 1);
                self.eval_unary(*operator, operand, node.span, sink)
            }
            AstKind::Binary {
                operator,
                left,
                right,
            } => {
                // A flat, left-associative chain such as `1 + 1 + ...` adds
                // one evaluator level per link without adding any parser
                // nesting, so recursing per link would reject arbitrarily
                // long chains the parser accepts. Flattening the left spine
                // into an explicit loop keeps recursion depth bounded by
                // expression nesting instead of chain length. Operands are
                // still evaluated strictly left to right.
                let mut operators = vec![*operator];
                let mut operands = vec![right];
                let mut spans = vec![node.span];
                let mut current: &AstNode = left;
                while let AstKind::Binary {
                    operator: nested_operator,
                    left: nested_left,
                    right: nested_right,
                } = &current.kind
                {
                    operators.push(*nested_operator);
                    operands.push(nested_right);
                    spans.push(current.span);
                    current = nested_left;
                }

                let mut accumulator = self.eval(current, source, environment, sink, depth + 1);
                for index in (0..operators.len()).rev() {
                    let operator = operators[index];
                    // Short-circuiting: `&` skips its right-hand side once the
                    // left is false and `|` skips it once the left is true.
                    if operator == BinaryOperator::And && accumulator == Value::Boolean(false) {
                        continue;
                    }
                    if operator == BinaryOperator::Or && accumulator == Value::Boolean(true) {
                        continue;
                    }
                    let operand = self.eval(operands[index], source, environment, sink, depth + 1);
                    accumulator =
                        self.eval_binary(operator, accumulator, operand, spans[index], sink);
                }
                accumulator
            }
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_value = self.eval(condition, source, environment, sink, depth + 1);
                match condition_value {
                    Value::Boolean(true) => {
                        self.eval(then_branch, source, environment, sink, depth + 1)
                    }
                    Value::Boolean(false) => match else_branch {
                        Some(branch) => self.eval(branch, source, environment, sink, depth + 1),
                        None => Value::Unit,
                    },
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "condition of `if` must be a boolean, found `{}`",
                                other.type_name()
                            ))
                            .at(condition.span),
                        );
                        Value::Unit
                    }
                }
            }
            AstKind::Function {
                name,
                parameters,
                body,
                ..
            } => {
                let mut names = Vec::with_capacity(parameters.len());
                let mut seen = HashSet::new();
                for parameter_span in parameters {
                    let Some(parameter) = source.slice(*parameter_span) else {
                        sink.emit(
                            Diagnostic::error("function declaration has an invalid parameter span")
                                .at(node.span),
                        );
                        return Value::Unit;
                    };
                    if !seen.insert(parameter) {
                        sink.emit(
                            Diagnostic::error(format!(
                                "duplicate function parameter `{parameter}`"
                            ))
                            .at(*parameter_span),
                        );
                        return Value::Unit;
                    }
                    names.push(parameter.to_owned());
                }

                // Capture-by-value: snapshot the non-global bindings visible
                // where the function is created. Globals stay dynamic so a
                // top-level function can still resolve itself recursively.
                let captured = environment.capture_non_globals();
                let function = Value::Function(FunctionValue {
                    parameters: names,
                    body: (**body).clone(),
                    captured,
                    source: std::sync::Arc::new(source.clone()),
                });

                match name {
                    Some(name_span) => {
                        // A named declaration binds the name and evaluates to
                        // unit; an anonymous literal is itself the value.
                        let Some(name) = source.slice(*name_span) else {
                            sink.emit(
                                Diagnostic::error("function declaration has an invalid name span")
                                    .at(node.span),
                            );
                            return Value::Unit;
                        };
                        environment.define(name, function);
                        Value::Unit
                    }
                    None => function,
                }
            }
            AstKind::Call { callee, arguments } => {
                let callable = self.eval(callee, source, environment, sink, depth + 1);
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(self.eval(argument, source, environment, sink, depth + 1));
                }

                match callable {
                    Value::Builtin(builtin) => self.eval_builtin(builtin, &values, node.span, sink),
                    Value::Function(function) => {
                        if function.parameters.len() != values.len() {
                            sink.emit(
                                Diagnostic::error(format!(
                                    "function expected {} argument(s), received {}",
                                    function.parameters.len(),
                                    values.len()
                                ))
                                .at(node.span),
                            );
                            return Value::Unit;
                        }
                        let call_depth = self.call_depth.get();
                        if call_depth >= MAX_CALL_DEPTH {
                            sink.emit(
                                Diagnostic::error("function call depth is too deep").at(node.span),
                            );
                            return Value::Unit;
                        }

                        self.call_depth.set(call_depth + 1);
                        let caller_scopes = environment.begin_call(&function.captured);
                        for (parameter, value) in function.parameters.iter().zip(values) {
                            environment.define(parameter, value);
                        }
                        // The body is evaluated against the source the function was
                        // defined in, not the caller's; spans inside the body point
                        // there.
                        let value = self.eval(
                            &function.body,
                            &function.source,
                            environment,
                            sink,
                            depth + 1,
                        );
                        environment.end_call(caller_scopes);
                        // Consume whatever control-flow signal the body
                        // executed. A `return` supplies the call's value; a
                        // `break` or `continue` never had a matching loop in
                        // the body, which is an error at the call site.
                        let value = match self.pending_flow.borrow_mut().take() {
                            Some(Flow::Return(returned)) => returned,
                            Some(Flow::Break) => {
                                sink.emit(
                                    Diagnostic::error("`break` outside of a loop").at(callee.span),
                                );
                                Value::Unit
                            }
                            Some(Flow::Continue) => {
                                sink.emit(
                                    Diagnostic::error("`continue` outside of a loop")
                                        .at(callee.span),
                                );
                                Value::Unit
                            }
                            None => value,
                        };
                        self.call_depth.set(call_depth);
                        value
                    }
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "cannot call value of type `{}`",
                                other.type_name()
                            ))
                            .at(callee.span),
                        );
                        Value::Unit
                    }
                }
            }
            AstKind::While { condition, body } => {
                let mut iterations = 0u64;
                loop {
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        sink.emit(
                            Diagnostic::error("loop exceeded the maximum number of iterations")
                                .at(node.span),
                        );
                        break;
                    }
                    let condition_value =
                        self.eval(condition, source, environment, sink, depth + 1);
                    match condition_value {
                        Value::Boolean(true) => {
                            self.eval(body, source, environment, sink, depth + 1);
                            // A `break` or `continue` from the body is
                            // consumed by this loop; a pending `return` (or
                            // exhausted resources) stops iteration without
                            // being consumed so it propagates outward.
                            let exit_for_break = self.take_loop_signal();
                            if self.has_pending_flow()
                                || self.resource_exhausted.get()
                                || sink.is_full()
                                || exit_for_break
                            {
                                break;
                            }
                        }
                        Value::Boolean(false) => break,
                        other => {
                            sink.emit(
                                Diagnostic::error(format!(
                                    "condition of `while` must be a boolean, found `{}`",
                                    other.type_name()
                                ))
                                .at(condition.span),
                            );
                            break;
                        }
                    }
                }
                Value::Unit
            }
            AstKind::Assignment { target, value } => {
                let name = match &target.kind {
                    AstKind::Identifier => match source.slice(target.span) {
                        Some(name) => name.to_owned(),
                        None => {
                            sink.emit(
                                Diagnostic::error("invalid assignment target span").at(target.span),
                            );
                            return Value::Unit;
                        }
                    },
                    _ => {
                        sink.emit(Diagnostic::error("invalid assignment target").at(target.span));
                        return Value::Unit;
                    }
                };
                // Fast path: `name = name + <pure expression>` appends to the
                // existing string in place instead of rebuilding it from a
                // fresh copy on every iteration. This keeps accumulation loops
                // such as `acc = acc + "!"` linear in total work.
                if let AstKind::Binary {
                    operator: BinaryOperator::Add,
                    left,
                    right,
                } = &value.kind
                {
                    if matches!(left.kind, AstKind::Identifier)
                        && source.slice(left.span) == Some(name.as_str())
                        && !Self::may_mutate_bindings(right)
                        && environment.lookup(&name).is_some()
                    {
                        if let Some(value) = self.try_append_in_place(
                            &name,
                            value.span,
                            right,
                            source,
                            environment,
                            sink,
                            depth,
                        ) {
                            return value;
                        }
                    }
                }
                let value = self.eval(value, source, environment, sink, depth + 1);
                if !environment.assign(&name, value.clone()) {
                    sink.emit(
                        Diagnostic::error(format!("cannot assign to undefined variable `{name}`"))
                            .at(target.span),
                    );
                }
                value
            }
            AstKind::Return { value } => {
                // Record the value for the enclosing call boundary to consume;
                // enclosing loops and blocks stop early once it is set.
                let returned = match value {
                    Some(expression) => self.eval(expression, source, environment, sink, depth + 1),
                    None => Value::Unit,
                };
                *self.pending_flow.borrow_mut() = Some(Flow::Return(returned.clone()));
                returned
            }
            AstKind::Break => {
                // The innermost enclosing `while` consumes the signal;
                // boundaries that never see a loop report the error instead.
                *self.pending_flow.borrow_mut() = Some(Flow::Break);
                Value::Unit
            }
            AstKind::Continue => {
                *self.pending_flow.borrow_mut() = Some(Flow::Continue);
                Value::Unit
            }
            AstKind::Use { path, alias, .. } => {
                self.eval_use(path, alias, source, environment, sink, depth)
            }
        }
    }

    /// Evaluates a callable from the built-in prelude.
    fn eval_builtin(
        &self,
        builtin: BuiltinFunction,
        values: &[Value],
        span: Span,
        sink: &mut DiagnosticSink,
    ) -> Value {
        match builtin {
            BuiltinFunction::Len => {
                if !Self::check_arity("len", values, 1, span, sink) {
                    return Value::Unit;
                }
                match &values[0] {
                    Value::Str(value) => Value::Integer(value.chars().count() as i64),
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "`len` expects a string argument, found `{}`",
                                other.type_name()
                            ))
                            .at(span),
                        );
                        Value::Unit
                    }
                }
            }
            BuiltinFunction::Str => {
                if !Self::check_arity("str", values, 1, span, sink) {
                    return Value::Unit;
                }
                let text = values[0].display_text();
                if !self.check_string_size(text.len(), span, sink) {
                    return Value::Unit;
                }
                Value::Str(text)
            }
            BuiltinFunction::Type => {
                if !Self::check_arity("type", values, 1, span, sink) {
                    return Value::Unit;
                }
                Value::Str(values[0].type_name().to_owned())
            }
            BuiltinFunction::Upper | BuiltinFunction::Lower => {
                let name = builtin.name();
                if !Self::check_arity(name, values, 1, span, sink) {
                    return Value::Unit;
                }
                match &values[0] {
                    Value::Str(value) => {
                        let mapped = match builtin {
                            BuiltinFunction::Upper => value.to_uppercase(),
                            _ => value.to_lowercase(),
                        };
                        if !self.check_string_size(mapped.len(), span, sink) {
                            return Value::Unit;
                        }
                        Value::Str(mapped)
                    }
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "`{name}` expects a string argument, found `{}`",
                                other.type_name()
                            ))
                            .at(span),
                        );
                        Value::Unit
                    }
                }
            }
            BuiltinFunction::Contains => {
                if !Self::check_arity("contains", values, 2, span, sink) {
                    return Value::Unit;
                }
                let (haystack, needle) = (&values[0], &values[1]);
                if let Value::Str(haystack) = haystack {
                    if let Value::Str(needle) = needle {
                        return Value::Boolean(haystack.contains(needle.as_str()));
                    }
                    sink.emit(
                        Diagnostic::error(format!(
                            "`contains` expects a string needle, found `{}`",
                            needle.type_name()
                        ))
                        .at(span),
                    );
                    return Value::Unit;
                }
                sink.emit(
                    Diagnostic::error(format!(
                        "`contains` expects a string haystack, found `{}`",
                        haystack.type_name()
                    ))
                    .at(span),
                );
                Value::Unit
            }
        }
    }

    /// Checks that a built-in call received exactly `expected` arguments,
    /// emitting an arity error and returning false otherwise.
    fn check_arity(
        name: &str,
        values: &[Value],
        expected: usize,
        span: Span,
        sink: &mut DiagnosticSink,
    ) -> bool {
        if values.len() == expected {
            return true;
        }
        sink.emit(
            Diagnostic::error(format!(
                "`{name}` expected {expected} argument(s), received {}",
                values.len()
            ))
            .at(span),
        );
        false
    }

    fn eval_unary(
        &self,
        operator: char,
        operand: Value,
        span: Span,
        sink: &mut DiagnosticSink,
    ) -> Value {
        match (&operand, operator) {
            (Value::Integer(value), '+') => Value::Integer(*value),
            (Value::Integer(value), '-') => match value.checked_neg() {
                Some(negated) => Value::Integer(negated),
                None => overflow(span, sink),
            },
            (Value::Boolean(value), '!') => Value::Boolean(!value),
            _ => unary_type_error(operator, &operand, span, sink),
        }
    }

    fn eval_binary(
        &self,
        operator: BinaryOperator,
        left: Value,
        right: Value,
        span: Span,
        sink: &mut DiagnosticSink,
    ) -> Value {
        use BinaryOperator::*;
        match operator {
            Add => {
                // `+` is overloaded: integer addition or string concatenation.
                match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => match a.checked_add(b) {
                        Some(sum) => Value::Integer(sum),
                        None => overflow(span, sink),
                    },
                    (Value::Str(a), Value::Str(b)) => {
                        let length = a.len().saturating_add(b.len());
                        if !self.check_string_size(length, span, sink) {
                            return Value::Unit;
                        }
                        let mut concatenated = String::with_capacity(length);
                        concatenated.push_str(&a);
                        concatenated.push_str(&b);
                        Value::Str(concatenated)
                    }
                    (l, r) => binary_type_error(operator, &l, &r, span, sink),
                }
            }
            Sub | Mul | Div | Rem | Pow => {
                let (a, b) = match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                match operator {
                    Sub => match a.checked_sub(b) {
                        Some(difference) => Value::Integer(difference),
                        None => overflow(span, sink),
                    },
                    Mul => match a.checked_mul(b) {
                        Some(product) => Value::Integer(product),
                        None => overflow(span, sink),
                    },
                    Div if b == 0 => division_by_zero(span, sink),
                    Div => match a.checked_div(b) {
                        Some(quotient) => Value::Integer(quotient),
                        None => overflow(span, sink),
                    },
                    Rem if b == 0 => division_by_zero(span, sink),
                    Rem => match a.checked_rem(b) {
                        Some(remainder) => Value::Integer(remainder),
                        None => overflow(span, sink),
                    },
                    Pow if b < 0 => {
                        sink.emit(
                            Diagnostic::error("negative exponents are not supported").at(span),
                        );
                        Value::Unit
                    }
                    Pow => {
                        let exponent = match u32::try_from(b) {
                            Ok(exponent) => exponent,
                            // The exponent exceeds `u32::MAX`. Only the bases
                            // 0, 1, and -1 can produce an in-range result from
                            // such an exponent; every other base overflows the
                            // signed 64-bit range.
                            Err(_) => {
                                return match a {
                                    0 => Value::Integer(0),
                                    1 => Value::Integer(1),
                                    -1 => Value::Integer(if b % 2 == 0 { 1 } else { -1 }),
                                    _ => overflow(span, sink),
                                };
                            }
                        };
                        match a.checked_pow(exponent) {
                            Some(power) => Value::Integer(power),
                            None => overflow(span, sink),
                        }
                    }
                    _ => unreachable!("`operator` is restricted to arithmetic operators"),
                }
            }
            Less | Greater | LessEqual | GreaterEqual => {
                let order = match (left, right) {
                    // Relational ordering is defined for two integers
                    // numerically and for two strings lexicographically (by
                    // Unicode scalar value); mixing types is an error.
                    (Value::Integer(a), Value::Integer(b)) => a.cmp(&b),
                    (Value::Str(a), Value::Str(b)) => a.as_str().cmp(b.as_str()),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                Value::Boolean(match operator {
                    Less => order == Ordering::Less,
                    Greater => order == Ordering::Greater,
                    LessEqual => order != Ordering::Greater,
                    GreaterEqual => order != Ordering::Less,
                    _ => unreachable!("`operator` is restricted to relational operators"),
                })
            }
            Equal | NotEqual => {
                // Equality is defined for two integers, two booleans, or two
                // strings; comparing values of different types is an error.
                match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => {
                        Value::Boolean(if operator == Equal { a == b } else { a != b })
                    }
                    (Value::Boolean(a), Value::Boolean(b)) => {
                        Value::Boolean(if operator == Equal { a == b } else { a != b })
                    }
                    (Value::Str(a), Value::Str(b)) => {
                        Value::Boolean(if operator == Equal { a == b } else { a != b })
                    }
                    (l, r) => binary_type_error(operator, &l, &r, span, sink),
                }
            }
            And | Or => {
                // Reached only when short-circuiting did not skip the
                // right-hand side (see the `Binary` arm in `eval`).
                let (a, b) = match (left, right) {
                    (Value::Boolean(a), Value::Boolean(b)) => (a, b),
                    (l, r) => return binary_type_error(operator, &l, &r, span, sink),
                };
                Value::Boolean(if operator == And { a && b } else { a || b })
            }
        }
    }

    /// Returns whether a string with `length` UTF-8 bytes is within the
    /// evaluator's deterministic string-value limit.
    fn check_string_size(&self, length: usize, span: Span, sink: &mut DiagnosticSink) -> bool {
        if length <= MAX_STRING_BYTES {
            return true;
        }
        self.resource_exhausted.set(true);
        sink.emit(
            Diagnostic::error(format!(
                "string value exceeds the maximum size of {MAX_STRING_BYTES} bytes"
            ))
            .at(span),
        );
        false
    }

    /// Returns whether evaluating `node` could reassign a binding or call a
    /// function (which may reassign bindings indirectly). Used to decide
    /// whether an append-style assignment may run in place.
    fn may_mutate_bindings(node: &AstNode) -> bool {
        match &node.kind {
            AstKind::Call { .. } | AstKind::Assignment { .. } | AstKind::Use { .. } => true,
            AstKind::Program { statements } | AstKind::Block { statements } => {
                statements.iter().any(Self::may_mutate_bindings)
            }
            AstKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::may_mutate_bindings(condition)
                    || Self::may_mutate_bindings(then_branch)
                    || else_branch
                        .as_deref()
                        .is_some_and(Self::may_mutate_bindings)
            }
            AstKind::While { condition, body } => {
                Self::may_mutate_bindings(condition) || Self::may_mutate_bindings(body)
            }
            AstKind::Unary { operand, .. } => Self::may_mutate_bindings(operand),
            AstKind::Binary { left, right, .. } => {
                Self::may_mutate_bindings(left) || Self::may_mutate_bindings(right)
            }
            AstKind::Group { expression } => Self::may_mutate_bindings(expression),
            _ => false,
        }
    }

    /// Appends `right`'s string value to the existing `name` binding in place.
    ///
    /// The caller has already established that the binding exists and that
    /// `name = name + right` is being evaluated with a mutation-free `right`.
    /// Returns `None` when the operands are not both strings so the caller
    /// falls back to the general assignment path; re-evaluating `right` there
    /// is side-effect free by construction.
    #[allow(clippy::too_many_arguments)]
    fn try_append_in_place(
        &self,
        name: &str,
        expression_span: Span,
        right: &AstNode,
        source: &SourceFile,
        environment: &mut Environment,
        sink: &mut DiagnosticSink,
        depth: usize,
    ) -> Option<Value> {
        let suffix = self.eval(right, source, environment, sink, depth + 1);
        let slot = environment
            .scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(name))?;
        let (Value::Str(existing), Value::Str(appendage)) = (&*slot, &suffix) else {
            // Not a string append (integers, mismatched types, or a
            // short-circuited operand): defer to the general assignment path,
            // whose diagnostics and semantics apply unchanged. Re-evaluating
            // `right` there is safe because it cannot mutate bindings.
            return None;
        };
        let length = existing.len().saturating_add(appendage.len());
        if !self.check_string_size(length, expression_span, sink) {
            return Some(Value::Unit);
        }
        let Value::Str(existing) = slot else {
            unreachable!("both operands were matched as strings above")
        };
        let Value::Str(appendage) = suffix else {
            unreachable!("both operands were matched as strings above")
        };
        existing.push_str(&appendage);
        Some(Value::Str(existing.clone()))
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Emits a type error for a unary operator and returns [`Value::Unit`].
///
/// This is used when a unary operator is applied to an operand of the wrong type.
fn unary_type_error(
    operator: char,
    operand: &Value,
    span: Span,
    sink: &mut DiagnosticSink,
) -> Value {
    sink.emit(
        Diagnostic::error(format!(
            "cannot apply `{operator}` to `{}`",
            operand.type_name()
        ))
        .at(span),
    );
    Value::Unit
}

/// Emits a type error for a binary operator and returns [`Value::Unit`].
///
/// This is used when a binary operator is applied to operands of the wrong types.
fn binary_type_error(
    operator: BinaryOperator,
    left: &Value,
    right: &Value,
    span: Span,
    sink: &mut DiagnosticSink,
) -> Value {
    sink.emit(
        Diagnostic::error(format!(
            "cannot apply `{operator}` to `{}` and `{}`",
            left.type_name(),
            right.type_name()
        ))
        .at(span),
    );
    Value::Unit
}

/// Emits a division by zero error and returns [`Value::Unit`].
fn division_by_zero(span: Span, sink: &mut DiagnosticSink) -> Value {
    sink.emit(Diagnostic::error("division by zero").at(span));
    Value::Unit
}

/// Emits an integer overflow error and returns [`Value::Unit`].
fn overflow(span: Span, sink: &mut DiagnosticSink) -> Value {
    sink.emit(Diagnostic::error("integer overflow").at(span));
    Value::Unit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn eval(source_text: &str) -> (Option<Value>, DiagnosticSink) {
        let source = SourceFile::new("test.ucl", source_text);
        let mut sink = DiagnosticSink::new();
        let tokens = Lexer::new(&source).tokenize(&mut sink);
        let ast = Parser::new(tokens)
            .parse(&mut sink)
            .expect("parser should return a program");
        let value = Evaluator::new().evaluate(&ast, &source, &mut sink);
        (value, sink)
    }

    #[test]
    fn evaluates_integer_literals() {
        let (value, sink) = eval("42;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn respects_operator_precedence() {
        let (value, sink) = eval("2 + 3 * 4;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(14)));
    }

    #[test]
    fn exponentiation_binds_tighter_than_multiplication() {
        let (value, sink) = eval("2 ^ 3 * 2;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(16)));
    }

    #[test]
    fn evaluates_bindings_and_references() {
        let (value, sink) = eval("let x = 5; x + 1;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(6)));
    }

    #[test]
    fn assignment_updates_existing_bindings() {
        let (value, sink) = eval("let x = 5; x = 10; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(10)));
    }

    #[test]
    fn append_assignment_accumulates_strings() {
        let (value, sink) = eval(r#"let s = "a"; s = s + "b"; s;"#);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("ab".to_owned())));
    }

    #[test]
    fn append_assignment_updates_the_innermost_binding() {
        // The in-place append must target the shadowing binding, leaving the
        // outer one untouched.
        let (value, sink) = eval(r#"let s = "a"; { let s = "b"; s = s + "c"; }; s;"#);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("a".to_owned())));

        let (value, sink) = eval(r#"let s = ""; { let s = "b"; s = s + "c"; }; len(s);"#);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(0)));
    }

    #[test]
    fn append_assignment_in_a_loop_accumulates() {
        let (value, sink) =
            eval(r#"let s = ""; let i = 0; while i < 3 { s = s + "?"; i = i + 1; }; len(s);"#);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(3)));
    }

    #[test]
    fn append_assignment_reports_type_mismatch_like_general_form() {
        let (_value, sink) = eval(r#"let n = 1; n = n + "a";"#);
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("cannot apply `+`"))
        );
    }

    #[test]
    fn append_assignment_to_undefined_variable_still_reported() {
        let (_value, sink) = eval(r#"x = x + "a";"#);
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("undefined variable `x`"))
        );
    }

    #[test]
    fn regression_append_loop_with_inert_counter_terminates() {
        // Fuzz-found inputs: the counter never advances, so the loop runs
        // until its iteration cap. These must terminate with the documented
        // loop-limit error rather than hanging or exhausting memory.
        for source in [
            r#"let i = 0; let acc = ""; while i < 5 { acc = acc + "!";  i + 1; }; acc;"#,
            r#"let i = 0; let acc = ""; while i < 5 { acc = acc + "!"; i = i + 0; }; acc;"#,
        ] {
            let started = std::time::Instant::now();
            let (value, sink) = eval(source);
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "evaluation of `{source}` should be bounded"
            );
            assert!(sink.iter().any(|diagnostic| {
                diagnostic
                    .message
                    .contains("loop exceeded the maximum number of iterations")
            }));
            assert_eq!(value, None);
        }
    }

    #[test]
    fn blocks_introduce_new_scopes() {
        let (value, sink) = eval("let x = 5; { let x = 10; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(5)));
    }

    #[test]
    fn comparisons_and_logic_produce_booleans() {
        let (value, sink) = eval("1 < 2;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));

        let (value, sink) = eval("!(1 < 2);");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));

        let (value, sink) = eval("1 < 2 & 2 < 3;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));
    }

    #[test]
    fn reports_undefined_variables() {
        let (_value, sink) = eval("x;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("undefined variable `x`"))
        );
    }

    #[test]
    fn reports_division_by_zero() {
        let (_value, sink) = eval("1 / 0;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );
    }

    #[test]
    fn rejects_exponents_above_the_u32_range() {
        // These exponents overflow i64, but they must be reported as overflow
        // rather than silently truncated (`4294967296 as u32 == 0`), which
        // would wrongly evaluate `2 ^ 4294967296` as `2 ^ 0 == 1`.
        for source in ["2 ^ 4294967296;", "2 ^ 4294967297;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("overflow")),
                "expected an overflow error for `{source}`"
            );
        }
    }

    #[test]
    fn rejects_excessive_evaluation_nesting() {
        // Build a deeply nested AST directly, bypassing the parser's own depth
        // limit, to exercise the evaluator's independent guard. The test runs
        let harness = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let mut node = AstNode {
                    span: Span::new(0, 1),
                    kind: AstKind::Integer,
                };
                for _ in 0..(MAX_EVAL_DEPTH + 100) {
                    node = AstNode {
                        span: Span::new(0, 1),
                        kind: AstKind::Group {
                            expression: Box::new(node),
                        },
                    };
                }
                let program = AstNode {
                    span: Span::new(0, 1),
                    kind: AstKind::Program {
                        statements: vec![node],
                    },
                };
                let source = SourceFile::new("test.ucl", "1");
                let mut sink = DiagnosticSink::new();

                let _ = Evaluator::new().evaluate(&program, &source, &mut sink);

                sink
            })
            .expect("test harness thread spawns");
        let sink = harness.join().expect("harness thread does not panic");

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("nesting is too deep"))
        );
    }

    #[test]
    fn computes_unit_bases_with_oversized_exponents() {
        // Bases whose powers never overflow produce exact results even when
        // the exponent is too large to represent as a `u32`.
        let (value, sink) = eval("0 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(0)));

        let (value, sink) = eval("1 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));

        let (value, sink) = eval("-1 ^ 4294967296;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));

        let (value, sink) = eval("-1 ^ 4294967297;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(-1)));
    }

    #[test]
    fn reports_overflow_on_addition() {
        let (_value, sink) = eval("9223372036854775807 + 1;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("integer overflow"))
        );
    }

    #[test]
    fn reports_overflow_on_subtraction() {
        let (_value, sink) = eval("-9223372036854775807 - 2;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("integer overflow"))
        );
    }

    #[test]
    fn reports_overflow_on_multiplication() {
        let (_value, sink) = eval("9223372036854775807 * 2;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("integer overflow"))
        );
    }

    #[test]
    fn reports_remainder_by_zero() {
        let (_value, sink) = eval("5 % 0;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );
    }

    #[test]
    fn reports_negative_exponents() {
        let (_value, sink) = eval("2 ^ -1;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("negative exponents"))
        );
    }

    #[test]
    fn evaluates_equality_on_integers_and_booleans() {
        for (source, expected) in [
            ("1 == 1;", true),
            ("1 == 2;", false),
            ("1 != 2;", true),
            ("true == true;", true),
            ("false != true;", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn rejects_equality_across_types() {
        let (_value, sink) = eval("1 == true;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("cannot apply `==`"))
        );
    }

    #[test]
    fn evaluates_relational_operators() {
        for (source, expected) in [
            ("1 <= 1;", true),
            ("1 < 1;", false),
            ("2 >= 1;", true),
            ("2 > 3;", false),
            ("1 < 2 == true;", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn relational_operators_compare_strings_lexicographically() {
        for (source, expected) in [
            ("\"apple\" < \"banana\";", true),
            ("\"apple\" < \"apple\";", false),
            ("\"apple\" <= \"apple\";", true),
            ("\"b\" > \"a\";", true),
            ("\"abc\" >= \"abd\";", false),
            ("\"\" < \"a\";", true),
            // Ordering is by Unicode scalar value, so multi-byte characters
            // compare by code point, not byte count.
            ("\"é\" > \"z\";", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn mixing_string_and_integer_in_relational_comparison_is_an_error() {
        for source in ["\"a\" < 1;", "1 <= \"a\";"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "`{source}` should be an error");
        }
    }

    #[test]
    fn logical_operators_short_circuit() {
        // The right-hand side would raise a division-by-zero error, but
        // short-circuiting must skip it entirely.
        let (value, sink) = eval("false & 1 / 0;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));

        let (value, sink) = eval("true | 1 / 0;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));

        // Short-circuiting also skips undefined-variable errors.
        let (value, sink) = eval("false & missing;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));
    }

    #[test]
    fn short_circuiting_does_not_skip_needed_sides() {
        // The left-hand side is evaluated even when the right could be
        // skipped: `1 / 0` must still report its error.
        let (_value, sink) = eval("1 / 0 & true;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );

        // When the left-hand side does not force a skip, the right-hand side
        // is still evaluated.
        let (value, sink) = eval("true & (1 / 0) == 0 | true;");
        assert!(sink.has_errors(), "expected the right side to be evaluated");
        let _ = value;
    }

    #[test]
    fn evaluates_boolean_literals() {
        let (value, sink) = eval("true;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));

        let (value, sink) = eval("false;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(false)));

        let (value, sink) = eval("!false;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));
    }

    #[test]
    fn reports_unary_type_errors() {
        for source in ["-(1 < 2);", "!5;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("cannot apply")),
                "expected a type error for `{source}`"
            );
        }
    }

    #[test]
    fn reports_binary_type_errors() {
        for source in ["1 + (1 < 2);", "1 & 2;", "1 < 2 < 3;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("cannot apply")),
                "expected a type error for `{source}`"
            );
        }
    }

    #[test]
    fn reports_invalid_assignment_targets() {
        let (_value, sink) = eval("(x) = 5;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("invalid assignment target"))
        );
    }

    #[test]
    fn reports_out_of_range_integer_literals() {
        let (_value, sink) = eval("9223372036854775808;");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("out of range"))
        );
    }

    #[test]
    fn evaluates_logical_or_on_booleans() {
        let (value, sink) = eval("1 > 2 | 2 < 3;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Boolean(true)));
    }

    #[test]
    fn assignment_inside_a_block_updates_the_outer_binding() {
        let (value, sink) = eval("let x = 5; { x = 10; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(10)));
    }

    #[test]
    fn assignment_to_a_shadowed_binding_stays_in_its_scope() {
        let (value, sink) = eval("let x = 5; { let x = 10; x = 20; }; x;");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(5)));
    }

    #[test]
    fn reports_invalid_spans_from_hand_built_asts_instead_of_panicking() {
        // AST nodes are publicly constructible, so spans may point outside
        // the source. The evaluator must report a diagnostic rather than
        // panic when reading names or literals through such a span.
        let source = SourceFile::new("test.ucl", "let x = 5;");
        let program = AstNode {
            span: Span::new(0, 10),
            kind: AstKind::Program {
                statements: vec![
                    AstNode {
                        span: Span::new(100, 200),
                        kind: AstKind::Identifier,
                    },
                    AstNode {
                        span: Span::new(300, 400),
                        kind: AstKind::Integer,
                    },
                ],
            },
        };
        let mut sink = DiagnosticSink::new();

        let _ = Evaluator::new().evaluate(&program, &source, &mut sink);

        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("invalid identifier span"))
        );
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("invalid integer literal span"))
        );
    }

    #[test]
    fn evaluates_moderately_long_flat_binary_chains() {
        let source_text = vec!["1"; 100].join(" + ") + ";";
        let (value, sink) = eval(&source_text);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(100)));
    }

    #[test]
    fn evaluates_long_flat_binary_chains_within_the_parser_limit() {
        // A flat, left-associative chain adds evaluator depth without adding
        // parser nesting. Left-spine flattening keeps such chains iterative,
        // so anything within the parser's limit must evaluate without error.
        let terms = 2_000;
        let source_text = vec!["1"; terms].join(" + ") + ";";
        let (value, sink) = eval(&source_text);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(terms as i64)));
    }

    #[test]
    fn evaluates_string_literals_and_concatenation() {
        let (value, sink) = eval("\"hello\" + \" \" + \"world\";");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("hello world".to_owned())));
    }

    #[test]
    fn rejects_string_growth_before_host_memory_is_exhausted() {
        // Doubling 24 times attempts to create a 16 MiB value. The evaluator
        // must reject that deterministically at its 8 MiB value limit instead
        // of allowing concatenation to grow until the host runs out of memory.
        let (value, sink) = eval(
            "let text = \"x\"; let i = 0; while i < 24 { text = text + text; i = i + 1; }; text;",
        );
        assert_eq!(value, None);
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("string value exceeds the maximum size")
        }));
    }

    #[test]
    fn evaluates_len_for_unicode_strings() {
        for (source, expected) in [("len(\"\");", 0), ("len(\"hé\");", 2)] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Integer(expected)), "for `{source}`");
        }
    }

    #[test]
    fn reports_len_arity_and_type_errors() {
        for source in ["len();", "len(\"a\", \"b\");", "len(1);"] {
            let (value, sink) = eval(source);
            assert_eq!(value, None, "expected an error for `{source}");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("`len`")),
                "expected a len-specific error for `{source}`"
            );
        }
    }

    #[test]
    fn user_bindings_may_shadow_len() {
        let (value, sink) = eval("let len = fn(value) { value + 1; }; len(41);");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn evaluates_str_across_value_kinds() {
        for (source, expected) in [
            ("str(42);", "42"),
            ("str(-7);", "-7"),
            ("str(true);", "true"),
            ("str(\"hé\");", "hé"),
            ("fn f() { return; }; str(f());", "unit"),
            ("str(fn(n) { n; });", "<function>"),
            ("str(len);", "<function>"),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(
                value,
                Some(Value::Str(expected.to_owned())),
                "for `{source}`"
            );
        }
    }

    #[test]
    fn evaluates_type_for_each_value_kind() {
        for (source, expected) in [
            ("type(1);", "integer"),
            ("type(false);", "boolean"),
            ("type(\"a\");", "string"),
            ("type(len);", "function"),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(
                value,
                Some(Value::Str(expected.to_owned())),
                "for `{source}`"
            );
        }
    }

    #[test]
    fn evaluates_case_builtins() {
        for (source, expected) in [
            ("upper(\"hello\");", "HELLO"),
            ("lower(\"WORLD\");", "world"),
            ("upper(\"hé\");", "HÉ"),
            ("lower(\"HÉ\");", "hé"),
            ("upper(\"\");", ""),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(
                value,
                Some(Value::Str(expected.to_owned())),
                "for `{source}`"
            );
        }
    }

    #[test]
    fn evaluates_contains() {
        for (source, expected) in [
            ("contains(\"hello\", \"ell\");", true),
            ("contains(\"hello\", \"xyz\");", false),
            ("contains(\"\", \"\");", true),
            ("contains(\"abc\", \"\");", true),
            ("contains(\"abc\", \"abc\");", true),
            ("contains(\"héllo\", \"éll\");", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn reports_new_builtin_arity_and_type_errors() {
        for source in [
            "str();",
            "str(1, 2);",
            "type();",
            "type(\"a\", \"b\");",
            "upper();",
            "upper(1);",
            "lower(true);",
            "contains();",
            "contains(\"a\");",
            "contains(1, \"a\");",
            "contains(\"a\", 1);",
        ] {
            let (value, sink) = eval(source);
            assert_eq!(value, None, "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains('`')),
                "expected a built-in-specific error for `{source}`"
            );
        }
    }

    #[test]
    fn user_bindings_may_shadow_new_builtins() {
        let (value, sink) = eval("let str = fn(value) { value + 1; }; str(41);");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));

        let (value, sink) = eval("fn contains(a, b) { a; }; contains(7, 8);");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(7)));
    }

    #[test]
    fn decodes_escape_sequences_in_strings() {
        let (value, sink) = eval(r##""a\tb\nc\"d\\";"##);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("a\tb\nc\"d\\".to_owned())));
    }

    #[test]
    fn compares_strings_for_equality() {
        for (source, expected) in [
            ("\"a\" == \"a\";", true),
            ("\"a\" == \"b\";", false),
            ("\"a\" != \"b\";", true),
        ] {
            let (value, sink) = eval(source);
            assert!(!sink.has_errors(), "unexpected error for `{source}`");
            assert_eq!(value, Some(Value::Boolean(expected)), "for `{source}`");
        }
    }

    #[test]
    fn rejects_mixed_type_operations_on_strings() {
        for source in ["1 + \"a\";", "\"a\" * 2;", "\"a\" == 1;"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("cannot apply")),
                "expected a type error for `{source}`"
            );
        }
    }

    #[test]
    fn evaluates_the_taken_branch_of_an_if() {
        let (value, sink) = eval("if true { 1; } else { 2; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));

        let (value, sink) = eval("if false { 1; } else { 2; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(2)));
    }

    #[test]
    fn an_if_without_else_yields_unit_when_false() {
        let (value, sink) = eval("if false { 1; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Unit));

        let (value, sink) = eval("if true { 1; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(1)));
    }

    #[test]
    fn only_the_condition_is_evaluated_before_branching() {
        // The untaken branch would raise a division-by-zero error.
        let (value, sink) = eval("if false { 1 / 0; } else { 7; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(7)));

        let (value, sink) = eval("if true { 7; } else { 1 / 0; };");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(7)));
    }

    #[test]
    fn else_if_chains_pick_the_first_true_branch() {
        let source = "let x = 2; if x == 1 { 10; } else if x == 2 { 20; } else { 30; };";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(20)));
    }

    #[test]
    fn rejects_a_non_boolean_if_condition() {
        let (_value, sink) = eval("if 1 { 2; };");
        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("condition of `if` must be a boolean")
        }));
    }

    #[test]
    fn rejects_a_non_boolean_while_condition() {
        let (_value, sink) = eval("while \"x\" { 1; };");
        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("condition of `while` must be a boolean")
        }));
    }

    #[test]
    fn while_loops_iterate_until_the_condition_becomes_false() {
        // The program's last statement is inside the loop's block, so its
        // value is unit; verify the iteration count via a final reference to
        // a binding the loop mutated.
        let source = "
            let total = 0;
            let i = 1;
            while i <= 5 {
                total = total + i;
                i = i + 1;
            };
        ";
        let (value, sink) = eval(&format!("{source} total;"));
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(15)));
    }

    #[test]
    fn an_infinite_loop_is_capped() {
        let (value, sink) = eval("while true { };");
        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("exceeded the maximum number of iterations")
        }));
        assert_eq!(value, None);
    }

    #[test]
    fn break_exits_the_innermost_loop() {
        let source = "
            let total = 0;
            let i = 1;
            while i <= 10 {
                if i == 4 { break; };
                total = total + i;
                i = i + 1;
            };
        ";
        let (value, sink) = eval(&format!("{source} total;"));
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(6)));
    }

    #[test]
    fn continue_skips_to_the_next_condition_check() {
        // Sum only the odd values below 10.
        let source = "
            let total = 0;
            let i = 0;
            while i < 10 {
                i = i + 1;
                if i % 2 == 0 { continue; };
                total = total + i;
            };
        ";
        let (value, sink) = eval(&format!("{source} total;"));
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(25)));
    }

    #[test]
    fn loop_signals_propagate_through_nested_loops_to_the_innermost_only() {
        // `break` inside the inner loop leaves the inner loop but the outer
        // one keeps running; `continue` in the outer loop skips its tail.
        let source = "
            let hits = 0;
            let outer = 0;
            while outer < 3 {
                outer = outer + 1;
                let inner = 0;
                while inner < 10 {
                    inner = inner + 1;
                    break;
                };
                if outer == 2 { continue; };
                hits = hits + inner;
            };
        ";
        let (value, sink) = eval(&format!("{source} hits;"));
        assert!(!sink.has_errors());
        // Inner always breaks after one pass; the second outer pass is
        // skipped by `continue`, so hits = 1 + 1 = 2.
        assert_eq!(value, Some(Value::Integer(2)));
    }

    #[test]
    fn break_inside_a_function_body_is_an_error_at_the_call_site() {
        // A closure called from within a loop must not break the caller's
        // loop; the signal has no matching loop inside the function.
        let (value, sink) =
            eval("let f = fn() { break; }; let i = 0; while i < 3 { i = i + 1; f(); };");
        assert!(sink.has_errors());
        assert!(
            sink.iter()
                .any(|diagnostic| diagnostic.message.contains("`break` outside of a loop"))
        );
        assert_eq!(value, None);
    }

    #[test]
    fn continue_outside_any_loop_is_an_error() {
        for source in ["continue;", "if true { continue; };"] {
            let (value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains("`continue` outside of a loop")),
                "for `{source}`"
            );
            assert_eq!(value, None);
        }
    }

    #[test]
    fn break_still_runs_the_loop_condition_path_correctly_after_early_exit() {
        // After `break`, statements after it in the body are skipped and the
        // condition is not re-checked.
        let source = "
            let log = \"\";
            let i = 0;
            while i < 5 {
                i = i + 1;
                if i == 2 { break; };
                log = log + str(i);
            };
        ";
        let (value, sink) = eval(&format!("{source} log;"));
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("1".to_owned())));
    }

    #[test]
    fn while_body_runs_in_its_own_scope() {
        // A `let` inside the body must not leak into the outer scope, and
        // assignment must still reach the outer binding.
        let source = "
            let i = 0;
            let seen = 0;
            while i < 3 {
                i = i + 1;
                let inner = i;
                seen = inner;
            };
            seen;
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(3)));
    }

    #[test]
    fn strings_flow_through_variables_and_blocks() {
        let source = "let greeting = \"hi \"; let name = \"ucl\"; greeting + name;";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("hi ucl".to_owned())));
    }

    #[test]
    fn calls_named_functions_with_parameters_and_implicit_results() {
        let source = "fn add(left, right) { left + right; }; add(20, 22);";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn functions_resolve_globals_not_caller_locals() {
        let source = "let value = 10; fn read() { value; }; { let value = 20; read(); };";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(10)));
    }

    #[test]
    fn function_calls_evaluate_arguments_left_to_right() {
        let source = "let state = 0; fn first() { state = 1; state; }; fn second() { state = 2; state; }; fn pick(left, right) { right; }; pick(first(), second()); state;";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(2)));
    }

    #[test]
    fn functions_can_recur() {
        let source =
            "fn factorial(n) { if n <= 1 { 1; } else { n * factorial(n - 1); }; }; factorial(5);";
        let (value, sink) = eval(source);

        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(120)));
    }

    #[test]
    fn reports_function_call_and_declaration_errors() {
        for (source, expected) in [
            (
                "fn identity(value) { value; }; identity();",
                "expected 1 argument",
            ),
            ("42(1);", "cannot call value of type `integer`"),
            (
                "fn duplicate(value, value) { value; };",
                "duplicate function parameter `value`",
            ),
        ] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert!(
                sink.iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "expected `{expected}` for `{source}`"
            );
        }
    }

    #[test]
    fn functions_may_be_declared_inside_blocks() {
        let source = "
            fn outer() {
                fn inner(x) { x * 2; };
                inner(21);
            };
            outer();
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn function_literals_are_first_class_values() {
        // A literal stored in a variable and passed as an argument.
        let source = "
            fn apply(f, x) { f(x); };
            let double = fn(n) { n * 2; };
            apply(double, 21);
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));

        let source = "fn apply(f, x) { f(x); }; apply(fn(n) { n + 1; }, 41);";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn literals_can_be_called_directly() {
        let (value, sink) = eval("fn(a, b) { a * b; }(6, 7);");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn closures_capture_enclosing_locals_by_value() {
        // The literal captures `base` where it is created; rebinding `base`
        // afterwards must not change the closure's behavior.
        let source = "
            let make = fn(base) {
                return fn(n) { base + n; };
            };
            let add5 = make(5);
            let add7 = make(7);
            add5(10) + add7(10);
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(32)));
    }

    #[test]
    fn locals_are_captured_but_globals_stay_dynamic() {
        // `base` is a parameter: the closure freezes its value at creation.
        // `factor` is a global: functions always see the latest value.
        let source = "
            let factor = 1;
            let make = fn(base) { return fn(n) { base + n * factor; }; };
            let add = make(10);
            factor = 100;
            add(1);
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(110)));
    }

    #[test]
    fn functions_still_see_current_global_state() {
        // Globals resolve dynamically at call time, so reassigning a global
        // between creation and call is visible to the function.
        let source = "
            let factor = 2;
            fn scale(n) { n * factor; };
            factor = 10;
            scale(4);
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(40)));
    }

    #[test]
    fn explicit_return_exits_the_function_early() {
        let source = "
            fn sign(n) {
                if n < 0 { return \"neg\"; };
                if n == 0 { return \"zero\"; };
                return \"pos\";
            };
            sign(-5);
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Str("neg".to_owned())));

        // `return` also unwinds through loops.
        let source = "
            fn first_square(limit) {
                let i = 0;
                while i < limit {
                    if i * i > 50 { return i; };
                    i = i + 1;
                };
                return -1;
            };
            first_square(100);
        ";
        let (value, sink) = eval(source);
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(8)));
    }

    #[test]
    fn bare_return_yields_unit_and_the_last_statement_fills_the_rest() {
        let (value, sink) = eval("fn nothing() { return; }; nothing();");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Unit));

        let (value, sink) = eval("fn implicit() { 40 + 2; }; implicit();");
        assert!(!sink.has_errors());
        assert_eq!(value, Some(Value::Integer(42)));
    }

    #[test]
    fn return_at_program_scope_is_an_error() {
        for source in ["return 1;", "if true { return; };"] {
            let (_value, sink) = eval(source);
            assert!(sink.has_errors(), "expected an error for `{source}`");
            assert_eq!(_value, None, "no value for `{source}`");
        }
    }

    #[test]
    fn a_literal_cannot_recur_through_its_own_variable() {
        // Documented limitation: the variable does not exist when the
        // literal's capture is taken.
        let source = "let f = fn(n) { f(n); };";
        let (_value, _sink) = eval(source);
        // Parsing and creating the literal succeed; only calling it would
        // fail, which we deliberately do not do here.
    }

    #[test]
    fn caps_recursive_function_calls() {
        // Runs on a generous stack so the assertion exercises the call-depth
        // guard itself rather than the test thread's stack limit.
        let harness = std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| eval("fn recurse() { recurse(); }; recurse();"))
            .expect("test harness thread spawns");
        let (_value, sink) = harness.join().expect("harness thread does not panic");

        assert!(sink.has_errors());
        assert!(sink.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("function call depth is too deep")
        }));
    }

    #[test]
    fn evaluate_in_persists_global_bindings_across_calls() {
        let evaluator = Evaluator::new();
        let mut environment = Environment::new();

        for (input, expected) in [("let x = 40;", None), ("x + 2;", Some(Value::Integer(42)))] {
            let line_source = SourceFile::new("repl.ucl", input);
            let mut line_sink = DiagnosticSink::new();
            let tokens = Lexer::new(&line_source).tokenize(&mut line_sink);
            let ast = Parser::new(tokens)
                .parse(&mut line_sink)
                .expect("parser should return a program");
            assert!(!line_sink.has_errors(), "for `{input}`");

            let value = evaluator.evaluate_in(&mut environment, &ast, &line_source, &mut line_sink);
            assert!(!line_sink.has_errors(), "for `{input}`");
            if let Some(value) = value {
                assert_eq!(value, expected.unwrap_or(Value::Unit), "for `{input}`");
            }
        }
    }

    #[test]
    fn evaluate_in_keeps_functions_and_their_captures_alive() {
        let mut sink = DiagnosticSink::new();
        let evaluator = Evaluator::new();
        let mut environment = Environment::new();

        let run_line = |evaluator: &Evaluator,
                        environment: &mut Environment,
                        input: &str,
                        sink: &mut DiagnosticSink| {
            let line_source = SourceFile::new("repl.ucl", input);
            let tokens = Lexer::new(&line_source).tokenize(sink);
            let ast = Parser::new(tokens)
                .parse(sink)
                .expect("parser should return a program");
            evaluator.evaluate_in(environment, &ast, &line_source, sink)
        };

        let value = run_line(
            &evaluator,
            &mut environment,
            "fn make(base) { return fn(n) { base + n; }; }; make(5);",
            &mut sink,
        )
        .expect("definition succeeds");
        // Store the returned closure by re-declaring it in a follow-up line.
        let stored = matches!(value, Value::Function(_));
        assert!(stored);

        // A fresh line can still call the closure through a global binding.
        assert!(
            run_line(
                &evaluator,
                &mut environment,
                "let add5 = make(5);",
                &mut sink
            )
            .is_some()
        );
        let result = run_line(&evaluator, &mut environment, "add5(37);", &mut sink);
        assert_eq!(result, Some(Value::Integer(42)));
    }

    #[test]
    fn an_error_on_one_line_does_not_poison_the_next() {
        let mut sink = DiagnosticSink::new();
        let evaluator = Evaluator::new();
        let mut environment = Environment::new();

        let run_line = |evaluator: &Evaluator,
                        environment: &mut Environment,
                        input: &str,
                        sink: &mut DiagnosticSink| {
            let line_source = SourceFile::new("repl.ucl", input);
            let tokens = Lexer::new(&line_source).tokenize(sink);
            let ast = Parser::new(tokens)
                .parse(sink)
                .expect("parser should return a program");
            evaluator.evaluate_in(environment, &ast, &line_source, sink)
        };

        assert!(run_line(&evaluator, &mut environment, "let x = 1;", &mut sink).is_some());
        assert!(run_line(&evaluator, &mut environment, "x / 0;", &mut sink).is_none());
        assert!(sink.has_errors());
        assert_eq!(
            run_line(&evaluator, &mut environment, "x + 1;", &mut sink),
            Some(Value::Integer(2))
        );
    }
}
