use super::*;
use printnest_model::Catalogue;
use printnest_model::project::{Align, Crop};
use printnest_model::safe_path::SafePath;

fn cat() -> Catalogue {
    Catalogue::builtin().unwrap()
}

/// 2×2 photo: red, green / blue, white.
fn quad_photo() -> Arc<Photo> {
    Arc::new(Photo {
        width: 2,
        height: 2,
        rgba: vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ],
    })
}

fn photo_el(rect: Rect) -> Element {
    Element::Photo {
        id: "p".into(),
        rect,
        src: Some(SafePath::new("photos/q.png").unwrap()),
        crop: Crop::default(),
    }
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 40)
}

#[test]
fn page_sizes_at_print_resolution() {
    let c = cat();
    let a4 = c.product("poster-a4").unwrap();
    assert_eq!(page_pixels(a4, Area::Trim, 300.0).unwrap(), (2480, 3508));
    let (w, h) = page_pixels(a4, Area::Artwork, 300.0).unwrap();
    assert!(w > 2480 && h > 3508);
    for bad in [0.0, 10.0, f64::NAN, f64::INFINITY, 5000.0] {
        assert_eq!(page_pixels(a4, Area::Trim, bad), Err(RenderError::BadPpi));
    }
    let big = c.products().iter().max_by(|a, b| {
        (a.trim().width.0 * a.trim().height.0).total_cmp(&(b.trim().width.0 * b.trim().height.0))
    });
    assert!(matches!(
        page_pixels(big.unwrap(), Area::Trim, 1200.0),
        Err(RenderError::TooLarge(..))
    ));
}

#[test]
fn background_and_missing_photo() {
    let c = cat();
    let prod = c.product("print-6x4").unwrap();
    let mut p = Project::new_blank(&c, "print-6x4", "t").unwrap();
    p.pages[0].background = "#336699".into();
    p.pages[0].elements.push(photo_el(Rect {
        x: 0.0,
        y: 0.0,
        w: 50.0,
        h: 50.0,
        rotation: 0.0,
    }));
    let r = render_page(&p, prod, 0, HashMap::new(), 50.0, Area::Trim, &|| false).unwrap();
    assert_eq!(r.pixel(5, 5), Some([0x33, 0x66, 0x99]));
    assert_eq!(
        r.rgb.len() as u64,
        u64::from(r.width) * u64::from(r.height) * 3
    );
    assert!(render_page(&p, prod, 9, HashMap::new(), 50.0, Area::Trim, &|| false).is_err());
}

#[test]
fn photo_fills_its_frame_the_right_way_up() {
    let c = cat();
    let prod = c.product("poster-a4").unwrap();
    let mut p = Project::new_blank(&c, "poster-a4", "t").unwrap();
    // Square frame of a square photo: no crop.
    p.pages[0].elements.push(photo_el(Rect {
        x: 10.0,
        y: 10.0,
        w: 100.0,
        h: 100.0,
        rotation: 0.0,
    }));
    let photos = HashMap::from([("photos/q.png".to_owned(), quad_photo())]);
    let ppi = 50.8; // 2 px per mm
    let r = render_page(&p, prod, 0, photos.clone(), ppi, Area::Trim, &|| false).unwrap();
    let at = |x: u32, y: u32| r.pixel(x * 2, y * 2).unwrap();
    assert!(near(at(30, 30), [255, 0, 0]), "top-left red");
    assert!(near(at(90, 30), [0, 255, 0]), "top-right green");
    assert!(near(at(30, 90), [0, 0, 255]), "bottom-left blue");
    assert!(near(at(90, 90), [255, 255, 255]));
    // Outside the frame: page background (white).
    assert_eq!(at(5, 5), [255, 255, 255]);
    assert_eq!(at(150, 150), [255, 255, 255]);
}

#[test]
fn rotation_matches_the_editor() {
    let c = cat();
    let prod = c.product("poster-a4").unwrap();
    let mut p = Project::new_blank(&c, "poster-a4", "t").unwrap();
    let rect = Rect {
        x: 50.0,
        y: 50.0,
        w: 100.0,
        h: 100.0,
        rotation: 90.0,
    };
    p.pages[0].elements.push(photo_el(rect));
    let photos = HashMap::from([("photos/q.png".to_owned(), quad_photo())]);
    let r = render_page(&p, prod, 0, photos.clone(), 50.8, Area::Trim, &|| false).unwrap();
    // The editor's TL corner after a 90° clockwise turn is at top-right.
    let tl = printnest_layout::geometry::corners(&rect)[0];
    let (x, y) = (tl.x - 15.0, tl.y + 15.0); // just inside that corner
    assert!(
        near(r.pixel(x as u32 * 2, y as u32 * 2).unwrap(), [255, 0, 0]),
        "red follows the top-left corner: {:?}",
        r.pixel(x as u32 * 2, y as u32 * 2)
    );
}

#[test]
fn crop_zoom_shows_the_centre() {
    let c = cat();
    let prod = c.product("poster-a4").unwrap();
    let mut p = Project::new_blank(&c, "poster-a4", "t").unwrap();
    p.pages[0].elements.push(Element::Photo {
        id: "p".into(),
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            rotation: 0.0,
        },
        src: Some(SafePath::new("photos/q.png").unwrap()),
        // Zoomed right in, panned fully to the top-left quadrant.
        crop: Crop {
            zoom: 4.0,
            pan_x: -1.0,
            pan_y: -1.0,
        },
    });
    let photos = HashMap::from([("photos/q.png".to_owned(), quad_photo())]);
    let r = render_page(&p, prod, 0, photos.clone(), 50.8, Area::Trim, &|| false).unwrap();
    assert!(near(r.pixel(40, 40).unwrap(), [255, 0, 0]));
    assert!(
        near(r.pixel(160, 160).unwrap(), [255, 0, 0]),
        "only red visible"
    );
}

