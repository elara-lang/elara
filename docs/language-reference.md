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

`elara` looks for a logical name in this order. The first hit wins.

1. The script's directory. `-e` and the REPL use `.`.
2. The current directory, when it is not already listed.
3. Each `--lib <dir>`, in the order written on the command line.
4. Each `lib_path` in the local `elara.ini`, in file order.
5. Each directory in `ELARA_PATH`, split on `:`. Empty segments are dropped.
6. Each `lib_path` in the system ini.

Inside one directory the candidates are `name.elac`, `name.ela`,
`name/init.elac`, `name/init.ela`, and, when native libraries are allowed,
`name.so`. A bytecode file is preferred over source in the same directory.
There is no unsuffixed file candidate.

A small library tree:

```
proj/
  main.ela
  elara.ini
  libs/
    vec/
      init.ela
      geom.ela
```

`proj/libs/vec/geom.ela`:

```
export const n = 7
```

`proj/libs/vec/init.ela`:

```
import "./geom" as g
export const geom = g
export const n = g.n
```

`proj/main.ela`:

```
import "vec" as v
print(v.n)
```

`proj/elara.ini` is the file in the next section. From any working directory:

```bash
./bin/elara proj/main.ela
```

The same library loads without an ini file:

```bash
./bin/elara --lib proj/libs proj/main.ela
ELARA_PATH=proj/libs ./bin/elara proj/main.ela
```

`--lib` is repeatable. A later directory is consulted only when every earlier
directory missed that name.

### Relative imports

`import "./geom" as g` and `import "../util" as u` resolve against the directory
of the file that contains the import, then search only that file's search root.
In the tree above, `vec/init.ela` was found under `proj/libs`, so `./geom`
becomes `vec/geom` and is opened as `proj/libs/vec/geom.ela`.

A `..` that leaves the root, an absolute path (`/…`), or a backslash is
rejected. A relative import in a chunk that has no file anchor (a byte buffer
the host loaded without `elara_set_module_anchor`) fails the same way.

### elara.ini

Only the `elara` command reads ini files. `elarac` does not, and an embedded
host does not either unless it copies the directories into
`elara_add_module_search_dir`. A missing file is ignored. A syntax error stops
the CLI before the script runs.

Create `elara.ini` next to the script (for `-e` and the REPL, next to the
current directory):

```
; project libraries
# one directory per line, file order is search order
lib_path = libs
lib_path = ../shared
```

`lib_path` is the only key. A relative value is joined to the directory of the
ini file itself, not to the current directory. `libs` in `proj/elara.ini` is
`proj/libs` even when you launch `elara` from somewhere else. `..` in that
value is allowed and is not canonicalized. The directory does not have to
exist when the CLI starts.

Comments are full lines that, after trimming, start with `;` or `#`. Blank
lines are skipped. A `[section]` header, a line without `=`, an unknown key, or
an empty `lib_path` is an error. The file cannot set gas, the heap ceiling, or
capabilities. `--sandbox` still refuses to read `.ela`, `.elac`, and `.so`
files.

The system file is `/etc/elara/elara.ini`. Create it the same way when every
user should see a shared root:

```
lib_path = /usr/local/share/elara
```

`ELARA_SYS_INI=/other/elara.ini` reads that path instead. `ELARA_SYS_INI=`
(empty) skips the system file. Unset `ELARA_SYS_INI` means
`/etc/elara/elara.ini`.

### Native libraries

A `name.so` on the search path is the last candidate for that directory. The
default `elara` process may open it. `--sandbox` hides it, and the import fails
as "module not found" rather than as a capability error. How to write and
compile the `.so` is in the C FFI guide.

## Command line

`elara` runs source or bytecode. Paths after the script become `sys.args` when
the `sys` capability is enabled.

```bash
./bin/elara script.ela
./bin/elara -e 'print(1)'
./bin/elara --lib libs --lib /opt/extra script.ela
./bin/elara --sandbox --gas 100000 --mem-limit 1048576 script.ela
./bin/elara --disasm script.elac
```

| Option | Effect |
| --- | --- |
| `-e <code>` | Evaluate one snippet and exit. The anchor directory is `.` |
| `--gas <N>` | Instruction budget. Exhaustion stops the script |
| `--mem-limit <N>` | Heap ceiling in bytes |
| `--sandbox` | Safe intrinsics plus `print`. No disk libraries and no `.so` |
| `--lib <dir>` | Append one search directory. Repeat the flag to append more |
| `--disasm` | Print bytecode before running |
| `-v`, `--version` | Print the version |
| `-h`, `--help` | Print the option list |

`elarac` compiles one `.ela` file. It does not search libraries and does not
read `elara.ini`.

```bash
./bin/elarac -o script.elac script.ela
./bin/elarac --check script.ela
./bin/elarac --strip --disasm script.ela
```

| Option | Effect |
| --- | --- |
| `-o <out.elac>` | Output path. The default replaces `.ela` with `.elac` |
| `--strip` | Set the strip_debug_info bit in the `.elac` header |
| `--check` | Compile and verify, then write nothing |
| `--disasm` | Print the compiled prototype |
| `-v`, `--version` | Print the version |
| `-h`, `--help` | Print the option list |

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

```
print(math.abs(-3))          /* 3 */
print(string.slice("elara", 1, 4))  /* lar */
```

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
