//! Evaluator: executes an abstract syntax tree.
//!
//! The evaluator walks the AST produced by the [`crate::parser::Parser`]
//! and computes a [`Value`]. The [`SourceFile`] is needed alongside the AST
//! because nodes carry [`Span`]s rather than their text, so identifier names
//! and integer literals are read back out of the source.
//!
//! Operator and statement semantics are normative in the language
//! specification (`docs/spec.md`); this module implements them.
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
use std::collections::HashSet;
use std::rc::Rc;

use crate::diagnostic::{Diagnostic, DiagnosticSink, Severity};
use crate::lexer::unescape_string;
use crate::parser::{AstKind, AstNode, BinaryOperator};
use crate::source::{SourceFile, Span};

mod builtins;
mod environment;
#[cfg(test)]
mod tests;
mod value;

pub use builtins::BuiltinFunction;
pub use environment::Environment;
pub use value::{FunctionValue, ModuleValue, Value};
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

/// The cumulative byte budget for value data built during one evaluation.
///
/// Per-loop iteration caps bound how many times code runs, not how much
/// data each run moves: a capped loop whose body concatenates a growing
/// string performs quadratic work and can take tens of seconds even though
/// every individual operation is legal and bounded. This budget charges the
/// data cost of building values — UTF-8 bytes for strings, element counts
/// for lists — across the whole evaluation, so total work stays
/// deterministic and small regardless of program shape.
///
/// Charging follows each operation's real data movement: string operations
/// charge the UTF-8 bytes they copy, so accumulate-in-a-loop concatenation
/// — quadratic in practice — trips the budget within a fraction of a
/// second. List growth charges only newly added elements, so ordinary list
/// building stays unaffected at any practical size.
const MAX_TOTAL_VALUE_BYTES: usize = 256 * 1024 * 1024;

