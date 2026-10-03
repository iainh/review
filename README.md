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
search and previews. Previously visited files restore their page, scroll
position, zoom, layout and rotation. Cancelling the dialog or failing to open
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
display their effective percentage. Large pages and high zoom render only the
visible region in overlapping tiles, without allocating a whole-page image.
If the visible tiles alone exceed 128 MiB, raster density falls while page
geometry, zoom and overlays stay unchanged. Fit modes also support huge pages
at percentages below the explicit 10% minimum.

Use **Layout** to choose **Single page**, **Continuous** vertical scrolling or
**Facing pages** in continuous pairs (1–2, 3–4, with an unpaired final page).
Pages retain their proportions and share one scale; multi-page fit modes use
the largest page or spread so scrolling does not change the zoom. Only visible
pages are displayed, with at most 12 visible pages and a 128 MiB tile-texture budget.
Use **Rotate left/right** or **Shift+R**/**R** to rotate in 90° steps. Rotation
also applies to links, search and text-selection highlights; it does not change
the PDF or printed pages. **Hand tool** pans by dragging; **H** toggles between
hand panning and text selection. **Ctrl+wheel** or a pinch gesture over a page
zooms around the pointer. **F11** toggles fullscreen; **Escape** leaves it after
dismissing focused editing or clearing selection.

Use **Ctrl+G** (**Cmd+G** on macOS) to select the page field. Enter a page
number or exact PDF page label, such as `iv` or `A-1`, then press **Enter** or
click **Go**. Numbers always select physical pages, starting at one. Labels
appear beside physical numbers in the toolbar, title, outline and previews.
Invalid or duplicate labels leave the current page unchanged; use a physical
number to disambiguate. **Escape** cancels page editing.

Use **Ctrl+F** (**Cmd+F** on macOS) or **Search** to find text. Press **Enter**
or click **Find** to search the document. Results appear while a background
worker scans, starting at the current page. The progress shows pages scanned
and matches found so far. Click a page-labelled snippet or use **Enter**/**F3**
to advance; **Shift+Enter**/**Shift+F3** goes back. Navigation wraps through
the available results, even while scanning. Later results never move your
selected occurrence.

Search ignores case by default using Unicode case folding. Enable **Case
sensitive** or **Whole words** to narrow matches; changing either option
restarts a submitted search. Whole words follow Unicode word boundaries,
including combining marks. Spaces, tabs and line breaks compare as one space,
so phrases can span lines. Search is literal, not a regular expression, and
does not equate accented and unaccented characters. Editing the query cancels
old work and clears its results; press **Enter** or **Find** to submit again.
The current occurrence is orange and other matches on the page are yellow.
**Escape** or **Close** cancels search and clears highlights. Search uses the
shared native or session OCR text. Recognition and layer changes restart an
active search automatically; native PDF text remains authoritative.

Drag across page text to select it, double-click a word or triple-click a
paragraph. Use **Ctrl+A** (**Cmd+A** on macOS) to select all text on the
active page, then **Ctrl+C** (**Cmd+C**) or right-click **Copy**. Drag between
visible pages in continuous or facing layouts to copy across pages in document
order. Selection is blue, survives scrolling, zoom and rotation, and clears on
explicit page or destination jumps. **Escape** clears
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
position, zoom, layout and rotation at each jump, including later scrolling
before going back. A new jump after going back clears the forward history.
Opening another PDF starts a new history.

## Local OCR for scanned pages

Install [Tesseract](https://tesseract-ocr.github.io/tessdoc/Installation.html)
and the recognition languages you need yourself. Make `tesseract` available on
`PATH`. OCR is optional: Review works without it and shows installation guidance
when the engine or its language data is missing. Review never uploads PDFs or
downloads language data.

Click **OCR**, choose one of the installed languages, then click **Recognize
page N**. Recognition runs only for that page, after your explicit request,
and only if it has no native text. Native text is never replaced. The panel
reports rendering and recognition progress; **Cancel**, **Close OCR** or opening
another PDF discards pending recognition without blocking navigation or zoom.

Recognized Unicode text and page coordinates feed the same text model used by
selection, assistive reading and search. A completed recognition clears stale
selection and restarts an active search. **Ctrl+A**, **Ctrl+C** and the context
menu use recognized text only when PDF copy permission allows it. There is no
separate OCR export that bypasses that permission. The **Page text** pane remains
available for assistive reading when copying is disabled.

OCR text stays in this session. Recognition does not change or save the original PDF,
create a sidecar or add a persistent text layer. Reopening the PDF loses OCR text.
Changing visible PDF layers or editing the document clears session OCR text.
Tesseract word boxes are subdivided into approximate grapheme boxes; recognition,
reading order and highlights can be inaccurate, especially for complex layouts
or rotated scans. OCR images are limited to 8 million pixels and 8192 pixels per
side, at up to 300 dpi, and removed from a private temporary directory after
success, failure or cancellation. An in-flight MuPDF raster finishes off the UI
thread before cancellation cleanup; the Tesseract process is killed and reaped.

## Annotations and saving

Select page text, then choose **Highlight**, **Underline** or **Strike-through**.
Open **Annotations**, choose **Note** and click the page, or choose **Ink** and
drag to draw a stroke. Each ink drag creates one annotation. **Escape** cancels
the active tool. The panel lists supported annotations on the current page;
select a row or an annotation on the page to view its comment, change its colour
or comment, adjust ink width, or delete it. **Apply changes** commits panel edits.
Geometry of existing annotations is preserved when editing their properties.
Other annotation types remain in the PDF but are not editable in this panel.

Annotation changes require the PDF's annotation permission, independently of
copy permission. Text can be selected for markup when annotations are allowed
but copying is denied; clipboard copying remains blocked. Locked and read-only
annotations can be viewed but cannot be edited or deleted.

Use **Undo**/**Redo**, **Ctrl+Z**/**Ctrl+Shift+Z** (**Cmd+Z**/**Cmd+Shift+Z**), or
**Ctrl+Y** to undo and redo committed document edits. When editing a text field,
these shortcuts affect that field instead. History retains up to 32 document
versions and is bounded by 256 MB, except that it always retains the current and
previous version. Edits serialize and reopen a full PDF snapshot, so large PDFs
can pause briefly and consume additional memory.

