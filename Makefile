# Top-level build driver. Delegates to cargo for the workspace and to
# per-platform make / xtask commands for the native integrations.

CARGO ?= cargo

# Default to debug builds. Set RELEASE=1 to build release everywhere.
RELEASE ?=
ifeq ($(RELEASE),1)
RELEASE_FLAG := --release
else
RELEASE_FLAG :=
endif

.PHONY: all workspace app browser macos clean
.PHONY: install-browser install-macos

all: workspace browser macos

# Pure cargo crates: core library, valetd daemon, CLI, GUI-less checks.
workspace:
	$(CARGO) build --workspace $(RELEASE_FLAG)

# macOS GUI app bundle (separate from the AutoFill extension below).
app:
	$(CARGO) bundle $(RELEASE_FLAG) --bin gui --features gui

# Browser extension WASM package.
browser:
	$(CARGO) browser-xtask build-wasm $(RELEASE_FLAG)

# Build native-host shim + valetd and register the browser's native
# messaging manifest so the extension can talk to the daemon.
install-browser:
	$(CARGO) browser-xtask build-install $(RELEASE_FLAG)

# macOS AutoFill credential provider extension. Its own Makefile handles
# codesigning and bundle layout; forward variables via the environment.
macos:
	$(MAKE) -C platform/macos RELEASE=$(RELEASE)

# Copy the signed AutoFill app bundle into /Applications.
install-macos:
	$(MAKE) -C platform/macos install RELEASE=$(RELEASE)

clean:
	$(CARGO) clean
	$(MAKE) -C platform/macos clean
