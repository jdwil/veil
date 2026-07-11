# VEIL — Build orchestration
#
# Targets:
#   make veil           — build the VEIL compiler
#   make serve          — IDE for one PROJECT root + viewer
#   make serve-examples — demo: veil serve examples/ + viewer
#   make serve-stop     — stop backend + frontend on default ports
#   make serve-api      — API only (no viewer)
#   make serve-ui       — viewer only (expects API already on PORT)
#   make projects       — list products under VEIL_PROJECTS_DIR
#   make runtime        — transpile + compile the runtime
#
# Projects (config: ~/.veil/config.json projects_dir; env overrides):
#   veil projects list          # first run prompts for projects dir
#   veil projects create my-app
#   make serve PROJECT=$(veil projects path my-app)
# Multi-project one-process host: docs/IDE_RUNTIME.md

VEIL_BIN    := target/release/veil
RUNTIME_SRC := runtime/src/runtime.veil
RUNTIME_OUT := runtime/generated
STUB_DIR    := runtime/src/stubs
EXAMPLES    := examples
VIEWER_DIR  := veil-viewer
# Backend API (viewer hardcodes localhost:3001 — change both if you override)
PORT        ?= 3001
# Vite / SvelteKit dev server
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
VEIL_ACP_CWD        ?= $(CURDIR)

export VEIL_MODEL_PROVIDER
export VEIL_ACP_COMMAND
export VEIL_ACP_ARGS
export VEIL_ACP_CWD
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

.PHONY: veil serve serve-examples serve-stop serve-api serve-ui viewer-install \
	projects runtime runtime-serve pure-runtime pure-runtime-build gen-runtime build-runtime \
	clean-runtime stubs check test test-roundtrip

# ─── Compiler ───────────────────────────────────────────────────────────────

veil:
	cargo build -p veil-cli --release

# ─── Dev stack: API + viewer ────────────────────────────────────────────────

# Install viewer deps if needed
viewer-install:
	@if [ ! -d "$(VIEWER_DIR)/node_modules" ]; then \
		echo "Installing $(VIEWER_DIR) dependencies…"; \
		cd $(VIEWER_DIR) && npm install; \
	fi

# List products in the projects hub (not the IDE).
projects: veil
	@$(VEIL_BIN) projects list

