# MuPDF Rust wrapper patch

`mupdf/` contains the library source, normalized manifest, README and licence
from the crates.io `mupdf` 0.8.0 package. MuPDF itself and `mupdf-sys` still use
the registry versions. A small additional C binding is built locally.
Upstream: <https://github.com/messense/mupdf-rs/tree/v0.8.0>.
Original crate checksum: `185fb74927a40c569b7152c01c73f4c0206059b8b3d37c7eb007cc3f63453b64`.

In `src/pdf/document.rs`, `permissions()` uses
`Permission::from_bits_truncate` instead of strict `from_bits` with a fallback
to all permissions. PDF permission words contain reserved bits. The strict
conversion rejects them and incorrectly grants all permissions, including for
user-password access. Truncation preserves MuPDF's effective permission bits
and its owner-password override.

Exact library diff:

```diff
-        Permission::from_bits(bits as u32).unwrap_or_else(Permission::all)
+        Permission::from_bits_truncate(bits as u32)
```

There is no accurate alternative in the supported 0.8 API: reading
`trailer()/Encrypt/P` gives user restrictions but loses the owner override,
and `Document::authenticate()` returns a Boolean rather than MuPDF's user/owner
access bits. The wrapper exposes neither `has_permission` nor its raw context
and document pointers. Vendoring avoids layout-dependent unsafe access and
keeps permission evaluation in MuPDF on the document's owning thread.

The upstream examples, integration tests and binary fixtures are omitted, and
their manifest entries are removed. Review's document tests cover encrypted
user and owner authentication, print quality, copying and empty passwords.
`Document::from_shared_bytes` opens an independently seekable native memory
stream over immutable `Arc<Vec<u8>>` bytes. A native stream drop callback owns
the Arc so native document clones and pages cannot outlive the backing bytes.
`native/review.c` catches MuPDF exceptions in C, including failed opens; no
longjmp crosses a Rust frame. `native/include` contains the unmodified subset
of AGPL-3.0-or-later headers needed by this shim, copied from MuPDF 1.27.2 in
`mupdf-sys` 0.8.0 (upstream commit
`73d3100d46d8a9ad634f6ef035bbe78f0f947886`). Update those headers together with
the native dependency. Review tests cover native-handle lifetime, failure
cleanup, encrypted access, independent worker snapshots and saved replacement.
`PdfDocument::set_runtime_layers` applies view-only optional-content settings
through native layer APIs instead of editing PDF dictionaries and reopening a
rendering copy. It validates references first and maps MuPDF 1.27.2's descending
xref order explicitly. Existing layer tests cover pixels, text, worker views,
printing, encryption and saving with the original layer defaults preserved.
`PdfDocument::to_vec_with_options` pre-sizes a Rust byte vector to the native
serialized length. It still copies the native buffer and releases it through
MuPDF's allocator; it does not transfer allocator ownership or provide zero-copy
output. Review's isolated edit candidate uses this API with `Encryption::Keep`.
Remove these patches when upstream provides equivalent safe APIs and fixes
permission conversion; keep their regression tests.

# egui-desktop compatibility adaptation

`egui-desktop/` contains the MIT-licensed library source, README and licence
from the published `egui-desktop` 0.2.5 crate, upstream
<https://github.com/PxlSyl/egui-desktop/tree/v0.2.5>.
Original crate SHA-256:
`340fa91344f2f378f3f3e4996d459594a6a6f9b1281cb232c320a94aa5c40a43`.

The published manifest requires egui/eframe 0.33.3. Its context and widget
types cannot interoperate with Review's egui 0.34.3. This local dependency
uses egui 0.34.3 without downgrading Review or changing winit/wgpu. The
eframe-only rounded-corner and OS interop modules, examples and unrelated
image loaders are omitted. Optional image icons remain supported by egui;
Review uses a text brand instead of the upstream default app icon.

Library adaptations:

- Render titlebars with egui 0.34's `Panel::show_inside` on the root `Ui`.
  Update renamed methods and explicit `f32` stroke widths.
- Keep shortcut hints, but do not automatically run shortcut callbacks while
  drawing. Review owns dispatch, focused-field gates and modal protection.
- Activate menu navigation with Ctrl+F2, not a held Alt modifier, preserving
  Alt+Left/Right document history and text editing.
- Label custom-painted menus and window controls for AccessKit. Pass macOS
  control labels independently of whether their hover glyph is visible.
- Disable both unavailable submenu interactions and their accessible nodes.
- Display the host command modifier as Ctrl on Linux/Windows, Cmd on macOS.
- Keep resize-handle areas immovable so only the native window owns resizing.
- Apply rustfmt to the vendored Rust source.

Review derives shell colours from its existing egui visuals, including native
system-theme changes and high contrast. It handles the package's window
commands through egui-winit, while titlebar Close goes through App's normal
dirty-document confirmation. Native pointer grabs can consume the mouse release;
the renderer ends egui's gesture and queues its release when handing off a move
or resize. The Wayland test asserts move followed by resize changes geometry.
Review owns native corner rounding: winit requests DWM rounding on Windows 11,
and AppKit clips the transparent macOS window's content layer at a 12-point
radius (zero while maximized or fullscreen). Linux rounding remains compositor
policy, as in upstream. The AppKit view is borrowed from the live winit window;
there is no cached pointer or title-based window lookup. Remove this adaptation
when upstream supports the current egui generation and host-managed
shortcuts/root-Ui rendering.