**Save** (**Ctrl+S** / **Cmd+S**) replaces the current PDF. **Save As…**
(**Ctrl+Shift+S** / **Cmd+Shift+S**) chooses another file and makes it the current
destination. The title shows `*` while document changes are unsaved. Closing the
window or opening another PDF offers **Save**, **Save As**, **Discard** and
**Cancel**; a cancelled or failed save leaves the current document open.
Saving writes a temporary file beside the destination, reopens and authenticates
it, checks its pages and permissions, then replaces the destination. Encryption
and passwords are preserved. Background rendering, previews and printing use
the current document, including unsaved annotations.

This is annotation editing, not page-content editing, redaction or signature
creation. Review does not execute PDF JavaScript or submit PDF data to services.

### Fill existing forms

Use **Forms** to open the current page's fields, or click a field on any visible
page, including rotated or facing pages. The panel supports single-line and
multiline text, checkboxes, radio groups, combo boxes (including editable
choices), and single/multiple-selection lists. Changes enter the live PDF
immediately and use the same undo/redo, Save/Save As and unsaved-change protection
as annotations. Radio groups stay synchronized across pages. Choice labels are
displayed; their export values and exact option indices are saved, including
duplicate exports.

Use **Tab**/**Shift+Tab** to move through controls and **Space** to select buttons
or list choices. **F6**/**Shift+F6** also includes editable form text fields.
Page-navigation keys do not turn pages while a field owns keyboard focus.
Inherited read-only flags, locked widgets and text-length limits are enforced.
Filling requires form-filling or annotation permission, independently of copy
permission. Copy and Cut remain blocked when the PDF denies copying.

