//! Permission-neutral, owned page text shared by selection and accessibility.
//!
//! MuPDF segments whitespace-separated regions (including columns). We retain
//! its depth-first block/line/Unicode order, not a global y/x sort. This remains
//! a layout heuristic, not a promise of semantic reading order for every PDF.

use std::ops::Range;

use anyhow::{Context, Result, ensure};
use unicode_segmentation::UnicodeSegmentation;

pub type Quad = [[f32; 2]; 4];

#[derive(Clone, Debug)]
pub struct TextChar {
    pub ch: char,
    /// Normalized page coordinates in perimeter order; None for separators.
    pub quad: Option<Quad>,
    pub bidi: u16,
}

#[derive(Clone, Debug)]
pub struct TextLine {
    pub chars: Range<usize>,
    /// Baseline direction in PDF coordinates, including vertical/rotated text.
    pub direction: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct PageText {
    pub chars: Vec<TextChar>,
    pub lines: Vec<TextLine>,
    pub paragraphs: Vec<Range<usize>>,
    pub(crate) size: [f32; 2],
}

pub struct Hit {
    pub glyph: usize,
    pub caret: usize,
}

impl PageText {
    /// XML is MuPDF 0.8's safe API for nested segmented regions and bidi levels;
    /// its structured() helper omits both. Descendants traverse regions in order.
    pub fn from_xml(xml: &str, bounds: mupdf::Rect) -> Result<Self> {
        let size = [bounds.x1 - bounds.x0, bounds.y1 - bounds.y0];
        ensure!(
            size.iter().all(|v| v.is_finite() && *v > 0.0),
            "invalid page bounds"
        );
        let xml = roxmltree::Document::parse(xml).context("invalid MuPDF text XML")?;
        let mut page = Self {
            size,
            ..Self::default()
        };
        for block in xml.descendants().filter(|node| node.has_tag_name("block")) {
            let start = page.chars.len();
            for line in block.children().filter(|node| node.has_tag_name("line")) {
                let start = page.chars.len();
                let direction =
                    numbers::<2>(line.attribute("dir").context("missing line direction")?)?;
                for ch in line.descendants().filter(|node| node.has_tag_name("char")) {
                    let value = ch.attribute("c").context("missing character")?;
                    let coordinates =
                        numbers::<8>(ch.attribute("quad").context("missing character quad")?)?;
                    let points = [
                        [coordinates[0], coordinates[1]],
                        [coordinates[2], coordinates[3]],
                        [coordinates[6], coordinates[7]],
                        [coordinates[4], coordinates[5]],
                    ];
                    let quad = points
                        .map(|p| [(p[0] - bounds.x0) / size[0], (p[1] - bounds.y0) / size[1]]);
                    let bidi = ch
                        .attribute("bidi")
                        .context("missing bidi level")?
                        .parse()?;
                    for ch in value.chars() {
                        page.chars.push(TextChar {
                            ch,
                            quad: Some(quad),
                            bidi,
                        });
                    }
                }
                if page.chars.len() > start {
                    page.lines.push(TextLine {
                        chars: start..page.chars.len(),
                        direction,
                    });
                    page.separator();
                }
            }
            if page.chars.len() > start {
                page.paragraphs.push(start..page.chars.len() - 1);
                page.separator();
            }
        }
        // No artificial trailing blank line in clipboard/accessibility text.
        while page.chars.last().is_some_and(|ch| ch.quad.is_none()) {
            page.chars.pop();
        }
        Ok(page)
    }

    pub(crate) fn separator(&mut self) {
        self.chars.push(TextChar {
            ch: '\n',
            quad: None,
            bidi: 0,
        });
    }

    pub fn plain_text(&self) -> String {
        self.text(0..self.chars.len())
    }

    pub fn text(&self, range: Range<usize>) -> String {
        self.chars[range].iter().map(|ch| ch.ch).collect()
    }

    pub fn paragraph(&self, glyph: usize) -> Range<usize> {
        self.paragraphs
            .iter()
            .find(|range| range.contains(&glyph))
            .cloned()
            .unwrap_or(glyph..glyph)
    }

    /// Unicode word boundaries support punctuation, CJK and combining marks.
    pub fn word(&self, glyph: usize) -> Range<usize> {
        self.boundary(glyph, false)
    }

    pub fn grapheme(&self, glyph: usize) -> Range<usize> {
        self.boundary(glyph, true)
    }

