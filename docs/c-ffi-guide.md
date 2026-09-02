# C FFI Guide

This guide shows how to embed Elara from C99, C++, or Rust hosts through
`elara/include/elara.h` and `bin/libelara.a` / `bin/libelara.so`.

## Opening a State

```c
#include "elara.h"

elara_State *S = elara_open();
/* … */
elara_close(S);
```

`elara_open` uses `elara_default_config()`. Prefer `elara_open_with_config` when you
need quotas or a restricted capability set.

## Configuration and Allocators

```c
elara_Config cfg = elara_default_config();
cfg.gas_limit = 100000;                 /* 0 = unlimited */
cfg.mem_limit_bytes = 8 * 1024 * 1024;  /* 0 = unlimited */
cfg.pause_budget_us = 500;
cfg.capabilities = ELARA_CAP_SAFE_DEFAULT | ELARA_CAP_STDIO;
cfg.alloc_fn = NULL;                    /* default System allocator */
cfg.alloc_ud = NULL;
cfg.hash_seed = 0;

elara_State *S = elara_open_with_config(&cfg);
```

Custom allocators implement `elara_Alloc` and may be used to route every heap byte
through a host arena. After open, `elara_set_gas_limit` and `elara_set_mem_limit`
adjust quotas dynamically. `elara_gc_collect` and `elara_gc_step` drive the
collector from the host.

## Evaluating Source and Bytecode

```c
elara_Value result;
elara_Status st = elara_eval(S, "chunk", "return 1 + 2", &result);
if (st != ELARA_OK) {
    fprintf(stderr, "%s\n", elara_last_error(S));
}

uint8_t *buf = NULL;
size_t len = 0;
st = elara_compile_to_bytecode(S, "chunk", "return 40 + 2", &buf, &len);
elara_Value closure;
st = elara_load_bytecode(S, buf, len, &closure);
elara_free_buffer(S, buf, len);
```

`elara_compile_to_bytecode` verifies the proto tree before serialization. Loaded
closures are ordinary callable values.

## Calling Values

```c
elara_Value args[2] = { elara_val_i48(20), elara_val_i48(22) };
elara_Value outs[4];
uint16_t nret = 0;
st = elara_call(S, callable, args, 2, outs, 4, &nret);
```

Argument and result slots are 0-based. `max_results` caps how many returns are
copied into `outs`; `out_num_results` reports how many were produced.

## Native Functions

Host natives use the direct `elara_CallContext` API (no stack shuffling):

```c
static elara_Status host_mul(elara_State *S, elara_CallContext *ctx) {
    int64_t a = elara_to_int(elara_arg(ctx, 0));
    int64_t b = elara_to_int(elara_arg(ctx, 1));
    elara_return(ctx, elara_val_int(S, a * b));
    return ELARA_OK;
}

elara_Value fn = elara_val_native_fn(S, host_mul, NULL);
elara_set_global(S, "host_mul", fn);
```

`elara_argc`, `elara_arg`, `elara_return`, and `elara_return_multi` are the only
required entry points inside a native. Optional host userdata is available through
`elara_context_userdata`.

## Userdata and Finalizers

```c
static void on_finalize(elara_State *S, void *data, size_t size) {
    (void)S; (void)size;
    /* release native resources owned by payload */
}

static const elara_UserdataDesc desc = {
    "File",
    0xF11Eu,
    on_finalize,
    NULL,   /* optional GC trace for embedded Elara references */
};

elara_Value ud;
void *payload = elara_new_userdata(S, sizeof(int64_t), &desc, &ud);
*(int64_t *)payload = 0;
assert(elara_check_userdata(S, ud, 0xF11Eu) == payload);
```

When the userdata becomes unreachable, the incremental GC invokes `finalize` during
sweep. `elara_check_userdata` verifies the type tag before the host touches the
payload.

## Sandboxing

Capability bits gate intrinsic modules:

