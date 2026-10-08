# gray-archify

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

## Upstream

[tt-a1i/archify](https://github.com/tt-a1i/archify) — MIT. Behavioral port
scoped to the agent-facing core (repo → one HTML module map + markdown
outline); upstream is a broader interactive-visual generator, nothing was
copied verbatim.

## Install

```sh
cargo install --path .
gray plugin install ~/.cargo/bin/gray-archify
```
