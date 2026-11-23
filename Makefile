SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help
.NOTPARALLEL: verify

.PHONY: help verify fmt lint test compose-config repo-check updater-test

help: ## Show available commands
	@awk 'BEGIN {FS = ":.*##"; printf "Commands:\n"} /^[a-zA-Z0-9_-]+:.*##/ {printf "  %-18s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

verify: fmt-check lint test compose-config repo-check updater-test ## Run all checks

fmt: ## Format source files
	cargo fmt --all

lint: ## Run static analysis
	cargo clippy --locked --all-targets -- -D warnings

test: ## Run tests without watch mode
	cargo test --locked --all

updater-test: ## Test the Raspberry Pi update script
	python3 -m unittest discover -s deploy/pi -p 'test_*.py'

compose-config: ## Validate local Compose configuration
	@if [ -f docker-compose.yml ]; then docker compose config --quiet; else echo 'skip compose config'; fi

repo-check: ## Check the GitHub CI and ruleset files
	@grep -q '^name: PR CI$$' .github/workflows/pr-ci.yml
	@grep -q '^    name: CI gate$$' .github/workflows/pr-ci.yml
	@python3 -m json.tool .github/rulesets/main.json >/dev/null
	@grep -q '"context": "CI gate"' .github/rulesets/main.json

.PHONY: fmt-check
fmt-check: ## Check Rust formatting
	cargo fmt --all -- --check