    fn boundary(&self, glyph: usize, grapheme: bool) -> Range<usize> {
        let text = self.plain_text();
        let pieces: Vec<&str> = if grapheme {
            text.graphemes(true).collect()
        } else {
            text.split_word_bounds().collect()
        };
        let mut start = 0;
        for piece in pieces {
            let end = start + piece.chars().count();
            if (start..end).contains(&glyph) {
                return start..end;
            }
            start = end;
        }
        glyph..glyph
    }

    /// Find the nearest oriented glyph, then its logical insertion edge.
    /// Distances use PDF points rather than anisotropic normalized coordinates.
    pub fn hit(&self, point: [f32; 2]) -> Option<Hit> {
        let point = [point[0] * self.size[0], point[1] * self.size[1]];
        let (glyph, quad) = self
            .chars
            .iter()
            .enumerate()
            .filter_map(|(i, ch)| {
                let quad = ch.quad?.map(|p| [p[0] * self.size[0], p[1] * self.size[1]]);
                Some((i, quad))
            })
            .min_by(|(_, a), (_, b)| distance(point, *a).total_cmp(&distance(point, *b)))?;
        let line = self.lines.iter().find(|line| line.chars.contains(&glyph))?;
        let center = [
            quad.iter().map(|p| p[0]).sum::<f32>() / 4.0,
            quad.iter().map(|p| p[1]).sum::<f32>() / 4.0,
        ];
        let after = ((point[0] - center[0]) * line.direction[0]
            + (point[1] - center[1]) * line.direction[1]
            >= 0.0)
            ^ (self.chars[glyph].bidi % 2 == 1);
        let cluster = self.grapheme(glyph);
        Some(Hit {
            glyph,
            caret: if after { cluster.end } else { cluster.start },
        })
    }
}

fn numbers<const N: usize>(value: &str) -> Result<[f32; N]> {
    let values: Vec<f32> = value
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    ensure!(
        values.len() == N && values.iter().all(|v| v.is_finite()),
        "invalid text geometry"
    );
    Ok(values.try_into().unwrap())
}

fn distance(point: [f32; 2], quad: Quad) -> f32 {
    let mut positive = false;
    let mut negative = false;
    let mut distance = f32::INFINITY;
    let mut area = 0.0;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        area += a[0] * b[1] - a[1] * b[0];
        let edge = [b[0] - a[0], b[1] - a[1]];
        let p = [point[0] - a[0], point[1] - a[1]];
        let cross = edge[0] * p[1] - edge[1] * p[0];
        positive |= cross > 0.0;
        negative |= cross < 0.0;
        let length = edge[0] * edge[0] + edge[1] * edge[1];
        let t = if length > 0.0 {
            ((p[0] * edge[0] + p[1] * edge[1]) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        distance = distance.min((p[0] - t * edge[0]).powi(2) + (p[1] - t * edge[1]).powi(2));
    }
    if (positive && negative) || area.abs() < f32::EPSILON {
        distance
    } else {
        0.0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::document::{PdfDocument, tests::sample_pdf};

    pub fn selection_pdf() -> Vec<u8> {
        // Content operators deliberately interleave rows across the columns.
        sample_pdf(
            "BT /F1 16 Tf 40 350 Td (Left first) Tj ET
            BT /F1 16 Tf 180 350 Td (Right first) Tj ET
            BT /F1 16 Tf 40 330 Td (Left second) Tj ET
            BT /F1 16 Tf 180 330 Td (Right second) Tj ET
            BT /F1 16 Tf 0 1 -1 0 60 150 Tm (Rotated) Tj ET",
            false,
        )
    }

    fn multilingual_pdf() -> Vec<u8> {
        // ActualText is the PDF's Unicode mapping, not the visible font glyphs.
        sample_pdf(
            "/Span << /ActualText <FEFF0065030100204E2D6587002005D005D1> >> BDC
            BT /F1 16 Tf 40 350 Td (mapped text) Tj ET EMC",
            false,
        )
    }

    fn extract(bytes: &[u8]) -> PageText {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), bytes).unwrap();
        PdfDocument::open(file.path())
            .unwrap()
            .structured_text(0)
            .unwrap()
    }

    #[test]
    fn segmentation_reads_columns_before_rows_and_keeps_rotated_text() {
        let page = extract(&selection_pdf());
        let words: Vec<_> = page
            .plain_text()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            words,
            [
                "Left", "first", "Left", "second", "Right", "first", "Right", "second", "Rotated"
            ]
        );
        let first = page.chars[0].quad.unwrap()[0];
        assert!(
            (first[0] - 0.1).abs() < 0.001,
            "normalize nonzero MediaBox origin"
        );
        let rotated = page
            .lines
            .iter()
            .find(|line| page.text(line.chars.clone()) == "Rotated")
            .unwrap();
        assert_eq!(rotated.direction, [0.0, -1.0]);
        let index = rotated.chars.start;
        let quad = page.chars[index].quad.unwrap();
        let center = [
            quad.iter().map(|p| p[0]).sum::<f32>() / 4.0,
            quad.iter().map(|p| p[1]).sum::<f32>() / 4.0,
        ];
        assert_eq!(
            page.hit([center[0], center[1] + 0.002]).unwrap().caret,
            index
        );
        assert_eq!(
            page.hit([center[0], center[1] - 0.002]).unwrap().caret,
            index + 1
        );
    }

