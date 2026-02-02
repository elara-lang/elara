MAKEFLAGS += --no-print-directory

.DEFAULT_GOAL := help

.PHONY: build check clean fmt help install install-system lint quality run setup test uninstall uninstall-system

help: ## Show available targets
	@echo "elara - Available targets"
	@echo ""
	@grep -hE '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*## "} {printf "  %-15s %s\n", $$1, $$2}'

setup: ## Fetch crate dependencies
	@../.make/setup.sh

build: ## Release-build and copy binary to bin/
	@../.make/build.sh

run: ## Run binary
	@../.make/run.sh

test: ## Run cargo test
	@../.make/test.sh

install: ## Install to ~/.local/bin (requires prior make build)
	@../.make/install.sh

install-system: ## Install to /usr/local/bin (requires prior make build; sudo only for copy)
	@SYSTEM=1 ../.make/install.sh

uninstall: ## Remove from ~/.local/bin (sudo only if needed)
	@../.make/uninstall.sh

uninstall-system: ## Remove from /usr/local/bin (sudo only if needed)
	@SYSTEM=1 ../.make/uninstall.sh

check: ## Type-check with cargo check
	@../.make/check.sh

lint: ## Run clippy
	@../.make/lint.sh

fmt: ## Format with rustfmt
	@../.make/fmt.sh

quality: ## fmt + clippy + test
	@../.make/quality.sh

clean: ## Remove target/ and bin/
	@../.make/clean.sh

