# Architecture

Elara is a register-based bytecode virtual machine and AOT compiler written in Rust
(Edition 2024) with zero external crate dependencies. This guide describes the core
runtime data layouts and control planes that every subsystem shares.

## Value Representation

Every runtime value is a single `u64` packed with IEEE-754 Quiet-NaN tagging. Finite
floats keep their native bit pattern. Non-float payloads live in the Quiet-NaN space
so the VM can discriminate tags with a handful of masks.

### NaN-Tagging Layout

```
  63                48 47                              0
 +--------------------+--------------------------------+
 | tag / QNaN marker  | payload (i48 / ptr / bool / …) |
 +--------------------+--------------------------------+

 Finite f64     : IEEE-754 bits unchanged
 NIL            : QNaN | tag=0
 BOOL           : QNaN | tag=1 | low bit = 0/1
 INT (i48)      : QNaN | tag=2 | signed 48-bit payload
 LIGHTUD        : QNaN | tag=3 | host light pointer
 OBJ            : QNaN | tag=4 | GC pointer (8-byte aligned)
```

Object pointers are stored in the low 48 bits and must remain 8-byte aligned so the
tag bits never collide with address bits on current platforms.

### Integer Promotion

Integers that fit in signed 48-bit range (`[-2^47, 2^47 - 1]`) stay inline. Values
outside that range are transparently boxed as `ObjBoxedI64` heap objects. Arithmetic,
bitwise ops, hashing, and equality unify inline `i48`, boxed `i64`, and exact-integer
`f64` so scripts never observe the representation split.

## Strings

Elara uses a dual-tier string pool:

```
 short  (len <= 64)  : eagerly interned, content-addressed, O(1) identity compare
 long   (len >  64)  : heap object with lazily computed FNV-1a hash on first use
```

Short strings dominate identifiers, field names, and message fragments. Long strings
avoid hashing cost until they enter a hash table or otherwise need a stable hash.

## Tables

`ObjTable` is sized to a single 64-byte CPU cache line and hybridizes two partitions:

```
 +---------------- ObjTable (64 bytes) ----------------+
 | header | metatable | array ptr/len/cap | hash ptr… |
 +-----------------------------------------------------+
         |                         |
         v                         v
   dense 0-based vector      Robin Hood open addressing
   (push/pop/O(1) #t)        (backward-shift delete)
```

Array indices are strictly 0-based (`0 .. #t - 1`). Negative indices normalize from
the end in standard-library helpers. Hash keys reject `nil` and `NaN`. Metatables
provide operator overloads (`__add`, `__tostring`, `__index`, `__newindex`, `__call`,
`__close`) and weak modes via `__mode`.

## Virtual Machine

The interpreter is a register machine. Each call frame addresses a window of the
fiber stack by base index rather than raw pointers so stack growth never invalidates
frames.

### Call Frames

```
 fiber.stack
 +-----+-----+-----+-----+-----+-----+-----+
 | …   | R0  | R1  | R2  | …   | Rn  | …   |
 +-----+-----+-----+-----+-----+-----+-----+
         ^
         frame.base_idx

 CallFrame {
   closure, base_idx, pc, num_results,
   vararg_count, …
 }
```

Opcodes are fixed-width 32-bit instructions (`iABC`, `iABx`, `iAsBx`, `iSj`). Field
loads and stores use a one-slot inline cache that invalidates when a table rehashes.

### Tail Calls and Upvalues

`return f(...)` rewrites to `OP_TAILCALL` when the function scope has no active
`defer` / `__close` slots, keeping recursive state machines in O(1) frames.

Open upvalues point at live stack slots. Closing a frame migrates them to a heap
`closed` cell. Multiple closures that capture the same local share one upvalue
object so mutations stay coherent before and after return.

### Defer and Close

`defer { … }` registers a closeable slot. `OP_CLOSE` drains registered slots in LIFO
order: defer closures run first, then metatable `__close` hooks. Unwinding through
`pcall` executes the same LIFO protocol before the error propagates to the host.

## Garbage Collector

Elara uses a tri-color incremental mark-and-sweep collector with a configurable pause
budget (default 500 µs per `gc.step`).

### Phases

```
 Pause ──► Propagate ──► Atomic ──► SweepString ──► SweepObj ──► Pause
              │              │
              │              └─ clear weak entries, flip white
              └─ gray worklist + barrier-dirtied tables
```

### Write Barriers

Backward write barriers shade white objects inserted into black tables during
Propagate, preserving the incremental invariant without stopping the mutator for a
full heap walk.

### Weak Tables

A metatable field `__mode = "k" | "v" | "kv"` marks keys and/or values as weak. Weak
entries whose referent is still white after Propagate are cleared in Atomic so caches
evict unreachable objects without retaining them as strong roots.

## Module Loading

`OP_IMPORT` keys a single cache of exports tables. An entry that is still
loading is a circular import. The normalized names `math`, `string`, `table`,
`bit`, `sys`, `fiber`, and `gc` resolve inside the VM before any host opener,
byte loader, or directory is considered, so a file or a `.so` cannot replace
them.

The CLI builds the directory list (`--lib`, `elara.ini`, `ELARA_PATH`) and
stores it on the state. The VM only walks that list. Reading `.ela` and
`.elac` requires `ELARA_CAP_SYS_ENV`. Opening a `.so` requires
`ELARA_CAP_NATIVE`. A host-registered opener runs without that bit. The search
order and the ini file format are specified in the language reference.

## Elara vs Lua

| Topic              | Elara 1.0                                      | Lua 5.5                                      |
| ------------------ | ---------------------------------------------- | -------------------------------------------- |
| Indexing           | Strict 0-based arrays                          | 1-based arrays                               |
| Integers           | Inline i48 + transparent `ObjBoxedI64`         | Integer subtype with separate semantics      |
| Values             | 8-byte Quiet-NaN tags                          | Tagged unions / NaN tricks vary by build     |
| Tables             | 64-byte hybrid array + Robin Hood map          | Array + hash parts, different layout         |
| Scoping            | Explicit `let` / `const`, static undeclared ban| Implicit globals by default                  |
| Cleanup            | `defer` + `__close` LIFO                       | `<close>` to-be-closed variables             |
| Concurrency        | Stackful cooperative fibers                    | Coroutines (asymmetric)                      |
| Sandbox            | Gas + heap ceiling + `ELARA_CAP_*` bitmask     | Debug hooks / host policy                    |
| Host API           | `elara.h` C99, 0-based `CallContext`           | Stack-based C API                            |
| Dependencies       | Rust `std` only                                | C runtime                                    |
