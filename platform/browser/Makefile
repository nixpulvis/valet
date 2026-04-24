# Browser extension build driver. Builds the WASM package via wasm-pack
# and writes the WebExtensions native-messaging manifest pointing at the
# `valetd` binary at the Firefox-specific per-OS location.

include ../../Makefile.common

WASM_PACK ?= wasm-pack

# wasm-pack takes --dev for debug and no flag for release (inverse of
# cargo's --release flag), so translate separately from RELEASE_FLAG.
ifeq ($(RELEASE),1)
WASM_MODE_FLAG :=
else
WASM_MODE_FLAG := --dev
endif

REPO_ROOT := $(abspath ../..)
VALETD    := $(REPO_ROOT)/target/$(PROFILE_DIR)/valetd
MANIFEST  := com.nixpulvis.valet.json

UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
NM_DIR := $(HOME)/Library/Application Support/Mozilla/NativeMessagingHosts
else ifeq ($(UNAME_S),Linux)
NM_DIR := $(HOME)/.mozilla/native-messaging-hosts
else
$(error Unsupported OS: $(UNAME_S))
endif

.PHONY: all install

# Default: build the WASM extension package.
all:
	cd $(REPO_ROOT) && $(WASM_PACK) build --target web $(WASM_MODE_FLAG) platform/browser

# Build valetd and register the native-messaging manifest. Depends on
# `all` so the WASM package is rebuilt alongside.
install: all
	cd $(REPO_ROOT) && $(CARGO) build --bin valetd --features protocol-all $(RELEASE_FLAG)
	@[ -x "$(VALETD)" ] || { echo "Build did not produce $(VALETD)"; exit 1; }
	@mkdir -p "$(NM_DIR)"
	@printf '%s\n' \
	    '{' \
	    '  "name": "com.nixpulvis.valet",' \
	    '  "description": "Valet password manager native host",' \
	    '  "path": "$(VALETD)",' \
	    '  "type": "stdio",' \
	    '  "allowed_extensions": ["valet@nixpulvis.com"]' \
	    '}' \
	    > "$(NM_DIR)/$(MANIFEST)"
	@chmod 600 "$(NM_DIR)/$(MANIFEST)"
	@echo "Installed native messaging manifest:"
	@echo "  $(NM_DIR)/$(MANIFEST)"
	@echo "Pointing at:"
	@echo "  $(VALETD)"
	@echo
	@echo "Run a long-lived daemon with: $(VALETD) --transport=socket"
	@echo "Override via VALET_DB / VALET_SOCKET / VALET_BACKEND before launching the browser."
	@if pkill -x valetd >/dev/null 2>&1; then \
	    echo; \
	    echo "Stopped running valetd. Reload the extension to pick up the rebuilt daemon."; \
	fi
