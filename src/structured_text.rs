//! Permission-neutral, owned page text shared by selection and accessibility.
//!
//! MuPDF segments whitespace-separated regions (including columns). We retain
//! its depth-first block/line/Unicode order, not a global y/x sort. This remains
//! a layout heuristic, not a promise of semantic reading order for every PDF.

use std::ops::Range;

use anyhow::{Result, ensure};
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
    /// Copy native text in depth-first region order. No native handles escape
    /// this conversion, so selection and accessibility can outlive the page.
    pub fn from_native(text: &mupdf::TextPage, bounds: mupdf::Rect) -> Result<Self> {
        let size = [bounds.x1 - bounds.x0, bounds.y1 - bounds.y0];
        ensure!(
            size.iter().all(|v| v.is_finite() && *v > 0.0),
            "invalid page bounds"
        );
        let mut page = Self {
            size,
            ..Self::default()
        };
        page.append_blocks(text.blocks(), bounds)?;
        // No artificial trailing blank line in clipboard/accessibility text.
        while page.chars.last().is_some_and(|ch| ch.quad.is_none()) {
            page.chars.pop();
        }
        Ok(page)
    }

    fn append_blocks(
        &mut self,
        blocks: mupdf::text_page::TextBlockIter<'_>,
        bounds: mupdf::Rect,
    ) -> Result<()> {
        for block in blocks {
            if block.r#type() == mupdf::text_page::TextBlockType::Struct {
                self.append_blocks(block.children(), bounds)?;
                continue;
            }
            let start = self.chars.len();
            for line in block.lines() {
                let start = self.chars.len();
                let direction = line.direction();
                let direction = [direction.x, direction.y];
                ensure!(
                    direction.iter().all(|v| v.is_finite()),
                    "invalid text geometry"
                );
                for ch in line.chars() {
                    let quad = ch.quad();
                    // Native quads are UL, UR, LL, LR; consumers use perimeter order.
                    let points = [quad.ul, quad.ur, quad.lr, quad.ll];
                    ensure!(
                        points.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
                        "invalid text geometry"
                    );
                    let quad = points.map(|p| {
                        [
                            (p.x - bounds.x0) / self.size[0],
                            (p.y - bounds.y0) / self.size[1],
                        ]
                    });
                    self.chars.push(TextChar {
                        // Preserve valid scalars, including XML-forbidden controls.
                        // A non-Unicode native CID/GID has no Rust char equivalent.
                        ch: ch.char().unwrap_or(char::REPLACEMENT_CHARACTER),
                        quad: Some(quad),
                        bidi: ch.bidi(),
                    });
                }
                if self.chars.len() > start {
                    self.lines.push(TextLine {
                        chars: start..self.chars.len(),
                        direction,
                    });
                    self.separator();
                }
            }
            if self.chars.len() > start {
                self.paragraphs.push(start..self.chars.len() - 1);
                self.separator();
            }
        }
        Ok(())
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

    fn extract(bytes: &[u8]) -> std::sync::Arc<PageText> {
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
        assert_eq!(page.word(1), 0..2);
        assert_eq!(page.word(3), 3..4);
        assert_eq!(page.word(7), 6..8);
        assert_eq!(page.paragraph(4), 0..8);
        assert!(page.chars.iter().filter(|ch| ch.quad.is_some()).count() >= 8);
    }

    #[test]
    fn native_bidi_keeps_logical_order_and_reverses_caret_edges() {
        // Map real glyphs rather than ActualText so the PDF interpreter assigns
        // bidi levels. Logical alef is positioned to the right of bet.
        let content = "BT /F1 16 Tf 60 350 Td (A) Tj -10 0 Td (B) Tj ET";
        let cmap = "begincmap 2 beginbfchar <41> <05D0> <42> <05D1> endbfchar endcmap";
        let bytes = crate::document::tests::pdf_objects(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [10 20 310 420] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>".into(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            format!("<< /Length {} >>\nstream\n{cmap}\nendstream", cmap.len()),
        ]);
        let page = extract(&bytes);
        assert_eq!(page.plain_text(), "אב");
        assert_eq!(
            page.chars.iter().map(|c| c.bidi).collect::<Vec<_>>(),
            [1, 1]
        );
        assert_eq!(page.word(1), 0..2);
        let quad = page.chars[0].quad.unwrap();
        let center = [
            quad.iter().map(|p| p[0]).sum::<f32>() / 4.0,
            quad.iter().map(|p| p[1]).sum::<f32>() / 4.0,
        ];
        assert_eq!(page.hit([center[0] + 0.002, center[1]]).unwrap().caret, 0);
        assert_eq!(page.hit([center[0] - 0.002, center[1]]).unwrap().caret, 1);
    }

    #[test]
    fn native_text_preserves_xml_forbidden_controls_and_supplementary_unicode() {
        let page = extract(&sample_pdf(
            "/Span << /ActualText <FEFF004100010026003C003ED83DDE00> >> BDC
            BT /F1 16 Tf 40 350 Td (mapped) Tj ET EMC",
            false,
        ));
        assert_eq!(page.plain_text(), "A\u{1}&<>😀");
        assert_eq!(page.chars.len(), 6);
        assert!(page.chars.iter().all(|c| c.quad.is_some()));
        assert_eq!(page.lines[0].chars, 0..6);
        assert_eq!(page.paragraph(5), 0..6);
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
