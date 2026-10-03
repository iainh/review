//! Virtualized reading surface; the worker still owns all PDF rasterization.
use std::collections::BTreeMap;

use anyhow::{Result, bail};
use egui::{Color32, Pos2, Rect, Vec2};

use crate::{
    document::PdfDocument,
    layout::{LayoutMode, PageLayout, PageTransform, Rotation},
    links::{self, LinkTarget, PageLink},
    navigation::ViewState,
    render_worker::{Priority, RenderKey, RenderWorker},
    search::Search,
    selection::Selection,
    zoom::{POINT_SCALE, Zoom},
};

const TEXTURE_BYTES: usize = 128 * 1024 * 1024;
const VISIBLE_PAGES: usize = 12;

#[derive(Default)]
pub struct DisplayPage {
    pub texture: Option<egui::TextureHandle>,
    pub rendered: Option<RenderKey>,
    pub links: Vec<PageLink>,
}

#[derive(Default)]
pub struct ReadingSurface {
    pub mode: LayoutMode,
    pub rotation: Rotation,
    pub hand: bool,
    pub effective_zoom: f32,
    sizes: Vec<(f32, f32)>,
    pub displayed: BTreeMap<usize, DisplayPage>,
    pub screen_pages: Vec<(usize, PageTransform)>,
    last_view: Option<(Zoom, LayoutMode, Rotation, Vec2, usize)>,
    offset: Vec2,
    pub viewport: Option<Rect>,
    pan_delta: Vec2,
}

pub struct ReadingFrame<'a> {
    pub document: &'a PdfDocument,
    pub state_key: &'a std::path::Path,
    pub worker: &'a mut RenderWorker,
    pub selection: &'a mut Selection,
    pub search: &'a Search,
    pub selecting: bool,
    pub view: &'a mut ViewState,
    pub restore: &'a mut bool,
    pub reveal_match: &'a mut bool,
}

