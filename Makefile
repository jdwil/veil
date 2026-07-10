# VEIL — Build orchestration
#
# Targets:
#   make veil           — build the VEIL compiler
#   make serve          — backend (veil serve examples/) + frontend (veil-viewer)
#   make serve-examples — alias for serve
#   make serve-stop     — stop backend + frontend on default ports
#   make serve-api      — API only (no viewer)
#   make serve-ui       — viewer only (expects API already on PORT)
#   make runtime        — transpile + compile the runtime
#   make gen-runtime    — transpile runtime.veil → Rust (in runtime/generated/)
#   make build-runtime  — cargo build the generated runtime
#   make clean-runtime  — remove generated output
#   make stubs          — generate all .stub files for runtime dependencies
#   make check          — run veil check on runtime source

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

# Agent backend — Ollama by default for local make serve
# Override: make serve VEIL_MODEL_PROVIDER=echo
#           make serve VEIL_MODEL_NAME=llama3.2
#           make serve VEIL_MODEL_PROVIDER=acp          # Kiro via ACP
#           make serve VEIL_MODEL_PROVIDER=acp VEIL_ACP_AGENT=personal
VEIL_MODEL_PROVIDER ?= ollama
VEIL_MODEL_NAME     ?= qwen3.5:9b
# Optional: VEIL_MODEL_BASE_URL=http://127.0.0.1:11434
# ACP / Kiro
VEIL_ACP_COMMAND    ?= kiro-cli
VEIL_ACP_ARGS       ?= acp --trust-all-tools
VEIL_ACP_CWD        ?= $(CURDIR)

export VEIL_MODEL_PROVIDER
export VEIL_MODEL_NAME
export VEIL_ACP_COMMAND
export VEIL_ACP_ARGS
export VEIL_ACP_CWD
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
	runtime gen-runtime build-runtime clean-runtime stubs check test test-roundtrip

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

# Full stack: veil serve (backend) + veil-viewer (frontend).
# Alias: serve-examples
# Stop: Ctrl-C  or  make serve-stop
# Ports: PORT=3001 (API), VIEWER_PORT=5173 (UI). Viewer fetches API at :3001.
serve serve-examples: veil viewer-install
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
	@echo "Starting VEIL dev stack…"
	@echo "  Backend:  http://localhost:$(PORT)   (veil serve $(EXAMPLES))"
	@echo "  Frontend: http://localhost:$(VIEWER_PORT)  (veil-viewer)"
	@echo "  Open:     http://localhost:$(VIEWER_PORT)"
	@echo "  Agent:    VEIL_MODEL_PROVIDER=$(VEIL_MODEL_PROVIDER)  model=$(VEIL_MODEL_NAME)"
	@if [ "$(VEIL_MODEL_PROVIDER)" = "ollama" ]; then \
		if curl -sf http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then \
			echo "  Ollama:   up at :11434"; \
		else \
			echo "  Ollama:   WARN not reachable at :11434 (start ollama or use VEIL_MODEL_PROVIDER=echo)"; \
		fi; \
	fi
	@if [ "$(VEIL_MODEL_PROVIDER)" = "acp" ] || [ "$(VEIL_MODEL_PROVIDER)" = "kiro" ]; then \
		echo "  ACP:      $(VEIL_ACP_COMMAND) $(VEIL_ACP_ARGS)"; \
		echo "  ACP cwd:  $(VEIL_ACP_CWD)"; \
		if command -v $(VEIL_ACP_COMMAND) >/dev/null 2>&1; then \
			echo "  ACP bin:  ok ($$(command -v $(VEIL_ACP_COMMAND)))"; \
		else \
			echo "  ACP bin:  WARN $(VEIL_ACP_COMMAND) not on PATH — install Kiro CLI"; \
		fi; \
	fi
	@echo "  Stop:     Ctrl-C  or  make serve-stop"
	@echo ""
	@$(VEIL_BIN) serve $(EXAMPLES) -p $(PORT) & echo $$! > $(API_PID); \
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

# API only (no viewer)
serve-api: veil
	@if ss -tln 2>/dev/null | grep -qE ":$(PORT)\\b" || \
	   netstat -tln 2>/dev/null | grep -qE ":$(PORT)\\b"; then \
		echo "error: port $(PORT) is already in use.  make serve-stop"; \
		exit 1; \
	fi
	@echo "API only: http://localhost:$(PORT)  (Ctrl-C to stop)"
	@echo "  Agent: VEIL_MODEL_PROVIDER=$(VEIL_MODEL_PROVIDER)  model=$(VEIL_MODEL_NAME)"
	$(VEIL_BIN) serve $(EXAMPLES) -p $(PORT)

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
	@-pkill -f 'veil serve $(EXAMPLES)' 2>/dev/null || true
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
