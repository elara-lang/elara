# Language Reference

Programmer's reference for Elara source language, runtime values, and the intrinsic
standard library. Code samples use the same syntax accepted by `bin/elara` and
`bin/elarac`.

## Lexical Rules

Source files are UTF-8. An optional UTF-8 BOM and a Unix shebang (`#!…`) on the first
line are skipped. Line comments start with `//`. Block comments nest with `/* … */`.

Identifiers are ASCII letters, digits, and `_`, and must not start with a digit.
Keywords are reserved and cannot be rebound:

```
let const fn if else while for in return break defer yield
import export as true false nil
```

String literals use `"…"` with escapes (`\n`, `\t`, `\\`, `\"`, `\u{…}`). Adjacent
interpolation segments write `"text {expr} more"`. Triple-quoted strings allow
nested quotes without escaping.

Integer literals accept decimal, `0x` / `0o` / `0b` radices, and `_` digit separators.
Floats use decimal points and optional exponents. Inclusive ranges use `a..b`; a
stepped form `a..b..s` is reserved for numeric `for` loops.

## Types and Values

Runtime values are 8-byte tagged cells:

| Type        | Examples                         |
| ----------- | -------------------------------- |
| `nil`       | `nil`                            |
| `boolean`   | `true`, `false`                  |
| `int`       | `0`, `-1`, `0xff`, boxed `i64`   |
| `float`     | `3.14`, `1e-3`                   |
| `string`    | `"hi"`, `"x={x}"`                |
| `table`     | `{}`, `{1, 2}`, `{x: 1}`         |
| `function`  | `fn() {}`, closures, natives     |
| `fiber`     | `fiber.create(f)`                |
| `userdata`  | host-managed payloads            |

`type(v)` returns the type name as a string. `tostring` / `tonumber` convert with
optional radix for `tonumber`. Only `nil` and `false` are falsy.

## Operators

Arithmetic: `+ - * / %` (and unary `-`). Bitwise ops live in the `bit` module.
Comparison: `== != < <= > >=`. Logical: `and or not` (short-circuit). Length: `#t`
is O(1) for the array partition. Indexing: `t[i]`, `t.field`. Calls: `f(a, b)`.

Compound assignment updates a local or upvalue in place: `+= -= *= /=`.

Metamethods consulted on tables include `__add`, `__tostring`, `__index`,
`__newindex`, `__call`, and `__close`.

## Bindings

```
let x = 1
const PI = 3.141592653589793
x += 2
```

`let` is mutable; `const` rejects assignment after initialization. Undeclared
identifiers are compile errors. Shadowing in nested `{ }` blocks is allowed.
Self-initialization (`let x = x`) is rejected.

Multi-binding unpacks call results into consecutive locals:

```
let ok, err = pcall(fn() { panic("x") })
```

## Control Flow

```
if cond {
    …
} else if other {
    …
} else {
    …
}

while cond {
    …
}

for i in 0..n {
    …
}

break
return expr
return
```

`for` over an integer range compiles to specialized loop opcodes. `break` leaves the
innermost loop.

## Defer

```
{
    defer { print("second") }
    defer { print("first") }
    /* body */
}
```

`defer` registers a closeable slot. Leaving the block (normally or via error unwind)
runs registered hooks in LIFO order. Tables with a `__close` metamethod participate
in the same protocol when marked closeable.

## Functions and Closures

```
fn add(a, b) {
    return a + b
}

let make = fn(n) {
    return fn() { n += 1; return n }
}
```

Nested functions capture enclosing `let` bindings as upvalues. Captures from `for`
loop variables are per-iteration when bound with `let` inside the loop body.
`return f(...)` may become a tail call when no `defer` is active in the frame.

## Tables and Metatables

```
let arr = {10, 20, 30}     /* array part, indices 0..2 */
let map = {x: 1, y: 2}     /* hash part */
arr[0]
map.x
#arr                       /* => 3 */

let mt = {__tostring: fn(self) { return "point" }}
table.set_metatable(map, mt)
```

Array indices are 0-based. `table.*` helpers honor negative index normalization.
Weak tables use `table.set_metatable(t, {__mode: "k"|"v"|"kv"})`.

## Fibers

```
fn producer() {
    yield 1
    yield 2
}

let f = fiber.create(producer)
print(fiber.status(f))     /* "suspended" */
print(fiber.resume(f))     /* 1 */
fiber.resume(f)
fiber.resume(f)            /* finishes; status -> "dead" */
```

