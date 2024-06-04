SHELL:=/bin/bash
CARGO_TARGET_DIR?=target
all: clean rpm oci

clean:
	sudo rm -rf out

# CARGO_TARGET_DIR faff is to workaround https://github.com/cat-in-136/cargo-generate-rpm/issues/77 for
# maintainers who set CARGO_TARGET_DIR in their environment
rpm: clean
	cargo build --release
	mkdir -p target/release/
	cp $(CARGO_TARGET_DIR)/release/rpmoci target/release/ || /bin/true
	CARGO_TARGET_DIR=target cargo generate-rpm
	mkdir -p out
	cp target/generate-rpm/rpmoci-`$(CARGO_TARGET_DIR)/release/rpmoci --version | cut -d ' ' -f 2`*.rpm out/

oci: rpm
	sudo $(CARGO_TARGET_DIR)/release/rpmoci build -v --image out/rpmoci --tag `$(CARGO_TARGET_DIR)/release/rpmoci --version | cut -d ' ' -f 2`
