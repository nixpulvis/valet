# Build driver for the Valet macOS AutoFill credential provider extension.
#
# Produces build/ValetAutoFill.app containing build/ValetAutoFillExt.appex in
# Contents/PlugIns, both signed with ${SIGN}. Requires only command-line
# tooling: xcrun, swiftc, codesign, plutil, cargo, make.

include ../../Makefile.common

ARCH            ?= arm64
# Rust uses aarch64 where Apple tooling uses arm64; translate for the Rust target triple.
RUST_ARCH       := $(if $(filter arm64,$(ARCH)),aarch64,$(ARCH))
TARGET_TRIPLE   ?= $(RUST_ARCH)-apple-darwin
MACOS_VERSION   ?= 14.0
SIGN            ?= -
DEVELOPMENT_TEAM ?=

# Toggle between real socket mode (default) and in-process embedded
# mode. `make BUILD_MODE=embedded` links the embedded entry point and
# defines VALET_FFI_PROTOCOL_EMBEDDED for the Swift sources so
# ValetClient.default() uses valet_ffi_client_new_embedded.
BUILD_MODE      ?= socket

ifeq ($(BUILD_MODE),embedded)
CARGO_FEATURES  := --no-default-features --features "ffi protocol-embedded"
# -Xcc -D VALET_FFI_PROTOCOL_EMBEDDED unwraps the embedded constructor
# in valet.h; Swift's ValetClient.default() branches on the matching
# -D on the Swift side.
SWIFT_DEFINES   := -D VALET_FFI_PROTOCOL_EMBEDDED -Xcc -DVALET_FFI_PROTOCOL_EMBEDDED
else
CARGO_FEATURES  := --no-default-features --features "ffi protocol-socket"
# -Xcc -DVALET_FFI_PROTOCOL_SOCKET unwraps the socket constructor and
# the shared per-method prototypes in valet.h. No Swift-side `-D` is
# needed because ValetClient.swift's default branch is the socket one.
SWIFT_DEFINES   := -Xcc -DVALET_FFI_PROTOCOL_SOCKET
endif

SDK             := $(shell xcrun --sdk macosx --show-sdk-path)
SWIFT_TARGET    := $(ARCH)-apple-macos$(MACOS_VERSION)

VALET_CRATE_DIR := ../..
VALET_LIB_DIR   := ../../target/$(TARGET_TRIPLE)/$(PROFILE_DIR)
# Profile-agnostic include dir written by build.rs (see valet-build/src/lib.rs).
# Holds valet.h; modulemap exposes it as the `Valet` Swift module.
VALET_INCLUDE_DIR := ../../target/$(TARGET_TRIPLE)/include
VALET_STATICLIB := $(VALET_LIB_DIR)/libvalet.a
VALET_MODMAP    := $(VALET_INCLUDE_DIR)/module.modulemap

BUILD           := build
APP_BUNDLE      := $(BUILD)/ValetAutoFill.app
APP_BIN         := $(BUILD)/ValetAutoFill
EXT_BUNDLE      := $(BUILD)/ValetAutoFillExt.appex
EXT_BIN         := $(BUILD)/ValetAutoFillExt
EXT_OBJDIR      := $(BUILD)/ext-obj
APP_OBJDIR      := $(BUILD)/app-obj

SHARED_SWIFT    := Shared/ValetClient.swift Shared/IdentitySync.swift
EXT_SWIFT       := Extension/main.swift Extension/CredentialProviderViewController.swift Extension/CredentialListView.swift Extension/UnlockView.swift
APP_SWIFT       := App/main.swift

EXT_ALL_SWIFT   := $(SHARED_SWIFT) $(EXT_SWIFT)
APP_ALL_SWIFT   := $(SHARED_SWIFT) $(APP_SWIFT)

