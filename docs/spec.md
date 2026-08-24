# Universal Coding Language (UCL) — Language Specification

> **Status:** stable as of version 1.2.0. This document specifies the language
> implemented by the compiler pipeline (lexer → parser → evaluator) and is
> the normative definition of that language.

## 1. Overview

UCL is an interpreted, expression-oriented language. A program is a sequence
of statements; executing a program produces a single value, which is the value
of its last statement (or *unit* if the program is empty or an error
occurred).

The current implementation is intentionally small. It provides:

- six value types: *unit*, *integer*, *boolean*, *string*, *function*, and
  read-only *module namespaces* created by aliased imports;
- integer, boolean, and string operators;
- `let` declarations, assignment, blocks, and lexical scoping;
- named functions with positional parameters, calls, and recursion;
- immutable list values with literals, indexing, equality, and iteration;
- a built-in prelude: `len(string)`, `str(value)`, `type(value)`,
  `upper(string)`, `lower(string)`, `contains(haystack, needle)`, `int(value)`,
  `find(haystack, needle)`, `replace(source, pattern, replacement)`,
  `trim(value)`, and `slice(value, start, end)`;
- conditional expressions (`if`/`else`), `while` and `for` loops, and
  `break`/`continue` loop control;
- local-file modules with flat or read-only namespaced imports, extensionless
  import paths, and configurable search directories; and
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
`else`, `while`, `for`, `in`, `return`, `break`, `continue`, and `use` are
reserved keywords: the lexer produces dedicated keyword tokens for them, and
they cannot be used as identifiers. The parser recognizes declarations from
their keyword tokens rather than from token shape.

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

The characters `( ) { } , ; = + - * / % ^ < > & | ! .` are significant. The
two-character sequences `<=`, `>=`, `==`, and `!=` are tokenized as single
operator tokens (§4.2), and the two-character sequence `..` is tokenized as a
range separator, legal only in a `for` header (§5.4). Any other ASCII
punctuation character is tokenized as punctuation; one that no parser
production accepts produces an error at that position. The two-character
sequence `//` begins a comment (§2.3) and is not tokenized as punctuation.

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
| `list`    | An ordered, immutable sequence of values. | `[1, 2, 3]`, `[]` |
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
                     | primary ("[" expression "]")*
argument-list      ::= expression ("," expression)*
primary            ::= integer-literal
                   | boolean-literal
                   | string-literal
                   | list-literal
                   | function-literal
                   | identifier
                   | "(" expression ")"
                   | block