Review never runs PDF JavaScript, calculations, validation scripts or widget
actions, and never submits form data. Push buttons, signatures, XFA forms,
file-selection fields and password entry remain read-only. MuPDF can render
password values as plain text, so Review does not allow entering them. Rich-text
fields are edited as plain text. Form contents are not stored in reading/session
metadata. MuPDF can draw combo export codes instead of display labels and does
not highlight selected list-box rows in PDF appearances; the panel displays the
labels and saved selection state. Unicode values are preserved, but egui's
bundled fonts may lack glyphs for some labels.

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
lifetime, including when opening another document, and are saved between sessions.

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
no-text message with guidance for optional local OCR. Recognizing the displayed
page refreshes this pane without navigation, including its assistive text.
The raster page itself has no screen-reader text navigation. Linux AT-SPI text,
focus and action integration
are tested; VoiceOver, Narrator and NVDA need native-platform manual validation.

## Document inspection

Use **Properties** or **Ctrl+D** (**Cmd+D** on macOS) to inspect metadata, page
dimensions, attachments, layers and signatures. **Escape** closes the inspector.
Metadata dates are shown as stored in the PDF. The MuPDF label binding returns
at most 127 UTF-8 bytes; physical page numbers remain authoritative.

Attachments in the embedded-files name tree, including nested entries, can be
explicitly saved with **Save as…**. Review never opens the saved file and rejects
replacing the open PDF. Annotation-only attachments and associated-file arrays
are not enumerated. Saving checks declared and decoded sizes against 64 MiB,
but the binding decodes an entire stream in memory, so this is not a strict
decompression-memory limit. Treat embedded files as untrusted.

Layer checkboxes change the default configuration for this viewing session;
they never save the PDF. Locked layers and radio/usage-controlled configurations
are read-only. The safe binding has no runtime layer API: Review rebuilds an
in-memory rendering copy after changes, which may be slow for large documents.

Signature inspection reads AcroForm fields, including invisible fields.
**Signature present does not mean valid or trusted.** Review does not verify
digests, certificate chains, revocation, timestamps or post-signing changes;
the safe binding exposes no cryptographic verifier. Claimed signer names and
dates are unverified. Document timestamps outside the AcroForm tree are not
enumerated. Document JavaScript and file-launch actions remain disabled.

## Reading state and privacy

Review remembers page, scroll position, zoom mode/percentage, layout and rotation
for the 32 most recently opened files. Sidebar visibility, width and selected
tab, and normal window size, position, maximized state and appearance persist
between sessions. Window placement is restored only where the window system
allows it; Wayland leaves placement to the compositor. Review does not
automatically reopen a document when launched without a path.

Use **Recent files** to reopen a document. **Personal bookmarks** saves a page,
zoom, scroll position, layout and rotation without modifying the PDF. Use
**Ctrl+B** (**Cmd+B** on macOS) to add or remove the current page bookmark, or
use the menu. Select a saved location to navigate; **×** removes it. Up to 512
personal bookmarks are kept, independently of the PDF's embedded outline.
Reopening an encrypted document still prompts for its password before restoring
a saved location.

**Recent files → Clear history…** removes recent files and saved reading
positions after confirmation. It keeps personal bookmarks and sidebar/window
preferences. Remove personal bookmarks separately in their menu. Clearing
history does not close the current PDF or silently add it back to history;
opening it again starts a new history entry.

Only paths and view metadata are saved, never PDF passwords, extracted text,
search terms or rendered page contents. Writes atomically replace `state.json`,
with ordinary interaction writes throttled to twice per second. Clearing history
and normal exit flush immediately.
Missing state uses defaults; corrupt or unsupported state shows a warning and
uses defaults. Storage is private to the current user:

- Linux: `$XDG_STATE_HOME/review/state.json`, defaulting to
  `~/.local/state/review/state.json`.
- macOS: `~/Library/Application Support/org.spiralpoint.Review/state.json`.
- Windows: `%LOCALAPPDATA%\spiralpoint\Review\data\state.json`.

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
- The worker keeps a 128 MiB pixel cache, with up to 128 entries and at most
  12 whole-page results. Large pages use 1024-pixel tile cores and one-pixel
  overlap for filtering. Large renders skip neighbour prefetching; off-screen
  page tiles and preview textures are released.
- wgpu uploads visible tiles once and composites their cores through the shared
  page transform. Rotation changes vertex positions, not cached raster pixels.
- The window redraws on demand rather than continuously.
- Native egui controls share the winit window and wgpu surface with the page;
  there is no web runtime.

