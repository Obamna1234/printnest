//! A deliberately tiny PDF writer: pages that show JPEG images, thin lines
//! (crop marks, cut guides) and short Helvetica labels. Nothing else: no
//! JavaScript, forms, links, attachments, launch actions or metadata
//! streams (SPEC section 7, rule 9). Output is deterministic: no dates,
//! no random IDs.

use std::fmt::Write as _;

/// Millimetres to PDF points.
#[must_use]
pub fn pt(mm: f64) -> f64 {
    mm * 72.0 / 25.4
}

/// A JPEG image to place (already encoded, baseline sRGB).
#[derive(Debug, Clone)]
pub struct Jpeg {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// One drawing operation on a page. Coordinates are points from the
/// page's bottom-left corner (PDF convention).
#[derive(Debug, Clone)]
pub enum Op {
    /// Draw image `index` into the rectangle `x, y, w, h`, clipped to
    /// `clip` if given.
    Image {
        index: usize,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        clip: Option<[f64; 4]>,
    },
    /// A thin black line.
    Line {
        from: (f64, f64),
        to: (f64, f64),
        width: f64,
        grey: f64,
    },
    /// ASCII text in Helvetica.
    Label {
        x: f64,
        y: f64,
        size: f64,
        text: String,
    },
}

/// One page.
#[derive(Debug, Clone)]
pub struct Page {
    /// Width and height in points.
    pub media: (f64, f64),
    /// Optional trim and bleed boxes `[x0, y0, x1, y1]` for print shops.
    pub trim: Option<[f64; 4]>,
    pub bleed: Option<[f64; 4]>,
    pub ops: Vec<Op>,
}

fn num(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.to_owned()
    }
}

fn rect(r: [f64; 4]) -> String {
    let [a, b, c, d] = r;
    format!("[{} {} {} {}]", num(a), num(b), num(c), num(d))
}

/// A PDF text string: ASCII printable as a literal, anything else as
/// UTF-16BE hex with a byte-order mark.
#[must_use]
pub fn text_string(s: &str) -> String {
    if s.chars().all(|c| (' '..='~').contains(&c)) {
        let mut out = String::from("(");
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        out
    } else {
        let mut out = String::from("<FEFF");
        for u in s.encode_utf16() {
            let _ = write!(out, "{u:04X}");
        }
        out.push('>');
        out
    }
}

/// Label text limited to printable ASCII (the built-in font's range).
fn label_text(s: &str) -> String {
    let ascii: String = s
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .take(200)
        .collect();
    text_string(&ascii)
}

