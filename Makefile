# DUNE may point at a switch, e.g. DUNE="opam exec --switch 5.2.0+ox -- dune".
DUNE ?= dune
.PHONY: build test rust ocaml clean
build: rust ocaml
rust:
	cd rust && cargo build --release
ocaml: rust
	cd ocaml && $(DUNE) build @install
test: rust
	cd rust && cargo test --release && cargo clippy --release --all-targets -- -D warnings
	cd ocaml && $(DUNE) test --force
clean:
	cd rust && cargo clean
	cd ocaml && $(DUNE) clean