Fibers are stackful and cooperative. `yield` suspends the current fiber;
`fiber.resume` continues it and returns yielded or final values. Status strings are
`"suspended"`, `"running"`, and `"dead"`.

## Modules

```
/* lib/math_ext.ela */
export fn add(a, b) { return a + b }
export const VERSION = 1

/* main */
import "lib/math_ext" as mx
print(mx.add(2, 3))
```

`export let` / `export const` / `export fn` bind a local and publish it on the module
exports table. `import "spec" as alias` loads a singleton exports table (cached).
Circular imports fail with a module error. Intrinsic modules (`math`, `string`,
`table`, `bit`, `sys`, `fiber`, `gc`) resolve without files. A trailing `.ela` or
`.elac` on the specifier is stripped, so `math.ela` is still the intrinsic `math`.

### Search path

For any other name the CLI searches, in order: the script's directory (`.` for
`-e` and the REPL), the current directory when it is not already listed, each
`--lib <dir>` in order, `lib_path` entries from a local `elara.ini`, directories
in `ELARA_PATH` (split on `:`, empty segments dropped), then `lib_path` entries
from the system file. The first existing candidate wins. Inside one directory
the candidates are `name.elac`, `name.ela`, `name/init.elac`, `name/init.ela`,
and, when native libraries are allowed, `name.so`.

`import "./geom" as g` and `import "../util" as u` resolve against the directory
of the module that contains the import, then search only that module's root.
`..` that escapes the root, an absolute path, or a backslash is rejected.

### elara.ini

Only the `elara` CLI reads ini files. `elarac` does not. A missing file is
ignored. The local file is `elara.ini` in the script directory (`.` for `-e`
and the REPL). The system file is `/etc/elara/elara.ini` unless `ELARA_SYS_INI`
names another path; an empty `ELARA_SYS_INI` skips the system file. Lines are
`lib_path = <dir>`. A relative directory is joined to the directory of the ini
file that declared it. `;` and `#` start a comment line. The file cannot change
gas, the heap ceiling, or capabilities. `--sandbox` still refuses to read
libraries from disk.

## Errors

```
assert(cond, "message")
panic("message")
let ok, err = pcall(fn() { panic("boom") })
```

`pcall` returns `true, …` on success or `false, message` on failure. Open `defer` /
`__close` hooks still run while unwinding. Host gas exhaustion bypasses script
`pcall` and surfaces as `ELARA_ERR_GAS` at the host boundary.

## Standard Library

Intrinsic modules are imported automatically into script prologues when the host
capability bitmask allows them.

### math

Constants: `pi`, `huge`, `max_i48`, `min_i48`, `max_int`, `min_int`.

Functions: `abs`, `floor`, `ceil`, `round`, `idiv`, `sqrt`, `sin`, `cos`, `tan`,
`asin`, `acos`, `atan2`, `exp`, `log`, `min`, `max`, `clamp`, `random`, `randomseed`.

### string

0-based byte indexing with negative-index normalization:
`len`, `slice`, `byte`, `char`, `find`, `replace`, `split`, `upper`, `lower`,
`trim`, `starts_with`, `ends_with`, `rep`.

### table

0-based array ops: `push`, `pop`, `insert`, `remove`, `concat`, `sort`, `keys`,
`values`, `clone`, `set_metatable`, `get_metatable`.

### bit

48/64-bit integer ops: `band`, `bor`, `bxor`, `bnot`, `shl`, `shr`, `ushr`, `rol`,
`ror`, `count_ones`, `leading_zeros`, `trailing_zeros`.

### sys

Always: `clock_ns()`, `mem_used()`, `gas_remaining()`, `version`.
With `ELARA_CAP_SYS_ENV`: `env(name)`, `args`, `exit(code?)`.

### fiber

`create`, `resume`, `status`, plus the `yield` statement.

### gc

`collect()` runs a full cycle. `step(budget_us?)` runs incremental work under the
pause budget and returns whether the collector returned to `Pause`.

## Core Globals

| Name       | Role                                      |
| ---------- | ----------------------------------------- |
| `print`    | Tab-separated stdout line (`ELARA_CAP_STDIO`) |
| `type`     | Type name string                          |
| `tostring` | String conversion / `__tostring`          |
| `tonumber` | Parse / convert numbers                   |
| `assert`   | Truth check or runtime error              |
| `panic`    | Unconditional runtime error               |
| `pcall`    | Protected call                            |
