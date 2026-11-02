slug: phidentify
shelf: photography, unix
default_edition: preview

# Publication contract

This repository owns the code, the manual in `docs/phidentify-manual.md`, the cover
and headboard in `cover/`, the build inputs in `book.build.json` and `docs/book/`,
the built artifacts in `dist/`, and the blog post in `docs/blog/phidentify/`.
FirstPair owns catalog entries, stable download and reader routes, and Blob
manifests. Follow the shared `~/src/firstpair/AGENTS.md` publication and
source-ownership rules.

The manual is by Alexy Khrabrov, the author on the cover and in all title metadata.
Claude is credited as co-author only on the information page ("About this manual")
and in the blog post's closing colophon. The code and manual are
MIT. The manual describes the Lightroom Classic fingerprint at the level of this
repository's source only: no Adobe code, binaries, disassembly or decompiled bodies.
Photos, catalogs, personal paths and catalog or cloud identifiers stay out.

`cover/phidentify-cover-text.png` is the book cover and
`cover/phidentify-headboard.png`, without text, is the shelf and blog headboard.
Both are the user's art, used unchanged.

## Ownership

- Source: `docs/phidentify-manual.md`, `docs/book/*`, `book.build.json`, `cover/*`.
- Generated and committed: `dist/` (PDF, EPUB, single-file HTML, chapters,
  `VERSION.md`) and `docs/blog/phidentify/dist/`.
- FirstPair: `public/catalog.json`, `public/phidentify/`, `book-uploads/*`, route maps.

## Build

Commit and push the complete source, then run the canonical preflight, builder and
verifier from the repository root:

```sh
uv run --no-project python "$HOME/src/firstpair/publishing/scripts/git_publish_preflight.py" .
"$HOME/src/firstpair/publishing/scripts/build-library-book.sh" --repo-root "$PWD" --print-plan
"$HOME/src/firstpair/publishing/scripts/build-library-book.sh" --repo-root "$PWD"
"$HOME/src/firstpair/publishing/scripts/verify-library-book.sh" "$PWD/dist"
```

Review every PDF page, and the EPUB and both HTML readers (single file and
chapters) for contents, links, cover, and version and commit binding. Then commit
and push `dist/`.

## Publish

The title is published as a **preview** edition. Publish only this title through
the shared FirstPair publisher, dry run first:

```sh
cd "$HOME/src/firstpair"
npm run library:publish -- "$HOME/src/phidentify" \
  --slug phidentify --shelf photography,unix --title 'phidentify' \
  --description 'Separate, named identities for photo files: whole-file SHA-256, DNG encoded raw blocks, the Lightroom Classic original fingerprint, a metadata record and the preserved complement digest, from Rust, C, OCaml and OxCaml.' \
  --kicker 'Fingerprints for photos' \
  --tags photography,dng,lightroom,rust,ocaml,oxcaml \
  --source https://github.com/firstpair/phidentify \
  --dry-run --no-build --no-smoke --no-deploy --no-icloud
```

then the same command without the dry-run flags. Never pass `--full` without the
user's explicit confirmation: it replaces the public preview with the full edition.

Both repositories must be clean and pushed before uploading. Commit and push the
FirstPair manifests (`book-uploads/blob-manifest.json`, `public/catalog.json`,
`deliverable-map.mjs`, `reader-map.mjs`, `public/phidentify/`) in the same session.
Reader-facing URLs are `https://firstpair.org/phidentify/pdf/`, `/phidentify/epub/`,
`/read/phidentify/` and `/read/phidentify/chapters/`; the shelf entry is
`https://firstpair.org/books/phidentify/`.

## Blog

The canonical post and TextPack live in `docs/blog/phidentify/`, slug `phidentify`,
destination `https://firstpair.press/phidentify/`. Commit and push the post and its
assets, stamp the TextPack with `stamp-versioned-blog.sh`, commit and push the
handoff, then deliver it to `~/icloud/blogs` with `publish-versioned-blog.sh`.
File delivery does not post to Ghost; posting to any blog or forum needs the
user's explicit instruction. Add a library `post` link only after the post is live.
