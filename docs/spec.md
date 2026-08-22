# Universal Coding Language (UCL) — Language Specification

> **Status:** stable as of version 1.1.0. This document specifies the language
> implemented by the compiler pipeline (lexer → parser → evaluator) and is
> the normative definition of that language.

## 1. Overview

UCL is an interpreted, expression-oriented language. A program is a sequence
of statements; executing a program produces a single value, which is the value
of its last statement (or *unit* if the program is empty or an error
occurred).

The current implementation is intentionally small. It provides:

- five value types: *unit*, *integer*, *boolean*, *string*, and *function*;
- integer, boolean, and string operators;
- `let` declarations, assignment, blocks, and lexical scoping;
- named functions with positional parameters, calls, and recursion;
- a built-in prelude with Unicode-aware `len(string)`;
- conditional expressions (`if`/`else`) and `while` loops;
- structured diagnostics with source excerpts.

## 2. Lexical structure

### 2.1 Source text

Source files are UTF-8 text. All positions and spans are byte offsets into the
source; a span is a half-open range `start..end` where `start` is inclusive and
`end` is exclusive.

### 2.2 Whitespace

ASCII whitespace (spaces, tabs, and newlines) separates tokens and is otherwise
ignored.

### 2.3 Comments

A line comment begins with `//` and runs to the end of the line; the
terminating newline is not part of the comment. Comments are ignored by the
lexer and produce no tokens, so a comment may appear wherever whitespace may.

```
comment ::= line-comment | block-comment
line-comment  ::= "//" [^\n]*
block-comment ::= "/*" block-comment-body* "*/"
```

A line comment runs from `//` to the end of the line. A block comment runs
from `/*` to the next matching `*/`, may span multiple lines, and *nests*:
inner `/* ... */` pairs must be closed before the outer comment ends, so code
containing block comments can itself be commented out. An unterminated block
comment is an error. Comments may appear wherever whitespace may and are not
tokenized.

### 2.4 Identifiers

```
identifier ::= [A-Za-z_] [A-Za-z0-9_]*
```

Identifiers name bindings. The words `let`, `fn`, `true`, `false`, `if`,
`else`, `while`, `return`, and `use` are reserved keywords: the lexer produces
dedicated keyword tokens for them, and they cannot be used as identifiers. The
parser recognizes declarations from their keyword tokens rather than from
token shape.

### 2.5 String literals

```
string-literal ::= '"' string-character* '"'
string-character ::= [^"\n\\] | escape-sequence
escape-sequence ::= "\\" ("n" | "t" | "\" | "\\")
```

A string literal is a sequence of Unicode characters between two double
quotes. A raw newline may not appear inside a string literal, and the literal
must be terminated on the same line it starts on. Within a string, the escape
sequences `\n` (newline), `\t` (tab), `\"` (double quote), and `\\`
(backslash) decode to the corresponding characters; any other escape
sequence is an error. An unterminated string literal is an error.

### 2.6 Boolean literals

```
boolean-literal ::= "true" | "false"
```

### 2.7 Integer literals

```
integer-literal ::= [0-9]+
```

An integer literal is a non-negative decimal numeral. It must fit in a signed
64-bit integer; a literal outside that range is an error. Negative numbers are
written as a unary `-` applied to a positive literal (there is no negative
literal syntax).

### 2.8 Punctuation

The characters `( ) { } , ; = + - * / % ^ < > & | !` are significant. The
two-character sequences `<=`, `>=`, `==`, and `!=` are tokenized as single
operator tokens (§4.2). Any other ASCII punctuation character is tokenized as
punctuation; one that no parser production accepts produces an error at that
position. The two-character sequence `//` begins a comment (§2.3) and is not
tokenized as punctuation.

### 2.9 Unrecognized characters

A non-ASCII character (or any character not covered above) is reported as an
error and skipped, so scanning continues after it.

## 3. Values and types

| Type    | Description                              | Example / source |
|---------|------------------------------------------|------------------|
| `unit`  | A single value with no contents.         | result of a declaration |
| `integer` | A signed 64-bit integer.               | `42`, `2 + 3`    |
| `boolean` | `true` or `false`.                     | `true`, `1 < 2`  |
| `string`  | A sequence of Unicode characters.      | `"hello"`, `"a" + "b"` |
| `function` | A callable with positional parameters. | `fn add(a, b) { a + b; }`, `fn(n) { n; }` |

