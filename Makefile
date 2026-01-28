.DEFAULT_GOAL := main

.PHONY: .cargo
.cargo: ## Check that cargo is installed
	@cargo --version || echo 'Please install cargo: https://github.com/rust-lang/cargo'

.PHONY: .uv
.uv: ## Check that uv is installed
	@uv --version || echo 'Please install uv: https://docs.astral.sh/uv/getting-started/installation/'

.PHONY: .pre-commit
.pre-commit: ## Check that pre-commit is installed
	@pre-commit -V || echo 'Please install pre-commit: https://pre-commit.com/'

.PHONY: install-py
install-py: .uv ## Install python dependencies
	# --only-dev to avoid building the python package, use make dev-py for that
	uv sync --all-packages --only-dev

.PHONY: install-js
install-js: ## Install JS package dependencies
	cd crates/monty-js && npm install

.PHONY: install
install: .cargo .pre-commit install-py install-js ## Install the package, dependencies, and pre-commit for local development
	cargo check --workspace
	pre-commit install --install-hooks

.PHONY: dev-py
dev-py: ## Install the python package for development
	uv run maturin develop --uv -m crates/monty-python/Cargo.toml

.PHONY: dev-js
dev-js: ## Build the JS package (debug)
	cd crates/monty-js && npm run build:debug

.PHONY: lint-js
lint-js: install-js ## Lint JS code with oxlint
	cd crates/monty-js && npm run lint

.PHONY: test-js
test-js: dev-js ## Build and test the JS package
	cd crates/monty-js && npm test

.PHONY: smoke-test-js
smoke-test-js: ## Run smoke test for JS package (builds, packs, and tests installation)
	cd crates/monty-js && npm run smoke-test

.PHONY: dev-py-release
dev-py-release: ## Install the python package for development with a release build
	uv run maturin develop --uv -m crates/monty-python/Cargo.toml --release

.PHONY: dev-js-release
dev-js-release: ## Build the JS package (release)
	cd crates/monty-js && npm run build

.PHONY: dev-py-pgo
dev-py-pgo: ## Install the python package for development with profile-guided optimization
	$(eval PROFDATA := $(shell mktemp -d))
	RUSTFLAGS='-Cprofile-generate=$(PROFDATA)' uv run maturin develop --uv -m crates/monty-python/Cargo.toml --release
	uv run --package pydantic-monty --only-dev pytest crates/monty-python/tests -k "not test_parallel_exec"
	$(eval LLVM_PROFDATA := $(shell rustup run stable bash -c 'echo $$RUSTUP_HOME/toolchains/$$RUSTUP_TOOLCHAIN/lib/rustlib/$$(rustc -Vv | grep host | cut -d " " -f 2)/bin/llvm-profdata'))
	$(LLVM_PROFDATA) merge -o $(PROFDATA)/merged.profdata $(PROFDATA)
	RUSTFLAGS='-Cprofile-use=$(PROFDATA)/merged.profdata' $(uv-run-no-sync) maturin develop --uv -m crates/monty-python/Cargo.toml --release
	@rm -rf $(PROFDATA)

.PHONY: format-rs
format-rs:  ## Format Rust code with fmt
	@cargo +nightly fmt --version
	cargo +nightly fmt --all

.PHONY: format-py
format-py: ## Format Python code - WARNING be careful about this command as it may modify code and break tests silently!
	uv run ruff format
	uv run ruff check --fix --fix-only

.PHONY: format-js
format-js: install-js ## Format JS code with prettier
	cd crates/monty-js && npm run format:prettier

.PHONY: format
format: format-rs format-py format-js ## Format Rust code, this does not format Python code as we have to be careful with that

.PHONY: lint-rs
lint-rs:  ## Lint Rust code with clippy and import checks
	@cargo clippy --version
	cargo clippy --workspace --tests --bench main -- -D warnings
	cargo clippy --workspace --tests --all-features -- -D warnings
	uv run scripts/check_imports.py

.PHONY: clippy-fix
clippy-fix: ## Fix Rust code with clippy
	cargo clippy --workspace --tests --bench main --all-features --fix --allow-dirty

.PHONY: lint-py
lint-py: dev-py ## Lint Python code with ruff
	uv run ruff format --check
	uv run ruff check
	uv run basedpyright
	# mypy-stubtest requires a build of the python package, hence dev-py
	uv run -m mypy.stubtest pydantic_monty._monty --ignore-disjoint-bases

.PHONY: lint
lint: lint-rs lint-py ## Lint the code with ruff and clippy