# Full stack: single-project IDE API + viewer.
# Requires PROJECT= path to a product root (git repo under VEIL_PROJECTS_DIR).
# Stop: Ctrl-C  or  make serve-stop
serve: veil viewer-install
	@if [ -z "$(strip $(PROJECT))" ]; then \
		echo "error: set PROJECT to a product root."; \
		echo "  export VEIL_PROJECTS_DIR=$$HOME/dev/veil-projects"; \
		echo "  make projects"; \
		echo "  veil projects create my-app"; \
		echo "  make serve PROJECT=$$VEIL_PROJECTS_DIR/my-app"; \
		echo ""; \
		echo "Demo sandbox: make serve-examples"; \
		exit 1; \
	fi
	@mkdir -p $(PID_DIR)
	@if ss -tln 2>/dev/null | grep -qE ":$(PORT)\\b" || \
	   netstat -tln 2>/dev/null | grep -qE ":$(PORT)\\b"; then \
		echo "error: API port $(PORT) is already in use."; \
		echo "  make serve-stop   or   make serve PORT=…"; \
		ss -tlnp 2>/dev/null | grep -E ":$(PORT)\\b" || true; \
		exit 1; \
	fi
	@if ss -tln 2>/dev/null | grep -qE ":$(VIEWER_PORT)\\b" || \
	   netstat -tln 2>/dev/null | grep -qE ":$(VIEWER_PORT)\\b"; then \
		echo "error: viewer port $(VIEWER_PORT) is already in use."; \
		echo "  make serve-stop   or   make serve VIEWER_PORT=…"; \
		ss -tlnp 2>/dev/null | grep -E ":$(VIEWER_PORT)\\b" || true; \
		exit 1; \
	fi
	@echo "Starting VEIL IDE (single project)…"
	@echo "  Project:  $(PROJECT)"
	@echo "  Hub:      VEIL_PROJECTS_DIR=$(VEIL_PROJECTS_DIR)"
	@echo "  Backend:  http://localhost:$(PORT)   (veil serve $(PROJECT))"
	@echo "  Frontend: http://localhost:$(VIEWER_PORT)  (veil-viewer)"
	@echo "  Open:     http://localhost:$(VIEWER_PORT)"
	@echo "  Agent:    VEIL_MODEL_PROVIDER=$(VEIL_MODEL_PROVIDER)$(if $(VEIL_MODEL_NAME),  model=$(VEIL_MODEL_NAME),)"
	@if [ "$(VEIL_MODEL_PROVIDER)" = "acp" ] || [ "$(VEIL_MODEL_PROVIDER)" = "kiro" ]; then \
		echo "  ACP:      $(VEIL_ACP_COMMAND) $(VEIL_ACP_ARGS)"; \
		echo "  ACP cwd:  $(VEIL_ACP_CWD)"; \
		if [ -n "$(VEIL_ACP_MODEL)" ]; then echo "  ACP model: $(VEIL_ACP_MODEL)"; else echo "  ACP model: (kiro default / auto — set VEIL_ACP_MODEL to pin)"; fi; \
		if command -v $(VEIL_ACP_COMMAND) >/dev/null 2>&1; then \
			echo "  ACP bin:  ok ($$(command -v $(VEIL_ACP_COMMAND)))"; \
		else \
			echo "  ACP bin:  WARN $(VEIL_ACP_COMMAND) not on PATH — install Kiro CLI + kiro-cli login"; \
		fi; \
	fi
	@if [ "$(VEIL_MODEL_PROVIDER)" = "ollama" ]; then \
		if curl -sf http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then \
			echo "  Ollama:   up at :11434"; \
		else \
			echo "  Ollama:   WARN not reachable at :11434 (start ollama or use VEIL_MODEL_PROVIDER=acp)"; \
		fi; \
	fi
	@echo "  Stop:     Ctrl-C  or  make serve-stop"
	@echo ""
	@$(VEIL_BIN) serve $(PROJECT) -p $(PORT) & echo $$! > $(API_PID); \
	API_PID_VAL=$$(cat $(API_PID)); \
	cleanup() { \
		echo ""; \
		echo "Stopping dev stack…"; \
		kill $$API_PID_VAL 2>/dev/null || true; \
		fuser -k $(PORT)/tcp 2>/dev/null || true; \
		fuser -k $(VIEWER_PORT)/tcp 2>/dev/null || true; \
		rm -f $(API_PID) $(UI_PID); \
	}; \
	trap cleanup EXIT INT TERM; \
	i=0; \
	while [ $$i -lt 30 ]; do \
		if curl -sf "http://127.0.0.1:$(PORT)/api/files" >/dev/null 2>&1; then break; fi; \
		if ! kill -0 $$API_PID_VAL 2>/dev/null; then \
			echo "error: veil serve exited early"; exit 1; \
		fi; \
		i=$$((i+1)); sleep 0.5; \
	done; \
	if ! curl -sf "http://127.0.0.1:$(PORT)/api/files" >/dev/null 2>&1; then \
		echo "error: API did not become ready on port $(PORT)"; \
		exit 1; \
	fi; \
	echo "API ready — starting viewer…"; \
	cd $(VIEWER_DIR) && npm run dev -- --host 127.0.0.1 --port $(VIEWER_PORT)

# Demo / CI: serve monorepo examples/ (not the product default)
serve-examples: veil viewer-install
	@$(MAKE) serve PROJECT=$(EXAMPLES) PORT=$(PORT) VIEWER_PORT=$(VIEWER_PORT)

# API only (no viewer) — requires PROJECT=
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

# Viewer only (expects veil serve already on PORT)
serve-ui: viewer-install
	@echo "Viewer: http://localhost:$(VIEWER_PORT)  (API expected at :$(PORT))"
	cd $(VIEWER_DIR) && npm run dev -- --host 127.0.0.1 --port $(VIEWER_PORT)

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
		./runtime/bootstrap/target/release/veil-runtime

# PVR-031 / CAP-005: gen SPA (dist/) + build host trampoline
pure-runtime-build: veil
	@echo "==> gen runtime-ui.veil → static/app (SPA dist CAP-005)"
	@$(VEIL_BIN) check runtime/src/runtime-ui.veil || true
	@$(VEIL_BIN) gen runtime/src/runtime-ui.veil -o runtime/bootstrap/static/app -t typescript
	@# Prefer generated dist/ as primary shell (ProductHost serves dist/ first)
	@if [ -f runtime/bootstrap/static/app/dist/index.html ]; then \
		mkdir -p runtime/bootstrap/static/dist; \
		cp -f runtime/bootstrap/static/app/dist/index.html runtime/bootstrap/static/dist/; \
		cp -f runtime/bootstrap/static/app/dist/spa.js runtime/bootstrap/static/dist/ 2>/dev/null || true; \
		cp -f runtime/bootstrap/static/app/src/spa.js runtime/bootstrap/static/dist/ 2>/dev/null || true; \
	fi
	@echo "==> gen host.veil (CAP-002/006 product host bin)"
	@$(VEIL_BIN) gen runtime/src/host.veil -o runtime/generated-host -t rust || true
	@echo "==> build veil-runtime trampoline"
	@cargo build --release --manifest-path runtime/bootstrap/Cargo.toml
	@echo "✓ pure-runtime build ready (shell: static/dist or static/app)"

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

# Round-trip suite only (examples/** + runtime/src/**)
test-roundtrip:
	cargo test -p veil-parser --test roundtrip_suite
