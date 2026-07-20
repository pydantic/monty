.DEFAULT_GOAL := main

ifeq ($(OS),Windows_NT)
EXE_EXT := .exe
else
EXE_EXT :=
endif

.PHONY: .cargo
.cargo: ## Check that cargo is installed
	@cargo --version || echo 'Please install cargo: https://github.com/rust-lang/cargo'

.PHONY: .uv
.uv: ## Check that uv is installed
	@uv --version || echo 'Please install uv: https://docs.astral.sh/uv/getting-started/installation/'

.PHONY: install-py
install-py: .uv ## Install python dependencies
	# --only-dev to avoid building the python package, use make dev-py for that
	uv sync --all-packages --only-dev

.PHONY: install-js
install-js: ## Install JS package dependencies
	cd crates/monty-js && npm install

.PHONY: install
install: .cargo install-py install-js ## Install the package, dependencies, and prek for local development
	cargo check --workspace
	uvx prek install --install-hooks

.PHONY: dev-py
dev-py: ## Install the python package for development
	uv run maturin develop --uv -m crates/monty-runtime/Cargo.toml
	uv run maturin develop --uv -m crates/monty-python/Cargo.toml

.PHONY: build-js
build-js: install-js ## Build the JS package (napi debug build + TypeScript)
	cd crates/monty-js && npm run build:debug

.PHONY: lint-js
lint-js: install-js ## Lint JS code with oxlint
	cd crates/monty-js && npm run lint

.PHONY: test-js
test-js: build-js ## Test the JS package (builds the monty binary the workers run)
	cargo build -p monty-runtime
	cd crates/monty-js && MONTY_BIN="$${CARGO_TARGET_DIR:-../../target}/debug/monty$(EXE_EXT)" npm test

.PHONY: dev-py-release
dev-py-release: ## Install the python package for development with a release build
	uv run maturin develop --uv -m crates/monty-runtime/Cargo.toml --release
	uv run maturin develop --uv -m crates/monty-python/Cargo.toml --release

.PHONY: build-wasm
build-wasm: install-js ## Build the lean wasm worker module (requires the wasm32-wasip1 target)
	cd crates/monty-js && npm run build:wasm && npm run build:ts

.PHONY: test-browser
test-browser: install-js ## Browser (Vitest) test of the wasm path in a real headless browser
	cd crates/monty-js && npm run build:wasm && npm run build:ts && npx playwright install chromium && npm run test:browser

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
	cd crates/monty-js && npm run format

.PHONY: format
format: format-rs format-py format-js ## Format Rust code, this does not format Python code as we have to be careful with that

.PHONY: lint-rs
lint-rs:  ## Lint Rust code with clippy and import checks
	@cargo clippy --version
	cargo clippy --workspace --tests -p monty-bench --benches -- -D warnings
	cargo clippy --workspace --tests --all-features -- -D warnings
	./scripts/check_imports.py

.PHONY: clippy-fix
clippy-fix: ## Fix Rust code with clippy
	cargo clippy --workspace --tests -p monty-bench --benches --all-features --fix --allow-dirty

.PHONY: generate-proto
generate-proto: ## Regenerate monty-proto's checked-in code from the .proto schema
	cargo run -p monty-proto --features generate --bin generate-proto
	cargo +nightly fmt -p monty-proto

.PHONY: check-proto
check-proto: generate-proto ## Verify monty-proto's checked-in code matches the .proto schema
	git diff --exit-code crates/monty-proto/src/generated crates/monty-proto/tests/oracle

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
	cargo test -p monty -p monty-fs
	cargo run -p monty-datatest

.PHONY: test-memory-model-checks
test-memory-model-checks: ## Run rust tests with memory-model-checks enabled - THIS IS EXTREMELY SLOW, SHOULD MOSTLY BE RUN IN CI OR IF ABSOLUTELY NECESSARY
	cargo test -p monty --features "memory-model-checks test-hooks"
	cargo run -p monty-datatest --features memory-model-checks

