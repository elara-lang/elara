# Arquitetura

Elara é uma máquina virtual de bytecode baseada em registradores e um compilador AOT
escritos em Rust (Edition 2024) com zero dependências de crates externos. Este guia
descreve os layouts de dados e planos de controle compartilhados por todos os
subsistemas.

## Representação de Valores

Cada valor de runtime é um único `u64` empacotado com etiquetagem Quiet-NaN IEEE-754.
Floats finitos preservam o padrão de bits nativo. Cargas que não são float vivem no
espaço Quiet-NaN para que a VM discrimine etiquetas com poucas máscaras.

### Layout NaN-Tagging

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

Ponteiros de objeto ficam nos 48 bits baixos e devem permanecer alinhados em 8 bytes
para que os bits de etiqueta nunca colidam com bits de endereço nas plataformas
atuais.

### Promoção de Inteiros

Inteiros que cabem no intervalo assinado de 48 bits (`[-2^47, 2^47 - 1]`) permanecem
inline. Valores fora desse intervalo são transparentemente encapsulados como objetos
heap `ObjBoxedI64`. Aritmética, operações bit a bit, hashing e igualdade unificam
`i48` inline, `i64` encapsulado e `f64` inteiro exato para que scripts nunca observem
a divisão de representação.

## Strings

Elara usa um pool de strings em duas camadas:

```
 short  (len <= 64)  : eagerly interned, content-addressed, O(1) identity compare
 long   (len >  64)  : heap object with lazily computed FNV-1a hash on first use
```

Strings curtas dominam identificadores, nomes de campos e fragmentos de mensagem.
Strings longas evitam o custo de hashing até entrarem em uma tabela hash ou
precisarem de um hash estável.

## Tabelas

`ObjTable` cabe em uma única linha de cache de 64 bytes da CPU e hibridiza duas
partições:

```
 +---------------- ObjTable (64 bytes) ----------------+
 | header | metatable | array ptr/len/cap | hash ptr… |
 +-----------------------------------------------------+
         |                         |
         v                         v
   dense 0-based vector      Robin Hood open addressing
   (push/pop/O(1) #t)        (backward-shift delete)
```

Índices de array são estritamente baseados em 0 (`0 .. #t - 1`). Índices negativos
normalizam a partir do fim nos auxiliares da biblioteca padrão. Chaves hash rejeitam
`nil` e `NaN`. Metatabelas oferecem sobrecarga de operadores (`__add`, `__tostring`,
`__index`, `__newindex`, `__call`, `__close`) e modos fracos via `__mode`.

## Máquina Virtual

O interpretador é uma máquina de registradores. Cada quadro de chamada endereça uma
janela da pilha da fiber por índice base em vez de ponteiros brutos, para que o
crescimento da pilha nunca invalide quadros.

### Quadros de Chamada

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

Opcodes são instruções de largura fixa de 32 bits (`iABC`, `iABx`, `iAsBx`, `iSj`).
Cargas e armazenamentos de campo usam um inline cache de um slot que invalida quando
a tabela rehash.

### Chamadas de Cauda e Upvalues

`return f(...)` é reescrito para `OP_TAILCALL` quando o escopo da função não tem
slots ativos de `defer` / `__close`, mantendo máquinas de estado recursivas em
quadros O(1).

Upvalues abertos apontam para slots vivos da pilha. Fechar um quadro os migra para
uma célula heap `closed`. Múltiplas closures que capturam o mesmo local compartilham
um objeto upvalue para que mutações permaneçam coerentes antes e depois do retorno.

### Defer e Close

`defer { … }` registra um slot fechável. `OP_CLOSE` drena os slots registrados em
ordem LIFO: closures de defer executam primeiro, depois os hooks `__close` da
metatable. O desenrolamento via `pcall` executa o mesmo protocolo LIFO antes de o
erro propagar ao host.

## Coletor de Lixo

Elara usa um coletor mark-and-sweep incremental tricolor com orçamento de pausa
configurável (padrão 500 µs por `gc.step`).

### Fases

```
 Pause ──► Propagate ──► Atomic ──► SweepString ──► SweepObj ──► Pause
              │              │
              │              └─ clear weak entries, flip white
              └─ gray worklist + barrier-dirtied tables
```

### Barreiras de Escrita

Barreiras de escrita reversas sombreiam objetos brancos inseridos em tabelas pretas
durante Propagate, preservando o invariante incremental sem parar o mutator para um
percurso completo do heap.

### Tabelas Fracas

Um campo de metatable `__mode = "k" | "v" | "kv"` marca chaves e/ou valores como
fracos. Entradas fracas cujo referente ainda é branco após Propagate são limpas em
Atomic para que caches evictem objetos inalcançáveis sem retê-los como raízes fortes.

## Elara vs Lua

| Tópico             | Elara 1.0                                      | Lua 5.5                                      |
| ------------------ | ---------------------------------------------- | -------------------------------------------- |
| Indexação          | Arrays estritamente baseados em 0              | Arrays baseados em 1                         |
| Inteiros           | i48 inline + `ObjBoxedI64` transparente        | Subtipo inteiro com semântica separada       |
| Valores            | Etiquetas Quiet-NaN de 8 bytes                 | Uniões etiquetadas / NaN conforme o build    |
| Tabelas            | Híbrido 64 bytes array + mapa Robin Hood       | Partes array + hash, layout diferente        |
| Escopo             | `let` / `const` explícitos, banimento estático | Globais implícitos por padrão                |
| Limpeza            | `defer` + `__close` LIFO                       | Variáveis to-be-closed `<close>`             |
| Concorrência       | Fibers cooperativas com pilha                  | Corrotinas (assimétricas)                    |
| Sandbox            | Gás + teto de heap + bitmask `ELARA_CAP_*`     | Hooks de debug / política do host            |
| API de Host        | `elara.h` C99, `CallContext` 0-based           | API C baseada em pilha                       |
| Dependências       | Apenas Rust `std`                              | Runtime C                                    |
