use mupdf::DestinationKind;

use crate::{
    layout::{LayoutMode, Rotation},
    zoom::{POINT_SCALE, Zoom},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    pub page: usize,
    /// Viewport top-left in original page-relative PDF points, independent of
    /// zoom/DPI. May lie outside the active page in a multi-page layout.
    pub position: [f32; 2],
    pub zoom: Zoom,
    pub layout: LayoutMode,
    pub rotation: Rotation,
}

#[derive(Default)]
pub struct History {
    back: Vec<ViewState>,
    forward: Vec<ViewState>,
}

impl History {
    pub fn visit(&mut self, current: ViewState, next: ViewState) {
        if current != next {
            self.back.push(current);
            self.forward.clear();
        }
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn back(&mut self, current: ViewState) -> Option<ViewState> {
        let previous = self.back.pop()?;
        self.forward.push(current);
        Some(previous)
    }

    pub fn forward(&mut self, current: ViewState) -> Option<ViewState> {
        let next = self.forward.pop()?;
        self.back.push(current);
        Some(next)
    }
}

/// MuPDF has already applied the destination page's crop/rotation transform.
/// Its XYZ zoom is a percentage, unlike the factor stored in a PDF array.
pub fn destination_view(
    current: ViewState,
    page: usize,
    kind: DestinationKind,
    bounds: mupdf::Rect,
    viewport: [f32; 2],
) -> ViewState {
    let mut next = ViewState { page, ..current };
    let size = [bounds.x1 - bounds.x0, bounds.y1 - bounds.y0];
    let corner = current.rotation.inverse([0.0; 2]);
    let viewport = current.rotation.size(egui::Vec2::from(viewport));
    let x = |value: f32| (value - bounds.x0).max(0.0);
    let y = |value: f32| (value - bounds.y0).max(0.0);
    match kind {
        DestinationKind::Fit | DestinationKind::FitB => {
            // Multi-page toolbar fit modes use the largest page/spread. A PDF
            // destination instead fits its own page, regardless of neighbours.
            next.zoom = if current.layout == LayoutMode::Single {
                Zoom::FitPage
            } else {
                let scale = ((viewport.x - 32.0).max(1.0) / size[0])
                    .min((viewport.y - 32.0).max(1.0) / size[1]);
                Zoom::Percent((scale / POINT_SCALE).clamp(0.1, 16.0))
            };
            next.position = [corner[0] * size[0], corner[1] * size[1]];
        }
        DestinationKind::FitH { top } | DestinationKind::FitBH { top } => {
            next.zoom = if current.layout != LayoutMode::Single
                || matches!(
                    current.rotation,
                    Rotation::Clockwise | Rotation::Counterclockwise
                ) {
                Zoom::Percent(
                    (((viewport.x - 32.0).max(1.0) / size[0]) / POINT_SCALE).clamp(0.1, 16.0),
                )
            } else {
                Zoom::FitWidth
            };
            next.position[0] = corner[0] * size[0];
            if let Some(top) = top.filter(|v| v.is_finite()) {
                next.position[1] = y(top);
            }
        }
        DestinationKind::FitV { left } | DestinationKind::FitBV { left } => {
            next.zoom = Zoom::Percent(
                (((viewport.y - 32.0).max(1.0) / size[1]) / POINT_SCALE).clamp(0.1, 16.0),
            );
            next.position[1] = corner[1] * size[1];
            if let Some(left) = left.filter(|v| v.is_finite()) {
                next.position[0] = x(left);
            }
        }
        DestinationKind::XYZ { left, top, zoom } => {
            if let Some(left) = left.filter(|v| v.is_finite()) {
                next.position[0] = x(left);
            }
            if let Some(top) = top.filter(|v| v.is_finite()) {
                next.position[1] = y(top);
            }
            if let Some(zoom) = zoom.filter(|v| v.is_finite() && *v > 0.0) {
                next.zoom = Zoom::Percent((zoom / 100.0).clamp(0.1, 16.0));
            }
        }
        DestinationKind::FitR {
            left,
            bottom,
            right,
            top,
        } => {
            // Fitz normalizes FitR's bottom/top to minimum/maximum Y.
            let width = right - left;
            let height = top - bottom;
            if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
                let scale = ((viewport.x - 32.0).max(1.0) / width)
                    .min((viewport.y - 32.0).max(1.0) / height);
                next.zoom = Zoom::Percent((scale / POINT_SCALE).clamp(0.1, 16.0));
                next.position = [
                    x(if corner[0] == 0.0 { left } else { right }),
                    y(if corner[1] == 0.0 { bottom } else { top }),
                ];
            }
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(page: usize, position: [f32; 2], zoom: Zoom) -> ViewState {
        ViewState {
            page,
            position,
            zoom,
            layout: LayoutMode::Single,
            rotation: Rotation::None,
        }
    }

    #[test]
    fn history_snapshots_scroll_and_zoom_and_discards_only_branched_forward_history() {
        let a = state(0, [17.0, 193.0], Zoom::Percent(1.375));
        let b = state(3, [41.0, 67.0], Zoom::FitWidth);
        let c = state(3, [92.0, 371.0], Zoom::Percent(2.25));
        let mut history = History::default();
        assert_eq!(history.back(a), None);
        history.visit(a, b);
        assert_eq!(history.back(c), Some(a));
        assert_eq!(history.forward(a), Some(c));
        assert_eq!(history.back(c), Some(a));
        history.visit(a, a);
        assert!(history.can_forward());
        history.visit(a, b);
        assert!(!history.can_forward());
        assert_eq!(history.back(b), Some(a));
    }

    #[test]
    fn destinations_preserve_null_axes_and_use_nonzero_asymmetric_page_bounds() {
        let current = state(0, [23.0, 171.0], Zoom::Percent(1.375));
        let bounds = mupdf::Rect::new(10.0, 20.0, 310.0, 820.0);
        let view = destination_view(
            current,
            4,
            DestinationKind::XYZ {
                left: Some(91.0),
                top: Some(327.0),
                zoom: Some(225.0),
            },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(view, state(4, [81.0, 307.0], Zoom::Percent(2.25)));
        let null = destination_view(
            current,
            1,
            DestinationKind::XYZ {
                left: None,
                top: Some(57.0),
                zoom: Some(0.0),
            },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(null, state(1, [23.0, 37.0], current.zoom));
        let fit = destination_view(
            current,
            2,
            DestinationKind::FitH { top: Some(173.0) },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(fit, state(2, [0.0, 153.0], Zoom::FitWidth));
        let rectangle = destination_view(
            current,
            1,
            DestinationKind::FitR {
                left: 40.0,
                bottom: 95.0,
                right: 240.0,
                top: 195.0,
            },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(rectangle, state(1, [30.0, 75.0], Zoom::Percent(2.25)));
    }

    #[test]
    fn rotated_fit_destinations_fit_original_axes_and_reveal_the_rotated_top_left() {
        let mut current = state(0, [23.0, 171.0], Zoom::Percent(1.375));
        current.rotation = Rotation::Clockwise;
        current.layout = LayoutMode::Continuous;
        let bounds = mupdf::Rect::new(10.0, 20.0, 310.0, 820.0);
        let fit = destination_view(current, 1, DestinationKind::Fit, bounds, [632.0, 432.0]);
        assert_eq!(fit.position, [0.0, 800.0]);
        assert_eq!(fit.zoom, Zoom::Percent(0.5625));
        let horizontal = destination_view(
            current,
            1,
            DestinationKind::FitH { top: Some(137.0) },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(horizontal.position, [0.0, 117.0]);
        assert_eq!(horizontal.zoom, Zoom::Percent(1.0));
        let vertical = destination_view(
            current,
            1,
            DestinationKind::FitV { left: Some(91.0) },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(vertical.position, [81.0, 800.0]);
        assert_eq!(vertical.zoom, Zoom::Percent(0.5625));
        let rectangle = destination_view(
            current,
            1,
            DestinationKind::FitR {
                left: 40.0,
                bottom: 95.0,
                right: 240.0,
                top: 195.0,
            },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(rectangle.position, [30.0, 175.0]);
        assert_eq!(rectangle.zoom, Zoom::Percent(1.5));
        assert_eq!(rectangle.rotation, current.rotation);
        assert_eq!(rectangle.layout, current.layout);
    }

    #[test]
    fn facing_fit_destinations_fit_the_target_page_not_the_spread() {
        let mut current = state(0, [23.0, 171.0], Zoom::FitWidth);
        current.layout = LayoutMode::Facing;
        let bounds = mupdf::Rect::new(10.0, 20.0, 310.0, 820.0);
        let fit = destination_view(current, 1, DestinationKind::Fit, bounds, [632.0, 432.0]);
        assert_eq!(fit.zoom, Zoom::Percent(0.375));
        assert_eq!(fit.position, [0.0, 0.0]);
        let horizontal = destination_view(
            current,
            1,
            DestinationKind::FitH { top: Some(137.0) },
            bounds,
            [632.0, 432.0],
        );
        assert_eq!(horizontal.zoom, Zoom::Percent(1.5));
        assert_eq!(horizontal.position, [0.0, 117.0]);
    }
}
