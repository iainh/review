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
**-** to zoom, **0** to fit the page, and **Q** or **Escape** to quit when not
editing a field. Zoomed pages can be scrolled horizontally and vertically.

Use **Ctrl+G** (**Cmd+G** on macOS) to select the page field. Enter a page
number and press **Enter** or click **Go**. Page numbers start at one; invalid
numbers leave the current page unchanged. **Escape** cancels page editing.

## Design

- MuPDF rasterizes only the current page at the display scale.
- wgpu uploads that page once and composites it as a texture on the GPU.
- The window redraws on demand rather than continuously.
- Native egui controls share the winit window and wgpu surface with the page;
  there is no web runtime.

## Testing

Run `cargo test` and `cargo clippy --all-targets -- -D warnings`.
`bash tests/wayland.sh` exercises native keyboard and pointer input under Sway
using the OpenID Connect handbook. It requires `swaymsg`, `wtype`, `grim`,
`jq`, and `curl`, plus a running Sway session with `WAYLAND_DISPLAY`,
`XDG_RUNTIME_DIR`, and `SWAYSOCK` set. Use a disposable session: the test
opens and closes its own Review window. Set `REVIEW_TEST_PDF` to a local copy
of the handbook and `REVIEW_SCREENSHOTS` to save screenshots for inspection.

This is a foundation, not a complete viewer. Search, text selection, links,
annotations, tabs, and persistent preferences are not implemented yet.

## Licence

Review is licensed under the [GNU Affero General Public License v3.0](LICENSE)
only (`AGPL-3.0-only`). MuPDF is also AGPLv3-licensed; a commercial MuPDF
licence is required for distribution that does not comply with the AGPL.
