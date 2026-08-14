MAKEFLAGS += --no-print-directory

.DEFAULT_GOAL := help

.PHONY: build check clean fmt help install install-libs install-libs-system install-system lint quality run setup test uninstall uninstall-libs uninstall-libs-system uninstall-system

help: ## Show available targets
	@echo "elara - Available targets"
	@echo ""
	@grep -hE '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) \
		| sort \
		| awk 'BEGIN {FS = ":.*## "} {printf "  %-22s %s\n", $$1, $$2}'

setup: ## Fetch crate dependencies
	@../.make/setup.sh

build: ## Release-build and copy binaries/libs to bin/
	@../.make/build.sh

run: ## Run binary
	@../.make/run.sh

test: ## Run cargo test
	@../.make/test.sh

install: ## Install elara and elarac to ~/.local/bin (requires prior make build)
	@../.make/install.sh

install-system: ## Install elara and elarac to /usr/local/bin (requires prior make build; sudo only for copy)
	@SYSTEM=1 ../.make/install.sh

install-libs: ## Install libelara.a, libelara.so, and elara.h to ~/.local (requires prior make build)
	@../.make/install-libs.sh

install-libs-system: ## Install libraries and header to /usr/local (requires prior make build; sudo only for copy)
	@SYSTEM=1 ../.make/install-libs.sh

uninstall: ## Remove elara and elarac from ~/.local/bin (sudo only if needed)
	@../.make/uninstall.sh

uninstall-system: ## Remove elara and elarac from /usr/local/bin (sudo only if needed)
	@SYSTEM=1 ../.make/uninstall.sh

uninstall-libs: ## Remove libraries and header from ~/.local (sudo only if needed)
	@../.make/uninstall-libs.sh

uninstall-libs-system: ## Remove libraries and header from /usr/local (sudo only if needed)
	@SYSTEM=1 ../.make/uninstall-libs.sh

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
