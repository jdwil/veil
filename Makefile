# VEIL — Build orchestration
#
# Targets:
#   make veil           — build the VEIL compiler
#   make serve          — single-project veil serve API
#   make serve-examples — demo: veil serve examples/
#   make serve-stop     — stop veil serve on default port
#   make serve-api      — alias of serve
#   make serve-ui       — points at scripts/dev-stack.sh (ProductHost UI)
#   make projects       — list products under VEIL_PROJECTS_DIR
#   make runtime        — transpile + compile the runtime
#
# Projects (config: ~/.veil/config.json projects_dir; env overrides):
#   veil projects list          # first run prompts for projects dir
#   veil projects create my-app
#   make serve PROJECT=$(veil projects path my-app)
# Multi-project one-process host: docs/IDE_RUNTIME.md

VEIL_BIN    := target/release/veil
STUB_DIR    := stubs
EXAMPLES    := examples
# Backend API for `make serve` (single-project veil serve).
PORT        ?= 3001
# Vite / SvelteKit (single-project viewer, if used)
VIEWER_PORT ?= 5173
PID_DIR     := .veil-dev
API_PID     := $(PID_DIR)/api.pid
UI_PID      := $(PID_DIR)/ui.pid

# Optional session override of config projects_dir.
# VEIL_PROJECTS_DIR ?= $(HOME)/dev/veil-projects
# Single project root for `make serve` (required for product IDE).
PROJECT     ?=

# Agent backend — Kiro via ACP by default for local make serve
# Override: make serve VEIL_MODEL_PROVIDER=ollama VEIL_MODEL_NAME=qwen3.5:9b
#           make serve VEIL_MODEL_PROVIDER=echo
#           make serve VEIL_ACP_AGENT=personal
#           make serve VEIL_MODEL_PROVIDER=openai VEIL_MODEL_NAME=gpt-4o
VEIL_MODEL_PROVIDER ?= acp
# Leave empty for ACP: Kiro uses its default model (often `auto` from ~/.kiro).
# Do NOT set this to "kiro" — that is not a model id. For a specific model:
#   make serve VEIL_ACP_MODEL=<kiro-model-id>
VEIL_MODEL_NAME     ?=
# Optional: VEIL_MODEL_BASE_URL=http://127.0.0.1:11434
# ACP / Kiro (spawned by veil-server on agent turns when provider=acp)
VEIL_ACP_COMMAND    ?= kiro-cli
VEIL_ACP_ARGS       ?= acp --trust-all-tools
# ACP session cwd: project root (so .kiro/settings/mcp.json + workspace files resolve)
VEIL_ACP_CWD        ?= $(if $(PROJECT),$(PROJECT),$(CURDIR))
# Agent role (kiro agent name); override: make serve VEIL_ACP_AGENT=personal
VEIL_ACP_AGENT      ?= hive

export VEIL_MODEL_PROVIDER
export VEIL_ACP_COMMAND
export VEIL_ACP_ARGS
export VEIL_ACP_CWD
export VEIL_ACP_AGENT
ifneq ($(origin VEIL_PROJECTS_DIR), undefined)
export VEIL_PROJECTS_DIR
endif
# Only export model name when non-empty (avoids forcing --model kiro)
ifneq ($(strip $(VEIL_MODEL_NAME)),)
export VEIL_MODEL_NAME
endif
ifneq ($(origin VEIL_MODEL_BASE_URL), undefined)
export VEIL_MODEL_BASE_URL
endif
ifneq ($(origin VEIL_ACP_AGENT), undefined)
export VEIL_ACP_AGENT
endif
ifneq ($(origin VEIL_ACP_MODEL), undefined)
export VEIL_ACP_MODEL
endif

# External crates that need stubs
STUB_CRATES := aws-sdk-s3 aws-sdk-dynamodb aws-sdk-lambda aws-sdk-sns aws-sdk-sqs \
               aws-config gix rig-core axum tokio-tungstenite tower-http \
               sha2 zip tempfile schemars

