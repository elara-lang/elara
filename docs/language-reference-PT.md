# Referência da Linguagem

Referência do programador para a linguagem-fonte Elara, valores de runtime e a
biblioteca padrão intrínseca. Os exemplos usam a mesma sintaxe aceita por
`bin/elara` e `bin/elarac`.

## Regras Lexicais

Arquivos-fonte são UTF-8. Um BOM UTF-8 opcional e um shebang Unix (`#!…`) na
primeira linha são ignorados. Comentários de linha começam com `//`. Comentários
de bloco aninham com `/* … */`.

Identificadores são letras ASCII, dígitos e `_`, e não podem começar com dígito.
Palavras-chave são reservadas e não podem ser religadas:

```
let const fn if else while for in return break defer yield
import export as true false nil
```

Literais de string usam `"…"` com escapes (`\n`, `\t`, `\\`, `\"`, `\u{…}`).
Segmentos de interpolação adjacentes escrevem `"text {expr} more"`. Strings com
aspas triplas permitem aspas aninhadas sem escape.

Literais inteiros aceitam decimal, radices `0x` / `0o` / `0b` e separadores `_`.
Floats usam ponto decimal e expoentes opcionais. Intervalos inclusivos usam
`a..b`; a forma com passo `a..b..s` é reservada para laços `for` numéricos.

## Tipos e Valores

Valores de runtime são células etiquetadas de 8 bytes:

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

`type(v)` devolve o nome do tipo como string. `tostring` / `tonumber` convertem com
radix opcional para `tonumber`. Apenas `nil` e `false` são falsy.

## Operadores

Aritméticos: `+ - * / %` (e `-` unário). Ops bit a bit ficam no módulo `bit`.
Comparação: `== != < <= > >=`. Lógicos: `and or not` (curto-circuito). Comprimento:
`#t` é O(1) para a partição array. Indexação: `t[i]`, `t.field`. Chamadas:
`f(a, b)`.

Atribuição composta atualiza um local ou upvalue no lugar: `+= -= *= /=`.

Metamétodos consultados em tabelas incluem `__add`, `__tostring`, `__index`,
`__newindex`, `__call` e `__close`.

## Ligações

```
let x = 1
const PI = 3.141592653589793
x += 2
```

`let` é mutável; `const` rejeita atribuição após a inicialização. Identificadores
não declarados são erros de compilação. Sombreamento em blocos `{ }` aninhados é
permitido. Autoinicialização (`let x = x`) é rejeitada.

Multi-ligação desempacota resultados de chamada em locais consecutivos:

```
let ok, err = pcall(fn() { panic("x") })
```

## Controle de Fluxo

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

`for` sobre um intervalo inteiro compila para opcodes de laço especializados.
`break` deixa o laço mais interno.

## Defer

```
{
    defer { print("second") }
    defer { print("first") }
    /* body */
}
```

`defer` registra um slot fechável. Sair do bloco (normalmente ou via desenrolamento
de erro) executa os hooks registrados em ordem LIFO. Tabelas com metamétodo
`__close` participam do mesmo protocolo quando marcadas como fecháveis.

## Funções e Closures

```
fn add(a, b) {
    return a + b
}

let make = fn(n) {
    return fn() { n += 1; return n }
}
```

Funções aninhadas capturam ligações `let` envolventes como upvalues. Capturas de
variáveis de laço `for` são por iteração quando ligadas com `let` dentro do corpo.
`return f(...)` pode tornar-se chamada de cauda quando nenhum `defer` está ativo no
quadro.

## Tabelas e Metatabelas

```
let arr = {10, 20, 30}     /* array part, indices 0..2 */
let map = {x: 1, y: 2}     /* hash part */
arr[0]
map.x
#arr                       /* => 3 */

let mt = {__tostring: fn(self) { return "point" }}
table.set_metatable(map, mt)
```

Índices de array são baseados em 0. Auxiliares `table.*` honram normalização de
índice negativo. Tabelas fracas usam
`table.set_metatable(t, {__mode: "k"|"v"|"kv"})`.

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

Fibers são stackful e cooperativas. `yield` suspende a fiber atual; `fiber.resume`
a continua e devolve valores yielded ou finais. Strings de status são
`"suspended"`, `"running"` e `"dead"`.

## Módulos

```
/* lib/math_ext.ela */
export fn add(a, b) { return a + b }
export const VERSION = 1

/* main */
import "lib/math_ext" as mx
print(mx.add(2, 3))
```

