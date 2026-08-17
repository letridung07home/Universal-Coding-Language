# Universal Coding Language (UCL) — Language Specification

> **Status:** initial version. This document codifies the subset of the
> language implemented by the current compiler pipeline (lexer → parser →
> evaluator). Sections marked *future work* describe features that are not yet
> implemented.

## 1. Overview

UCL is an interpreted, expression-oriented language. A program is a sequence
of statements; executing a program produces a single value, which is the value
of its last statement (or *unit* if the program is empty or an error
occurred).

The current implementation is intentionally small. It provides:

- three value types: *unit*, *integer*, and *boolean*;
- integer and boolean operators;
- `let` declarations, assignment, blocks, and lexical scoping;
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
comment ::= "//" [^\n]*
```

*Future work.* Block comments (`/* ... */`), which may span multiple lines.

### 2.4 Identifiers

```
identifier ::= [A-Za-z_] [A-Za-z0-9_]*
```

Identifiers name bindings. There are no reserved words in the current lexer;
the `let` declaration form is recognized by the parser from its token shape
(`identifier identifier =`), not from a keyword token kind.

### 2.5 Integer literals

```
integer-literal ::= [0-9]+
```

An integer literal is a non-negative decimal numeral. It must fit in a signed
64-bit integer; a literal outside that range is an error. Negative numbers are
written as a unary `-` applied to a positive literal (there is no negative
literal syntax).

### 2.6 Punctuation

The characters `( ) { } ; = + - * / % ^ < > & | !` are significant. Any other
ASCII punctuation character is tokenized as punctuation; one that no parser
production accepts produces an error at that position. The two-character
sequence `//` begins a comment (§2.3) and is not tokenized as punctuation.

### 2.7 Unrecognized characters

A non-ASCII character (or any character not covered above) is reported as an
error and skipped, so scanning continues after it.

## 3. Values and types

| Type    | Description                              | Example / source |
|---------|------------------------------------------|------------------|
| `unit`  | A single value with no contents.         | result of a declaration |
| `integer` | A signed 64-bit integer.               | `42`, `2 + 3`    |
| `boolean` | `true` or `false`.                      | `1 < 2`          |

There are no boolean *literals*: boolean values arise only from the comparison
and boolean operators described below.

*Future work.* Strings, functions, and the rest of the type system.

## 4. Expressions

```
expression      ::= binary-expression
binary-expression ::= unary-expression
                    | binary-expression binary-operator unary-expression
unary-expression ::= prefix-operator unary-expression
                    | primary
primary         ::= integer-literal
                  | identifier
                  | "(" expression ")"
                  | block
```

A *primary* is:

- an **integer literal**, evaluating to that integer;
- an **identifier**, evaluating to the value of the referenced binding (an
  unbound identifier is an error);
- a **parenthesized expression** `( expression )`, evaluating to the inner
  expression;
- a **block** `{ statements }`, described in §5.

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
| 6 (highest) | `^` | integer, integer | integer | exponentiation; the exponent must be non-negative |
| 5 | `*` `/` `%` | integer, integer | integer | checked arithmetic; `/` and `%` by zero are errors |
| 4 | `+` `-` | integer, integer | integer | checked arithmetic |
| 3 | `<` `>` | integer, integer | boolean | comparison |
| 2 | `&` | boolean, boolean | boolean | logical and |
| 1 (lowest) | `\|` | boolean, boolean | boolean | logical or |

Because every binary operator is left-associative, `2 ^ 3 ^ 2` is `(2 ^ 3) ^
2` (that is, `64`), and `a - b - c` is `(a - b) - c`.

`&` and `|` are logical operators on booleans only, not bitwise operators on
integers. There are no `<=`, `>=`, `==`, or `!=` operators.

Integer arithmetic is *checked*: addition, subtraction, multiplication,
negation, and exponentiation that overflow the signed 64-bit range, and
division or remainder by zero, are errors. A negative exponent is an error.

Applying a binary operator to operands of the wrong type is an error.

## 5. Statements and blocks

```
program   ::= statement*
statement ::= declaration
            | assignment
            | expression-statement
            | block
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

### 5.2 Assignment

```
assignment ::= identifier "=" expression
```

An assignment evaluates its right-hand side and assigns the result to an
existing binding, searching scopes from innermost to outermost. Assigning to
an unbound name is an error; assignment does not create a binding. The
assignment expression evaluates to the assigned value. The left-hand side must
be an identifier; any other target is an error.

### 5.3 Expression statement

An expression may stand alone as a statement; its value is computed and then
discarded (except that the value of a program's last statement is the
program's result).

### 5.4 Blocks and scope

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
is empty or an error occurred. The `ucl` CLI prints the program's value, except
that `unit` prints nothing.

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
- operators applied to operands of the wrong type.

Lexical and syntactic analysis recover and continue after an error where
possible, so a single run may report more than one diagnostic. Diagnostics are
rendered with a source excerpt pointing at the offending span.