UCL is **dynamically typed**. Values carry runtime types and operators
validate their operands when evaluated; UCL performs no static type checking,
type annotations, or inference.

UCL also provides a small built-in prelude. Its bindings resolve after every
user scope, so a declaration in any user scope may shadow a built-in without
mutating the prelude. Built-in names are ordinary identifiers, not keywords.

## 4. Expressions

```
expression         ::= binary-expression
binary-expression  ::= unary-expression
                     | binary-expression binary-operator unary-expression
                     | if-expression
unary-expression   ::= prefix-operator unary-expression
                     | postfix-expression
postfix-expression ::= primary ("(" argument-list? ")")*
argument-list      ::= expression ("," expression)*
primary            ::= integer-literal
                   | boolean-literal
                   | string-literal
                   | function-literal
                   | identifier
                   | "(" expression ")"
                   | block
if-expression   ::= "if" expression block ("else" (block | if-expression))?
```

A *primary* is:

- an **integer literal**, evaluating to that integer;
- a **boolean literal**, evaluating to that boolean;
- a **string literal**, evaluating to that string with escapes decoded;
- a **function literal**, evaluating to a `function` value (§4.4);
- an **identifier**, evaluating to the value of the referenced binding (an
  unbound identifier is an error);
- a **parenthesized expression** `( expression )`, evaluating to the inner
  expression;
- a **block** `{ statements }`, described in §5.

Parentheses around an `if` condition are optional: both `if x < 0 { ... }`
and `if (x < 0) { ... }` are accepted.

### 4.1 Unary operators

Unary operators bind tighter than every binary operator. They are applied
left-to-right, so `- - x` is `-(-x)`. Note that because unary operators bind
tighter than `^`, `-2 ^ 2` is `(-2) ^ 2`.

| Operator | Operand | Result |
|----------|---------|--------|
| `+` | integer | the operand, unchanged |
| `-` | integer | the operand, negated (checked) |
| `!` | boolean | the logical negation |

Applying a unary operator to any other type is an error.

### 4.2 Binary operators

Binary operators are infix and left-associative. Precedence is ordered from
tightest to loosest below:

| Precedence | Operators | Operand types | Result | Notes |
|------------|-----------|---------------|--------|-------|
| 7 (highest) | `^` | integer, integer | integer | exponentiation; the exponent must be non-negative |
| 6 | `*` `/` `%` | integer, integer | integer | checked arithmetic; `/` and `%` by zero are errors |
| 5 | `+` `-` | both integers or both strings | integer / string | addition or concatenation; checked arithmetic |
| 4 | `<` `>` `<=` `>=` | both integers or both strings | boolean | relational comparison, lexicographic for strings |
| 3 | `==` `!=` | two integers, booleans, or strings | boolean | equality |
| 2 | `&` | boolean, boolean | boolean | logical and, short-circuiting |
| 1 (lowest) | `\|` | boolean, boolean | boolean | logical or, short-circuiting |

Because every binary operator is left-associative, `2 ^ 3 ^ 2` is `(2 ^ 3) ^
2` (that is, `64`), and `a - b - c` is `(a - b) - c`.

`+` is overloaded by operand type: adding two integers performs checked
addition, and adding two strings concatenates them. Mixing types is an error.

Every evaluated string value is limited to **8 MiB of UTF-8 bytes**. This limit
applies to decoded string literals and concatenation results. A construction
that would exceed it is a runtime error; evaluation stops rather than allowing
unbounded string growth to exhaust host memory.

`&` and `|` are logical operators on booleans only, not bitwise operators on
integers. Both *short-circuit*: `&` does not evaluate its right-hand side when
the left-hand side is `false`, and `|` does not evaluate it when the left-hand
side is `true`. Errors in a skipped operand are therefore not reported.

Equality (`==`, `!=`) is defined for two integers, two booleans, or two
strings; comparing values of different types is an error. Relational
operators (`<`, `>`, `<=`, `>=`) are defined for two integers (numeric
order) and for two strings (lexicographic order by Unicode scalar value);
mixing types is an error.

Integer arithmetic is *checked*: addition, subtraction, multiplication,
negation, and exponentiation that overflow the signed 64-bit range, and
division or remainder by zero, are errors. A negative exponent is an error.
The `-` operator remains integer-only; there is no string repetition.

Applying a binary operator to operands of the wrong type is an error.

### 4.3 Function calls