.PHONY: test-ref-count-return
test-ref-count-return: ## Run rust tests with ref-count-return enabled
	cargo test -p monty --features ref-count-return
	cargo run -p monty-datatest --features ref-count-return

.PHONY: test-cases
test-cases: ## Run tests cases only
	cargo run -p monty-datatest

.PHONY: miri
miri: ## Run library inline tests under miri (particularly relevant for heap.rs)
	cargo +nightly miri test -p monty --lib

.PHONY: miri-test-cases
miri-test-cases: ## Run library inline tests under miri (particularly relevant for heap.rs)
	MIRIFLAGS=-Zmiri-disable-isolation cargo +nightly miri run -p monty-datatest -- run_test_cases_monty

.PHONY: test-type-checking
test-type-checking: ## Run rust tests on monty-type-checking
	cargo test -p monty-type-checking -p monty-typeshed

.PHONY: test-subprocess
test-subprocess: ## Run subprocess protocol, child-mode, and worker-pool tests
	cargo build -p monty-runtime
	cargo test -p monty-proto -p monty-runtime -p monty-pool

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
test: test-memory-model-checks test-ref-count-return test-no-features test-type-checking test-subprocess test-py miri ## Run rust tests

.PHONY: testcov
testcov: ## Run Rust tests with coverage, print table, and generate HTML report
	@cargo llvm-cov --version > /dev/null 2>&1 || echo 'Please run: `cargo install cargo-llvm-cov`'
	cargo llvm-cov clean --workspace
	echo "coverage for `make test-no-features`"
	cargo llvm-cov --no-report -p monty
	cargo llvm-cov run --no-report -p monty-datatest
	echo "coverage for `make test-memory-model-checks`"
	cargo llvm-cov --no-report -p monty --features memory-model-checks
	cargo llvm-cov run --no-report -p monty-datatest --features memory-model-checks
	echo "coverage for `make test-ref-count-return`"
	cargo llvm-cov --no-report -p monty --features ref-count-return
	cargo llvm-cov run --no-report -p monty-datatest --features ref-count-return
	echo "coverage for `make test-type-checking`"
	cargo llvm-cov --no-report -p monty-type-checking -p monty-typeshed
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
	cargo bench -p monty-bench --bench main

.PHONY: bench-pool
bench-pool: ## Run subprocess pool benchmarks (spawn, checkout, wire round-trips)
	cargo build -p monty-runtime --release
	MONTY_TEST_BIN=$(CURDIR)/target/release/monty cargo bench -p monty-bench --bench pool

.PHONY: dev-bench
dev-bench: ## Run benchmarks to test with dev profile
	cargo bench --profile dev -p monty-bench --bench main -- --test

.PHONY: profile
profile: ## Profile the code with pprof and generate flamegraphs
	cargo bench -p monty-bench --bench main --profile profiling -- --profile-time=10
	uv run scripts/flamegraph_to_text.py

.PHONY: type-sizes
type-sizes: ## Write type sizes for the crate to ./type-sizes.txt (requires nightly and top-type-sizes)
	RUSTFLAGS="-Zprint-type-sizes" cargo +nightly build -j1 2>&1 | top-type-sizes -f '^monty.*' > type-sizes.txt
	@echo "Type sizes written to ./type-sizes.txt"

.PHONY: fuzz-string_input_panic
fuzz-string_input_panic: ## Run the `string_input_panic` fuzz target
	cargo +nightly fuzz run --fuzz-dir crates/fuzz string_input_panic

.PHONY: fuzz-tokens_input_panic
fuzz-tokens_input_panic: ## Run the `tokens_input_panic` fuzz target (structured token input)
	cargo +nightly fuzz run --fuzz-dir crates/fuzz tokens_input_panic

.PHONY: main
main: lint test-memory-model-checks test-subprocess test-py ## run linting and the most important tests

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
