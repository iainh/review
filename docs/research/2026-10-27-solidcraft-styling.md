# Research: Solidcraft-inspired styling for Review

**Date**: 2026-10-27
**Question**: Which parts of Solidcraft's advanced styling should Review adopt?
**Status**: Research complete; first docked styling port implemented locally.

## Context

This records the initial research and Painter exploration. After the request to
upgrade egui and begin porting, Review moved to egui 0.36.2 and wgpu 30. The first
port uses docked concept A as a working assumption, not an explicit user selection.

Inspected Review's current source and Solidcraft upstream commit
`367aa249bce2eea58346c41a40e7f9f3c677112d`. The local Solidcraft checkout is older
and is not the basis for the current capability comparison. Review's prior
implemented studio/tab screenshot supplied the Review visual reference. It is not
a fresh capture of the current application.

## Findings

### Solidcraft's main advantage is a shared design system

Its semantic tokens cover colours, readable foreground/background pairs, spacing,
typography, radii and control metrics. A shared container policy distinguishes
docked surfaces from floating cards, controlling adjoining borders, exposed
corners, padding and elevation. Its component kit applies that policy to fields,
segmented selectors, menus, windows and confirmations while retaining caller-owned
actions and editor behaviour.

The current kit uses 28-point controls, 40-point window headers and 48-point
footers. These are useful starting points, not requirements for Review. Source
guards and component gallery/interaction tests prevent styling bypasses.

Border roles, inset outlines, bevels and shadows are optional theme treatments.
They do not require a new graphics backend. CAD viewport materials, grids, bloom,
HDR and selection rendering are separate and should not be imported into a PDF
viewer shell.

### Review already has the right integration points

- `src/native_ui.rs`: `Appearance::apply`, `studio_visuals`, `panel_bevel` and
  `studio_button` already centralize much of the shell styling. Extend this owner
  rather than adding a parallel theme authority.
- `src/desktop.rs`: titlebar/menu styling mostly follows egui visuals, but the dark
  titlebar still has its own grey value.
- `src/icons.rs`: shared icon buttons can adopt consistent control states.
- `src/tabs.rs`: retain the previously selected flat tabs, active underline,
  independent accessible close controls and dirty indicators.
- `src/sidebar.rs`: apply a shared surface/header treatment and a consistent
  Outline/Pages selector without changing preview virtualization or navigation.
- `src/viewer.rs`: align toolbar groups and fields, then style the existing find
  panel. Preserve its search results and keyboard ownership.
- `src/native_ui.rs` settings/help, `src/inspector.rs` document properties and
  `src/printing.rs` offer later opportunities for consistent dialog framing.
- `src/fonts.rs`: preserve system UI fonts and fallbacks. Solidcraft's bundled IBM
  Plex family is not necessary to achieve hierarchy in Review.
- `src/reading.rs`: leave page rasterization, tile caching and document colours
  separate from shell styling.

Review already supports System, Light, Dark and High contrast. Preserve explicit
high-contrast colours, visible keyboard focus, accessible labels, enabled states
and native window/menu behaviour on each platform.

### Adapt selectively; do not depend on the CAD UI crate

At the research baseline, Review used egui 0.34.3 and wgpu 29. Current Solidcraft
uses egui 0.36 and wgpu 30. The relevant frame, painting and styling primitives
already existed in Review's stack, so the design did not inherently require an
upgrade. The subsequent upgrade was requested explicitly; source reuse still
requires selective adaptation rather than drop-in compatibility.

Solidcraft code is MIT OR Apache-2.0; copied code needs the corresponding notices.
Bundled fonts and third-party assets/palettes have separate provenance. Borrowing
the architecture and keeping Review's existing assets avoids unnecessary imports.

## Options considered

| Option | Advantages | Tradeoffs | Relative scope |
| --- | --- | --- | --- |
| A: docked studio | Preserves page space and familiar layout; consistent controls and surface hierarchy | Less visually dramatic | Smallest |
| B: card workspace | Stronger Solidcraft-like separation and rounded frames | Gutters/padding reduce page space and need more responsive-layout work | Larger |
| Import the whole theme/component system | Many existing components and themes | CAD coupling, version mismatch and excessive migration scope | Not recommended |

## Recommendation

Choose A. Start with a small Review-owned semantic palette/metrics structure in
the existing appearance module. Feed egui visuals and existing custom painters
from it; use the resolved egui theme rather than another global dark-mode flag.
Name only the roles Review needs: chrome, toolbar, sidebar, canvas, fields,
floating surfaces, text, muted text, border, accent, selection and focus.

Then align toolbar fields and icon states, restyle Outline/Pages and unify existing
dialog headers, bodies and action areas. Docked panels should remain flush;
floating menus/dialogs can have modest radii and shadows. Do not build a generic
container framework until repeated Review-specific compositions justify it.

