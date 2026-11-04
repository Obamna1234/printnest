use super::*;
use printnest_model::Catalogue;
use printnest_model::project::{Crop, Element, Rect};
use std::collections::HashMap;

fn tmp(tag: &str) -> PathBuf {
    // nosemgrep: rust.lang.security.temp-dir.temp-dir -- test-only scratch folder
    let d = std::env::temp_dir().join(format!("printnest-export-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn project(cat: &Catalogue, product: &str) -> Project {
    let mut p = Project::new_blank(cat, product, "Beach day!").unwrap();
    let prod = cat.product(product).unwrap();
    let t = prod.trim();
    let b = prod.bleed_mm;
    for page in &mut p.pages {
        page.elements.push(Element::Photo {
            id: "p".into(),
            rect: Rect {
                x: -b,
                y: -b,
                w: t.width.0 + 2.0 * b,
                h: t.height.0 + 2.0 * b,
                rotation: 0.0,
            },
            src: Some(SafePath::new("photos/a.jpg").unwrap()),
            crop: Crop::default(),
        });
    }
    p
}

fn loader() -> impl FnMut(&str, u32) -> Option<Arc<Photo>> {
    |_src: &str, edge: u32| {
        let e = edge.clamp(1, 64);
        Some(Arc::new(Photo {
            width: e,
            height: e,
            rgba: [200u8, 30, 30, 255].repeat((e * e) as usize),
        }))
    }
}

fn sizes() -> HashMap<String, (u32, u32)> {
    HashMap::from([("photos/a.jpg".to_owned(), (64, 64))])
}

fn no_progress() -> impl FnMut(usize, usize) -> bool {
    |_, _| true
}

#[test]
fn base_names_are_safe() {
    assert_eq!(safe_base_name("Beach day!"), "Beach day");
    assert_eq!(safe_base_name("../../etc"), "etc");
    assert_eq!(safe_base_name("CON"), "PrintNest");
    assert_eq!(safe_base_name("   "), "PrintNest");
    assert_eq!(safe_base_name("a\u{202e}b"), "a_b");
}

#[test]
fn kiosk_jpeg_is_exact_size_without_metadata() {
    let cat = Catalogue::builtin().unwrap();
    let prod = cat.product("print-6x4").unwrap();
    let p = project(&cat, "print-6x4");
    let dir = tmp("kiosk");
    let rep = export(
        &p,
        prod,
        Profile::Kiosk,
        &dir,
        None,
        &sizes(),
        &mut loader(),
        &mut no_progress(),
        &|| false,
    )
    .unwrap();
    assert!(rep.folder.ends_with("Beach day - kiosk"));
    assert!((rep.ppi - 300.0).abs() < 1e-9);
    let jpg = rep.folder.join("Beach day.jpg");
    let bytes = fs::read(&jpg).unwrap();
    assert!(bytes.starts_with(&[0xFF, 0xD8]));
    assert!(!bytes.windows(4).any(|w| w == b"Exif"), "no EXIF");
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap();
    let (w, h) = (img.width(), img.height());
    let (ew, eh) = page_pixels(prod, Area::Trim, 300.0).unwrap();
    assert_eq!((w, h), (ew, eh));
    let px = img.to_rgb8().get_pixel(w / 2, h / 2).0;
    assert!(px[0] > 150 && px[1] < 80, "photo colour {px:?}");
    let note = fs::read_to_string(rep.folder.join("PRINT-ME.txt")).unwrap();
    assert!(note.contains("Print size"));

    // A second export never touches the first.
    let again = export(
        &p,
        prod,
        Profile::Kiosk,
        &dir,
        None,
        &sizes(),
        &mut loader(),
        &mut no_progress(),
        &|| false,
    )
    .unwrap();
    assert!(again.folder.ends_with("Beach day - kiosk (2)"));
    assert_eq!(fs::read(&jpg).unwrap(), bytes);
    // Deterministic.
    assert_eq!(fs::read(again.folder.join("Beach day.jpg")).unwrap(), bytes);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn print_shop_pdf_has_bleed_and_marks() {
    let cat = Catalogue::builtin().unwrap();
    let prod = cat.product("print-6x4").unwrap();
    let p = project(&cat, "print-6x4");
    let dir = tmp("shop");
    let rep = export(
        &p,
        prod,
        Profile::PrintShop,
        &dir,
        None,
        &sizes(),
        &mut loader(),
        &mut no_progress(),
        &|| false,
    )
    .unwrap();
    let pdf = fs::read(rep.folder.join("Beach day - print shop.pdf")).unwrap();
    let s = String::from_utf8_lossy(&pdf);
    assert!(s.contains("/TrimBox") && s.contains("/BleedBox"));
    assert!(s.contains("/DCTDecode"));
    assert!(s.matches(" l S Q").count() >= 8, "crop marks");
    assert!(rep.folder.join("PRINT-ME.txt").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn home_pdf_fits_or_tiles() {
    let cat = Catalogue::builtin().unwrap();
    let dir = tmp("home");
    // A 6x4 print fits on one A4 sheet.
    let small = project(&cat, "print-6x4");
    let prod = cat.product("print-6x4").unwrap();
    let rep = export(
        &small,
        prod,
        Profile::Home(Paper::A4),
        &dir,
        None,
        &sizes(),
        &mut loader(),
        &mut no_progress(),
        &|| false,
    )
    .unwrap();
    let s = String::from_utf8_lossy(&fs::read(rep.files.first().unwrap()).unwrap()).into_owned();
    assert!(s.contains("/Count 1"));
    // An A4 poster goes on one A4 sheet (shrunk to the printable area).
    let a4 = cat.product("poster-a4").unwrap();
    assert_eq!(
        home_pages(a4.trim().width.0, a4.trim().height.0, Paper::A4, 1).len(),
        1
    );
    // An A3 poster needs several A4 sheets (layout checked without the
    // slow full-size render).
    let a3 = cat.product("poster-a3").unwrap();
    let pages = home_pages(a3.trim().width.0, a3.trim().height.0, Paper::A4, 1);
    assert!(
        pages.len() >= 4,
        "A3 on A4 takes several sheets, got {}",
        pages.len()
    );
    let labels: Vec<String> = pages
        .iter()
        .flat_map(|p| p.ops.iter())
        .filter_map(|o| match o {
            pdf::Op::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(labels.iter().any(|l| l.contains("sheet A1")));
    assert!(labels.iter().any(|l| l.contains("sheet B2")));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn every_tile_page_covers_its_window() {
    // Tiles must cover the whole design with overlap: check geometry.
    let cat = Catalogue::builtin().unwrap();
    let prod = cat.product("poster-a2").unwrap();
    let pages = home_pages(prod.trim().width.0, prod.trim().height.0, Paper::A4, 1);
    assert!(pages.len() > 1);
    for pg in &pages {
        let clip = pg.ops.iter().find_map(|o| match o {
            pdf::Op::Image { clip, .. } => *clip,
            _ => None,
        });
        let [x, y, w, h] = clip.unwrap();
        assert!(x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0);
        assert!(x + w <= pg.media.0 + 0.01 && y + h <= pg.media.1 + 0.01);
    }
}

#[test]
fn cancelling_stops_the_export() {
    let cat = Catalogue::builtin().unwrap();
    let prod = cat.product("print-6x4").unwrap();
    let p = project(&cat, "print-6x4");
    let dir = tmp("cancel");
    let r = export(
        &p,
        prod,
        Profile::Kiosk,
        &dir,
        None,
        &sizes(),
        &mut loader(),
        &mut |_, _| false,
        &|| false,
    );
    assert!(matches!(r, Err(ExportError::Cancelled)));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn kiosk_resolution_is_capped_for_big_posters() {
    let cat = Catalogue::builtin().unwrap();
    for prod in cat.products() {
        let ppi = choose_ppi(prod, Area::Trim, KIOSK_MAX_EDGE);
        let (w, h) = page_pixels(prod, Area::Trim, ppi).unwrap();
        assert!(w <= KIOSK_MAX_EDGE && h <= KIOSK_MAX_EDGE, "{}", prod.id);
        assert!(ppi <= TARGET_PPI);
    }
}

#[test]
fn undecodable_photos_are_reported_and_cancel_cleans_up() {
    let cat = Catalogue::builtin().unwrap();
    let prod = cat.product("print-6x4").unwrap();
    let p = project(&cat, "print-6x4");
    let dir = tmp("missing");
    let rep = export(
        &p,
        prod,
        Profile::Kiosk,
        &dir,
        None,
        &sizes(),
        &mut |_: &str, _: u32| None,
        &mut no_progress(),
        &|| false,
    )
    .unwrap();
    assert_eq!(rep.missing, vec!["photos/a.jpg".to_owned()]);
    // No size known at all: also missing, never silently blank.
    let rep = export(
        &p,
        prod,
        Profile::Kiosk,
        &dir,
        None,
        &HashMap::new(),
        &mut loader(),
        &mut no_progress(),
        &|| false,
    )
    .unwrap();
    assert_eq!(rep.missing.len(), 1);
    // No .partial files are left behind.
    for entry in fs::read_dir(&rep.folder).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(!name.to_string_lossy().ends_with(".partial"));
    }
    // A cancelled export removes its own folder.
    let before = fs::read_dir(&dir).unwrap().count();
    let r = export(
        &p,
        prod,
        Profile::PrintShop,
        &dir,
        None,
        &sizes(),
        &mut loader(),
        &mut no_progress(),
        &|| true,
    );
    assert!(matches!(r, Err(ExportError::Cancelled)));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), before);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn long_titles_fit_file_name_limits() {
    let name = safe_base_name(&"\u{20000}".repeat(60));
    assert!(name.len() <= 100, "{}", name.len());
    assert!(format!("{name} - print shop.pdf.partial").len() < 255);
}

#[test]
fn books_export_interior_and_cover() {
    let cat = Catalogue::builtin().unwrap();
    let prod = cat.product("book-20x20").unwrap();
    let mut p = Project::new_blank(&cat, "book-20x20", "Holiday").unwrap();
    // Photo on the front cover panel.
    if let Some(cover) = p.cover_design.as_mut() {
        cover.elements.push(Element::Photo {
            id: "c".into(),
            rect: Rect {
                x: 250.0,
                y: 20.0,
                w: 150.0,
                h: 150.0,
                rotation: 0.0,
            },
            src: Some(SafePath::new("photos/a.jpg").unwrap()),
            crop: Crop::default(),
        });
    }
    let dir = tmp("book");
    let rep = export(
        &p,
        prod,
        Profile::PrintShop,
        &dir,
        cover_geometry(&p, &cat),
        &sizes(),
        &mut loader(),
        &mut no_progress(),
        &|| false,
    )
    .unwrap();
    let names: Vec<String> = rep
        .files
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.contains(&"Holiday - print shop.pdf".to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&"Holiday - cover.pdf".to_owned()),
        "{names:?}"
    );
    let cover = fs::read(rep.folder.join("Holiday - cover.pdf")).unwrap();
    let s = String::from_utf8_lossy(&cover);
    assert!(s.contains("/Count 1") && s.contains("/TrimBox"));
    // Crop marks (8) + spine marks (4).
    assert!(
        s.matches(" l S Q").count() >= 12,
        "{}",
        s.matches(" l S Q").count()
    );
    let interior = fs::read(rep.folder.join("Holiday - print shop.pdf")).unwrap();
    assert!(String::from_utf8_lossy(&interior).contains("/Count 20"));
    let _ = fs::remove_dir_all(&dir);
}