.PHONY: veil serve serve-examples serve-stop serve-api serve-ui \
	projects runtime runtime-serve pure-runtime pure-runtime-build pure-runtime-smoke gen-runtime build-runtime \
	clean-runtime stubs check test test-roundtrip

# ─── Compiler ───────────────────────────────────────────────────────────────

veil:
	cargo build -p veil-cli --release

# ─── Dev stack: single-project API (`veil serve`) ────────────────────────────
# Product UX (projects, agent, sign-off) is scripts/dev-stack.sh + ui/.

# List products in the projects hub (not the IDE).
projects: veil
	@$(VEIL_BIN) projects list

# Single-project IDE API. Product UX is scripts/dev-stack.sh + ui/.
serve: serve-api

# Demo / CI: serve monorepo examples/ (not the product default)
serve-examples: veil
	@$(MAKE) serve-api PROJECT=$(EXAMPLES) PORT=$(PORT)

# API only — requires PROJECT=
serve-api: veil
	@if [ -z "$(strip $(PROJECT))" ]; then \
		echo "error: set PROJECT=…  (or make serve-api PROJECT=examples)"; \
		exit 1; \
	fi
	@if ss -tln 2>/dev/null | grep -qE ":$(PORT)\\b" || \
	   netstat -tln 2>/dev/null | grep -qE ":$(PORT)\\b"; then \
		echo "error: port $(PORT) is already in use.  make serve-stop"; \
		exit 1; \
	fi
	@echo "API only: http://localhost:$(PORT)  project=$(PROJECT)  (Ctrl-C to stop)"
	@echo "  Agent: VEIL_MODEL_PROVIDER=$(VEIL_MODEL_PROVIDER)  model=$(VEIL_MODEL_NAME)"
	@if [ "$(VEIL_MODEL_PROVIDER)" = "acp" ] || [ "$(VEIL_MODEL_PROVIDER)" = "kiro" ]; then \
		echo "  ACP:    $(VEIL_ACP_COMMAND) $(VEIL_ACP_ARGS)  (cwd=$(VEIL_ACP_CWD))"; \
	fi
	$(VEIL_BIN) serve $(PROJECT) -p $(PORT)

# ProductHost UI (Vite). Backend is scripts/dev-stack.sh, not veil serve.
serve-ui:
	@echo "Product UI is ui/ via scripts/dev-stack.sh (Vite :5180 → API :8080)"
	@echo "  scripts/dev-stack.sh ui"

# Stop API + viewer (default ports) and any recorded PIDs.
serve-stop:
	@echo "Stopping VEIL dev stack (ports $(PORT), $(VIEWER_PORT))…"
	@if [ -f $(API_PID) ]; then kill $$(cat $(API_PID)) 2>/dev/null || true; fi
	@if [ -f $(UI_PID) ]; then kill $$(cat $(UI_PID)) 2>/dev/null || true; fi
	@-fuser -k $(PORT)/tcp 2>/dev/null || true
	@-fuser -k $(VIEWER_PORT)/tcp 2>/dev/null || true
	@# also kill stray veil serve / vite for this project
	@-pkill -x veil 2>/dev/null || true
	@-pkill -f 'vite dev' 2>/dev/null || true
	@rm -f $(API_PID) $(UI_PID)
	@sleep 0.3
	@echo "Done."

# ─── Runtime ────────────────────────────────────────────────────────────────

runtime: gen-runtime build-runtime

gen-runtime: veil
	$(VEIL_BIN) gen $(RUNTIME_SRC) -o $(RUNTIME_OUT)

build-runtime: gen-runtime
	cargo build --manifest-path $(RUNTIME_OUT)/Cargo.toml

