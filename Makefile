.PHONY: all build release clean

all: release

release:
	cargo build --release

build:
	cargo build

clean:
	cargo clean
