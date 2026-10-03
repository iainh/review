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

Use **Ctrl+F** (**Cmd+F** on macOS) or **Search** to find text. Press **Enter**
or click **Find** to search the document, ignoring case. **Enter**/**F3**
advances to the next occurrence; **Shift+Enter**/**Shift+F3** goes back.
Search wraps at both ends. The current occurrence is orange and other matches
on the page are yellow. **Escape** or **Close** closes search and clears highlights.
Search uses the PDF's text layer; scanned images without text require OCR.

The sidebar has **Outline** and **Pages** tabs. Click a bookmark or preview to
navigate, and use the arrows to collapse or expand nested bookmarks. PDFs
without bookmarks show “This document has no outline.” The current preview is
highlighted and follows page changes. Drag the sidebar edge to resize it; use
**F9** or **Sidebar** to hide or show it.

## Downloads

[GitHub releases](https://github.com/iainh/review/releases) provide a Linux
x86-64 tarball, separate macOS disk images for Apple Silicon and Intel, and a
Windows x86-64 zip. Each package contains the executable, this README and the
licence. `SHA256SUMS` contains checksums for all packages.

Extract the tarball or zip, or mount the disk image and copy its contents to a
local directory. Start the viewer from a terminal with a PDF path:

```sh
./review /path/to/document.pdf
```

On Windows, use `.\review.exe C:\path\to\document.pdf`. The Linux build targets
Ubuntu 22.04 or newer and requires Fontconfig, X11/Wayland libraries and a
working graphics driver. The macOS and Windows builds are not developer-signed
or notarized; your operating system may require approval before running them.
The macOS disk image contains a command-line executable, not a Finder-launchable
`.app` bundle.

## Design

- MuPDF rasterizes the current page at the display scale and visible previews
  at thumbnail resolution. Off-screen preview textures are released.
- wgpu uploads that page once and composites it as a texture on the GPU.
- The window redraws on demand rather than continuously.
- Native egui controls share the winit window and wgpu surface with the page;
  there is no web runtime.

## Testing

Run `cargo test` and `cargo clippy --all-targets -- -D warnings`.
`bash tests/wayland.sh` exercises native keyboard and pointer input under Sway
using the OpenID Connect handbook. It requires `swaymsg`, `wtype`, `grim`,
`jq`, `curl`, a C compiler, `pkg-config`, and Wayland development headers and
`wayland-scanner`, plus a running Sway session with `WAYLAND_DISPLAY`,
`XDG_RUNTIME_DIR`, and `SWAYSOCK` set. Use a disposable session: the test
opens and closes its own Review window. Set `REVIEW_TEST_PDF` to a local copy
of the handbook and `REVIEW_SCREENSHOTS` to save screenshots for inspection.
With `REVIEW_TEST_PDF` set, run `cargo test handbook_ -- --ignored` to check
known search results and bounded preview caching. The handbook has no embedded
outline, so the Wayland test also generates a small nested-bookmark fixture.

For headless Wayland testing, start Sway with `tests/sway.conf`,
`WLR_BACKENDS=headless`, `WLR_RENDERER=pixman`, and `WLR_LIBINPUT_NO_DEVICES=1`.
Set `XDG_RUNTIME_DIR` to a private directory (mode 700). Point `WAYLAND_DISPLAY`
and `SWAYSOCK` at the sockets Sway creates in it. The test assumes a 1280×900
output at scale one. No X server is used.

This is a foundation, not a complete viewer. Text selection, links,
annotations, tabs, and persistent preferences are not implemented yet.

## CI and releases

GitHub Actions checks formatting, runs Clippy and tests, builds optimized
executables, and verifies the packages on all four native targets for pushes to
`main` and pull requests. Packages are available as workflow artifacts.

To publish a release, update the version in `Cargo.toml` and `Cargo.lock`, commit
the change, and push a matching tag, such as `v0.1.0`. The release workflow runs
the same checks and publishes all packages and checksums only after every
platform succeeds. A tag that does not match the package version is rejected.

## Licence

Review is licensed under the [GNU Affero General Public License v3.0](LICENSE)
only (`AGPL-3.0-only`). MuPDF is also AGPLv3-licensed; a commercial MuPDF
licence is required for distribution that does not comply with the AGPL.
