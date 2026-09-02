# Guia C-FFI

Este guia mostra como embutir Elara a partir de hosts C99, C++ ou Rust através de
`elara/include/elara.h` e `bin/libelara.a` / `bin/libelara.so`.

## Abrindo um Estado

```c
#include "elara.h"

elara_State *S = elara_open();
/* … */
elara_close(S);
```

`elara_open` usa `elara_default_config()`. Prefira `elara_open_with_config` quando
precisar de cotas ou de um conjunto restrito de capacidades.

## Configuração e Alocadores

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

Alocadores customizados implementam `elara_Alloc` e podem rotear cada byte do heap
por uma arena do host. Após abrir, `elara_set_gas_limit` e `elara_set_mem_limit`
ajustam cotas dinamicamente. `elara_gc_collect` e `elara_gc_step` acionam o coletor
a partir do host.

## Avaliando Fonte e Bytecode

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

`elara_compile_to_bytecode` verifica a árvore de protos antes da serialização.
Closures carregadas são valores chamáveis ordinários.

## Chamando Valores

```c
elara_Value args[2] = { elara_val_i48(20), elara_val_i48(22) };
elara_Value outs[4];
uint16_t nret = 0;
st = elara_call(S, callable, args, 2, outs, 4, &nret);
```

Slots de argumento e resultado são baseados em 0. `max_results` limita quantos
retornos são copiados para `outs`; `out_num_results` informa quantos foram
produzidos.

## Funções Nativas

Nativos do host usam a API direta `elara_CallContext` (sem embaralhamento de pilha):

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

`elara_argc`, `elara_arg`, `elara_return` e `elara_return_multi` são os únicos pontos
de entrada necessários dentro de um nativo. Userdata opcional do host está
disponível via `elara_context_userdata`.

## Userdata e Finalizadores

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

Quando o userdata se torna inalcançável, o GC incremental invoca `finalize` durante
o sweep. `elara_check_userdata` verifica a etiqueta de tipo antes de o host tocar o
payload.

## Sandboxing

Bits de capacidade controlam módulos intrínsecos:

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

Combustível de instruções (`gas_limit`) retorna `ELARA_ERR_GAS` quando esgotado.
Tetos de heap (`mem_limit_bytes`) retornam `ELARA_ERR_OOM` após uma coleta de
emergência falhar em liberar memória suficiente. Combine ambos com uma máscara
estreita de capacidades para plugins não confiáveis.
`ELARA_CAP_SAFE_DEFAULT` não inclui `ELARA_CAP_NATIVE`. `--sandbox` é essa
máscara mais `ELARA_CAP_STDIO`, então não lê bibliotecas do disco nem abre um
`.so`.

## Módulos Nativos

Um módulo nativo é uma tabela de exports produzida em C. Ele fica em cache sob
o mesmo nome lógico de um módulo em fonte. Um opener registrado para `math` (ou
qualquer outro intrínseco) é guardado e nunca chamado.

### Openers registrados

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

`elara_new_table` devolve nil em falta de memória e não dispara erro.
`elara_table_set` interna `key` e grava o campo. Uma chave NULL ou um receptor
que não é tabela devolve `ELARA_ERR_RUNTIME`. Esse registro funciona com ou sem
`ELARA_CAP_NATIVE`. A VM não libera `ud`. `elara_add_module_search_dir`
acrescenta um diretório de busca e `elara_set_module_anchor` define o diretório
de entrada usado por imports relativos de um módulo que o host entregou em
bytes.

### Bibliotecas compartilhadas

Com `ELARA_CAP_NATIVE` ligado, um arquivo `nome.so` no caminho de busca é
aberto com `dlopen` e precisa exportar:

```c
elara_Status elara_open_lib(elara_State *S, elara_Value *out_exports);
```

O handle permanece mapeado até `elara_close` terminar de liberar o heap. Uma
abertura que falha é fechada na hora. `bin/elara` é ligado com
`--export-dynamic` para a biblioteca chamar `elara_*`.
`demos/09_native_library.c` é um exemplo pequeno carregado por
`demos/09_native_library.ela`.

## Hooks de Debug

```c
void on_event(elara_State *S, const elara_DebugEvent *ev, void *ud) {
    (void)S; (void)ud;
    /* inspect ev->event, ev->line, ev->fn_name, … */
}

elara_set_hook(S, on_event, NULL,
               ELARA_HOOK_CALL | ELARA_HOOK_RET | ELARA_HOOK_LINE | ELARA_HOOK_COUNT,
               1000);
```

`elara_debug_get_frame`, `elara_debug_get_local` e `elara_debug_set_local` expõem
quadros vivos para ferramentas no estilo DAP. Hooks de contagem disparam a cada
`count_interval` instruções quando `ELARA_HOOK_COUNT` está ativo.

## Exemplo Completo

Consulte `demos/08_c_ffi_sandbox_host.c` para um programa C99 autocontido que
registra um nativo, anexa userdata com finalizer e verifica o sandbox de gás e
memória. Compile-o com:

```bash
make build
cc -std=c99 -Wall -Wextra -Werror \
  -Ielara/include demos/08_c_ffi_sandbox_host.c bin/libelara.a \
  -lm -lpthread -ldl -o /tmp/elara_ffi_demo
/tmp/elara_ffi_demo
```
