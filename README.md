# Elara

High-performance embeddable scripting language, register-based bytecode VM, AOT compiler, and C99/C++ FFI runtime written in Rust.

## Highlights

- Run a script, an inline snippet, or the REPL with the elara command
- Compile a source file to .elac bytecode ahead of time with elarac
- Embed the runtime in C99, C++, or Rust through one C header and a static or shared library
- Sandbox untrusted scripts with instruction fuel, a heap ceiling, and capability flags
- Write scripts with explicit let and const, 0-based tables, defer cleanup, and cooperative fibers
- Import a library from the script directory, --lib, ELARA_PATH, or elara.ini
- Load a native module the host registered, or a shared library on the search path
- Build the compiler and the VM with Rust std only and no external crates

## Overview

Elara is an embeddable scripting language written in Rust (Edition 2024) with no external crates. Scripts use 0-based indexing, `{ }` blocks, `let` and `const`, `defer`, and a sandbox for untrusted code.

The language, library loading, and the C API are documented in [the language reference](docs/language-reference.md), [the C FFI guide](docs/c-ffi-guide.md), and [the architecture guide](docs/architecture.md).

## Prerequisites

- **Rust 1.85+** (`cargo`, `rustc`, `rustfmt`, `clippy`) — required to build from source; [install via rustup](https://rustup.rs/)
- **C99 compiler** (`cc` / `gcc` / `clang`) — optional, used to compile C host FFI integration demos

## Installation

### Build from Source

```bash
git clone https://github.com/carlosrabelo/elara.git
cd elara
make build
```

Install binaries to `~/.local/bin` (default), or system-wide to `/usr/local/bin` (sudo only for the copy). Install libraries and the C header separately to `~/.local/lib` + `~/.local/include` (or `/usr/local/lib` + `/usr/local/include`):

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

## Quick Start

```bash
make build
./bin/elara --version
```

## Usage

### Run a script or start the interactive REPL

```bash
./bin/elara script.ela
./bin/elara -e 'let x = 42; print("Value: {x}")'
./bin/elara
```

### Compile source to AOT `.elac` binary bytecode

```bash
./bin/elarac -o script.elac script.ela
./bin/elara script.elac
```

### Execute with deterministic gas and memory limits

```bash
./bin/elara --sandbox --gas 100000 --mem-limit 1048576 script.ela
```

### Run the end-to-end demo suite

```bash
./demos/run_demos.sh
```

## Project Layout

```
elara/               # Rust Cargo package (core VM, compiler, stdlib, C-FFI, tests)
elara/include/       # Public C99/C++ host header (elara.h)
demos/               # End-to-end .ela and C99 FFI demonstration programs
docs/                # Bilingual architecture, language reference, and C-FFI guides
bin/                 # Compiled binaries and libraries (elara, elarac, libelara.a, libelara.so)
.make/               # Build, test, quality, and install automation scripts
```

## Development

```bash
make build                  # Release-build; copy elara, elarac, libelara.a, libelara.so to bin/
make test                   # Run cargo test
make quality                # Format, clippy, and test
make install                # Install elara and elarac to ~/.local/bin
make install-system         # Install elara and elarac to /usr/local/bin
make install-libs           # Install libelara.a, libelara.so, elara.h to ~/.local
make install-libs-system    # Install libraries and header to /usr/local
make uninstall              # Remove binaries from ~/.local/bin
make uninstall-system       # Remove binaries from /usr/local/bin
make uninstall-libs         # Remove libraries and header from ~/.local
make uninstall-libs-system  # Remove libraries and header from /usr/local
```

## License

This project is licensed under the MIT License — see [LICENSE](LICENSE) for details.