#[test]
fn text_is_drawn_in_its_box() {
    let c = cat();
    let prod = c.product("poster-a4").unwrap();
    let mut p = Project::new_blank(&c, "poster-a4", "t").unwrap();
    p.pages[0].elements.push(Element::Text {
        id: "t".into(),
        rect: Rect {
            x: 20.0,
            y: 20.0,
            w: 150.0,
            h: 30.0,
            rotation: 0.0,
        },
        text: "HELLO".into(),
        font: "sans".into(),
        size_pt: 72.0,
        colour: "#000000".into(),
        align: Align::Left,
    });
    let r = render_page(&p, prod, 0, HashMap::new(), 50.0, Area::Trim, &|| false).unwrap();
    let px_per_mm = 50.0 / 25.4;
    let dark = |x0: f64, y0: f64, x1: f64, y1: f64| {
        let mut n = 0;
        for y in (y0 * px_per_mm) as u32..(y1 * px_per_mm) as u32 {
            for x in (x0 * px_per_mm) as u32..(x1 * px_per_mm) as u32 {
                if r.pixel(x, y).is_some_and(|p| p[0] < 100) {
                    n += 1;
                }
            }
        }
        n
    };
    assert!(dark(20.0, 20.0, 170.0, 50.0) > 200, "text inside its box");
    assert_eq!(dark(0.0, 60.0, 210.0, 297.0), 0, "nothing below the box");
}

#[test]
fn rendering_is_deterministic() {
    let c = cat();
    let prod = c.product("print-6x4").unwrap();
    let mut p = Project::new_blank(&c, "print-6x4", "t").unwrap();
    p.pages[0].elements.push(photo_el(Rect {
        x: 3.0,
        y: 4.0,
        w: 70.0,
        h: 40.0,
        rotation: 12.5,
    }));
    let photos = HashMap::from([("photos/q.png".to_owned(), quad_photo())]);
    let a = render_page(&p, prod, 0, photos.clone(), 100.0, Area::Artwork, &|| false).unwrap();
    let b = render_page(&p, prod, 0, photos.clone(), 100.0, Area::Artwork, &|| false).unwrap();
    assert_eq!(a, b);
}

#[test]
fn bad_photo_buffers_are_skipped() {
    let c = cat();
    let prod = c.product("print-6x4").unwrap();
    let mut p = Project::new_blank(&c, "print-6x4", "t").unwrap();
    p.pages[0].elements.push(photo_el(Rect {
        x: 0.0,
        y: 0.0,
        w: 50.0,
        h: 50.0,
        rotation: 0.0,
    }));
    let wrong = Arc::new(Photo {
        width: 10,
        height: 10,
        rgba: vec![0; 7],
    });
    let photos = HashMap::from([("photos/q.png".to_owned(), wrong)]);
    let r = render_page(&p, prod, 0, photos.clone(), 50.0, Area::Trim, &|| false).unwrap();
    assert_eq!(r.pixel(10, 10), Some([255, 255, 255]));
}

#[test]
fn stacked_photo_bombs_are_refused_and_cancel_works() {
    let c = cat();
    let prod = c.product("print-6x4").unwrap();
    let mut p = Project::new_blank(&c, "print-6x4", "t").unwrap();
    for _ in 0..40 {
        p.pages[0].elements.push(photo_el(Rect {
            x: 0.0,
            y: 0.0,
            w: 152.0,
            h: 102.0,
            rotation: 0.0,
        }));
    }
    let photos = HashMap::from([("photos/q.png".to_owned(), quad_photo())]);
    assert_eq!(
        render_page(&p, prod, 0, photos.clone(), 50.0, Area::Trim, &|| false),
        Err(RenderError::TooComplex)
    );
    p.pages[0].elements.truncate(2);
    assert_eq!(
        render_page(&p, prod, 0, photos, 50.0, Area::Trim, &|| true),
        Err(RenderError::Cancelled)
    );
}

#[test]
fn photo_requests_follow_the_photo_shape_and_fit_the_budget() {
    let c = cat();
    let mut p = Project::new_blank(&c, "poster-a4", "t").unwrap();
    // Wide frame, portrait photo: the photo is shown much taller than the
    // frame, so more pixels are needed than the frame alone suggests.
    p.pages[0].elements.push(photo_el(Rect {
        x: 0.0,
        y: 0.0,
        w: 200.0,
        h: 100.0,
        rotation: 0.0,
    }));
    let e = needed_edge(&p, 0, "photos/q.png", (3000, 4000), 300.0, 100_000);
    // 200 mm wide -> photo shown 200 x 266.7 mm -> long side 3150 px.
    assert!((3140..=3160).contains(&e), "{e}");
    let fitted = fit_budget(&[(20_000, 1.0); 10]);
    let total: u64 = fitted.iter().map(|&e| u64::from(e) * u64::from(e)).sum();
    assert!(total <= MAX_PAGE_PHOTO_PIXELS);
    assert_eq!(fit_budget(&[(1000, 1.0)]), vec![1000]);
}