## Testing

Run `cargo test` and `cargo clippy --all-targets -- -D warnings`.
Generated PDF fixtures cover mixed page sizes, nonzero crop origins, intrinsic
rotation, transparency, Unicode, encryption, layers, links and outlines. Tests
also check cross-reference repair and rejected files without losing reading state.
`bash tests/linux-desktop.sh` checks Linux registration, path escaping and
removal in a disposable XDG home; it requires `gio` and optionally uses
`desktop-file-validate`. `bash tests/wayland-open.sh` checks native file dialogs,
cancellation, error recovery and replacement using a synthetic PDF in the Sway
session described below. It requires a GTK file-chooser portal or Zenity and
`python3-pyatspi` to activate native chooser controls through accessibility.
`bash tests/wayland-password.sh` uses the same session and chooser workflow to
check masked password entry, retry, keyboard and button cancellation, preserved
navigation, owner access and permission notices with synthetic encrypted PDFs.
`bash tests/wayland-inspection.sh` checks label navigation, inspector tabs,
rendered layer toggles and explicit attachment saving/cancellation using a
synthetic PDF. It additionally requires ImageMagick for pixel checks.

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

`bash tests/wayland-persistence.sh` checks restart/restore, scroll and sidebar
preferences, recent files, personal bookmarks, history clearing, encrypted
reopening, normal window size and corrupt-state recovery with disposable state
directories. It uses the same native Sway session and pointer tooling as the
test below, but does not need a file chooser.

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

With Tesseract and its `eng` language installed, run
`cargo test actual_local_tesseract -- --ignored` to exercise real OCR on generated
image-only PDFs, including password authentication, copy restrictions, per-page
recognition, search, shared worker revisions and unchanged source bytes.
`bash tests/wayland-ocr.sh` exercises OCR, cancellation, missing-engine guidance,
native-text refusal, clipboard restrictions and refreshed search under native
Wayland, including native AT-SPI read-only text before and after recognition.
It needs `python3-pyatspi` and Sway's D-Bus session, as above. It uses synthetic
PDFs and never downloads language data or submits print jobs. Set
`REVIEW_SCREENSHOTS` to capture the affected states.

`bash tests/wayland-search.sh` checks progressive result navigation, query
cancellation, matching options, snippet navigation and copy-independent search
using synthetic PDFs, including a long scan. Set `REVIEW_SCREENSHOTS` to capture
the affected native states for inspection.

`bash tests/wayland-annotations.sh` uses the same session and GTK chooser helper
to create all five annotation types, edit and delete a note, undo and redo across
a saved revision, cancel an unsaved close, and Save As. It reopens the resulting
PDFs to check annotation values, oriented line quads and encrypted permissions.
It also checks annotation permission independently of clipboard copy permission.

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

`bash tests/wayland-tiles.sh` checks a 20000×12000-point PDF with an offset
MediaBox. It verifies cross-tile selection, distant internal links and text,
panning, pixel-identical Back history, 1600% zoom, search and Fit page. Set
`REVIEW_SCREENSHOTS` to retain native captures for seam and overlay inspection.
Rust tests separately check asymmetric tile bounds/UVs, cropped pixels,
nonzero origins, intrinsic rotation, cache limits and stale results.

`tests/reading-layouts.py` checks native facing/continuous layouts, rotated
cross-page selection, pointer-centred zoom, hand panning, fullscreen and rotated
internal-link history. Run it inside the Sway D-Bus session with the Review
binary, exported fixture directory and compiled `wayland-pointer.c` helper as
arguments; an optional fourth argument saves screenshots. It uses isolated
reading state and never activates an external link.

`bash tests/wayland-forms.sh` drives native AT-SPI controls and keyboard focus
for text, checkbox, radio, combo and list values; checks inherited read-only and
fill-only/copy-denied permissions; and reopens saved PDFs to verify values and
encryption. It also clicks and edits a field on rotated facing page 2, checks
cross-page radio state and document undo/redo, and verifies those saved values.
Set `REVIEW_SCREENSHOTS` to capture representative form and unsaved-close states.

This is a foundation, not a complete viewer. Tabs are not implemented yet.

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