`export let` / `export const` / `export fn` ligam um local e o publicam na tabela de
exports do módulo. `import "spec" as alias` carrega uma tabela de exports singleton
(em cache). Imports circulares falham com erro de módulo. Módulos intrínsecos
(`math`, `string`, `table`, `bit`, `sys`, `fiber`, `gc`) resolvem sem arquivos.
Um sufixo `.ela` ou `.elac` no especificador é removido, então `math.ela` continua
sendo o intrínseco `math`.

### Caminho de busca

O `elara` procura um nome lógico nesta ordem. O primeiro acerto vence.

1. O diretório do script. `-e` e o REPL usam `.`.
2. O diretório atual, quando ele ainda não está na lista.
3. Cada `--lib <dir>`, na ordem da linha de comando.
4. Cada `lib_path` do `elara.ini` local, na ordem do arquivo.
5. Cada diretório em `ELARA_PATH`, separado por `:`. Segmentos vazios são ignorados.
6. Cada `lib_path` do ini de sistema.

Dentro de um diretório os candidatos são `nome.elac`, `nome.ela`,
`nome/init.elac`, `nome/init.ela` e, quando bibliotecas nativas são permitidas,
`nome.so`. Bytecode tem preferência sobre fonte no mesmo diretório. Não há
candidato sem sufixo.

Uma árvore pequena:

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

`proj/elara.ini` é o arquivo da seção seguinte. De qualquer diretório de
trabalho:

```bash
./bin/elara proj/main.ela
```

A mesma biblioteca carrega sem arquivo ini:

```bash
./bin/elara --lib proj/libs proj/main.ela
ELARA_PATH=proj/libs ./bin/elara proj/main.ela
```

`--lib` pode repetir. Um diretório posterior só é consultado quando todos os
anteriores falharam aquele nome.

### Imports relativos

`import "./geom" as g` e `import "../util" as u` resolvem a partir do diretório
do arquivo que contém o import e só procuram na raiz de busca daquele arquivo.
Na árvore acima, `vec/init.ela` foi achado em `proj/libs`, então `./geom` vira
`vec/geom` e abre `proj/libs/vec/geom.ela`.

Um `..` que sai da raiz, um caminho absoluto (`/…`) ou uma barra invertida é
rejeitado. Um import relativo num trecho sem âncora de arquivo (um buffer que o
host carregou sem `elara_set_module_anchor`) falha do mesmo modo.

### elara.ini

Só o comando `elara` lê arquivos ini. O `elarac` não lê, e um host embarcado
também não, a menos que copie os diretórios para
`elara_add_module_search_dir`. Um arquivo ausente é ignorado. Um erro de
sintaxe encerra a CLI antes do script.

Crie `elara.ini` ao lado do script (para `-e` e o REPL, ao lado do diretório
atual):

```
; bibliotecas do projeto
# um diretório por linha; a ordem do arquivo é a ordem de busca
lib_path = libs
lib_path = ../shared
```

`lib_path` é a única chave. Um valor relativo é juntado ao diretório do próprio
ini, não ao diretório atual. `libs` em `proj/elara.ini` é `proj/libs` mesmo
quando o `elara` é lançado de outro lugar. `..` nesse valor é permitido e não
é canonicalizado. O diretório não precisa existir quando a CLI inicia.

Comentários são linhas inteiras que, depois do trim, começam com `;` ou `#`.
Linhas em branco são puladas. Um cabeçalho `[seção]`, uma linha sem `=`, uma
chave desconhecida ou um `lib_path` vazio é erro. O arquivo não define gás, o
teto do heap nem capacidades. `--sandbox` continua sem ler `.ela`, `.elac` e
`.so`.

O arquivo de sistema é `/etc/elara/elara.ini`. Crie-o do mesmo jeito quando
todos os usuários devem ver uma raiz compartilhada:

```
lib_path = /usr/local/share/elara
```

`ELARA_SYS_INI=/outro/elara.ini` lê esse caminho. `ELARA_SYS_INI=` (vazio) pula
o arquivo de sistema. `ELARA_SYS_INI` ausente significa
`/etc/elara/elara.ini`.

### Bibliotecas nativas

Um `nome.so` no caminho de busca é o último candidato daquele diretório. O
processo `elara` padrão pode abri-lo. `--sandbox` o esconde, e o import falha
como módulo não encontrado, não como erro de capacidade. Como escrever e
compilar o `.so` está no guia C-FFI.

