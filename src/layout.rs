//! Page geometry in logical pixels. PDF coordinates always remain unrotated.
use egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::zoom::Zoom;

pub const GAP: f32 = 16.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutMode {
    #[default]
    Single,
    Continuous,
    Facing,
}

impl LayoutMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single page",
            Self::Continuous => "Continuous",
            Self::Facing => "Facing pages",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    None,
    Clockwise,
    Half,
    Counterclockwise,
}

impl Rotation {
    pub fn turn(&mut self, clockwise: bool) {
        *self = match (*self, clockwise) {
            (Self::None, true) | (Self::Half, false) => Self::Clockwise,
            (Self::Clockwise, true) | (Self::Counterclockwise, false) => Self::Half,
            (Self::Half, true) | (Self::None, false) => Self::Counterclockwise,
            _ => Self::None,
        };
    }

    pub fn size(self, size: Vec2) -> Vec2 {
        match self {
            Self::Clockwise | Self::Counterclockwise => Vec2::new(size.y, size.x),
            _ => size,
        }
    }

    pub fn forward(self, point: [f32; 2]) -> [f32; 2] {
        let [x, y] = point;
        match self {
            Self::None => [x, y],
            Self::Clockwise => [1.0 - y, x],
            Self::Half => [1.0 - x, 1.0 - y],
            Self::Counterclockwise => [y, 1.0 - x],
        }
    }

    pub fn inverse(self, point: [f32; 2]) -> [f32; 2] {
        match self {
            Self::Clockwise => Self::Counterclockwise.forward(point),
            Self::Counterclockwise => Self::Clockwise.forward(point),
            _ => self.forward(point),
        }
    }
}

#[derive(Clone, Copy)]
pub struct PageTransform {
    pub rect: Rect,
    pub rotation: Rotation,
}

impl PageTransform {
    pub fn screen(self, point: [f32; 2]) -> Pos2 {
        self.rect.min + Vec2::from(self.rotation.forward(point)) * self.rect.size()
    }

    pub fn normalized(self, point: Pos2) -> [f32; 2] {
        let point = (point - self.rect.min) / self.rect.size();
        self.rotation.inverse([point.x, point.y])
    }

    pub fn bounds(self, rect: Rect) -> Rect {
        let a = self.screen([rect.min.x, rect.min.y]);
        let b = self.screen([rect.max.x, rect.max.y]);
        Rect::from_min_max(a.min(b), a.max(b)).intersect(self.rect)
    }

    pub fn image(self, painter: &egui::Painter, texture: egui::TextureId) {
        let mut mesh = egui::Mesh::with_texture(texture);
        // Rotate vertex positions, not pixels: rendering remains in the worker,
        // with no second bitmap/cache or rounding-dependent annotation transform.
        for uv in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: self.screen(uv),
                uv: Pos2::from(uv),
                color: egui::Color32::WHITE,
            });
        }
        mesh.indices.extend([0, 1, 2, 0, 2, 3]);
        painter.add(egui::Shape::mesh(mesh));
    }
}

pub struct PagePlacement {
    pub page: usize,
    pub rect: Rect,
}

pub struct PageLayout {
    pub pages: Vec<PagePlacement>,
    pub size: Vec2,
    /// Logical pixels per original PDF point, shared across differently sized pages.
    pub scale: f32,
}

