#ifndef ELARA_H
#define ELARA_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct elara_State       elara_State;
typedef struct elara_CallContext elara_CallContext;
typedef uint64_t                 elara_Value;

typedef enum elara_Status {
    ELARA_OK          = 0,
    ELARA_YIELD       = 1,
    ELARA_ERR_SYNTAX  = 2,
    ELARA_ERR_RUNTIME = 3,
    ELARA_ERR_OOM     = 4,
    ELARA_ERR_GAS     = 5,
    ELARA_ERR_VERIFY  = 6,
    ELARA_ERR_MODULE  = 7
} elara_Status;

/* Sandbox Capability Bitmask */
#define ELARA_CAP_NONE   (0u)
#define ELARA_CAP_MATH   (1u << 0)
#define ELARA_CAP_STRING (1u << 1)
#define ELARA_CAP_TABLE  (1u << 2)
#define ELARA_CAP_BIT    (1u << 3)
#define ELARA_CAP_FIBER  (1u << 4)
#define ELARA_CAP_SYS_ENV (1u << 5)
#define ELARA_CAP_STDIO  (1u << 6)
#define ELARA_CAP_SAFE_DEFAULT \
    (ELARA_CAP_MATH | ELARA_CAP_STRING | ELARA_CAP_TABLE | ELARA_CAP_BIT | ELARA_CAP_FIBER)
#define ELARA_CAP_ALL    (0xFFFFFFFFu)

typedef void* (*elara_Alloc)(void* ud, void* ptr, size_t old_size, size_t new_size);
typedef elara_Status (*elara_NativeFn)(elara_State* S, elara_CallContext* ctx);
typedef void (*elara_Finalizer)(elara_State* S, void* data, size_t size);
typedef void (*elara_TraceFn)(elara_State* S, void* data);

typedef struct elara_UserdataDesc {
    const char*     type_name;
    uint32_t        type_tag;
    elara_Finalizer finalize;
    elara_TraceFn   trace;
} elara_UserdataDesc;

typedef struct elara_Config {
    elara_Alloc alloc_fn;          /* NULL uses default std::alloc::System wrapper */
    void*       alloc_ud;
    size_t      mem_limit_bytes;   /* 0 = unlimited */
    uint64_t    gas_limit;         /* 0 = unlimited */
    uint32_t    pause_budget_us;   /* Default 500 microseconds */
    uint32_t    capabilities;      /* Bitmask of ELARA_CAP_* */
    uint64_t    hash_seed;         /* Seed for FNV-1a / Robin Hood hashing */
} elara_Config;

/* Lifecycle & Configuration */
elara_Config elara_default_config(void);
elara_State* elara_open(void);
elara_State* elara_open_with_config(const elara_Config* cfg);
void         elara_close(elara_State* S);

/* Sandbox Quotas & Telemetry */
void     elara_set_gas_limit(elara_State* S, uint64_t gas);
uint64_t elara_get_gas_remaining(const elara_State* S);
void     elara_set_mem_limit(elara_State* S, size_t max_bytes);
size_t   elara_get_bytes_allocated(const elara_State* S);
void     elara_gc_collect(elara_State* S);
void     elara_gc_step(elara_State* S, uint32_t budget_us);

/* Compilation & Execution */
elara_Status elara_eval(elara_State* S, const char* name, const char* source, elara_Value* out_result);
elara_Status elara_compile_to_bytecode(
    elara_State* S,
    const char*  name,
    const char*  source,
    uint8_t**    out_buf,
    size_t*      out_len
);
void         elara_free_buffer(elara_State* S, uint8_t* buf, size_t len);
elara_Status elara_load_bytecode(elara_State* S, const uint8_t* buf, size_t len, elara_Value* out_closure);
elara_Status elara_call(
    elara_State*       S,
    elara_Value        callable,
    const elara_Value* args,
    uint16_t           num_args,
    elara_Value*       results,
    uint16_t           max_results,
    uint16_t*          out_num_results
);

/* Direct 0-Based CallContext API for C Native Functions */
uint16_t    elara_argc(const elara_CallContext* ctx);
elara_Value elara_arg(const elara_CallContext* ctx, uint16_t index);
void        elara_return(elara_CallContext* ctx, elara_Value val);
void        elara_return_multi(elara_CallContext* ctx, const elara_Value* vals, uint16_t count);
void*       elara_context_userdata(const elara_CallContext* ctx);

/* Value Constructors & Inspectors */
elara_Value elara_val_nil(void);
elara_Value elara_val_bool(bool b);
elara_Value elara_val_int(elara_State* S, int64_t i);
elara_Value elara_val_i48(int64_t i);
elara_Value elara_val_float(double d);
elara_Value elara_val_string(elara_State* S, const char* str, size_t len);
elara_Value elara_val_native_fn(elara_State* S, elara_NativeFn fn, void* userdata);
void*       elara_new_userdata(elara_State* S, size_t size, const elara_UserdataDesc* desc, elara_Value* out_val);
void*       elara_check_userdata(elara_State* S, elara_Value val, uint32_t expected_type_tag);

bool        elara_is_nil(elara_Value v);
bool        elara_is_int(elara_Value v);
bool        elara_is_float(elara_Value v);
bool        elara_is_string(elara_Value v);
int64_t     elara_to_int(elara_Value v);
double      elara_to_float(elara_Value v);
const char* elara_to_string(elara_State* S, elara_Value v, size_t* out_len);

/* Globals / Module Bindings, Custom Module Loader & Error Reporting */
typedef elara_Status (*elara_ModuleLoaderFn)(
    elara_State* S,
    const char*  specifier,
    const uint8_t** out_bytes,
    size_t*      out_len,
    bool*        out_is_bytecode,
    void*        ud
);

void        elara_set_module_loader(elara_State* S, elara_ModuleLoaderFn loader, void* ud);
void        elara_set_global(elara_State* S, const char* name, elara_Value val);
elara_Value elara_get_global(elara_State* S, const char* name);
const char* elara_last_error(const elara_State* S);

#ifdef __cplusplus
}
#endif

#endif /* ELARA_H */