# Mirror scripts/gen-output-file-map.py: slashes in source paths become
# underscores so Shared/Foo.swift and Extension/Foo.swift don't collide.
EXT_OBJS        := $(patsubst %.swift,$(EXT_OBJDIR)/%.o,$(subst /,_,$(EXT_ALL_SWIFT)))
APP_OBJS        := $(patsubst %.swift,$(APP_OBJDIR)/%.o,$(subst /,_,$(APP_ALL_SWIFT)))
EXT_OUTPUT_MAP  := $(EXT_OBJDIR)/output-file-map.json
APP_OUTPUT_MAP  := $(APP_OBJDIR)/output-file-map.json

SWIFTC          := xcrun -sdk macosx swiftc
SWIFT_OPT       ?= -O
SWIFTC_COMMON   := $(SWIFTC) -sdk $(SDK) -target $(SWIFT_TARGET) $(SWIFT_OPT) \
                  -Xcc -fmodule-map-file=$(VALET_MODMAP) \
                  -I $(VALET_INCLUDE_DIR) \
                  -L $(VALET_LIB_DIR) \
                  $(SWIFT_DEFINES)

CODESIGN_FLAGS  := --force --sign $(SIGN) --options runtime --timestamp=none

CC_JSON         := compile_commands.json

.PHONY: all rust app ext bundles install verify universal clean compile_commands

all: bundles $(CC_JSON)

rust:
	MACOSX_DEPLOYMENT_TARGET=$(MACOS_VERSION) \
	    cargo build $(RELEASE_FLAG) $(CARGO_FEATURES) --target $(TARGET_TRIPLE) --manifest-path $(VALET_CRATE_DIR)/Cargo.toml

$(VALET_STATICLIB): rust

# Written into the include dir so `header "valet.h"` resolves via the
# modulemap's own directory, keeping the map arch-agnostic.
# Content-preserving: only overwrite when the modmap text actually changes,
# so dependents aren't invalidated by an unchanged rewrite on every run.
$(VALET_MODMAP): rust
	@mkdir -p $(@D)
	@printf 'module Valet {\n    header "valet.h"\n    export *\n}\n' > $@.tmp
	@if cmp -s $@.tmp $@ 2>/dev/null; then rm $@.tmp; else mv $@.tmp $@; fi

$(EXT_OUTPUT_MAP): Makefile scripts/gen-output-file-map.py
	@mkdir -p $(EXT_OBJDIR)
	@python3 scripts/gen-output-file-map.py $(EXT_OBJDIR) $(EXT_ALL_SWIFT) > $@

$(APP_OUTPUT_MAP): Makefile scripts/gen-output-file-map.py
	@mkdir -p $(APP_OBJDIR)
	@python3 scripts/gen-output-file-map.py $(APP_OBJDIR) $(APP_ALL_SWIFT) > $@

# Two-phase build: swiftc -c with -incremental + -output-file-map lets swiftc
# skip unchanged files across invocations; the separate link step then assembles
# the executable from the per-file object files.
$(EXT_BIN): $(VALET_STATICLIB) $(VALET_MODMAP) $(EXT_ALL_SWIFT) $(EXT_OUTPUT_MAP)
	@mkdir -p $(BUILD) $(EXT_OBJDIR)
	$(SWIFTC_COMMON) \
	    -module-name ValetAutoFillExt \
	    -incremental \
	    -enable-batch-mode \
	    -output-file-map $(EXT_OUTPUT_MAP) \
	    -c \
	    $(EXT_ALL_SWIFT)
	$(SWIFTC) -sdk $(SDK) -target $(SWIFT_TARGET) \
	    -framework AuthenticationServices \
	    -framework SwiftUI \
	    -framework AppKit \
	    -framework Foundation \
	    -L $(VALET_LIB_DIR) -lvalet \
	    -emit-executable \
	    -o $@ \
	    $(EXT_OBJS)

$(APP_BIN): $(VALET_STATICLIB) $(VALET_MODMAP) $(APP_ALL_SWIFT) $(APP_OUTPUT_MAP)
	@mkdir -p $(BUILD) $(APP_OBJDIR)
	$(SWIFTC_COMMON) \
	    -module-name ValetAutoFill \
	    -incremental \
	    -enable-batch-mode \
	    -output-file-map $(APP_OUTPUT_MAP) \
	    -c \
	    $(APP_ALL_SWIFT)
	$(SWIFTC) -sdk $(SDK) -target $(SWIFT_TARGET) \
	    -framework AuthenticationServices \
	    -framework AppKit \
	    -framework Foundation \
	    -L $(VALET_LIB_DIR) -lvalet \
	    -emit-executable \
	    -o $@ \
	    $(APP_OBJS)

