/// PDF points are 1/72 inch; UI logical pixels follow the 96 dpi convention.
pub const POINT_SCALE: f32 = 96.0 / 72.0;

#[derive(Clone, Copy, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum Zoom {
    #[default]
    FitPage,
    FitWidth,
    Percent(f32),
}

impl Zoom {
    pub fn label(self) -> String {
        match self {
            Self::FitPage => "Fit page".into(),
            Self::FitWidth => "Fit width".into(),
            Self::Percent(value) => format!("{:.0}%", value * 100.0),
        }
    }

    pub fn scale(self, viewport: (u32, u32), page: (f32, f32), dpi: f32) -> f32 {
        let width = viewport.0.saturating_sub((32.0 * dpi) as u32).max(1) as f32;
        let height = viewport.1.saturating_sub((32.0 * dpi) as u32).max(1) as f32;
        match self {
            Self::FitPage => (width / page.0).min(height / page.1),
            Self::FitWidth => width / page.0,
            Self::Percent(value) => (value * POINT_SCALE * dpi).max(0.01),
        }
        .max(f32::MIN_POSITIVE)
    }

    pub fn change(&mut self, factor: f32, effective: f32) {
        *self = Self::Percent((effective * factor).clamp(0.1, 16.0));
    }

    pub fn parse(input: &str) -> Option<Self> {
        let value = input
            .trim()
            .trim_end_matches('%')
            .trim()
            .parse::<f32>()
            .ok()?;
        (value.is_finite() && (10.0..=1600.0).contains(&value))
            .then_some(Self::Percent(value / 100.0))
    }
}

#[cfg(test)]
mod tests {
    use super::{POINT_SCALE, Zoom};

    #[test]
    fn percentage_is_independent_of_viewport_and_tracks_display_scale() {
        let zoom = Zoom::Percent(1.5);
        assert_eq!(zoom.scale((800, 600), (300.0, 700.0), 1.0), 2.0);
        assert_eq!(zoom.scale((1400, 900), (300.0, 700.0), 1.0), 2.0);
        assert_eq!(zoom.scale((1400, 900), (300.0, 700.0), 2.0), 4.0);
    }

    #[test]
    fn fit_modes_use_the_correct_dimension_and_logical_margins() {
        assert_eq!(Zoom::FitPage.scale((632, 432), (300.0, 800.0), 1.0), 0.5);
        assert_eq!(Zoom::FitWidth.scale((632, 432), (300.0, 800.0), 1.0), 2.0);
        assert_eq!(Zoom::FitPage.scale((1264, 864), (300.0, 800.0), 2.0), 1.0);
        let mut zoom = Zoom::FitWidth;
        zoom.change(1.25, 2.0 / POINT_SCALE);
        assert_eq!(zoom.scale((632, 432), (300.0, 800.0), 1.0), 2.5);
    }

    #[test]
    fn huge_pages_fit_viewports_and_thumbnails_below_the_old_scale_floor() {
        let size = (1_000_000.0, 600_000.0);
        let page = Zoom::FitPage.scale((1032, 532), size, 1.0);
        assert!((page * size.1 - 500.0).abs() < 0.001);
        assert!((page * size.0 - 833.3333).abs() < 0.001);
        let width = Zoom::FitWidth.scale((1032, 532), size, 1.0);
        assert!((width * size.0 - 1000.0).abs() < 0.001);
        let thumbnail = Zoom::FitPage.scale((244, 244), size, 1.0);
        assert!((thumbnail * size.0 - 212.0).abs() < 0.001);
        assert!((thumbnail * size.1 - 127.2).abs() < 0.001);
        assert_eq!(Zoom::Percent(1.0).scale((244, 244), size, 1.0), POINT_SCALE);
    }

    #[test]
    fn explicit_zoom_rejects_invalid_and_out_of_range_values() {
        assert_eq!(Zoom::parse(" 137.5% "), Some(Zoom::Percent(1.375)));
        assert_eq!(Zoom::parse("10"), Some(Zoom::Percent(0.1)));
        assert_eq!(Zoom::parse("1600"), Some(Zoom::Percent(16.0)));
        for value in ["9.99", "1600.1", "NaN", "inf", "-20", "", "abc"] {
            assert_eq!(Zoom::parse(value), None, "{value}");
        }
    }
}
