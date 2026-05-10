.PHONY: build build-lite build-rust build-xcode build-x86_64 generate clean run dmg dmg-lite dmg-x86

ARCH := aarch64-apple-darwin
XCODE_ARCH := arm64

build: generate build-rust build-xcode install-cli

build-lite: generate
	cd KoeApp && xcodebuild -project Koe.xcodeproj -scheme Koe-lite -configuration Release ARCHS=arm64 build

build-x86_64: generate
	cd KoeApp && xcodebuild -project Koe.xcodeproj -scheme Koe-x86 -configuration Release ARCHS=x86_64 ONLY_ACTIVE_ARCH=NO build

generate:
	cd KoeApp && xcodegen generate

build-rust:
	cargo build --manifest-path koe-core/Cargo.toml --release --target $(ARCH)
	cargo build --package koe-cli --release --target $(ARCH)

build-xcode:
	cd KoeApp && xcodebuild -project Koe.xcodeproj -scheme Koe -configuration Release ARCHS=$(XCODE_ARCH) build

install-cli:
	@APP_DIR=$$(xcodebuild -project KoeApp/Koe.xcodeproj -scheme Koe -configuration Release -showBuildSettings 2>/dev/null | grep ' BUILD_DIR' | head -1 | awk '{print $$3}')/Release/Koe.app/Contents/MacOS; \
	cp target/$(ARCH)/release/koe "$$APP_DIR/koe-cli"; \
	echo "Installed koe-cli into $$APP_DIR"

clean:
	cargo clean
	cd KoeApp && xcodebuild -project Koe.xcodeproj -scheme Koe clean

run:
	open "$$(xcodebuild -project KoeApp/Koe.xcodeproj -scheme Koe -configuration Debug -showBuildSettings 2>/dev/null | grep ' BUILD_DIR' | head -1 | awk '{print $$3}')/Debug/Koe.app"

# ---------------------------------------------------------------------------
# DMG packaging
# ---------------------------------------------------------------------------
dmg: build
	@./scripts/package-dmg.sh Koe

dmg-lite: build-lite
	@./scripts/package-dmg.sh Koe-lite

dmg-x86: build-x86_64
	@./scripts/package-dmg.sh Koe-x86
