# Review

Review is an early-stage, lightweight cross-platform PDF viewer written in Rust.
It renders PDF pages with [MuPDF](https://mupdf.com/) and presents them through a
hardware-accelerated [wgpu](https://wgpu.rs/) surface.

## Run

Install a current stable Rust toolchain and a C/C++ toolchain with CMake and
libclang, then run:

```sh
cargo run --release -- document.pdf
```

Use **Left**/**Page Up** and **Right**/**Page Down** to change pages, **+** and
**-** to zoom, **0** to fit the page, and **Q** or **Escape** to quit.

## Design

- MuPDF rasterizes only the current page at the display scale.
- wgpu uploads that page once and composites it as a texture on the GPU.
- The window redraws on demand rather than continuously.
- The code uses winit directly; there is no web runtime or general-purpose UI
  framework in the rendering path.

This is a foundation, not a complete viewer. Search, text selection, links,
annotations, tabs, and persistent preferences are not implemented yet.

## Licence

Review is licensed under the [GNU Affero General Public License v3.0](LICENSE)
only (`AGPL-3.0-only`). MuPDF is also AGPLv3-licensed; a commercial MuPDF
licence is required for distribution that does not comply with the AGPL.
