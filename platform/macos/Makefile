# Build driver for the Valet macOS AutoFill credential provider extension.
#
# Produces build/ValetAutoFill.app containing build/ValetAutoFillExt.appex in
# Contents/PlugIns, both signed with ${SIGN}. Requires only command-line
# tooling: xcrun, swiftc, codesign, plutil, cargo, make.

ARCH            ?= arm64
# Rust uses aarch64 where Apple tooling uses arm64; translate for the Rust target triple.
RUST_ARCH       := $(if $(filter arm64,$(ARCH)),aarch64,$(ARCH))
TARGET_TRIPLE   ?= $(RUST_ARCH)-apple-darwin
MACOS_VERSION   ?= 14.0
SIGN            ?= -
DEVELOPMENT_TEAM ?=

# Toggle between real daemon mode (default) and in-process stub mode.
# `make BUILD_MODE=stub` links the stub entry point and defines VALETD_FFI_STUB
# for the Swift sources so ValetClient.default() uses valetd_ffi_client_new_stub.
BUILD_MODE      ?= connected

# Default to debug builds. Set RELEASE=1 to build release.
RELEASE         ?=
ifeq ($(RELEASE),1)
RELEASE_FLAG    := --release
PROFILE_DIR     := release
else
RELEASE_FLAG    :=
PROFILE_DIR     := debug
endif

ifeq ($(BUILD_MODE),stub)
CARGO_FEATURES  := --features stub
# -D VALETD_FFI_STUB is a Swift conditional; -Xcc -D... mirrors it to the C
# preprocessor so the gated prototype in valetd.h is visible to the importer.
SWIFT_DEFINES   := -D VALETD_FFI_STUB -Xcc -DVALETD_FFI_STUB
else
CARGO_FEATURES  := --no-default-features --features native
SWIFT_DEFINES   :=
endif

SDK             := $(shell xcrun --sdk macosx --show-sdk-path)
SWIFT_TARGET    := $(ARCH)-apple-macos$(MACOS_VERSION)

VALETD_CRATE_DIR := ../../valetd
VALETD_LIB_DIR  := ../../target/$(TARGET_TRIPLE)/$(PROFILE_DIR)
# Profile-agnostic include dir written by build.rs (see build_support/cbindgen.rs).
# Holds both valet.h and valetd.h; modulemap exposes them as one Swift module.
VALETD_INCLUDE_DIR := ../../target/$(TARGET_TRIPLE)/include
VALETD_STATICLIB := $(VALETD_LIB_DIR)/libvaletd.a
VALETD_MODMAP   := $(VALETD_INCLUDE_DIR)/module.modulemap

BUILD           := build
APP_BUNDLE      := $(BUILD)/ValetAutoFill.app
APP_BIN         := $(BUILD)/ValetAutoFill
EXT_BUNDLE      := $(BUILD)/ValetAutoFillExt.appex
EXT_BIN         := $(BUILD)/ValetAutoFillExt

SHARED_SWIFT    := Shared/ValetClient.swift Shared/IdentitySync.swift
EXT_SWIFT       := Extension/main.swift Extension/CredentialProviderViewController.swift Extension/CredentialListView.swift Extension/UnlockView.swift
APP_SWIFT       := App/main.swift

SWIFTC          := xcrun -sdk macosx swiftc
SWIFTC_COMMON   := $(SWIFTC) -sdk $(SDK) -target $(SWIFT_TARGET) -O \
                  -Xcc -fmodule-map-file=$(VALETD_MODMAP) \
                  -I $(VALETD_INCLUDE_DIR) \
                  -L $(VALETD_LIB_DIR) \
                  $(SWIFT_DEFINES)

CODESIGN_FLAGS  := --force --sign $(SIGN) --options runtime --timestamp=none

CC_JSON         := compile_commands.json

.PHONY: all rust app ext bundles install verify universal clean compile_commands

all: bundles $(CC_JSON)

rust:
	MACOSX_DEPLOYMENT_TARGET=$(MACOS_VERSION) \
	    cargo build $(RELEASE_FLAG) -p valetd $(CARGO_FEATURES) --target $(TARGET_TRIPLE)

$(VALETD_STATICLIB): rust

# Written into the include dir so `header "valet.h"` resolves via the
# modulemap's own directory, keeping the map arch-agnostic.
$(VALETD_MODMAP): rust
	@printf 'module Valetd {\n    header "valet.h"\n    header "valetd.h"\n    export *\n}\n' > $@

$(EXT_BIN): rust $(VALETD_MODMAP) $(EXT_SWIFT) $(SHARED_SWIFT)
	@mkdir -p $(BUILD)
	$(SWIFTC_COMMON) \
	    -framework AuthenticationServices \
	    -framework SwiftUI \
	    -framework AppKit \
	    -framework Foundation \
	    -lvaletd \
	    -module-name ValetAutoFillExt \
	    -emit-executable \
	    -o $@ \
	    $(SHARED_SWIFT) $(EXT_SWIFT)

$(APP_BIN): rust $(VALETD_MODMAP) $(APP_SWIFT) $(SHARED_SWIFT)
	@mkdir -p $(BUILD)
	$(SWIFTC_COMMON) \
	    -framework AuthenticationServices \
	    -framework AppKit \
	    -framework Foundation \
	    -lvaletd \
	    -module-name ValetAutoFill \
	    -emit-executable \
	    -o $@ \
	    $(SHARED_SWIFT) $(APP_SWIFT)

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
	    ../../target/aarch64-apple-darwin/$(PROFILE_DIR)/libvaletd.a \
	    ../../target/x86_64-apple-darwin/$(PROFILE_DIR)/libvaletd.a \
	    -output ../../target/universal-apple-darwin/$(PROFILE_DIR)/libvaletd.a
	$(MAKE) TARGET_TRIPLE=universal-apple-darwin SWIFT_TARGET="arm64-apple-macos$(MACOS_VERSION)" $(APP_BUNDLE)

compile_commands: $(CC_JSON)

# Emits a compile_commands.json so SourceKit-LSP can type-check the Swift
# sources with the same flags used by the build. Regenerated whenever the
# Makefile changes (flag set is derived from variables above).
$(CC_JSON): Makefile scripts/gen-compile-commands.py
	@CWD="$(CURDIR)" \
	 SDK="$(SDK)" \
	 TARGET="$(SWIFT_TARGET)" \
	 MODMAP="$(VALETD_MODMAP)" \
	 INCLUDE="$(VALETD_INCLUDE_DIR)" \
	 DEFINES="$(SWIFT_DEFINES)" \
	 EXT_FILES="$(SHARED_SWIFT) $(EXT_SWIFT)" \
	 APP_FILES="$(SHARED_SWIFT) $(APP_SWIFT)" \
	 python3 scripts/gen-compile-commands.py > $@
	@echo "wrote $@"

clean:
	rm -rf $(BUILD)
	rm -f $(CC_JSON)
	cargo clean -p valetd
