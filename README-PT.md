# Elara

Linguagem de script embarcável de alta performance, VM de bytecode baseada em registradores, compilador AOT e runtime FFI C99/C++ escritos em Rust.

## Destaques

- Compacta floats IEEE-754, inteiros de 48 bits com sinal, booleanos, nil e ponteiros GC em valores NaN-tagged de 8 bytes com promoção transparente para inteiros de 64 bits
- Executa bytecode de registradores de 32 bits com despacho fundido de comparação e salto, inline caching de 1 slot e otimização estrita de chamada de cauda
- Armazena arrays densos indexados em 0 junto a tabelas hash Robin Hood em um único cabeçalho de 64 bytes alinhado à linha de cache da CPU
- Interna strings UTF-8 curtas de até 64 bytes em O(1) enquanto calcula o hash de strings longas sob demanda para evitar pausas de alocação
- Impõe escopo léxico com declarações explícitas let e const, operadores de atribuição composta, interpolação aninhada de strings e limpeza determinística com defer em ordem LIFO
- Executa fibers cooperativas com pilha própria e coleta de lixo incremental tricolor com tabelas fracas sob orçamento determinístico de pausa em microssegundos
- Isola scripts não confiáveis com cotas de instruções em registrador de CPU, limite rígido de memória heap e módulos da biblioteca padrão controlados por capacidades
- Embarca em aplicações C99, C++ ou Rust através de biblioteca estática ou dinâmica sem dependências externas e uma API C direta baseada em slots

## Visão Geral

Elara é uma linguagem de script embarcável projetada do zero em Rust (Edition 2024) com zero dependências de crates externos. Ela combina o baixo consumo de memória e a portabilidade de ABI C das VMs embarcáveis clássicas com ergonomia moderna: indexação baseada em 0, blocos léxicos com `{ }`, verificação estática de escopo, gerenciamento determinístico de recursos com `defer` e sandboxing nativo.

## Pré-requisitos

- **Rust 1.85+** (`cargo`, `rustc`, `rustfmt`, `clippy`) — necessário para compilar a partir do código-fonte; [instale via rustup](https://rustup.rs/)
- **Compilador C99** (`cc` / `gcc` / `clang`) — opcional, utilizado para compilar as demonstrações de integração C-FFI

## Instalação

### Compilar a partir do Código-Fonte

```bash
git clone https://github.com/carlosrabelo/elara.git
cd elara
make build
```

Instale em `~/.local/bin` (padrão) ou globalmente no sistema em `/usr/local/bin` (sudo apenas para a cópia):

```bash
make install
make install-system
make uninstall
make uninstall-system
```

## Início Rápido

```bash
make build
./bin/elara --version
```

## Uso

### Executar um script ou iniciar o REPL interativo

```bash
./bin/elara script.ela
./bin/elara -e 'let x = 42; print("Value: {x}")'
./bin/elara
```

### Compilar código-fonte para bytecode binário AOT `.elac`

```bash
./bin/elarac -o script.elac script.ela
./bin/elara script.elac
```

### Executar com limites determinísticos de gás e memória

```bash
./bin/elara --sandbox --gas 100000 --mem-limit 1048576 script.ela
```

## Estrutura do Projeto

```
elara/               # Pacote Cargo Rust (VM principal, compilador, stdlib, C-FFI, testes)
elara/include/       # Cabeçalho público C99/C++ para host (elara.h)
demos/               # Programas de demonstração ponta a ponta em .ela e C99 FFI
docs/                # Guias bilíngues de arquitetura, referência da linguagem e C-FFI
bin/                 # Binários e bibliotecas compilados (elara, elarac, libelara.a, libelara.so)
.make/               # Scripts de automação de build, teste, qualidade e instalação
```

## Desenvolvimento

```bash
make build             # Compila em modo release e copia o binário para bin/elara
make test              # Executa cargo test
make quality           # Formata, executa clippy e roda os testes
make install           # Instala o binário em ~/.local/bin
make install-system    # Instala o binário em /usr/local/bin
make uninstall         # Remove de ~/.local/bin
make uninstall-system  # Remove de /usr/local/bin
```

## Licença

Este projeto é licenciado sob a Licença MIT — consulte o arquivo [LICENSE](LICENSE) para mais detalhes.