## Linha de comando

O `elara` executa fonte ou bytecode. Caminhos depois do script viram `sys.args`
quando a capacidade `sys` está ligada.

```bash
./bin/elara script.ela
./bin/elara -e 'print(1)'
./bin/elara --lib libs --lib /opt/extra script.ela
./bin/elara --sandbox --gas 100000 --mem-limit 1048576 script.ela
./bin/elara --disasm script.elac
```

| Opção | Efeito |
| --- | --- |
| `-e <code>` | Avalia um trecho e sai. O diretório âncora é `.` |
| `--gas <N>` | Orçamento de instruções. O esgotamento para o script |
| `--mem-limit <N>` | Teto do heap em bytes |
| `--sandbox` | Intrínsecos seguros mais `print`. Sem bibliotecas de disco e sem `.so` |
| `--lib <dir>` | Acrescenta um diretório de busca. Repita a flag para acrescentar mais |
| `--disasm` | Imprime o bytecode antes de executar |
| `-v`, `--version` | Imprime a versão |
| `-h`, `--help` | Imprime a lista de opções |

O `elarac` compila um arquivo `.ela`. Ele não procura bibliotecas e não lê
`elara.ini`.

```bash
./bin/elarac -o script.elac script.ela
./bin/elarac --check script.ela
./bin/elarac --strip --disasm script.ela
```

| Opção | Efeito |
| --- | --- |
| `-o <out.elac>` | Caminho de saída. O padrão troca `.ela` por `.elac` |
| `--strip` | Liga o bit strip_debug_info no cabeçalho `.elac` |
| `--check` | Compila e verifica, sem gravar arquivo |
| `--disasm` | Imprime o protótipo compilado |
| `-v`, `--version` | Imprime a versão |
| `-h`, `--help` | Imprime a lista de opções |

## Erros

```
assert(cond, "message")
panic("message")
let ok, err = pcall(fn() { panic("boom") })
```

`pcall` devolve `true, …` em sucesso ou `false, message` em falha. Hooks abertos de
`defer` / `__close` ainda executam durante o desenrolamento. Esgotamento de gás do
host ignora `pcall` do script e aparece como `ELARA_ERR_GAS` no limite do host.

## Biblioteca Padrão

Módulos intrínsecos são importados automaticamente nos prólogos de script quando a
bitmask de capacidades do host os permite.

```
print(math.abs(-3))          /* 3 */
print(string.slice("elara", 1, 4))  /* lar */
```

### math

Constantes: `pi`, `huge`, `max_i48`, `min_i48`, `max_int`, `min_int`.

Funções: `abs`, `floor`, `ceil`, `round`, `idiv`, `sqrt`, `sin`, `cos`, `tan`,
`asin`, `acos`, `atan2`, `exp`, `log`, `min`, `max`, `clamp`, `random`, `randomseed`.

### string

Indexação de bytes baseada em 0 com normalização de índice negativo:
`len`, `slice`, `byte`, `char`, `find`, `replace`, `split`, `upper`, `lower`,
`trim`, `starts_with`, `ends_with`, `rep`.

### table

Ops de array baseadas em 0: `push`, `pop`, `insert`, `remove`, `concat`, `sort`,
`keys`, `values`, `clone`, `set_metatable`, `get_metatable`.

### bit

Ops inteiras 48/64 bits: `band`, `bor`, `bxor`, `bnot`, `shl`, `shr`, `ushr`, `rol`,
`ror`, `count_ones`, `leading_zeros`, `trailing_zeros`.

### sys

Sempre: `clock_ns()`, `mem_used()`, `gas_remaining()`, `version`.
Com `ELARA_CAP_SYS_ENV`: `env(name)`, `args`, `exit(code?)`.

### fiber

`create`, `resume`, `status`, além da declaração `yield`.

### gc

`collect()` executa um ciclo completo. `step(budget_us?)` executa trabalho
incremental sob o orçamento de pausa e indica se o coletor voltou a `Pause`.

## Globais Principais

| Name       | Role                                      |
| ---------- | ----------------------------------------- |
| `print`    | Tab-separated stdout line (`ELARA_CAP_STDIO`) |
| `type`     | Type name string                          |
| `tostring` | String conversion / `__tostring`          |
| `tonumber` | Parse / convert numbers                   |
| `assert`   | Truth check or runtime error              |
| `panic`    | Unconditional runtime error               |
| `pcall`    | Protected call                            |
