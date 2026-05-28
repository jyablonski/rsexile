.PHONY: help build run test fmt fmt-check clippy check pre-commit release coverage coverage-html coverage-lcov

help:
	@printf "Available targets:\n"
	@printf "  make build         cargo build --release --locked\n"
	@printf "  make run           cargo run\n"
	@printf "  make test          cargo test --workspace --all-features --locked\n"
	@printf "  make fmt           cargo fmt --all\n"
	@printf "  make fmt-check     cargo fmt --all -- --check\n"
	@printf "  make clippy        cargo clippy --workspace --all-targets --all-features --locked -- -D warnings\n"
	@printf "  make check         fmt-check + clippy + test\n"
	@printf "  make coverage      cargo llvm-cov terminal summary\n"
	@printf "  make coverage-html HTML report at target/llvm-cov/html/index.html\n"
	@printf "  make coverage-lcov lcov.info for Codecov / Coveralls\n"
	@printf "  make pre-commit    pre-commit run --all-files\n"
	@printf "  make release VERSION=vX.Y.Z   Tag + push; CI builds the binary.\n"

build:
	cargo build --release --locked

run:
	cargo run

test:
	cargo test --workspace --all-features --locked

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

check: fmt-check clippy test

coverage:
	cargo llvm-cov --workspace --all-features --locked

coverage-html:
	cargo llvm-cov --workspace --all-features --locked --html
	@echo "Open target/llvm-cov/html/index.html"

coverage-lcov:
	cargo llvm-cov --workspace --all-features --locked --lcov --output-path lcov.info
	@echo "Wrote lcov.info"

pre-commit:
	pre-commit run --all-files

# example: make release VERSION=v0.2.0
release: ## Create a release tag and push it (requires VERSION=vX.Y.Z)
	@if [ -z "$(VERSION)" ] || [ "$(VERSION)" = "dev" ]; then \
		echo "Error: VERSION must be set (e.g., VERSION=v0.2.0)"; \
		exit 1; \
	fi
	@if ! echo "$(VERSION)" | grep -qE '^v[0-9]+\.[0-9]+\.[0-9]+$$'; then \
		echo "Error: VERSION must be in semantic version format (e.g., v0.2.0)"; \
		exit 1; \
	fi
	@echo "Checking git status..."
	@if [ -n "$$(git status --porcelain)" ]; then \
		echo "Error: Working directory is not clean. Commit or stash changes first."; \
		exit 1; \
	fi
	@echo "Checking if tag $(VERSION) already exists..."
	@if git rev-parse "$(VERSION)" >/dev/null 2>&1; then \
		echo "Error: Tag $(VERSION) already exists"; \
		exit 1; \
	fi
	@echo "Checking if we're on main branch..."
	@if [ "$$(git rev-parse --abbrev-ref HEAD)" != "main" ]; then \
		echo "Warning: Not on main branch. Continuing anyway..."; \
	fi
	@echo "Creating release tag $(VERSION)..."
	@git tag -a $(VERSION) -m "Release $(VERSION)"
	@echo "Pushing tag $(VERSION) to remote..."
	@git push origin $(VERSION)
	@echo "Release $(VERSION) created and pushed successfully!"
	@echo "GitHub Actions will build the binary and attach it to the release."
