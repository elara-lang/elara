/* Pure C99 host embedding libelara: natives, userdata finalizers, gas/mem sandbox. */

#include "elara.h"

#include <assert.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>

#define BOX_TAG 0xB0B0B0B0u

static int g_finalizer_hits = 0;

static void box_finalize(elara_State *S, void *data, size_t size) {
    (void)S;
    assert(size == sizeof(int64_t));
    assert(data != NULL);
    assert(*(int64_t *)data == (int64_t)0xDEADBEEF);
    g_finalizer_hits += 1;
}

static elara_Status host_mul(elara_State *S, elara_CallContext *ctx) {
    assert(elara_argc(ctx) == 2);
    int64_t a = elara_to_int(elara_arg(ctx, 0));
    int64_t b = elara_to_int(elara_arg(ctx, 1));
    elara_return(ctx, elara_val_int(S, a * b));
    return ELARA_OK;
}

static void expect_ok(elara_Status st, const char *what, elara_State *S) {
    if (st != ELARA_OK) {
        const char *err = elara_last_error(S);
        fprintf(stderr, "%s failed: status=%d err=%s\n", what, (int)st,
                err ? err : "(null)");
        abort();
    }
}

int main(void) {
    elara_Config cfg = elara_default_config();
    cfg.capabilities = ELARA_CAP_SAFE_DEFAULT | ELARA_CAP_STDIO;
    cfg.gas_limit = 100000;
    cfg.mem_limit_bytes = 8u * 1024u * 1024u;

    elara_State *S = elara_open_with_config(&cfg);
    assert(S != NULL);

    /* Register a host native and invoke it through the C call API. */
    elara_Value native = elara_val_native_fn(S, host_mul, NULL);
    assert(!elara_is_nil(native));
    elara_set_global(S, "host_mul", native);

    elara_Value args[2];
    args[0] = elara_val_i48(6);
    args[1] = elara_val_i48(7);
    elara_Value results[1];
    results[0] = elara_val_nil();
    uint16_t nret = 0;
    expect_ok(elara_call(S, native, args, 2, results, 1, &nret), "host_mul", S);
    assert(nret == 1);
    assert(elara_to_int(results[0]) == 42);

    /* Managed userdata with a finalizer that runs on GC sweep. */
    static const elara_UserdataDesc box_desc = {
        "box",
        BOX_TAG,
        box_finalize,
        NULL,
    };
    elara_Value box_val = elara_val_nil();
    void *payload = elara_new_userdata(S, sizeof(int64_t), &box_desc, &box_val);
    assert(payload != NULL);
    *(int64_t *)payload = (int64_t)0xDEADBEEF;
    assert(elara_check_userdata(S, box_val, BOX_TAG) == payload);

    elara_set_global(S, "box", box_val);
    elara_set_global(S, "box", elara_val_nil());
    elara_gc_collect(S);
    assert(g_finalizer_hits == 1);

    /* Gas sandbox: an infinite loop must terminate with ELARA_ERR_GAS. */
    elara_set_gas_limit(S, 64);
    elara_Value ignored = elara_val_nil();
    elara_Status gas_st = elara_eval(S, "gas", "while true { }", &ignored);
    assert(gas_st == ELARA_ERR_GAS);

    size_t live = elara_get_bytes_allocated(S);
    assert(live > 0);
    assert(elara_get_gas_remaining(S) == 0);

    elara_close(S);

    /* Memory ceiling: freeze the heap and refuse further userdata growth. */
    cfg = elara_default_config();
    cfg.capabilities = ELARA_CAP_SAFE_DEFAULT | ELARA_CAP_STDIO;
    cfg.gas_limit = 1000000;
    cfg.mem_limit_bytes = 4096;
    S = elara_open_with_config(&cfg);
    assert(S != NULL);
    live = elara_get_bytes_allocated(S);
    elara_set_mem_limit(S, live);
    elara_Value blocked = elara_val_nil();
    void *oom = elara_new_userdata(S, 2048, &box_desc, &blocked);
    assert(oom == NULL);

    elara_close(S);
    puts("c_ffi_sandbox_host: ok");
    return 0;
}