list-literal       ::= "[" expression-list? "]"
expression-list    ::= expression ("," expression)*
if-expression   ::= "if" expression block ("else" (block | if-expression))?
```

A *primary* is:

- an **integer literal**, evaluating to that integer;
- a **boolean literal**, evaluating to that boolean;
- a **string literal**, evaluating to that string with escapes decoded;
- a **list literal** `[ expression, ... ]`, evaluating to a `list` value
  holding the element expressions' values in source order; lists nest
  arbitrarily and an empty list is written `[]`;
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
| 5 | `+` `-` | both integers, both strings, or both lists | integer / string / list | addition, concatenation, or list concatenation; checked arithmetic |
| 4 | `<` `>` `<=` `>=` | both integers or both strings | boolean | relational comparison, lexicographic for strings |
| 3 | `==` `!=` | two integers, booleans, or strings | boolean | equality |
| 2 | `&` | boolean, boolean | boolean | logical and, short-circuiting |
| 1 (lowest) | `\|` | boolean, boolean | boolean | logical or, short-circuiting |

Because every binary operator is left-associative, `2 ^ 3 ^ 2` is `(2 ^ 3) ^
2` (that is, `64`), and `a - b - c` is `(a - b) - c`.

`+` is overloaded by operand type: adding two integers performs checked
addition, adding two strings concatenates them, and adding two lists
concatenates them into a new list. Mixing types is an error.

Every evaluated string value is limited to **8 MiB of UTF-8 bytes**. This limit
applies to decoded string literals, concatenation results, and strings produced
by built-in functions. A construction that would exceed it is a runtime error;
evaluation stops rather than allowing unbounded string growth to exhaust host
memory.

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

The prelude currently provides these built-ins:

| Call | Result |
|------|--------|
| `len(value)` | An integer equal to the number of Unicode scalar values in `value` when it is a string, or the number of elements when it is a list. |
| `str(value)` | A string holding the same text the CLI and REPL echo for `value`: integers and booleans render as written, strings are unchanged, lists render as `[` + elements + `]` with element strings quoted, functions render as `<function>`, modules render as `<module>`, and unit renders as `unit`. |
| `type(value)` | A string naming the value's type: `integer`, `boolean`, `string`, `list`, `function`, or `module`. |
| `upper(string)` | The string converted to upper case. |
| `lower(string)` | The string converted to lower case. |
| `contains(haystack, needle)` | A boolean reporting whether the string `haystack` contains the string `needle` as a substring, or whether the list `haystack` contains an element equal to `needle` (using `==`, so nested lists compare element by element). |
| `int(value)` | An integer parsed from `value`: strings must consist of an optional `+` or `-` sign followed by ASCII decimal digits, with no surrounding whitespace; integers pass through unchanged. Parsing failures, out-of-range values, and non-string arguments are runtime errors. |
| `find(haystack, needle)` | An integer giving the scalar-value index of the first occurrence of the string `needle` in the string `haystack`, or `-1` if it does not occur. When `haystack` is a list, returns the index of the first element equal to `needle` (using `==`), or `-1`. |
| `replace(source, pattern, replacement)` | A copy of the string `source` with every occurrence of the string `pattern` replaced by the string `replacement`. An empty `pattern` is a runtime error. |
| `trim(value)` | A copy of the string `value` with leading and trailing whitespace removed. |
| `slice(value, start, end)` | The substring of the string `value` from scalar-value index `start` (inclusive) to `end` (exclusive), or a new list holding the elements of the list `value` in that range. Indices must satisfy `0 <= start <= end <= len(value)`; violations, including negative indices, are runtime errors. |
| `append(list, item)` | A new list holding the elements of the list `list` followed by `item`. The original list is untouched: `append` does not mutate. |

For example, `len("hé")` evaluates to `2`, `upper("hé")` evaluates to `"HÉ"`,
`type(len)` evaluates to `"function"`, `int("-41") + 1` evaluates to
`-40`, and `slice("hello", 1, 3)` evaluates to `"el"`. Calling a built-in
with anything other than exactly its declared arguments, or with arguments
of the wrong types, is a runtime error.

Strings produced by built-ins are subject to the same deterministic value
limit as any other string.

At call time a function sees three layers of bindings: the *current* global
scope, its own captured bindings (§4.4), and a fresh scope holding its
parameters. It never resolves bindings from the caller's local blocks; UCL has
no dynamic scoping. Assignments to globals made by a function persist after
the call. A top-level function's own name resolves dynamically at call time,
allowing recursion.

### 4.3.2 Index expressions

An index expression has the form `object[index]` and may chain
(`matrix[0][1]`). The object is evaluated first, then the index, which must
evaluate to an integer.

Indexing a **list** yields the element at that position; indexing a
**string** yields the one-character string at that scalar-value position
(matching how `len` counts). Indices are zero-based and strict: a negative
or out-of-range index is a runtime error, never a silent lookup. Indexing
any other type is a runtime error.

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

Evaluation also carries a deterministic **cumulative allocation budget**
(currently 256 MiB): every string operation charges the UTF-8 bytes it
copies, and list growth charges the newly added elements. Exceeding the
budget aborts evaluation with an error, so programs whose work grows
quadratically — such as repeatedly concatenating onto a growing string —
stop quickly instead of running unbounded. Ordinary accumulation through
`acc = acc + text` and `items = append(items, x)` assignments is linear and
stays far below the budget.

### 4.7 For loops

```
for-statement ::= "for" identifier "in" expression block
                 | "for" identifier "in" expression ".." expression block