A call expression has the form `callee(argument, ...)`. Calls bind more tightly
than unary and binary operators, so `-negate(2)` means `-(negate(2))`. Calls may
be chained (`f(x)(y)`), since any expression that produces a function may be
called immediately.

The callee is evaluated first. Arguments are then evaluated left-to-right before
the function body begins. The callee must evaluate to a `function` and the
number of arguments must exactly equal the function's declared parameter count.
Each parameter receives its corresponding argument value in a fresh call scope.
A function evaluates to the value of an executed `return` statement, or the
value of the final statement in its body when no `return` ran.

### 4.3.1 Built-in functions

Built-in functions are ordinary function values supplied by the prelude. Their
names may be shadowed by `let` declarations, function declarations, parameters,
or inner block bindings; resetting a REPL session restores the original
prelude.

`len` is currently the only built-in:

| Call | Result |
|------|--------|
| `len(string)` | An integer equal to the number of Unicode scalar values in `string`. |

For example, `len("hé")` evaluates to `2`. Calling `len` with anything other
than exactly one string argument is a runtime error.

At call time a function sees three layers of bindings: the *current* global
scope, its own captured bindings (§4.4), and a fresh scope holding its
parameters. It never resolves bindings from the caller's local blocks; UCL has
no dynamic scoping. Assignments to globals made by a function persist after
the call. A top-level function's own name resolves dynamically at call time,
allowing recursion.

### 4.4 Function literals and closures

```
function-literal ::= "fn" "(" parameter-list? ")" block
```

An anonymous function literal is an expression whose value is a `function`.
Literals may be stored in variables, passed as arguments, returned from other
functions, and called immediately:

```
let double = fn(n) { n * 2; };
double(21);                       // 42
fn(a, b) { a + b; }(20, 22);      // 42, an immediately invoked literal
```

When a literal (or named function declared inside a block) is created it
*captures by value* every non-global binding visible at that point: later
changes to those bindings do not affect the already-created function, and two
closures created in the same scope do not observe each other's captures.
Global bindings are excluded from the capture and always resolve dynamically,
so functions see the latest global state. A consequence: a literal cannot
call itself through the variable being defined (`let f = fn(n) { f(n); }`
fails), while a named top-level declaration can recurse.

The maximum number of active function calls is fixed (currently 128);
exceeding that bound aborts evaluation with an error instead of overflowing
the host stack.

### 4.5 Conditional expressions

```
if-expression ::= "if" expression block ("else" (block | if-expression))?
```

An `if` expression evaluates its condition, which must produce a boolean;
anything else is an error. When the condition is `true` the `then` block runs,
and otherwise the `else` branch (if present) runs. The value of the whole
expression is the value of whichever branch ran; a missing `else` branch makes
the expression evaluate to `unit` when the condition is `false`.

An `else` branch may itself be another `if` expression, allowing `else if`
chains. Only the taken branch is evaluated: errors in the skipped branch are
not reported.

```
if x < 0 { "negative"; } else if x == 0 { "zero"; } else { "positive"; }
```

### 4.6 While loops

```
while-statement ::= "while" expression block
```

A `while` loop evaluates its condition before each iteration; the condition
must produce a boolean. The body block runs once per iteration while the
condition holds, introducing its own lexical scope. The statement evaluates
to `unit`.

A single loop may run at most a fixed number of iterations (currently
100,000); exceeding that bound aborts the loop with an error, so a condition
that never becomes `false` cannot hang the interpreter.

## 5. Statements and blocks

```
program   ::= statement*
statement ::= declaration
            | function-declaration
            | return-statement
            | assignment
            | while-statement
            | expression-statement
            | block
import    ::= "use" string-literal
```

Statements are separated by semicolons. A trailing semicolon after the final
statement of a program or block may be omitted. Empty statements (a lone `;`)
are permitted and ignored.

### 5.1 Declaration

```
declaration ::= "let" identifier "=" expression
```

A declaration evaluates its initializer and binds the identifier to the
result in the innermost enclosing scope. The declaration itself evaluates to
`unit`. Redeclaring a name in the same scope shadows the earlier binding; the
earlier binding remains intact in outer scopes.

### 5.2 Function declaration

```
function-declaration ::= "fn" identifier "(" parameter-list? ")" block
parameter-list       ::= identifier ("," identifier)*
```

A function declaration may appear in any scope. It binds the declared
identifier to a function value and itself evaluates to `unit`. Parameter names
must be unique. A declaration inside a block or function body additionally
captures the enclosing non-global bindings, like a literal (§4.4).