    #[test]
    fn empty_image_only_page_and_invalid_page_have_no_selectable_text() {
        for content in [
            "",
            "q 100 0 0 100 50 200 cm BI /W 1 /H 1 /CS /RGB /BPC 8 /F /AHx ID FF0000> EI Q",
        ] {
            let page = extract(&sample_pdf(content, false));
            assert!(page.plain_text().is_empty());
            assert!(page.hit([0.5, 0.5]).is_none());
        }
        assert!(
            crate::document::tests::sample_document()
                .structured_text(2)
                .is_err()
        );
    }

    #[test]
    fn mupdf_extracts_multilingual_actual_text_without_ascii_loss() {
        let page = extract(&multilingual_pdf());
        assert_eq!(page.plain_text(), "e\u{301} 中文 אב");
        assert_eq!(page.grapheme(1), 0..2);
        assert!(page.chars.iter().filter(|ch| ch.quad.is_some()).count() >= 8);
    }

    #[test]
    fn unicode_bidi_graphemes_words_and_nested_regions_are_preserved() {
        // The XML parser must descend segment regions, decode entities and keep
        // logical RTL order. Geometry puts logical alef to the right of bet.
        let xml = r#"<page><struct><block><line dir="1 0"><font>
            <char c="א" bidi="1" quad="60 10 70 10 60 20 70 20"/>
            <char c="ב" bidi="1" quad="50 10 60 10 50 20 60 20"/>
            <char c=" " bidi="0" quad="70 10 80 10 70 20 80 20"/>
            <char c="e" bidi="0" quad="80 10 90 10 80 20 90 20"/>
            <char c="&#x301;" bidi="0" quad="80 10 90 10 80 20 90 20"/>
            <char c="中" bidi="0" quad="90 10 100 10 90 20 100 20"/>
            <char c="文" bidi="0" quad="100 10 110 10 100 20 110 20"/>
            <char c="&amp;" bidi="0" quad="110 10 120 10 110 20 120 20"/>
            </font></line></block></struct></page>"#;
        let page = PageText::from_xml(xml, mupdf::Rect::new(0.0, 0.0, 200.0, 100.0)).unwrap();
        assert_eq!(page.plain_text(), "אב e\u{301}中文&");
        assert_eq!(page.word(1), 0..2);
        assert_eq!(page.word(4), 3..5);
        assert_eq!(page.grapheme(4), 3..5);
        assert_eq!(page.word(5), 5..6);
        assert_eq!(page.paragraph(6), 0..8);
        assert_eq!(page.hit([0.34, 0.15]).unwrap().caret, 0);
        assert_eq!(page.hit([0.31, 0.15]).unwrap().caret, 1);
        assert_eq!(page.hit([0.44, 0.15]).unwrap().caret, 5);
    }

    #[test]
    fn degenerate_quad_does_not_capture_every_pointer_position() {
        assert!(distance([100.0, 100.0], [[0.0, 0.0]; 4]) > 1000.0);
        // Bounding-box hit testing would incorrectly accept this outside point.
        assert!(distance([0.1, 0.1], [[0.0, 0.5], [0.5, 0.0], [1.0, 0.5], [0.5, 1.0]]) > 0.0);
    }

    #[test]
    #[ignore = "exports selection PDF to REVIEW_FIXTURE_DIR for native tests"]
    fn export_selection_fixture() {
        let directory = std::path::PathBuf::from(std::env::var("REVIEW_FIXTURE_DIR").unwrap());
        std::fs::write(directory.join("selection.pdf"), selection_pdf()).unwrap();
        std::fs::write(directory.join("unicode.pdf"), multilingual_pdf()).unwrap();
    }
}