$(EXT_BUNDLE): $(EXT_BIN) Extension/Info.plist Extension/ValetAutoFillExt.entitlements
	rm -rf $@
	mkdir -p $@/Contents/MacOS
	cp $(EXT_BIN) $@/Contents/MacOS/ValetAutoFillExt
	cp Extension/Info.plist $@/Contents/Info.plist
	plutil -convert binary1 $@/Contents/Info.plist
	codesign $(CODESIGN_FLAGS) --entitlements Extension/ValetAutoFillExt.entitlements $@

$(APP_BUNDLE): $(APP_BIN) App/Info.plist App/ValetAutoFill.entitlements $(EXT_BUNDLE)
	rm -rf $@
	mkdir -p $@/Contents/MacOS $@/Contents/PlugIns
	cp $(APP_BIN) $@/Contents/MacOS/ValetAutoFill
	cp App/Info.plist $@/Contents/Info.plist
	plutil -convert binary1 $@/Contents/Info.plist
	cp -R $(EXT_BUNDLE) $@/Contents/PlugIns/
	# Sign the app *without* --deep: the nested .appex is already signed with
	# its own entitlements (including app-sandbox, which pkd requires). --deep
	# would overwrite them with the app's entitlements.
	codesign $(CODESIGN_FLAGS) --entitlements App/ValetAutoFill.entitlements $@

bundles: $(APP_BUNDLE)

install: bundles
	rm -rf /Applications/ValetAutoFill.app
	cp -R $(APP_BUNDLE) /Applications/
	open /Applications/ValetAutoFill.app

verify: bundles
	codesign -dvvv $(APP_BUNDLE)
	codesign -dvvv $(APP_BUNDLE)/Contents/PlugIns/ValetAutoFillExt.appex
	pluginkit -m -v -p com.apple.authentication-services-credential-provider-ui | grep valet || true

universal:
	$(MAKE) ARCH=arm64 rust
	$(MAKE) ARCH=x86_64 rust
	@mkdir -p ../../target/universal-apple-darwin/$(PROFILE_DIR)
	lipo -create \
	    ../../target/aarch64-apple-darwin/$(PROFILE_DIR)/libvalet.a \
	    ../../target/x86_64-apple-darwin/$(PROFILE_DIR)/libvalet.a \
	    -output ../../target/universal-apple-darwin/$(PROFILE_DIR)/libvalet.a
	$(MAKE) TARGET_TRIPLE=universal-apple-darwin SWIFT_TARGET="arm64-apple-macos$(MACOS_VERSION)" $(APP_BUNDLE)

compile_commands: $(CC_JSON)

# Emits a compile_commands.json so SourceKit-LSP can type-check the Swift
# sources with the same flags used by the build. Regenerated whenever the
# Makefile changes (flag set is derived from variables above).
$(CC_JSON): Makefile scripts/gen-compile-commands.py
	@CWD="$(CURDIR)" \
	 SDK="$(SDK)" \
	 TARGET="$(SWIFT_TARGET)" \
	 MODMAP="$(VALET_MODMAP)" \
	 INCLUDE="$(VALET_INCLUDE_DIR)" \
	 DEFINES="$(SWIFT_DEFINES)" \
	 EXT_FILES="$(SHARED_SWIFT) $(EXT_SWIFT)" \
	 APP_FILES="$(SHARED_SWIFT) $(APP_SWIFT)" \
	 python3 scripts/gen-compile-commands.py > $@
	@echo "wrote $@"

clean:
	rm -rf $(BUILD)
	rm -f $(CC_JSON)
	cargo clean --manifest-path $(VALET_CRATE_DIR)/Cargo.toml
