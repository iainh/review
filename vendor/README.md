# MuPDF Rust wrapper patch

`mupdf/` contains the library source, normalized manifest, README and licence
from the crates.io `mupdf` 0.8.0 package. MuPDF itself and `mupdf-sys` still use
the registry versions; this does not change the renderer or native build.
Upstream: <https://github.com/messense/mupdf-rs/tree/v0.8.0>.
Original crate checksum: `185fb74927a40c569b7152c01c73f4c0206059b8b3d37c7eb007cc3f63453b64`.

The sole library change is in `src/pdf/document.rs`: `permissions()` uses
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
Remove the patch and vendored library when an upstream release fixes this
conversion; keep those regression tests.