/// The maximum number of active UCL function calls.
///
/// This prevents recursive programs from exhausting the host call stack.
/// Each call costs a bounded number of evaluator recursion levels, so this
/// stays safely below [`MAX_EVAL_DEPTH`].
const MAX_CALL_DEPTH: usize = 128;

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
    /// Cumulative bytes of value data built so far in this evaluation,
    /// charged against [`MAX_TOTAL_VALUE_BYTES`].
    allocated_bytes: Cell<usize>,
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
            allocated_bytes: Cell::new(0),
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
        self.allocated_bytes.set(0);
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
        // Exhaustive over `Flow`: a new control-flow signal must decide
        // here whether it exits the loop or propagates outward.
        match &*flow {
            Some(Flow::Break) => {
                *flow = None;
                true
            }
            Some(Flow::Continue) => {
                *flow = None;
                false
            }
            Some(Flow::Return(_)) | None => false,
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
            AstKind::List { elements } => {
                let mut values = Vec::with_capacity(elements.len());
                for element in elements {
                    values.push(self.eval(element, source, environment, sink, depth + 1));
                    if self.has_pending_flow() || self.resource_exhausted.get() || sink.is_full() {
                        return Value::Unit;
                    }
                }
                Value::List(Rc::new(values))
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
            AstKind::Index { object, index } => {
                let object_span = object.span;
                let object = self.eval(object, source, environment, sink, depth + 1);
                let index_value = self.eval(index, source, environment, sink, depth + 1);
                let Value::Integer(i) = index_value else {
                    sink.emit(
                        Diagnostic::error(format!(
                            "`index` must be an integer, found `{}`",
                            index_value.type_name()
                        ))
                        .at(index.span),
                    );
                    return Value::Unit;
                };
                // Bounds are strict, matching `slice`: negative and
                // out-of-range indices are runtime errors rather than
                // silent lookups.
                match object {
                    Value::List(elements) => {
                        if i < 0 || i as usize >= elements.len() {
                            sink.emit(Diagnostic::error("`index` is out of range").at(node.span));
                            return Value::Unit;
                        }
                        elements[i as usize].clone()
                    }
                    Value::Str(text) => {
                        let characters = text.chars().count();
                        if i < 0 || i as usize >= characters {
                            sink.emit(Diagnostic::error("`index` is out of range").at(node.span));
                            return Value::Unit;
                        }
                        Value::Str(
                            text.chars()
                                .nth(i as usize)
                                .expect("the bounds check above passed")
                                .to_string(),
                        )
                    }
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "cannot index a value of type `{}`",
                                other.type_name()
                            ))
                            .at(object_span),
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
            AstKind::For {
                variable,
                start,
                end,
                body,
            } => {
                let Some(name) = source.slice(*variable) else {
                    sink.emit(
                        Diagnostic::error("`for` has an invalid variable span").at(node.span),
                    );
                    return Value::Unit;
                };
                // The iteration sequence is fixed before the first pass:
                // range bounds are read once, and a string yields its
                // scalar values as one-character strings.
                let items = if let Some(start_expr) = start {
                    let from_value = self.eval(start_expr, source, environment, sink, depth + 1);
                    let to_value = self.eval(end, source, environment, sink, depth + 1);
                    match (from_value, to_value) {
                        (Value::Integer(from), Value::Integer(to)) => {
                            // Half-open like `slice`; inverted and empty
                            // ranges simply iterate zero times. Every
                            // yielded value is below `to`, so no addition
                            // can overflow.
                            let count = ((to as i128 - from as i128).max(0)) as u64;
                            // One extra item lets the executor distinguish a
                            // completed loop from one cut off by the cap.
                            let capped = count.min(MAX_LOOP_ITERATIONS + 1);
                            Some(
                                (0..capped)
                                    .map(|step| Value::Integer(from + step as i64))
                                    .collect::<Vec<Value>>(),
                            )
                        }
                        (from_value, to_value) => {
                            sink.emit(
                                Diagnostic::error(format!(
                                    "`for` range bounds must be integers, found `{}` and `{}`",
                                    from_value.type_name(),
                                    to_value.type_name()
                                ))
                                .at(node.span),
                            );
                            None
                        }
                    }
                } else {
                    let iterable = self.eval(end, source, environment, sink, depth + 1);
                    match iterable {
                        Value::Str(text) => Some(
                            text.chars()
                                .map(|character| Value::Str(character.to_string()))
                                .take(MAX_LOOP_ITERATIONS as usize + 1)
                                .collect(),
                        ),
                        Value::List(elements) => {
                            // One extra element lets the executor
                            // distinguish a completed loop from one cut
                            // off by the cap; the list is shared, so copy
                            // rather than truncate.
                            Some(
                                elements
                                    .iter()
                                    .take(MAX_LOOP_ITERATIONS as usize + 1)
                                    .cloned()
                                    .collect(),
                            )
                        }
                        other => {
                            sink.emit(
                                Diagnostic::error(format!(
                                    "`for` cannot iterate over `{}`, expected a string or range",
                                    other.type_name()
                                ))
                                .at(end.span),
                            );
                            None
                        }
                    }
                };
                let Some(items) = items else {
                    return Value::Unit;
                };
                let mut iterations = 0u64;
                for item in items {
                    iterations += 1;
                    if iterations > MAX_LOOP_ITERATIONS {
                        sink.emit(
                            Diagnostic::error("loop exceeded the maximum number of iterations")
                                .at(node.span),
                        );
                        break;
                    }
                    environment.push_scope();
                    environment.define(name, item);
                    self.eval(body, source, environment, sink, depth + 1);
                    // A `break` or `continue` from the body is consumed by
                    // this loop; a pending `return` (or exhausted resources)
                    // stops iteration without being consumed so it
                    // propagates outward.
                    let exit_for_break = self.take_loop_signal();
                    environment.pop_scope();
                    if self.has_pending_flow()
                        || self.resource_exhausted.get()
                        || sink.is_full()
                        || exit_for_break
                    {
                        break;
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
                // Fast paths: `name = name + <pure expression>` appends to the
                // existing string in place (or extends the existing list when
                // the right operand is a list literal), and `name =
                // append(name, <pure expression>)` pushes onto the existing
                // list in place, instead of rebuilding the value from a fresh
                // copy on every iteration. This keeps accumulation loops such
                // as `acc = acc + "!"`, `items = append(items, x)`, or
                // `items = items + [x]` linear in total work.
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
                        if let AstKind::List { elements } = &right.kind
                            && let Some(value) = self.try_list_concat_in_place(
                                &name,
                                value.span,
                                elements,
                                source,
                                environment,
                                sink,
                                depth,
                            )
                        {
                            return value;
                        }
                    }
                }
                // List fast path: `name = append(name, <pure expression>)`.
                if let AstKind::Call { callee, arguments } = &value.kind
                    && arguments.len() == 2
                    && matches!(callee.kind, AstKind::Identifier)
                    && source.slice(callee.span) == Some(BuiltinFunction::Append.name())
                    && matches!(arguments[0].kind, AstKind::Identifier)
                    && source.slice(arguments[0].span) == Some(name.as_str())
                    && !Self::may_mutate_bindings(&arguments[1])
                    && environment.lookup(&name).is_some()
                {
                    if let Some(value) = self.try_list_append_in_place(
                        &name,
                        value.span,
                        &arguments[1],
                        source,
                        environment,
                        sink,
                        depth,
                    ) {
                        return value;
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
                if !Self::check_arity(BuiltinFunction::Len.name(), values, 1, span, sink) {
                    return Value::Unit;
                }
                match &values[0] {
                    Value::Str(value) => Value::Integer(value.chars().count() as i64),
                    Value::List(elements) => Value::Integer(elements.len() as i64),
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "`len` expects a string or list argument, found `{}`",
                                other.type_name()
                            ))
                            .at(span),
                        );
                        Value::Unit
                    }
                }
            }
            BuiltinFunction::Str => {
                if !Self::check_arity(BuiltinFunction::Str.name(), values, 1, span, sink) {
                    return Value::Unit;
                }
                let text = values[0].display_text();
                if !self.check_string_size(text.len(), span, sink) {
                    return Value::Unit;
                }
                Value::Str(text)
            }
            BuiltinFunction::Type => {
                if !Self::check_arity(BuiltinFunction::Type.name(), values, 1, span, sink) {
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
                        if !self.check_string_size(mapped.len(), span, sink)
                            || !self.charge_allocation(mapped.len(), span, sink)
                        {
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
                if !Self::check_arity(BuiltinFunction::Contains.name(), values, 2, span, sink) {
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
                if let Value::List(elements) = haystack {
                    // List membership uses the same equality as `==`:
                    // elements match by value, recursing through nested
                    // lists.
                    return Value::Boolean(elements.iter().any(|element| element == needle));
                }
                sink.emit(
                    Diagnostic::error(format!(
                        "`contains` expects a string or list haystack, found `{}`",
                        haystack.type_name()
                    ))
                    .at(span),
                );
                Value::Unit
            }
            BuiltinFunction::Int => {
                if !Self::check_arity(BuiltinFunction::Int.name(), values, 1, span, sink) {
                    return Value::Unit;
                }
                match &values[0] {
                    Value::Integer(value) => Value::Integer(*value),
                    Value::Str(text) => match text.parse::<i64>() {
                        Ok(value) => Value::Integer(value),
                        Err(parse_error) => {
                            // Range failures get the same message as the
                            // arithmetic operators; everything else is a
                            // malformed integer.
                            if *parse_error.kind() == std::num::IntErrorKind::PosOverflow
                                || *parse_error.kind() == std::num::IntErrorKind::NegOverflow
                            {
                                overflow(span, sink)
                            } else {
                                sink.emit(
                                    Diagnostic::error(
                                        "`int` cannot parse the string as an integer",
                                    )
                                    .at(span),
                                );
                                Value::Unit
                            }
                        }
                    },
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "`int` expects a string argument, found `{}`",
                                other.type_name()
                            ))
                            .at(span),
                        );
                        Value::Unit
                    }
                }
            }
            BuiltinFunction::Find => {
                if !Self::check_arity(BuiltinFunction::Find.name(), values, 2, span, sink) {
                    return Value::Unit;
                }
                let (haystack, needle) = (&values[0], &values[1]);
                if let Value::Str(haystack) = haystack {
                    if let Value::Str(needle) = needle {
                        // `str::find` reports byte offsets; convert to the
                        // scalar-value indices every other built-in uses.
                        return Value::Integer(match haystack.find(needle.as_str()) {
                            Some(byte) => haystack[..byte].chars().count() as i64,
                            None => -1,
                        });
                    }
                    sink.emit(
                        Diagnostic::error(format!(
                            "`find` expects a string needle, found `{}`",
                            needle.type_name()
                        ))
                        .at(span),
                    );
                    return Value::Unit;
                }
                if let Value::List(elements) = haystack {
                    // List search uses the same equality as `==`, so
                    // nested lists match element by element.
                    return Value::Integer(match elements.iter().position(|e| e == needle) {
                        Some(index) => index as i64,
                        None => -1,
                    });
                }
                sink.emit(
                    Diagnostic::error(format!(
                        "`find` expects a string or list haystack, found `{}`",
                        haystack.type_name()
                    ))
                    .at(span),
                );
                Value::Unit
            }
            BuiltinFunction::Replace => {
                if !Self::check_arity(BuiltinFunction::Replace.name(), values, 3, span, sink) {
                    return Value::Unit;
                }
                let (source, pattern, replacement) = (&values[0], &values[1], &values[2]);
                if let Value::Str(source) = source {
                    if let Value::Str(pattern) = pattern {
                        if let Value::Str(replacement) = replacement {
                            if pattern.is_empty() {
                                sink.emit(
                                    Diagnostic::error("`replace` cannot replace an empty pattern")
                                        .at(span),
                                );
                                return Value::Unit;
                            }
                            let replaced = source.replace(pattern.as_str(), replacement);
                            if !self.charge_allocation(replaced.len(), span, sink) {
                                return Value::Unit;
                            }
                            return Value::Str(replaced);
                        }
                        sink.emit(
                            Diagnostic::error(format!(
                                "`replace` expects a string replacement, found `{}`",
                                replacement.type_name()
                            ))
                            .at(span),
                        );
                        return Value::Unit;
                    }
                    sink.emit(
                        Diagnostic::error(format!(
                            "`replace` expects a string pattern, found `{}`",
                            pattern.type_name()
                        ))
                        .at(span),
                    );
                    return Value::Unit;
                }
                sink.emit(
                    Diagnostic::error(format!(
                        "`replace` expects a string source, found `{}`",
                        source.type_name()
                    ))
                    .at(span),
                );
                Value::Unit
            }
            BuiltinFunction::Trim => {
                if !Self::check_arity(BuiltinFunction::Trim.name(), values, 1, span, sink) {
                    return Value::Unit;
                }
                match &values[0] {
                    Value::Str(value) => {
                        let trimmed = value.trim().to_owned();
                        if !self.charge_allocation(trimmed.len(), span, sink) {
                            return Value::Unit;
                        }
                        Value::Str(trimmed)
                    }
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "`trim` expects a string argument, found `{}`",
                                other.type_name()
                            ))
                            .at(span),
                        );
                        Value::Unit
                    }
                }
            }
            BuiltinFunction::Slice => {
                if !Self::check_arity(BuiltinFunction::Slice.name(), values, 3, span, sink) {
                    return Value::Unit;
                }
                let (source, start_value, end_value) = (&values[0], &values[1], &values[2]);
                if let Value::Str(source) = source {
                    if let (Value::Integer(start), Value::Integer(end)) = (start_value, end_value) {
                        let length = source.chars().count() as i64;
                        // Bounds are strict: negative or out-of-range indices,
                        // and an inverted range, are runtime errors rather
                        // than silent clamping.
                        if *start < 0 || *end < 0 || *start > *end || *end > length {
                            sink.emit(Diagnostic::error("`slice` index out of range").at(span));
                            return Value::Unit;
                        }
                        let (start, end) = (*start as usize, *end as usize);
                        let sliced = source
                            .chars()
                            .skip(start)
                            .take(end - start)
                            .collect::<String>();
                        if !self.charge_allocation(sliced.len(), span, sink) {
                            return Value::Unit;
                        }
                        return Value::Str(sliced);
                    }
                    sink.emit(
                        Diagnostic::error(format!(
                            "`slice` expects integer indices, found `{}` and `{}`",
                            start_value.type_name(),
                            end_value.type_name()
                        ))
                        .at(span),
                    );
                    return Value::Unit;
                }
                if let Value::List(elements) = source {
                    if let (Value::Integer(start), Value::Integer(end)) = (start_value, end_value) {
                        let length = elements.len() as i64;
                        // The same strict bounds as string slicing.
                        if *start < 0 || *end < 0 || *start > *end || *end > length {
                            sink.emit(Diagnostic::error("`slice` index out of range").at(span));
                            return Value::Unit;
                        }
                        let (start, end) = (*start as usize, *end as usize);
                        let charge = end - start;
                        if !self.charge_allocation(charge, span, sink) {
                            return Value::Unit;
                        }
                        return Value::List(Rc::new(elements[start..end].to_vec()));
                    }
                    sink.emit(
                        Diagnostic::error(format!(
                            "`slice` expects integer indices, found `{}` and `{}`",
                            start_value.type_name(),
                            end_value.type_name()
                        ))
                        .at(span),
                    );
                    return Value::Unit;
                }
                sink.emit(
                    Diagnostic::error(format!(
                        "`slice` expects a string or list argument, found `{}`",
                        source.type_name()
                    ))
                    .at(span),
                );
                Value::Unit
            }
            BuiltinFunction::Append => {
                if !Self::check_arity(BuiltinFunction::Append.name(), values, 2, span, sink) {
                    return Value::Unit;
                }
                match &values[0] {
                    Value::List(elements) => {
                        // Functional: the result is a list with one more
                        // element and the original is untouched. When the
                        // argument's buffer is not aliased (the usual
                        // `items = append(items, x)` case), `make_mut`
                        // reuses it in place instead of copying every
                        // element; an aliased list is copied transparently,
                        // so observable behavior is identical either way.
                        // Growth-based charge: see MAX_TOTAL_VALUE_BYTES.
                        if !self.charge_allocation(1, span, sink) {
                            return Value::Unit;
                        }
                        let mut appended = Rc::clone(elements);
                        Rc::make_mut(&mut appended).push(values[1].clone());
                        Value::List(appended)
                    }
                    other => {
                        sink.emit(
                            Diagnostic::error(format!(
                                "`append` expects a list argument, found `{}`",
                                other.type_name()
                            ))
                            .at(span),
                        );
                        Value::Unit
                    }
                }
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
                // `+` is overloaded: integer addition, string concatenation,
                // or list concatenation.
                match (left, right) {
                    (Value::Integer(a), Value::Integer(b)) => match a.checked_add(b) {
                        Some(sum) => Value::Integer(sum),
                        None => overflow(span, sink),
                    },
                    (Value::Str(a), Value::Str(b)) => {
                        let length = a.len().saturating_add(b.len());
                        if !self.check_string_size(length, span, sink)
                            || !self.charge_allocation(length, span, sink)
                        {
                            return Value::Unit;
                        }
                        let mut concatenated = String::with_capacity(length);
                        concatenated.push_str(&a);
                        concatenated.push_str(&b);
                        Value::Str(concatenated)
                    }
                    (Value::List(a), Value::List(b)) => {
                        // The result copies every element of both operands;
                        // the budget charges that copy. See
                        // MAX_TOTAL_VALUE_BYTES.
                        let charge = a.len().saturating_add(b.len());
                        if !self.charge_allocation(charge, span, sink) {
                            return Value::Unit;
                        }
                        let mut concatenated = a.as_ref().clone();
                        concatenated.extend(b.iter().cloned());
                        Value::List(Rc::new(concatenated))
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
                // Equality is defined for two integers, two booleans, two
                // strings, or two lists (compared element by element);
                // comparing values of different types is an error.
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
                    (Value::List(a), Value::List(b)) => {
                        // `Vec`'s equality compares element by element,
                        // recursing through nested lists.
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

    /// Charges `bytes` of newly built value data against the evaluation's
    /// cumulative allocation budget.
    ///
    /// Strings are charged their UTF-8 length; lists are charged element
    /// counts (the cost model is documented on
    /// [`MAX_TOTAL_VALUE_BYTES`]). When the budget is exhausted the
    /// resource-exhausted flag is set — every loop and expression site
    /// already stops on it — and an error is reported once.
    fn charge_allocation(&self, bytes: usize, span: Span, sink: &mut DiagnosticSink) -> bool {
        let total = self.allocated_bytes.get().saturating_add(bytes);
        if total > MAX_TOTAL_VALUE_BYTES {
            self.resource_exhausted.set(true);
            sink.emit(
                Diagnostic::error(format!(
                    "evaluation exceeded its total allocation budget of {MAX_TOTAL_VALUE_BYTES} bytes"
                ))
                .at(span),
            );
            return false;
        }
        self.allocated_bytes.set(total);
        true
    }

    /// Returns whether evaluating `node` could reassign a binding or call a
    /// function (which may reassign bindings indirectly). Used to decide
    /// whether an append-style assignment may run in place.
    ///
    /// The match below is deliberately exhaustive — no `_` arm. Adding an
    /// `AstKind` variant must force classification here; the compiler
    /// enforces it. A wildcard once hid `for` loops and `let` initializers
    /// from this check and made their side effects run twice (fixed in
    /// 1.12.1).
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
            AstKind::For {
                variable: _,
                start,
                end,
                body,
            } => {
                // The bounds are expressions like any other; the loop body
                // runs when the enclosing expression evaluates.
                start.as_deref().is_some_and(Self::may_mutate_bindings)
                    || Self::may_mutate_bindings(end)
                    || Self::may_mutate_bindings(body)
            }
            AstKind::Let { value, .. } => {
                // Declaring a binding evaluates its initializer, which may
                // call functions or assign.
                Self::may_mutate_bindings(value)
            }
            AstKind::Unary { operand, .. } => Self::may_mutate_bindings(operand),
            AstKind::Binary { left, right, .. } => {
                Self::may_mutate_bindings(left) || Self::may_mutate_bindings(right)
            }
            AstKind::Group { expression } => Self::may_mutate_bindings(expression),
            AstKind::List { elements } => elements.iter().any(Self::may_mutate_bindings),
            AstKind::Index { object, index } => {
                Self::may_mutate_bindings(object) || Self::may_mutate_bindings(index)
            }
            AstKind::Member { object, .. } => {
                // The object may be an arbitrary expression, such as a
                // block whose last statement calls a function.
                Self::may_mutate_bindings(object)
            }
            // Pure literals and names: evaluating them runs no user code.
            // (`Function` declares a function; its body only runs when the
            // resulting value is later called.)
            AstKind::Integer
            | AstKind::BooleanLiteral(_)
            | AstKind::StringLiteral
            | AstKind::Identifier
            | AstKind::Function { .. }
            | AstKind::Return { .. }
            | AstKind::Break
            | AstKind::Continue => false,
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
        if !self.check_string_size(length, expression_span, sink)
            || !self.charge_allocation(length, expression_span, sink)
        {
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

    /// Appends to a list binding in place for `name = append(name, expr)`.
    ///
    /// The binding's slot is the sole owner of the list unless the program
    /// aliased it (another `let`, a capture, or an argument); when aliased,
    /// `Rc::make_mut` copies transparently and the allocation budget is
    /// charged the copy. Returns `None` — deferring to the general
    /// assignment path with unchanged diagnostics — whenever the shape or
    /// types do not match; re-evaluating `expr` there is safe because it
    /// cannot mutate bindings.
    #[allow(clippy::too_many_arguments)]
    fn try_list_append_in_place(
        &self,
        name: &str,
        expression_span: Span,
        expr: &AstNode,
        source: &SourceFile,
        environment: &mut Environment,
        sink: &mut DiagnosticSink,
        depth: usize,
    ) -> Option<Value> {
        let appended = self.eval(expr, source, environment, sink, depth + 1);
        let slot = environment
            .scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(name))?;
        let Value::List(elements) = slot else {
            // Not a list binding (or a short-circuited operand): the general
            // path owns this case's diagnostics.
            return None;
        };
        let charge = if Rc::strong_count(elements) == 1 {
            1
        } else {
            elements.len().saturating_add(1)
        };
        if !self.charge_allocation(charge, expression_span, sink) {
            return Some(Value::Unit);
        }
        Rc::make_mut(elements).push(appended);
        Some(slot.clone())
    }

    /// Extends a list binding in place for `name = name + [e0, e1, ...]`
    /// where every element expression is pure.
    ///
    /// Mirrors [`Evaluator::try_list_append_in_place`]: when the binding's
    /// list is uniquely owned the extension reuses its buffer and charges
    /// only the added elements; when it may be aliased, `Rc::make_mut`
    /// copies transparently and the budget is charged the copy. Returns
    /// `None` - deferring to the general concatenation path with unchanged
    /// diagnostics - whenever the shape or types do not match;
    /// re-evaluating there is safe because the elements cannot mutate
    /// bindings.
    #[allow(clippy::too_many_arguments)]
    fn try_list_concat_in_place(
        &self,
        name: &str,
        expression_span: Span,
        element_expressions: &[AstNode],
        source: &SourceFile,
        environment: &mut Environment,
        sink: &mut DiagnosticSink,
        depth: usize,
    ) -> Option<Value> {
        let mut appended = Vec::with_capacity(element_expressions.len());
        for element in element_expressions {
            appended.push(self.eval(element, source, environment, sink, depth + 1));
        }
        let slot = environment
            .scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(name))?;
        let Value::List(elements) = slot else {
            // Not a list binding (or a short-circuited operand): the general
            // path owns this case's diagnostics.
            return None;
        };
        let charge = if Rc::strong_count(elements) == 1 {
            appended.len()
        } else {
            elements.len().saturating_add(appended.len())
        };
        if !self.charge_allocation(charge, expression_span, sink) {
            return Some(Value::Unit);
        }
        Rc::make_mut(elements).extend(appended);
        Some(slot.clone())
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