impl ReadingSurface {
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: ReadingFrame<'_>) -> Result<Option<LinkTarget>> {
        let ReadingFrame {
            document,
            state_key,
            worker,
            selection,
            search,
            selecting,
            view,
            restore,
            reveal_match,
        } = frame;
        if self.sizes.is_empty() {
            // Geometry only: no text or pixels for off-screen pages.
            self.sizes = (0..document.page_count())
                .map(|page| document.page_size(page))
                .collect::<Result<_>>()?;
        }
        let available = ui.available_size();
        let dpi = ui.ctx().pixels_per_point();
        let pointer = ui.input(|input| input.pointer.hover_pos());
        let factor = ui.input(|input| input.zoom_delta());
        let mut anchor = if ui.is_enabled()
            && factor != 1.0
            && !ui.ctx().egui_wants_keyboard_input()
        {
            pointer
                .filter(|p| self.viewport.is_some_and(|v| v.contains(*p)))
                .and_then(|p| {
                    self.screen_pages
                        .iter()
                        .find(|(_, t)| t.rect.contains(p))
                        .map(|(page, t)| (*page, t.normalized(p), p - self.viewport.unwrap().min))
                })
        } else {
            None
        };
        if anchor.is_some() {
            view.zoom.change(factor, self.effective_zoom);
        }
        if anchor.is_none()
            && !*restore
            && self.last_view.is_some_and(|last| last.2 != self.rotation)
            && let Some(viewport) = self.viewport
            && let Some((page, transform)) = self.screen_pages.iter().min_by(|(_, a), (_, b)| {
                a.rect
                    .distance_to_pos(viewport.center())
                    .total_cmp(&b.rect.distance_to_pos(viewport.center()))
            })
        {
            anchor = Some((
                *page,
                transform.normalized(viewport.center()),
                viewport.size() * 0.5,
            ));
        }
        let settings = (
            view.zoom,
            self.mode,
            self.rotation,
            available,
            if self.mode == LayoutMode::Single {
                view.page
            } else {
                0
            },
        );
        let changed = self.last_view.is_some_and(|last| last != settings);
        let restoring = *restore || changed || anchor.is_some();
        let layout = PageLayout::new(
            &self.sizes,
            view.page,
            self.mode,
            self.rotation,
            view.zoom,
            available,
        );
        self.effective_zoom = layout.scale / POINT_SCALE;
        let mut scroll = egui::ScrollArea::both()
            .id_salt(("page", state_key))
            .animated(false)
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource {
                drag: false,
                ..Default::default()
            });
        if let Some((page, point, screen)) = anchor {
            let target = PageTransform {
                rect: layout.page(page).rect,
                rotation: self.rotation,
            };
            scroll = scroll.scroll_offset(target.screen(point).to_vec2() - screen);
            view.page = page;
        } else if restoring {
            let size = self.sizes[view.page];
            let target = PageTransform {
                rect: layout.page(view.page).rect,
                rotation: self.rotation,
            };
            let point = target.screen([view.position[0] / size.0, view.position[1] / size.1]);
            scroll = scroll.scroll_offset(point.to_vec2());
        } else if self.pan_delta != Vec2::ZERO {
            scroll = scroll.scroll_offset(self.offset - self.pan_delta);
        }
        self.pan_delta = Vec2::ZERO;
        *restore = false;
        let old_offset = self.offset;
        let mut activated = None;
        let mut error = None;
        let mut rendering = false;
        let output = scroll.show_viewport(ui, |ui, viewport| {
            let origin = ui.min_rect().min;
            let content = Rect::from_min_size(origin, layout.size);
            ui.allocate_rect(content, egui::Sense::hover());
            let visible: Vec<_> = layout.visible(viewport).take(VISIBLE_PAGES).collect();
            if layout.visible(viewport).nth(VISIBLE_PAGES).is_some() {
                error = Some(
                    "More than 12 pages are visible; increase the zoom to render them all".into(),
                );
            }
            self.displayed
                .retain(|p, _| visible.iter().any(|v| v.page == *p));
            self.screen_pages = visible
                .iter()
                .map(|p| {
                    (
                        p.page,
                        PageTransform {
                            rect: p.rect.translate(origin.to_vec2()),
                            rotation: self.rotation,
                        },
                    )
                })
                .collect();
            let mut bytes: usize = self
                .displayed
                .values()
                .filter_map(|p| p.texture.as_ref())
                .map(|t| t.size()[0] * t.size()[1] * 4)
                .sum();
            let mut requested_bytes = 0.0;
            for (placement, &(_, transform)) in visible.iter().zip(&self.screen_pages) {
                let entry = self
                    .displayed
                    .entry(placement.page)
                    .or_insert_with(|| match document.links(placement.page) {
                        Ok(links) => DisplayPage {
                            links,
                            ..Default::default()
                        },
                        Err(e) => {
                            error = Some(format!("Cannot load page links: {e:#}"));
                            DisplayPage::default()
                        }
                    });
                let key = RenderKey::new(placement.page, layout.scale * dpi);
                let size = self.sizes[placement.page];
                let estimate = (size.0 * key.scale()).ceil() * (size.1 * key.scale()).ceil() * 4.0;
                requested_bytes += estimate;
                if entry.rendered != Some(key) {
                    let old_bytes = entry
                        .texture
                        .as_ref()
                        .map_or(0, |t| t.size()[0] * t.size()[1] * 4);
                    if requested_bytes <= TEXTURE_BYTES as f32
                        && (bytes - old_bytes) as f32 + estimate <= TEXTURE_BYTES as f32
                    {
                        match worker.image(key, Priority::Page) {
                            Some(Ok(image)) => {
                                let new_bytes = image.rgba.len();
                                if bytes - old_bytes + new_bytes <= TEXTURE_BYTES {
                                    entry.texture = Some(ui.ctx().load_texture(
                                        format!("PDF page {}", key.page + 1),
                                        egui::ColorImage::from_rgba_unmultiplied(
                                            [image.width as usize, image.height as usize],
                                            &image.rgba,
                                        ),
                                        egui::TextureOptions::LINEAR,
                                    ));
                                    bytes = bytes - old_bytes + new_bytes;
                                    entry.rendered = Some(key);
                                }
                            }
                            Some(Err(e)) => {
                                bytes -= old_bytes;
                                entry.texture = None;
                                entry.rendered = Some(key);
                                error = Some(e);
                            }
                            None => rendering = true,
                        }
                    } else {
                        error = Some(
                            "Visible pages exceed the rendering memory limit; reduce the zoom"
                                .into(),
                        );
                    }
                }
                ui.painter()
                    .rect_filled(transform.rect, 0.0, Color32::WHITE);
                if let Some(texture) = &entry.texture {
                    transform.image(ui.painter(), texture.id());
                } else {
                    ui.painter().text(
                        transform.rect.center(),
                        egui::Align2::CENTER_CENTER,
                        if rendering {
                            "Rendering page…"
                        } else {
                            "Page unavailable at this zoom"
                        },
                        egui::FontId::proportional(16.0),
                        Color32::DARK_GRAY,
                    );
                }
                for (index, hit) in search
                    .matches
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.page == placement.page)
                {
                    let selected = search.selected == Some(index);
                    let mut bounds = Rect::NOTHING;
                    for quad in &hit.quads {
                        let points: Vec<_> =
                            quad.iter().map(|&point| transform.screen(point)).collect();
                        for &p in &points {
                            bounds.extend_with(p);
                        }
                        ui.painter().add(egui::Shape::convex_polygon(
                            points,
                            if selected {
                                Color32::from_rgba_unmultiplied(255, 145, 0, 110)
                            } else {
                                Color32::from_rgba_unmultiplied(255, 225, 0, 75)
                            },
                            egui::Stroke::NONE,
                        ));
                    }
                    if selected && *reveal_match {
                        ui.scroll_to_rect(bounds.expand(24.0), None);
                        *reveal_match = false;
                    }
                }
            }
            if self.mode == LayoutMode::Single && requested_bytes < 32.0 * 1024.0 * 1024.0 {
                for page in [view.page.checked_sub(1), view.page.checked_add(1)]
                    .into_iter()
                    .flatten()
                {
                    if let Some(size) = self.sizes.get(page)
                        && size.0 * size.1 * layout.scale.powi(2) * dpi.powi(2) * 4.0
                            < 32.0 * 1024.0 * 1024.0
                    {
                        worker.image(RenderKey::new(page, layout.scale * dpi), Priority::Prefetch);
                    }
                }
            }
            if let Err(e) = selection.pages_ui(
                ui,
                document,
                &self.screen_pages,
                document.permissions().copy,
                selecting && !self.hand,
            ) {
                error = Some(format!("Failed to read page text: {e:#}"));
            }
            if selecting && self.hand {
                let response = ui.interact(
                    content,
                    ui.id().with("hand_tool"),
                    egui::Sense::click_and_drag(),
                );
                if response.hovered() || response.dragged() {
                    ui.ctx().set_cursor_icon(if response.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    });
                }
                if response.dragged_by(egui::PointerButton::Primary) {
                    self.pan_delta = ui.input(|input| input.pointer.delta());
                    ui.ctx().request_repaint();
                }
                if response.hovered() && ui.input(|input| input.pointer.primary_pressed()) {
                    response.surrender_focus();
                    ui.memory_mut(|memory| {
                        if let Some(id) = memory.focused() {
                            memory.surrender_focus(id);
                        }
                    });
                }
            }
            // Link click targets come after selection/hand drag targets.
            for &(page, transform) in self.screen_pages.iter().filter(|_| selecting) {
                ui.push_id(page, |ui| {
                    if let Some(target) = links::ui(ui, &self.displayed[&page].links, transform) {
                        activated = Some(target);
                    }
                });
            }
        });
        self.offset = output.state.offset;
        self.viewport = Some(output.inner_rect);
        let scroll_changed = old_offset != self.offset && !restoring;
        let pressed = ui.is_enabled() && ui.input(|input| input.pointer.primary_pressed());
        let pointed = pressed
            .then(|| {
                pointer
                    .filter(|p| output.inner_rect.contains(*p))
                    .and_then(|pos| {
                        self.screen_pages
                            .iter()
                            .find(|(_, t)| t.rect.contains(pos))
                            .map(|(p, _)| *p)
                    })
            })
            .flatten();
        if let Some(page) = pointed {
            view.page = page;
        } else if scroll_changed
            && self.mode != LayoutMode::Single
            && let Some(page) = layout.pages.iter().min_by(|a, b| {
                let center = self.offset + output.inner_rect.size() * 0.5;
                a.rect
                    .distance_to_pos(Pos2::ZERO + center)
                    .total_cmp(&b.rect.distance_to_pos(Pos2::ZERO + center))
            })
        {
            view.page = page.page;
        }
        let size = self.sizes[view.page];
        let page = layout.page(view.page).rect;
        let transform = PageTransform {
            rect: page,
            rotation: self.rotation,
        };
        let point = transform.normalized(Pos2::ZERO + self.offset);
        // The viewport can start in another page or a gap. Preserve that signed
        // original-page position: clamping would jump on Back or session restore.
        view.position = [point[0] * size.0, point[1] * size.1];
        self.last_view = Some(settings);
        if let Some(error) = error {
            bail!(error);
        }
        Ok(activated)
    }
}
