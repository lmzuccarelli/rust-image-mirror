.PHONY: all test build clean

all: clean test build

build: 
	cargo build

test: clean
	CARGO_INCREMENTAL=0 RUSTFLAGS='-Cinstrument-coverage' LLVM_PROFILE_FILE='cargo-test-%p-%m.profraw' cargo test  -- --nocapture

cover:
	grcov . --binary-path ./target/debug/deps/ -s . -t html --branch --ignore-not-existing --ignore '../*' --ignore "/*" --ignore "src/main.rs" --ignore "src/api/*"  -o target/coverage/html
	cp target/coverage/html/html/badges/flat.svg assets/

run:
	cargo run -- --config imagesetconfig.yaml --diff-tar true  --loglevel info

clean:
	rm -rf cargo-test*