| Macro                 | Module / feature        |
| --------------------- | ----------------------- |
| `ELARA_CAP_MATH`      | `math`                  |
| `ELARA_CAP_STRING`    | `string`                |
| `ELARA_CAP_TABLE`     | `table`                 |
| `ELARA_CAP_BIT`       | `bit`                   |
| `ELARA_CAP_FIBER`     | `fiber`                 |
| `ELARA_CAP_SYS_ENV`   | `sys.env` / `args` / FS |
| `ELARA_CAP_STDIO`     | `print`                 |
| `ELARA_CAP_NATIVE`    | load a `.so` from disk  |
| `ELARA_CAP_SAFE_DEFAULT` | math+string+table+bit+fiber |
| `ELARA_CAP_ALL`       | unrestricted            |

Instruction fuel (`gas_limit`) returns `ELARA_ERR_GAS` when exhausted. Heap ceilings
(`mem_limit_bytes`) return `ELARA_ERR_OOM` after an emergency collection fails to free
enough memory. Combine both with a narrow capability mask for untrusted plugins.
`ELARA_CAP_SAFE_DEFAULT` does not include `ELARA_CAP_NATIVE`. `--sandbox` is
that mask plus `ELARA_CAP_STDIO`, so it neither reads libraries from disk nor
opens a `.so`.

## Native Modules

A native module is an exports table produced by C. It is cached under the same
logical name as a source module. An opener registered for `math` (or any other
intrinsic) is stored and never called.

### Registered openers

```c
elara_Status open_geom(elara_State *S, void *ud, elara_Value *out) {
    (void)ud;
    elara_Value t = elara_new_table(S);
    if (elara_is_nil(t)) return ELARA_ERR_OOM;
    elara_Status st = elara_table_set(S, t, "n", elara_val_i48(7));
    if (st != ELARA_OK) return st;
    *out = t;
    return ELARA_OK;
}

elara_register_native_module(S, "geom", open_geom, NULL);
```

`elara_new_table` returns nil on out-of-memory and does not raise.
`elara_table_set` interns `key` and writes the field. A NULL key or a non-table
receiver returns `ELARA_ERR_RUNTIME`. This registration works with or without
`ELARA_CAP_NATIVE`. The VM does not free `ud`. `elara_add_module_search_dir`
appends a search directory and `elara_set_module_anchor` sets the entry
directory used by relative imports from a module the host supplied as bytes.

### Shared libraries

With `ELARA_CAP_NATIVE` set, a search-path file `name.so` is opened with
`dlopen` and must export:

```c
elara_Status elara_open_lib(elara_State *S, elara_Value *out_exports);
```

The handle stays mapped until `elara_close` finishes freeing the heap. A failed
open is closed immediately. `bin/elara` is linked with `--export-dynamic` so
the library can call `elara_*`. `demos/09_native_library.c` is a small fixture
loaded by `demos/09_native_library.ela`.

## Debug Hooks

```c
void on_event(elara_State *S, const elara_DebugEvent *ev, void *ud) {
    (void)S; (void)ud;
    /* inspect ev->event, ev->line, ev->fn_name, … */
}

elara_set_hook(S, on_event, NULL,
               ELARA_HOOK_CALL | ELARA_HOOK_RET | ELARA_HOOK_LINE | ELARA_HOOK_COUNT,
               1000);
```

`elara_debug_get_frame`, `elara_debug_get_local`, and `elara_debug_set_local` expose
live frames for DAP-style tooling. Count hooks fire every `count_interval`
instructions when `ELARA_HOOK_COUNT` is set.

## Complete Example

See `demos/08_c_ffi_sandbox_host.c` for a self-contained C99 program that registers a
native, attaches userdata with a finalizer, and asserts gas and memory sandbox
behavior. Build it with:

```bash
make build
cc -std=c99 -Wall -Wextra -Werror \
  -Ielara/include demos/08_c_ffi_sandbox_host.c bin/libelara.a \
  -lm -lpthread -ldl -o /tmp/elara_ffi_demo
/tmp/elara_ffi_demo
```