### 5.3 Return statement

```
return-statement ::= "return" expression? ";"
```

A `return` statement exits the innermost active function call immediately.
With an expression it makes the call evaluate to that expression's value;
without one the call evaluates to `unit`. `return` unwinds through nested
blocks and loop bodies. A `return` that executes outside any function call is
an error.

### 5.4 Imports

```
import ::= "use" string-literal ";"
```

A `use` statement imports a module from another source file. It may appear
only at the top level of a program; placing one inside a block or function
body is an error. The path is a string literal interpreted relative to the
directory of the importing file (in an interactive session, relative to the
process's working directory).

Importing evaluates the module file exactly once per session, in an isolated
global scope: the module cannot see or mutate the importer's bindings, and it
resolves its own `use` statements recursively. After evaluation succeeds,
every top-level binding of the module — including those it imported itself —
is copied into the importing global scope. If any copied name is already
bound there, the whole import fails with an error.

Errors during loading are reported anchored at the `use` site: unreadable
files, syntax errors inside the module, and circular import chains all abort
the import; a program that reports errors still runs its earlier statements
in interactive sessions but produces no value in file mode.

### 5.5 Assignment

```
assignment ::= identifier "=" expression
```

An assignment evaluates its right-hand side and assigns the result to an
existing binding, searching scopes from innermost to outermost. Assigning to
an unbound name is an error; assignment does not create a binding. The
assignment expression evaluates to the assigned value. The left-hand side must
be an identifier; any other target is an error.

### 5.6 Expression statement

An expression may stand alone as a statement; its value is computed and then
discarded (except that the value of a program's last statement is the
program's result).

### 5.7 Blocks and scope

```
block ::= "{" statement* "}"
```

A block introduces a new lexical scope nested inside the enclosing one. Name
resolution walks scopes from innermost to outermost: a reference resolves to
the nearest enclosing binding; a declaration binds in the current scope; an
assignment updates the nearest enclosing binding. The program itself is the
outermost scope.

A block evaluates to the value of its last statement, or `unit` if it is
empty.

## 6. Program evaluation

1. The source is tokenized (lexer).
2. The tokens are parsed into an abstract syntax tree (parser).
3. The tree is evaluated (evaluator).

A program's value is the value of its last statement, or `unit` if the program
is empty. If any stage of the pipeline reports an error, no value is produced:
later stages do not run, and the `ucl` CLI prints nothing and exits with a
failure status.

## 7. Errors and diagnostics

Diagnostics carry a severity (`error`, `warning`, or `note`) and an optional
source span. Errors include:

- unrecognized characters;
- malformed syntax (missing expressions, unbalanced delimiters, missing `;`
  between statements);
- references to unbound names;
- assignment to an unbound name or to a non-identifier target;
- integer literals outside the signed 64-bit range;
- integer overflow in checked arithmetic;
- division or remainder by zero;
- negative exponents;
- unterminated string literals and unknown or truncated escape sequences;
- operators applied to operands of the wrong type, including equality
  comparisons between values of different types;
- `if` or `while` conditions that do not produce a boolean;
- calls to non-function values or calls with an incorrect argument count;
- duplicate function parameters;
- a `return` statement that executes outside any function call;
- a `use` statement outside the top level of a program, with a non-string
  path, targeting an unreadable file, forming a circular import chain, or
  exporting a name that collides with an existing binding;
- a loop exceeding the maximum iteration count;
- constructing a string value larger than the 8 MiB UTF-8 byte limit;
- a function call exceeding the maximum active-call depth (currently 128);
- expression nesting that exceeds the parser's or evaluator's depth limit.

The parser rejects expressions nested more than a fixed depth (currently 256
levels) to prevent stack overflow on pathological input. The evaluator
enforces a separate, higher limit as a safety net for deeply nested ASTs
constructed through the library API; binary operator chains do not count
toward nesting and may be arbitrarily long. Deeper nesting is reported as an
error.

Lexical and syntactic analysis recover and continue after an error where
possible, so a single run may report more than one diagnostic. To prevent a
repeatedly failing program from exhausting host memory, at most 1,000
diagnostics are retained from one pipeline run; once that limit is reached,
evaluation stops. Diagnostics are rendered with a source excerpt pointing at
the offending span.

Each pipeline stage runs only if the previous stage completed without errors:
a program with lexical errors is not parsed, and one with syntax errors is not
evaluated.
