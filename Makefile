# VEIL — Build orchestration
#
# Targets:
#   make veil        — build the VEIL compiler
#   make runtime     — transpile + compile the runtime
#   make gen-runtime — transpile runtime.veil → Rust (in runtime/generated/)
#   make build-runtime — cargo build the generated runtime
#   make clean-runtime — remove generated output
#   make stubs       — generate all .stub files for runtime dependencies
#   make check       — run veil check on runtime source

VEIL_BIN    := target/release/veil
RUNTIME_SRC := runtime/src/runtime.veil
RUNTIME_OUT := runtime/generated
STUB_DIR    := runtime/src/stubs

# External crates that need stubs
STUB_CRATES := aws-sdk-s3 aws-sdk-dynamodb aws-sdk-lambda aws-sdk-sns aws-sdk-sqs \
               aws-config gix rig-core axum tokio-tungstenite tower-http \
               sha2 zip tempfile schemars

.PHONY: veil runtime gen-runtime build-runtime clean-runtime stubs check

# ─── Compiler ───────────────────────────────────────────────────────────────

veil:
	cargo build -p veil-cli --release

# ─── Runtime ────────────────────────────────────────────────────────────────

runtime: gen-runtime build-runtime

gen-runtime: veil
	$(VEIL_BIN) gen $(RUNTIME_SRC) -o $(RUNTIME_OUT)

build-runtime: gen-runtime
	cargo build --manifest-path $(RUNTIME_OUT)/Cargo.toml

clean-runtime:
	find $(RUNTIME_OUT) -mindepth 1 ! -name '.gitignore' -delete

# ─── Stubs ──────────────────────────────────────────────────────────────────

stubs: veil
	@mkdir -p $(STUB_DIR)
	@for crate in $(STUB_CRATES); do \
		echo "Generating stub: $$crate"; \
		$(VEIL_BIN) stub-gen $$crate -o $(STUB_DIR)/$$crate.stub || \
			echo "  ⚠ stub-gen failed for $$crate (fix and retry)"; \
	done

# ─── Validation ─────────────────────────────────────────────────────────────

check: veil
	$(VEIL_BIN) check $(RUNTIME_SRC)