impl PageLayout {
    pub fn new(
        sizes: &[(f32, f32)],
        current: usize,
        mode: LayoutMode,
        rotation: Rotation,
        zoom: Zoom,
        viewport: Vec2,
    ) -> Self {
        let sizes: Vec<_> = sizes
            .iter()
            .map(|&(x, y)| rotation.size(Vec2::new(x, y)))
            .collect();
        // Stable fit scale while scrolling mixed-size pages: changing the
        // active page must not resize every preceding row and jump the viewport.
        let fit_size = match mode {
            LayoutMode::Single => sizes[current],
            LayoutMode::Continuous => sizes.iter().copied().fold(Vec2::ZERO, Vec2::max),
            LayoutMode::Facing => sizes
                .chunks(2)
                .map(|pair| {
                    let left = pair[0];
                    let right = pair.get(1).copied().unwrap_or(Vec2::ZERO);
                    Vec2::new(left.x + right.x, left.y.max(right.y))
                })
                .fold(Vec2::ZERO, Vec2::max),
        };
        let fit_viewport =
            viewport - Vec2::new(if mode == LayoutMode::Facing { GAP } else { 0.0 }, 0.0);
        let scale = zoom.scale(
            (
                fit_viewport.x.max(1.0) as u32,
                fit_viewport.y.max(1.0) as u32,
            ),
            (fit_size.x, fit_size.y),
            1.0,
        );
        let mut pages = Vec::new();
        let mut y = GAP;
        let mut width = 0.0_f32;
        let indices = if mode == LayoutMode::Single {
            current..current + 1
        } else {
            0..sizes.len()
        };
        let step = if mode == LayoutMode::Facing { 2 } else { 1 };
        for i in indices.step_by(step) {
            let left = sizes[i] * scale;
            let right = if mode == LayoutMode::Facing {
                sizes.get(i + 1).copied().unwrap_or(Vec2::ZERO) * scale
            } else {
                Vec2::ZERO
            };
            let row_width = left.x + right.x + if right.x > 0.0 { GAP } else { 0.0 };
            width = width.max(row_width);
            pages.push(PagePlacement {
                page: i,
                rect: Rect::from_min_size(egui::pos2(0.0, y), left),
            });
            if right.x > 0.0 {
                pages.push(PagePlacement {
                    page: i + 1,
                    rect: Rect::from_min_size(egui::pos2(left.x + GAP, y), right),
                });
            }
            y += left.y.max(right.y) + GAP;
        }
        let mut size = viewport.max(Vec2::new(width + 2.0 * GAP, y));
        // Centre each row, rather than stretching pages or their text quads.
        for row in pages.chunks_mut(step) {
            let row_width = row.last().unwrap().rect.max.x;
            let x = (size.x - row_width) * 0.5;
            let extra_y = if mode == LayoutMode::Single {
                (size.y - y) * 0.5
            } else {
                0.0
            };
            for page in row {
                page.rect = page.rect.translate(Vec2::new(x, extra_y));
            }
        }
        // At explicit zoom, permit a destination near the bottom/right to
        // reach the viewport origin instead of silently clamping its PDF point.
        // Add space after the pages, without changing their centred positions.
        if matches!(zoom, Zoom::Percent(_)) {
            if size.x > viewport.x {
                size.x += (viewport.x - 2.0 * GAP).max(0.0);
            }
            if size.y > viewport.y {
                size.y += (viewport.y - 2.0 * GAP).max(0.0);
            }
        }
        Self { pages, size, scale }
    }

    pub fn page(&self, page: usize) -> &PagePlacement {
        self.pages.iter().find(|entry| entry.page == page).unwrap()
    }

    pub fn visible(&self, viewport: Rect) -> impl Iterator<Item = &PagePlacement> {
        self.pages
            .iter()
            .filter(move |p| p.rect.intersects(viewport))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations_preserve_asymmetric_points_and_overlay_bounds() {
        let point = [0.17, 0.63];
        for (rotation, expected) in [
            (Rotation::None, [0.17, 0.63]),
            (Rotation::Clockwise, [0.37, 0.17]),
            (Rotation::Half, [0.83, 0.37]),
            (Rotation::Counterclockwise, [0.63, 0.83]),
        ] {
            let transform = PageTransform {
                rect: Rect::from_min_size(egui::pos2(29.0, 73.0), Vec2::new(400.0, 700.0)),
                rotation,
            };
            let screen = transform.screen(point);
            assert!((screen.x - (29.0 + expected[0] * 400.0)).abs() < 0.001);
            assert!((screen.y - (73.0 + expected[1] * 700.0)).abs() < 0.001);
            let back = transform.normalized(screen);
            assert!((back[0] - point[0]).abs() < 0.00001);
            assert!((back[1] - point[1]).abs() < 0.00001);
        }
        let t = PageTransform {
            rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 300.0)),
            rotation: Rotation::Clockwise,
        };
        let b = t.bounds(Rect::from_min_max(
            egui::pos2(0.1, 0.2),
            egui::pos2(0.3, 0.6),
        ));
        assert!((b.min.x - 320.0).abs() < 0.001);
        assert!((b.min.y - 30.0).abs() < 0.001);
        assert!((b.max.x - 640.0).abs() < 0.001);
        assert!((b.max.y - 90.0).abs() < 0.001);
    }

    #[test]
    fn facing_pairs_keep_different_page_sizes_and_unpaired_last_page() {
        let layout = PageLayout::new(
            &[(300.0, 400.0), (500.0, 700.0), (200.0, 600.0)],
            0,
            LayoutMode::Facing,
            Rotation::None,
            Zoom::Percent(0.75),
            Vec2::new(900.0, 750.0),
        );
        assert_eq!(layout.scale, 1.0);
        assert_eq!(layout.page(0).rect.min, egui::pos2(42.0, 16.0));
        assert_eq!(layout.page(1).rect.min, egui::pos2(358.0, 16.0));
        assert_eq!(layout.page(2).rect.min, egui::pos2(350.0, 732.0));
        assert_eq!(layout.size, Vec2::new(900.0, 2066.0));
        let visible: Vec<_> = layout
            .visible(Rect::from_min_size(
                egui::pos2(0.0, 730.0),
                Vec2::new(900.0, 400.0),
            ))
            .map(|p| p.page)
            .collect();
        assert_eq!(visible, [2]);
    }
}
