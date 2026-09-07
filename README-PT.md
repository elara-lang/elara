# Elara

Linguagem de script embarcável de alta performance, VM de bytecode baseada em registradores, compilador AOT e runtime FFI C99/C++ escritos em Rust.

## Destaques

- Executa um script, um trecho inline ou o REPL com o comando elara
- Compila um arquivo-fonte para bytecode .elac antecipadamente com elarac
- Embarca o runtime em C99, C++ ou Rust por um cabeçalho C e uma biblioteca estática ou compartilhada
- Isola scripts não confiáveis com combustível de instruções, teto de heap e flags de capacidade
- Escreve scripts com let e const explícitos, tabelas indexadas em 0, limpeza com defer e fibers cooperativas
- Importa uma biblioteca do diretório do script, de --lib, de ELARA_PATH ou de elara.ini
- Carrega um módulo nativo registrado pelo host ou uma biblioteca compartilhada no caminho de busca
- Compila o compilador e a VM só com a std do Rust, sem crates externos

## Visão Geral

Elara é uma linguagem de script embarcável escrita em Rust (Edition 2024), sem crates externos. Os scripts usam indexação em 0, blocos `{ }`, `let` e `const`, `defer` e um sandbox para código não confiável.

A linguagem, o carregamento de bibliotecas e a API C estão na [referência da linguagem](docs/language-reference-PT.md), no [guia C-FFI](docs/c-ffi-guide-PT.md) e no [guia de arquitetura](docs/architecture-PT.md).

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

Instale os binários em `~/.local/bin` (padrão) ou globalmente em `/usr/local/bin` (sudo apenas para a cópia). Instale as bibliotecas e o cabeçalho C separadamente em `~/.local/lib` + `~/.local/include` (ou `/usr/local/lib` + `/usr/local/include`):

```bash
make install
make install-system
make install-libs
make install-libs-system
make uninstall
make uninstall-system
make uninstall-libs
make uninstall-libs-system
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

### Executar a suíte de demonstrações ponta a ponta

```bash
./demos/run_demos.sh
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
make build                  # Compila em release; copia elara, elarac, libelara.a, libelara.so para bin/
make test                   # Executa cargo test
make quality                # Formata, executa clippy e roda os testes
make install                # Instala elara e elarac em ~/.local/bin
make install-system         # Instala elara e elarac em /usr/local/bin
make install-libs           # Instala libelara.a, libelara.so e elara.h em ~/.local
make install-libs-system    # Instala bibliotecas e cabeçalho em /usr/local
make uninstall              # Remove binários de ~/.local/bin
make uninstall-system       # Remove binários de /usr/local/bin
make uninstall-libs         # Remove bibliotecas e cabeçalho de ~/.local
make uninstall-libs-system  # Remove bibliotecas e cabeçalho de /usr/local
```

## Licença

Este projeto é licenciado sob a Licença MIT — consulte o arquivo [LICENSE](LICENSE) para mais detalhes.