```

A `for` loop iterates over a sequence, binding the identifier to one element
per iteration. Two forms are supported:

- **Range form** `for i in start..end`: iterates over the integers from
  `start` (inclusive) to `end` (exclusive), exactly `end - start` iterations
  when both bounds are integers. Both bounds are evaluated once, before the
  first iteration.
- **String form** `for ch in value`: the expression must evaluate to a
  string, and the loop iterates over its Unicode scalar values in order,
  binding each as a one-character string.
- **List form** `for item in value`: the expression must evaluate to a list,
  and the loop iterates over its elements in order.

The two dot characters of `..` must be adjacent; the separator is legal only
in a `for` header and is not a general expression operator.

An empty range (`start == end`) or inverted range (`start > end`) performs
zero iterations; it is not an error. Iterating any other value — integers,
booleans, functions, modules — is a runtime error.
Each iteration introduces a fresh lexical scope holding only the loop
variable, nested inside the scope where the statement appears; the variable
is not visible after the loop ends, and an outer binding of the same name is
shadowed, not modified. Assigning to the loop variable inside the body
affects only that iteration's binding; the next iteration binds a fresh
value.

Like `while`, a single `for` loop may run at most a fixed number of
iterations (currently 100,000); exceeding that bound aborts the loop with an
error. `break` and `continue` apply to `for` loops with the same meaning as
in `while` loops (§5.4).

## 5. Statements and blocks

```
program   ::= statement*
statement ::= declaration
            | function-declaration
            | return-statement
            | assignment
            | while-statement
            | for-statement
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

### 5.4 Loop control statements

```
break-statement    ::= "break" ";"
continue-statement ::= "continue" ";"
```

A `break` statement exits the innermost enclosing `while` or `for` loop
immediately; execution resumes after the loop. A `continue` statement skips
the rest of the loop body and proceeds directly to the loop's next condition
check (in a `for` loop, to the next element of the sequence). Both evaluate
to `unit`.

Both statements unwind through nested blocks and `if` bodies until they reach
the innermost enclosing loop, which consumes them. A `break` or `continue`
that reaches a function-call boundary — or the program or module top level —
without having been consumed by a loop is an error: a function called from
inside a loop cannot break or continue that caller's loop.

### 5.5 Imports

```
import ::= "use" string-literal ";"
         | "use" string-literal "as" identifier ";"
member-expression ::= postfix-expression "." identifier
```

A `use` statement imports a module from another source file. It may appear
only at the top level of a program; placing one inside a block or function
body is an error.

The path string is resolved by trying candidate locations in a fixed order:
the directory of the importing file first (in an interactive session, the
process's working directory), then each configured *search directory* in
order. At every location the path is tried as written and, when it has no
`.ucl` extension, once more with `.ucl` appended; the first existing file
wins. Absolute import paths ignore the search directories entirely. Search
directories come from the session configuration: the `ucl` binary accepts
repeatable `-p/--path <dir>` options and consults `UCL_PATH` directories
(separated by the platform path list separator) after any `-p/--path`
options. When no candidate exists, the error lists every location tried.

The legacy form, `use "path.ucl";`, is a *flat import*: every top-level
binding of the completed module — including names it imported itself — is
copied into the importing global scope. If any copied name is already bound,
the whole import fails without copying a partial set of exports. Repeating a
successful flat import of the same canonical path in one session is a no-op.

The alias form, `use "path.ucl" as math;`, binds one read-only module
namespace named `math` instead of copying its exports. A member expression
such as `math.double(21)` or `math.answer` resolves an exported value from
that namespace. An alias must not already be globally bound. `as` is
contextual: it is recognized only after the path in a `use` statement and
remains a valid ordinary identifier everywhere else.

Importing evaluates the module file at most once per canonical path and
session, in an isolated global scope: the module cannot see or mutate the
importer's bindings, and it resolves its own `use` statements recursively.
The completed export map is cached, so flat imports and any number of aliases
of the same path reuse the same evaluation result. A missing namespace member
is an error, as is member access on a value other than a module. Member access
is read-only; only identifiers are valid assignment targets.

Errors during loading are reported anchored at the `use` site: unreadable
files, syntax errors inside the module, and circular import chains all abort
the import; a program that reports errors still runs its earlier statements
in interactive sessions but produces no value in file mode.

### 5.6 Assignment

```
assignment ::= identifier "=" expression
```

An assignment evaluates its right-hand side and assigns the result to an
existing binding, searching scopes from innermost to outermost. Assigning to
an unbound name is an error; assignment does not create a binding. The
assignment expression evaluates to the assigned value. The left-hand side must
be an identifier; any other target is an error.

### 5.7 Expression statement

An expression may stand alone as a statement; its value is computed and then
discarded (except that the value of a program's last statement is the
program's result).

### 5.8 Blocks and scope

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