Keep the existing platform fonts, tab design, functionality and PDF rendering.
Defer JSON theme import, large theme galleries, animation and AST enforcement.
They are independent features, not prerequisites for this improvement.

Implementation verification should include dark/light/high-contrast states,
system-theme switching, narrow windows, toolbar overflow, keyboard focus, tab
close/unsaved protection, find and settings. Capture and inspect actual native
screenshots and compare matched PDF page regions to ensure shell changes do not
alter document pixels. Existing tests, Clippy and formatting remain the baseline.

## Painter concepts

![A: docked studio; B: card workspace](https://ampcode.com/user-content/attachments/a0b3432fe71f6c52921411b2844e643c9ec670a868116cdadc63425dec82d1fc-file.png)

The comparison was generated from actual Review and Solidcraft visual references,
then refined to reduce heavy bevels. It is a styling reference, not running UI or
an interaction specification. Painter still substituted a zoom dropdown for
Review's zoom field/buttons and retained some gradients. Those are not proposed
behaviour changes. The generic desktop titlebar shown is not a proposal to replace
macOS native menus or window controls.

## First implementation

- Shared semantic light/dark palette and 28-point control metrics in the existing
  appearance module; system fonts and high contrast retained.
- Flush toolbar/find bands, distinct sidebar/canvas surfaces, equal-width
  Outline/Pages selectors and native egui button states instead of bevel painters.
- Rounded floating surfaces, modest shadows and padded settings/help dialogs.
- Existing flat tabs, accessible close controls and PDF rendering retained.
- Native captures inspected in dark, light, high contrast, 640-point narrow,
  find and settings states. Find/settings startup state was injected temporarily
  for capture because macOS event-posting permission was unavailable; those hooks
  were removed. Native mouse/keyboard interaction was not automated.

## Website comparison and follow-up

The first pass did not yet match the compositional polish of the screenshots on
solidcraft.io. In particular, Review's macOS font-name lookup silently substituted
Times New Roman for San Francisco. The follow-up loads Core Text's resolved file
directly, retaining collection face selection. A regression test checks the loaded
face against the system UI font and failed before the fix.

The toolbar now has outlined Navigation, Page, Zoom and Document groups, which
wrap intact in narrow windows. The sidebar has a small document header,
underlined Outline/Pages tabs, compact rows and separate muted page captions.
Variable-font weight coordinates give group and settings headings real weight
hierarchy without bundling another font family. A layout regression test checks
wrapping and rejoining after a resize.

The continuous-view boundary placeholder is fixed. Layout visibility previously
included pages touching the viewport with zero visible area; the tile planner
correctly requested no tiles for them. Visibility now requires a positive-area
intersection. A boundary regression failed before the fix and passes afterwards;
native before/after captures confirm the clipped text is gone.

A toolbar hover replay exposed a separate egui 0.36 layout change: hiding an
inactive button frame removed its border space, so hover enlarged an icon from
28 to 30 points wide and displaced neighbouring controls. Inactive buttons now
retain a transparent frame. Rectangle tests cover hover and pointer exit in
dark, light and high contrast. The final video records the native GPU-rendered
UI with synthetic egui pointer input and a yellow marker because macOS event
posting permission was unavailable. Temporary capture hooks were removed.

Theme import, motion and a larger component kit remain deferred. The Painter
image above is still a concept, not implementation evidence.

## References

- [Solidcraft UI kit](https://github.com/tangency-labs/solidcraft/blob/367aa249bce2eea58346c41a40e7f9f3c677112d/docs/ui-kit.md)
- [Semantic containers](https://github.com/tangency-labs/solidcraft/blob/367aa249bce2eea58346c41a40e7f9f3c677112d/crates/ui-egui/src/ui_kit/container.rs)
- [Theme and effects](https://github.com/tangency-labs/solidcraft/blob/367aa249bce2eea58346c41a40e7f9f3c677112d/crates/ui-egui/src/theme.rs)
- [Theme files](https://github.com/tangency-labs/solidcraft/blob/367aa249bce2eea58346c41a40e7f9f3c677112d/crates/ui-egui/src/theme_files.rs)
- [Styling source guards](https://github.com/tangency-labs/solidcraft/blob/367aa249bce2eea58346c41a40e7f9f3c677112d/crates/ui-egui/src/ui_kit_guard_tests.rs)
- [Solidcraft runtime screenshot](https://raw.githubusercontent.com/tangency-labs/solidcraft/367aa249bce2eea58346c41a40e7f9f3c677112d/docs/screenshots/sample-plate.png): older 0.22.1 capture, not proof of all current components.
- [Review studio styling and tab work](https://ampcode.com/threads/T-01a11e6a-9b05-721f-b4d7-83870b0cbce5)
