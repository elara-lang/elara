# Elara

High-performance embeddable scripting language, register-based bytecode VM, AOT compiler, and C99/C++ FFI runtime written in Rust.

## Highlights

- Pack IEEE-754 floats, signed 48-bit integers, booleans, nil, and tagged GC pointers into 8-byte NaN-tagged values with transparent 64-bit integer promotion
- Execute 32-bit register bytecode with fused compare-and-branch dispatch, 1-slot inline property caching, and strict tail-call optimization
- Store dense 0-based arrays alongside Robin Hood open-addressed hash tables inside a single 64-byte CPU cache-line header
- Intern short UTF-8 strings up to 64 bytes in O(1) while hashing long strings lazily to avoid allocation stalls
- Enforce lexical scoping with explicit let and const bindings, compound assignments, nested string interpolation, and LIFO defer resource cleanup
- Run stackful cooperative fibers and incremental tri-color garbage collection with weak tables under a deterministic microsecond pause budget
- Sandbox untrusted scripts with CPU register fuel quotas, hard heap memory ceilings, and capability-gated standard library modules
- Embed into C99, C++, or Rust hosts via a zero-dependency static or dynamic library and a direct slot-based C API

## Overview

Elara is a clean-slate embeddable scripting language engineered in Rust (Edition 2024) with zero external crate dependencies. It combines the tiny footprint and C ABI portability of classic embeddable VMs with modern language ergonomics: 0-based indexing, `{ }` lexical blocks, static scope verification, `defer` resource management, and deterministic sandboxing.

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

Install to `~/.local/bin` (default), or system-wide to `/usr/local/bin` (sudo only for the copy):

```bash
make install
make install-system
make uninstall
make uninstall-system
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
make build             # Release-build and copy binary to bin/elara
make test              # Run cargo test
make quality           # Format, clippy, and test
make install           # Install binary to ~/.local/bin
make install-system    # Install binary to /usr/local/bin
make uninstall         # Remove from ~/.local/bin
make uninstall-system  # Remove from /usr/local/bin
```

## License

This project is licensed under the MIT License — see [LICENSE](LICENSE) for details.

