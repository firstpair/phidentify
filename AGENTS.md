# phidentify: agent notes

Shared rules for every repository under `~/src` are in `~/src/firstpair/AGENTS.md`
(toolchains, OCaml vs OxCaml switches, Rust pins, exiftool-rs). This file only adds
local constraints.

- Each identity is separate. Never let one stand in for another, and never report
  a missing, failed or unsupported identity as a match.
- The Lightroom fingerprint is qualified only against fingerprints Lightroom Classic
  itself stored, for the DNG strata it accepts. Extending it to another format
  needs native oracles for that format first.
- Keep the raw-block identity equal to dng-compare's `dng-subset` comparison.
  Changing what it covers means a new domain string, since stored digests change.
- Tests use synthetic fixtures only. Never commit photos, catalogs, personal
  paths, catalog or cloud identifiers, Adobe binaries or disassembly.
- Build and test with `make test`, and the OCaml package in both switches:
  `opam exec --switch default -- dune test --force` and
  `opam exec --switch 5.2.0+ox -- dune test --force` in `ocaml/`.
- The TIFF/DNG walker is `tiffgraph`, its own repository shared with Semantic
  Unison; its grammars are Unison's negotiated profiles. Don't copy it back here.
- Publication (manual, cover, blog TextPack, First Pair shelf) follows `FIRSTPAIR.md`
  and `~/src/firstpair/AGENTS.md`. Keep the manual in step with the code.
