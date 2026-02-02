MAKEFLAGS += --no-print-directory

.DEFAULT_GOAL := help

.PHONY: build check clean fmt help install install-system lint quality run setup test uninstall uninstall-system

help: ## Show available targets
	@$(MAKE) -C elara help

setup: ## Fetch crate dependencies
	@$(MAKE) -C elara setup

build: ## Release-build and copy binary to bin/
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

install: ## Install to ~/.local/bin (requires prior make build)
	@$(MAKE) -C elara install

install-system: ## Install to /usr/local/bin (requires prior make build; sudo only for copy)
	@$(MAKE) -C elara install-system

uninstall: ## Remove from ~/.local/bin (sudo only if needed)
	@$(MAKE) -C elara uninstall

uninstall-system: ## Remove from /usr/local/bin (sudo only if needed)
	@$(MAKE) -C elara uninstall-system

clean: ## Remove target/ and bin/
	@$(MAKE) -C elara clean