# Product host: multi-project IDE kernel + shell UI (RTU-008 / PVR-031)
RUNTIME_PORT ?= 8080
runtime-serve: pure-runtime-build
	@echo ""
	@echo "Starting veil-runtime on :$(RUNTIME_PORT)"
	@echo "  Shell:    http://127.0.0.1:$(RUNTIME_PORT)/"
	@echo "  Projects: http://127.0.0.1:$(RUNTIME_PORT)/api/projects"
	@echo "  Viewer:   http://127.0.0.1:$(VIEWER_PORT)/?api=http://127.0.0.1:$(RUNTIME_PORT)"
	@echo "  (optional) make serve-ui VIEWER_PORT=$(VIEWER_PORT)"
	@echo ""
	@CI=1 VEIL_NONINTERACTIVE=1 VEIL_PORT=$(RUNTIME_PORT) VEIL_BIN=$(CURDIR)/$(VEIL_BIN) \
		./target/release/veil-runtime

# Product host binary (Rust). Shell UI is ui/ (Vite).
pure-runtime-build: veil
	@echo "==> build veil-runtime (ProductHost)"
	@cargo build --release -p veil-runtime
	@echo "✓ veil-runtime ready (UI: ui/ Vite :5180, API: :8080)"

# PVR-031 smoke: build + curl health/projects/config/SPA (no long-lived server)
pure-runtime-smoke:
	@bash scripts/pure_runtime_smoke.sh

pure-runtime: pure-runtime-build runtime-serve

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

# SER-004: unit + integration tests including fixture round-trips
test:
	cargo test --workspace

# ACS-003: multi-package dual-loop fixture (product + platform → one veil_bin)
# Not multi-project hub. Requires debug/release veil binary.
fixture-multi-harness: veil
	@OUT=$${OUT:-/tmp/veil-multi-harness}; \
	rm -rf "$$OUT"; \
	$(VEIL_BIN) check fixtures/multi_harness/product.veil; \
	$(VEIL_BIN) check fixtures/multi_harness/platform.veil; \
	$(VEIL_BIN) gen fixtures/multi_harness/product.veil -o "$$OUT" -t rust --no-prune; \
	$(VEIL_BIN) gen fixtures/multi_harness/platform.veil -o "$$OUT" -t rust --no-prune; \
	$(VEIL_BIN) gen-harness fixtures/multi_harness/product.veil fixtures/multi_harness/platform.veil -o "$$OUT"; \
	cd "$$OUT" && cargo check -p veil_bin; \
	echo "✓ multi_harness fixture OK ($$OUT)"

# ACS-006: complexity ladder L0–L3
fixture-ladder-l0: veil
	@OUT=$${OUT:-/tmp/veil-ladder-l0}; \
	rm -rf "$$OUT"; \
	$(VEIL_BIN) check fixtures/ladder/l0/hello.veil; \
	$(VEIL_BIN) gen fixtures/ladder/l0/hello.veil -o "$$OUT" -t rust; \
	cd "$$OUT" && cargo check -p veil_bin; \
	echo "✓ ladder L0 OK ($$OUT)"

fixture-ladder-l1: veil
	@OUT=$${OUT:-/tmp/veil-ladder-l1}; \
	rm -rf "$$OUT"; \
	$(VEIL_BIN) check fixtures/ladder/l1/crud.veil; \
	$(VEIL_BIN) gen fixtures/ladder/l1/crud.veil -o "$$OUT" -t rust; \
	cd "$$OUT" && cargo check -p veil_bin; \
	echo "✓ ladder L1 OK ($$OUT)"

fixture-ladder-l2: fixture-multi-harness
	@echo "✓ ladder L2 OK (multi_harness)"

fixture-ladder-l3: veil
	@OUT=$${OUT:-/tmp/veil-ladder-l3}; \
	rm -rf "$$OUT"; \
	$(VEIL_BIN) check fixtures/ladder/l3/app.veil; \
	$(VEIL_BIN) gen fixtures/ladder/l3/app.veil -o "$$OUT" -t rust; \
	cd "$$OUT" && cargo check -p veil_bin; \
	echo "✓ ladder L3 OK ($$OUT)"

fixture-ladder: fixture-ladder-l0 fixture-ladder-l1 fixture-ladder-l2 fixture-ladder-l3
	@echo "✓ ladder L0–L3 all green"

# Round-trip suite only (examples/**)
test-roundtrip:
	cargo test -p veil-parser --test roundtrip_suite
