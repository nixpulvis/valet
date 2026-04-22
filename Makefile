# Top-level build driver. Delegates to cargo for the workspace and to
# per-platform make targets for the native integrations.

include Makefile.common

.PHONY: all workspace app browser macos clean docs
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
	$(MAKE) -C platform/browser $(if $(RELEASE),RELEASE=$(RELEASE))

# Build valetd and register the browser's native-messaging manifest so
# the extension can talk to it.
install-browser:
	$(MAKE) -C platform/browser install $(if $(RELEASE),RELEASE=$(RELEASE))

# macOS AutoFill credential provider extension. Its own Makefile handles
# codesigning and bundle layout; forward variables via the environment.
macos:
	$(MAKE) -C platform/macos $(if $(RELEASE),RELEASE=$(RELEASE))

# Copy the signed AutoFill app bundle into /Applications.
install-macos:
	$(MAKE) -C platform/macos install $(if $(RELEASE),RELEASE=$(RELEASE))

clean:
	$(CARGO) clean
	$(MAKE) -C platform/macos clean

# Rustdoc for the workspace. Delegates to the `cargo docs` alias in
# .cargo/config.toml, which pins --all-features --no-deps so every
# intra-doc link resolves.
docs:
	$(CARGO) docs
