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

Launch without a path to open an empty window. Use **Open** or **Ctrl+O**
(**Cmd+O** on macOS) to choose a PDF in the native file dialog, or drop a PDF
onto the window. Opening another PDF replaces the current document and resets
navigation, zoom, search and previews. Cancelling the dialog or failing to open
a file leaves the current PDF unchanged; errors appear in the window.

Password-protected PDFs open through a masked password prompt. Press **Enter**
or **Open PDF** to unlock; an incorrect password clears the field for retry.
**Cancel** or **Escape** leaves the current document, page and zoom unchanged.
Passwords are never saved or logged. The unlocked document retains its password
only in memory for the session, so rendering workers can authenticate their
own MuPDF documents. The session password is cleared when its last document
or worker source is dropped.

PDF print and copy restrictions appear below the toolbar. Viewing and search
remain available even when copying is prohibited. Owner-password access grants
full permissions; PDFs with an empty user password open without prompting.

`review --help` prints usage without opening a window. Use `review -- -draft.pdf`
for a filename that starts with a dash. Review displays one PDF at a time.

Use **Left**/**Page Up** and **Right**/**Page Down** to change pages, **+** and
**-** to zoom, **0** to fit the page, **1** for 100%, **2** to fit the width,
and **Home**/**End** for the first/last page when no control has focus. Click
the page background to release focus. **Ctrl+Q**/**Ctrl+W** (**Cmd+Q**/**Cmd+W**
on macOS) closes the window; **Escape** dismisses UI and never quits. Zoomed pages
can be scrolled horizontally and vertically. Use **Ctrl+L** (**Cmd+L** on macOS) to
enter a zoom from 10 to 1600%; **Enter** applies it and **Escape** cancels.
Percentage zoom is independent of window size: 100% uses 96 logical pixels
per inch for PDF points (72 per inch). Fit modes adapt to the viewport and
display their effective percentage. Very large page renders report a memory
limit error instead of allocating an unbounded image.

Use **Ctrl+G** (**Cmd+G** on macOS) to select the page field. Enter a page
number and press **Enter** or click **Go**. Page numbers start at one; invalid
numbers leave the current page unchanged. **Escape** cancels page editing.

Use **Ctrl+F** (**Cmd+F** on macOS) or **Search** to find text. Press **Enter**
or click **Find** to search the document, ignoring case. **Enter**/**F3**
advances to the next occurrence; **Shift+Enter**/**Shift+F3** goes back.
Search wraps at both ends. The current occurrence is orange and other matches
on the page are yellow. **Escape** or **Close** closes search and clears highlights.
Search uses the PDF's text layer; scanned images without text require OCR.

Drag across page text to select it, double-click a word or triple-click a
paragraph. Use **Ctrl+A** (**Cmd+A** on macOS) to select all text on the
displayed page, then **Ctrl+C** (**Cmd+C**) or right-click **Copy**. Selection
is blue, survives zoom changes and clears on page changes. **Escape** clears
selection. These shortcuts still edit text when a toolbar
field has focus. Copying requires the PDF's copy permission. Extraction uses
MuPDF's column segmentation and Unicode text layer; complex layouts or PDFs
without accurate Unicode mappings may not copy in the intended reading order.

The sidebar has **Outline** and **Pages** tabs. Click a bookmark or preview to
navigate, and use the arrows to collapse or expand nested bookmarks. PDFs
without bookmarks show “This document has no outline.” The current preview is
highlighted and follows page changes. Drag the sidebar edge to resize it; use
**F9** or **Sidebar** to hide or show it.

Click a PDF link to follow an internal or named destination, including its
position and zoom. Outline entries use the same destination settings. Hover a
link to highlight it and see its destination; right-click a web or email link
for **Copy Link**. Only validated `http`, `https` and `mailto` URI actions can
open your browser or mail app, and only after a click. File links, Launch,
remote-document actions, JavaScript and automatic or chained actions are not
enabled. FitB, FitBH and FitBV destinations currently fit the page box rather
than the content bounding box; destination percentages use the 10–1600% limits.

Use **Back**/**Forward** or **Alt+Left**/**Alt+Right** to retrace page, link,
outline, preview and search jumps. History preserves the page, document
position and zoom at each jump, including later scrolling before going back.
A new jump after going back clears the forward history. Opening another PDF
starts a new history.

## Printing

Use **Print…** or **Ctrl+P** (**Cmd+P** on macOS). Choose **Fit** to scale each
page to the printable area, or **Actual size** for the PDF's physical dimensions
(one PDF point is 1/72 inch). Actual size can clip pages larger than the paper.
Display zoom does not affect printing. Continue to the system print dialog to
choose the printer, page ranges, paper and orientation. Two-sided options are
available only when the printer and driver support them; Review does not
simulate duplex. Cancelling either dialog leaves the document unchanged.
PDF permissions can disable printing or restrict source content to low-resolution
output (up to 150 dpi). Restricted output is rasterized on every platform.

- **Linux:** uses GTK 3's native print operation, including its print-to-file
  backend. Install the GTK 3 runtime and a print backend appropriate for your
  printer (usually CUPS). GTK is loaded only when printing; missing libraries
  produce an in-window error. The dialog cannot be parented to the winit window,
  so your window manager may place it separately. The viewer pauses while the
  native dialog and print rendering run.
- **macOS:** uses PDFKit and AppKit's native print panel, with a private,
  temporary PDF snapshot of the loaded document. No other viewer is launched.
- **Windows:** uses the native Windows print dialog and renders to its printer
  drawing context. Printer properties provide paper, orientation and supported
  duplex options. Up to 32 page ranges can be selected. Print-to-PDF requires a
  installed PDF printer such as Microsoft Print to PDF.

Linux and Windows print one rasterized page at a time, at up to 300 dpi, with
lower resolution for unusually large page sizes to bound memory. macOS retains
vector content when high-quality printing is permitted. These paths submit
actual print jobs only after confirmation in the native dialog; they do not
hand the PDF to another application.

## Accessibility and appearance

**Tab** and **Shift+Tab** move between controls. **Enter** or **Space** activates
a focused button. **F6**/**Shift+F6** cycles the page, zoom, search and page-text
fields that are present. Document navigation keys do not take arrows away from
focused controls. **F1** or **Shortcut help** opens keyboard help, focuses its
close button and restores previous focus on dismissal. Ctrl/Cmd zoom shortcuts
work while a field has focus.

**Appearance** offers **System**, **Light**, **Dark** and **High contrast**.
System follows the light/dark theme reported by the OS, falling back to egui's
dark theme when unavailable. High contrast is an explicit black-and-white UI
with yellow focus/selection borders; winit does not report OS high-contrast mode.
These choices affect controls, not PDF page colours, and last for the window's
lifetime, including when opening another document. Preferences are not saved.

Review connects egui's AccessKit tree to the native winit accessibility adapter
(Windows UI Automation, macOS accessibility and Linux AT-SPI). Controls expose
names, values, selection and focus; native actions are forwarded to egui.

**Accessible controls do not make a raster PDF accessible.** Open **Page text**
or press **Ctrl+Shift+T** (**Cmd+Shift+T** on macOS) to read the current page's
extracted text in a labelled, read-only, selectable text control. It receives
keyboard focus and exposes text and selection through AccessKit. Change pages
with the page field or toolbar to read another page. Only the current page is
exposed, not a continuous accessible document.

Assistive reading stays available after authentication even when PDF copying
is prohibited. Review follows the modern PDF policy that the legacy
accessibility permission is always granted; it does not gate reading on the
copy bit. The page-text pane suppresses clipboard copying for restricted PDFs
and shows that restriction without hiding text from assistive technology.

MuPDF extraction order is not guaranteed reading order. Columns, bidirectional
text, tables, headings, links, figures and PDF tags are not reconstructed as an
accessible semantic document. Scans without a text layer show an explicit
no-text message; Review does not perform OCR. The raster page itself has no
screen-reader text navigation. Linux AT-SPI text, focus and action integration
are tested; VoiceOver, Narrator and NVDA need native-platform manual validation.

## Downloads

[GitHub releases](https://github.com/iainh/review/releases) provide a Linux
x86-64 tarball, separate macOS disk images for Apple Silicon and Intel, and a
Windows x86-64 zip. Each package contains the executable, this README and the
licence. `SHA256SUMS` contains checksums for all packages.

Extract the tarball or zip to a permanent local directory. On macOS, mount the
disk image and drag `Review.app` to Applications, then launch it from Finder.
On Linux and Windows, launch `review` or `review.exe`, or pass a PDF path:

```sh
./review /path/to/document.pdf
```

On Windows, use `.\review.exe C:\path\to\document.pdf`. The Linux build targets
Ubuntu 22.04 or newer and requires Fontconfig, X11/Wayland libraries and a
working graphics driver. The macOS and Windows builds are not developer-signed
or notarized; your operating system may require approval before running them.

## PDF associations

Registration adds Review to **Open with** without changing your default PDF
viewer. Keep the executable at the registered path; rerun registration if you
move it. Linux and Windows scripts register only for the current user and do
not need administrator privileges.

### Linux

From the extracted package, run:

```sh
bash install-desktop.sh
```

From a source checkout, run this after building:

```sh
bash platform/linux/install-desktop.sh "$PWD/target/release/review"
```

This installs `review.desktop` under `$XDG_DATA_HOME/applications` (normally
`~/.local/share/applications`).
Choose Review in your file manager's **Open with** menu. To make it the default:

```sh
xdg-mime default review.desktop application/pdf
```

Run `bash uninstall-desktop.sh` to remove registration. If Review was your
default, choose another viewer before uninstalling. Native file dialogs require
a running XDG Desktop Portal with a GTK, GNOME or KDE file-chooser backend, or
Zenity as a fallback. A wlroots-only portal does not provide a file chooser.

### macOS

After copying `Review.app` to Applications, use Finder's **Open With > Review**.
To make it the default, select a PDF, open **Get Info**, choose Review under
**Open with**, then click **Change All**. Finder can send a PDF to an already
running Review window; it replaces the current document. For multiple files,
the last file is displayed. Remove the app to unregister it; choose another
default viewer first if needed.

### Windows

From PowerShell in the extracted package, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\Install-FileAssociation.ps1
```

This registers Review in **Open with** and **Settings > Apps > Default apps**.
Choose Review there to make it your default for `.pdf`; the script does not
modify Windows' protected `UserChoice` setting. From a source checkout, pass
`-Executable C:\path\to\review.exe` to `platform\windows\Install-FileAssociation.ps1`.

To remove registration, run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\Uninstall-FileAssociation.ps1
```

If Review was your default, choose another PDF viewer before uninstalling.

## Design

- A document worker rasterizes the visible page and previews with its own
  authenticated MuPDF document. No MuPDF pointers cross threads. Rendering
  completion wakes the window even when there is no keyboard or pointer input.
- Visible pages take priority over previews and nearby-page prefetching.
  Superseded queued work is cancelled; in-flight page operations finish but
  stale results are discarded. Replacing a PDF does not wait for old renders.
- The worker keeps at most 12 pixel results within a 128 MiB cache. Large
  renders skip prefetching, and off-screen preview textures are released.
- wgpu uploads that page once and composites it as a texture on the GPU.
- The window redraws on demand rather than continuously.
- Native egui controls share the winit window and wgpu surface with the page;
  there is no web runtime.

## Testing

Run `cargo test` and `cargo clippy --all-targets -- -D warnings`.
`bash tests/linux-desktop.sh` checks Linux registration, path escaping and
removal in a disposable XDG home; it requires `gio` and optionally uses
`desktop-file-validate`. `bash tests/wayland-open.sh` checks native file dialogs,
cancellation, error recovery and replacement using a synthetic PDF in the Sway
session described below. It requires a GTK file-chooser portal or Zenity and
`python3-pyatspi` to activate native chooser controls through accessibility.
`bash tests/wayland-password.sh` uses the same session and chooser workflow to
check masked password entry, retry, keyboard and button cancellation, preserved
navigation, owner access and permission notices with synthetic encrypted PDFs.

`bash tests/wayland-print.sh` checks the print modal, native range and orientation
controls, cancellation and local PDF export in the same disposable Sway session.
It requires GTK 3 and `python3-pyatspi` in addition to the tools below. It never
activates the native Print button or submits a printer job. GTK's export action
always exports all pages; range selection is checked through accessibility.

`tests/accessibility.py` checks Review's own native AT-SPI tree, text, action
forwarding, focus, help dismissal and appearance choices in the same disposable
Sway session and D-Bus session. It requires `python3-pyatspi`, `gdbus`, `wtype`
and `grim`. Build and export the synthetic fixture, then run:

```sh
cargo build
REVIEW_FIXTURE_DIR=/tmp/review-fixtures cargo test export_wayland_fixture -- --ignored
/usr/bin/python3 tests/accessibility.py target/debug/review /tmp/review-fixtures/outline.pdf
```

Create the fixture directory first. Pass an optional third argument to save
screenshots. The test must share Sway's D-Bus session, for example through
`swaymsg exec`, so the adapter and AT-SPI client share the accessibility bus.

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

Run `bash tests/wayland-selection.sh` in the same session, with `wl-clipboard`
installed, to check native selection and clipboard contents. It generates
interleaved-column, rotated-text and copy-restricted PDF fixtures and checks
that page, zoom and search fields retain their own editing shortcuts.

For headless Wayland testing, start Sway with `tests/sway.conf`,
`WLR_BACKENDS=headless`, `WLR_RENDERER=pixman`, and `WLR_LIBINPUT_NO_DEVICES=1`.
Set `XDG_RUNTIME_DIR` to a private directory (mode 700). Point `WAYLAND_DISPLAY`
and `SWAYSOCK` at the sockets Sway creates in it. The test assumes a 1280×900
output at scale one. No X server is used.

`bash tests/wayland-links.sh` uses a synthetic PDF to check internal and named
links, outline position/zoom, same-page Back/Forward and restoration after
scrolling. It captures native hover and Copy Link states and compares restored
page pixels with ImageMagick (`magick` is required in addition to the Wayland
tools above). It never activates external links; Rust tests inspect egui's URL
and clipboard requests without dispatching them to external applications.

This is a foundation, not a complete viewer. Annotations, tabs and persistent
preferences are not implemented yet.

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