.PHONY: format-lint-rs
format-lint-rs: format-rs lint-rs ## Format and lint Rust code with fmt and clippy

.PHONY: format-lint-py
format-lint-py: format-py lint-py ## Format and lint Python code with ruff

.PHONY: test-no-features
test-no-features: ## Run rust tests without any features enabled
	cargo test -p monty

.PHONY: test-ref-count-panic
test-ref-count-panic: ## Run rust tests with ref-count-panic enabled
	cargo test -p monty --features ref-count-panic

.PHONY: test-ref-count-return
test-ref-count-return: ## Run rust tests with ref-count-return enabled
	cargo test -p monty --features ref-count-return

.PHONY: test-cases
test-cases: ## Run tests cases only
	cargo test -p monty --test datatest_runner

.PHONY: test-type-checking
test-type-checking: ## Run rust tests on monty_type_checking
	cargo test -p monty_type_checking -p monty_typeshed

.PHONY: pytest
pytest: ## Run Python tests with pytest
	uv run --package pydantic-monty --only-dev pytest crates/monty-python/tests

.PHONY: test-py
test-py: dev-py pytest ## Build the python package (debug profile) and run tests

.PHONY: test-docs
test-docs: dev-py ## Test docs examples only
	uv run --package pydantic-monty --only-dev pytest crates/monty-python/tests/test_readme_examples.py
	cargo test --doc -p monty

.PHONY: test
test: test-ref-count-panic test-ref-count-return test-no-features test-type-checking test-py ## Run rust tests

.PHONY: testcov
testcov: ## Run Rust tests with coverage, print table, and generate HTML report
	@cargo llvm-cov --version > /dev/null 2>&1 || echo 'Please run: `cargo install cargo-llvm-cov`'
	cargo llvm-cov clean --workspace
	echo "coverage for `make test-no-features`"
	cargo llvm-cov --no-report -p monty
	echo "coverage for `make test-ref-count-panic`"
	cargo llvm-cov --no-report -p monty --features ref-count-panic
	echo "coverage for `make test-ref-count-return`"
	cargo llvm-cov --no-report -p monty --features ref-count-return
	echo "coverage for `make test-type-checking`"
	cargo llvm-cov --no-report -p monty_type_checking -p monty_typeshed
	echo "Generating reports:"
	cargo llvm-cov report --ignore-filename-regex '(tests/|test_cases/|/tests\.rs$$)'
	cargo llvm-cov report --html --ignore-filename-regex '(tests/|test_cases/|/tests\.rs$$)'
	@echo ""
	@echo "HTML report: $${CARGO_TARGET_DIR:-target}/llvm-cov/html/index.html"

.PHONY: complete-tests
complete-tests: ## Fill in incomplete test expectations using CPython
	uv run scripts/complete_tests.py

.PHONY: update-typeshed
update-typeshed: ## Update vendored typeshed from upstream
	uv run crates/monty-typeshed/update.py
	uv run ruff format
	uv run ruff check --fix --fix-only --silent

.PHONY: bench
bench: ## Run benchmarks
	cargo bench -p monty --bench main

.PHONY: dev-bench
dev-bench: ## Run benchmarks to test with dev profile
	cargo bench --profile dev -p monty --bench main -- --test

.PHONY: profile
profile: ## Profile the code with pprof and generate flamegraphs
	cargo bench -p monty --bench main --profile profiling -- --profile-time=10
	uv run scripts/flamegraph_to_text.py

.PHONY: type-sizes
type-sizes: ## Write type sizes for the crate to ./type-sizes.txt (requires nightly and top-type-sizes)
	RUSTFLAGS="-Zprint-type-sizes" cargo +nightly build -j1 2>&1 | top-type-sizes -f '^monty.*' > type-sizes.txt
	@echo "Type sizes written to ./type-sizes.txt"

.PHONY: main
main: lint test-ref-count-panic test-py ## run linting and the most important tests

# (must stay last!)
.PHONY: help
help: ## Show this help (usage: make help)
	@echo "Usage: make [recipe]"
	@echo "Recipes:"
	@awk '/^[a-zA-Z0-9_-]+:.*?##/ { \
	    helpMessage = match($$0, /## (.*)/); \
	        if (helpMessage) { \
	            recipe = $$1; \
	            sub(/:/, "", recipe); \
	            printf "  \033[36mmake %-20s\033[0m %s\n", recipe, substr($$0, RSTART + 3, RLENGTH); \
	    } \
	}' $(MAKEFILE_LIST)
