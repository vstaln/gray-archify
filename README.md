<p align="center">
  <img src="assets/gray-logo.svg" alt="gray" width="96">
</p>
<h1 align="center">gray-archify</h1>
<p align="center">Map a repository's architecture into one self-contained interactive HTML file.</p>
<p align="center">
  <a href="https://github.com/vstaln/gray-archify/blob/main/LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-7aa2f7.svg">
  <img alt="rust" src="https://img.shields.io/badge/built%20with-rust-orange.svg">
</p>

Map a repository's architecture into ONE self-contained interactive HTML
file — no external assets, opens offline. The artifact is the deliverable.

## Tools

- **`archify {path?, depth?, out?}`** — walks the repo (≤500 files, skips
  `node_modules`/`.git`/`target`/…), reads sizes/loc, parses import and
  include lines (Rust `use`/`mod`, JS/TS `import`/`require`, Python
  `import`/`from`, Go imports, C/C++ `#include "…"`, Java/Kotlin `import`)
  into a module graph, and writes `out` (default `./archify.html`).
  Replies `wrote <path> (N modules, M edges)`. `depth` (default 2) is the
  max directory depth walked.
- **`archify_prompt {question, path?, depth?}`** — the agent-facing half:
  same walk, returned as a compact markdown outline (dirs → files with loc
  and resolved import targets) for the model to reason over when answering
  `question`.

## The HTML

- Left: expandable directory tree (`<details>`) with per-file loc/size and
  entry-point markers (`main.*`, `index.*`, `lib.rs`, …).
- Right: force-layout SVG module graph — vendored inline JS (~100 lines:
  repulsion + springs + centering ticks, node drag, click to light a
  module's imports). Entry points rendered orange.
- JSON data is inlined with `<` escaped so it can't break the script tag;
  all names HTML-escaped.

## Wire methods used

`plugin/manifest`, `tool/call`, `command/run` (`/archify` → status),
`plugin/shutdown`. No hooks, no capabilities — protocol 1.1.

## Install

```sh
cargo install --path .
gray plugin install ~/.cargo/bin/gray-archify
```

---
Part of the [gray](https://github.com/vstaln/gray) plugin ecosystem —
the open-source AI agent harness. <https://gray.alignment.id>
