.PHONY: build
SHELL := /usr/bin/env bash

# Build in release mode by default, unless RELEASE=false
ifeq ($(RELEASE), false)
		cargoflag :=
		targetdir := debug
else
		cargoflag := --release
		targetdir := release
endif

build:
	cargo build $(cargoflag)

fix:
	cargo fmt
	cargo clippy --fix

package:
	# Clean and prepare target/package folder
	rm -rf target/package
	mkdir -p target/package
	# Copy binaries
	cp target/$(targetdir)/aw-notify target/package/aw-notify
	# Copy everything into `dist/aw-notify`
	mkdir -p dist
	rm -rf dist/aw-notify
	cp -rf target/package dist/aw-notify

clean:
	cargo clean