/// Builds the whole file.
#[must_use]
pub fn write(title: &str, pages: &[Page], images: &[Jpeg]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    // Object numbers: 1 catalog, 2 pages, 3 info, 4 font, then images,
    // then per page (page, content).
    let first_image = 5;
    let first_page = first_image + images.len();
    let page_obj = |i: usize| first_page + i * 2;

    let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };

    obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", page_obj(i)))
        .collect();
    obj(
        &mut out,
        &mut offsets,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    );
    obj(
        &mut out,
        &mut offsets,
        format!(
            "<< /Title {} /Creator (PrintNest) /Producer (PrintNest) >>",
            text_string(title)
        )
        .as_bytes(),
    );
    obj(
        &mut out,
        &mut offsets,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    for img in images {
        let mut body = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            img.width,
            img.height,
            img.data.len()
        )
        .into_bytes();
        body.extend_from_slice(&img.data);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offsets, &body);
    }
    for (i, page) in pages.iter().enumerate() {
        let mut content = String::new();
        let mut used: Vec<usize> = Vec::new();
        for op in &page.ops {
            match op {
                Op::Image {
                    index,
                    x,
                    y,
                    w,
                    h,
                    clip,
                } => {
                    if *index >= images.len() {
                        continue;
                    }
                    if !used.contains(index) {
                        used.push(*index);
                    }
                    content.push_str("q\n");
                    if let Some([cx, cy, cw, ch]) = clip {
                        let _ = writeln!(
                            content,
                            "{} {} {} {} re W n",
                            num(*cx),
                            num(*cy),
                            num(*cw),
                            num(*ch)
                        );
                    }
                    let _ = writeln!(
                        content,
                        "{} 0 0 {} {} {} cm /Im{index} Do\nQ",
                        num(*w),
                        num(*h),
                        num(*x),
                        num(*y)
                    );
                }
                Op::Line {
                    from,
                    to,
                    width,
                    grey,
                } => {
                    let _ = writeln!(
                        content,
                        "q {} w {} G {} {} m {} {} l S Q",
                        num(*width),
                        num(*grey),
                        num(from.0),
                        num(from.1),
                        num(to.0),
                        num(to.1)
                    );
                }
                Op::Label { x, y, size, text } => {
                    let _ = writeln!(
                        content,
                        "BT /F1 {} Tf {} {} Td {} Tj ET",
                        num(*size),
                        num(*x),
                        num(*y),
                        label_text(text)
                    );
                }
            }
        }
        let xobjects: Vec<String> = used
            .iter()
            .map(|i| format!("/Im{i} {} 0 R", first_image + i))
            .collect();
        let mut boxes = String::new();
        if let Some(t) = page.trim {
            let _ = write!(boxes, " /TrimBox {}", rect(t));
        }
        if let Some(b) = page.bleed {
            let _ = write!(boxes, " /BleedBox {}", rect(b));
        }
        obj(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox {}{boxes} \
                 /Resources << /XObject << {} >> /Font << /F1 4 0 R >> >> /Contents {} 0 R >>",
                rect([0.0, 0.0, page.media.0, page.media.1]),
                xobjects.join(" "),
                page_obj(i) + 1
            )
            .as_bytes(),
        );
        let mut body = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        body.extend_from_slice(content.as_bytes());
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offsets, &body);
    }

    let xref_at = out.len();
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in &offsets {
        // Exactly 20 bytes per entry: trailing space + newline.
        let _ = writeln!(xref, "{o:010} 00000 n ");
    }
    let _ = write!(
        xref,
        "trailer\n<< /Size {} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        offsets.len() + 1
    );
    out.extend_from_slice(xref.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_escaped() {
        assert_eq!(text_string("a(b)\\c"), "(a\\(b\\)\\\\c)");
        assert_eq!(text_string("é"), "<FEFF00E9>");
        assert_eq!(label_text("Été"), "(?t?)");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(2.0), "2");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(f64::NAN), "0");
    }

    #[test]
    fn structure_is_valid_and_offsets_match() {
        let img = Jpeg {
            width: 1,
            height: 1,
            data: vec![0xFF, 0xD8, 0xFF, 0xD9],
        };
        let page = Page {
            media: (100.0, 200.0),
            trim: Some([10.0, 10.0, 90.0, 190.0]),
            bleed: None,
            ops: vec![
                Op::Image {
                    index: 0,
                    x: 0.0,
                    y: 0.0,
                    w: 100.0,
                    h: 200.0,
                    clip: Some([0.0, 0.0, 50.0, 50.0]),
                },
                Op::Line {
                    from: (0.0, 0.0),
                    to: (10.0, 10.0),
                    width: 0.25,
                    grey: 0.0,
                },
                Op::Label {
                    x: 5.0,
                    y: 5.0,
                    size: 8.0,
                    text: "A1 (top)".into(),
                },
                Op::Image {
                    index: 7,
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    clip: None,
                },
            ],
        };
        let pdf = write("Holiday (2026)", &[page.clone(), page], &[img]);
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.starts_with("%PDF-1.4"));
        assert!(s.ends_with("%%EOF\n"));
        assert!(s.contains("/Count 2"));
        assert!(s.contains("/Title (Holiday \\(2026\\))"));
        assert!(!s.contains("/Im7"), "out-of-range image ignored");
        for banned in [
            "/JavaScript",
            "/JS",
            "/AA",
            "/OpenAction",
            "/Launch",
            "/URI",
            "/EmbeddedFile",
            "/AcroForm",
        ] {
            assert!(!s.contains(banned), "{banned}");
        }
        // Every xref offset points at "N 0 obj".
        // Offsets are byte positions (the file contains binary JPEG data).
        let xref = pdf.windows(5).position(|w| w == b"xref\n").unwrap();
        let tail = std::str::from_utf8(&pdf[xref..]).unwrap();
        let entries: Vec<usize> = tail
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .map(|l| l[..10].parse().unwrap())
            .collect();
        assert_eq!(entries.len(), 4 + 1 + 2 * 2);
        for (i, off) in entries.iter().enumerate() {
            assert!(pdf[*off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()));
        }
        // Deterministic.
        let again = write("Holiday (2026)", &[], &[]);
        assert_eq!(again, write("Holiday (2026)", &[], &[]));
    }
}
