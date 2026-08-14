MAKEFLAGS += --no-print-directory

.DEFAULT_GOAL := help

.PHONY: build check clean fmt help install install-libs install-libs-system install-system lint quality run setup test uninstall uninstall-libs uninstall-libs-system uninstall-system

help: ## Show available targets
	@$(MAKE) -C elara help

setup: ## Fetch crate dependencies
	@$(MAKE) -C elara setup

build: ## Release-build and copy binaries/libs to bin/
	@$(MAKE) -C elara build

run: ## Run binary
	@$(MAKE) -C elara run

test: ## Run cargo test
	@$(MAKE) -C elara test

check: ## Type-check with cargo check
	@$(MAKE) -C elara check

lint: ## Run clippy
	@$(MAKE) -C elara lint

fmt: ## Format with rustfmt
	@$(MAKE) -C elara fmt

quality: ## fmt + clippy + test
	@$(MAKE) -C elara quality

install: ## Install elara and elarac to ~/.local/bin (requires prior make build)
	@$(MAKE) -C elara install

install-system: ## Install elara and elarac to /usr/local/bin (requires prior make build; sudo only for copy)
	@$(MAKE) -C elara install-system

install-libs: ## Install libelara.a, libelara.so, and elara.h to ~/.local (requires prior make build)
	@$(MAKE) -C elara install-libs

install-libs-system: ## Install libraries and header to /usr/local (requires prior make build; sudo only for copy)
	@$(MAKE) -C elara install-libs-system

uninstall: ## Remove elara and elarac from ~/.local/bin (sudo only if needed)
	@$(MAKE) -C elara uninstall

uninstall-system: ## Remove elara and elarac from /usr/local/bin (sudo only if needed)
	@$(MAKE) -C elara uninstall-system

uninstall-libs: ## Remove libraries and header from ~/.local (sudo only if needed)
	@$(MAKE) -C elara uninstall-libs

uninstall-libs-system: ## Remove libraries and header from /usr/local (sudo only if needed)
	@$(MAKE) -C elara uninstall-libs-system

clean: ## Remove target/ and bin/
	@$(MAKE) -C elara clean
