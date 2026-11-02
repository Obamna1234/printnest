//! Golden render: a fixed design must keep rendering the same way.
//! Set PRINTNEST_UPDATE_GOLDEN=1 to regenerate after an intended change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use printnest_model::Catalogue;
use printnest_model::project::{Align, Crop, Element, Project, Rect};
use printnest_model::safe_path::SafePath;
use printnest_render::{Area, Photo, render_page};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/print-6x4-50ppi.png")
}

fn design(cat: &Catalogue) -> Project {
    let mut p = Project::new_blank(cat, "print-6x4", "Golden").unwrap();
    p.pages[0].background = "#f4efe6".into();
    let els = &mut p.pages[0].elements;
    els.push(Element::Photo {
        id: "a".into(),
        rect: Rect {
            x: 6.0,
            y: 6.0,
            w: 70.0,
            h: 50.0,
            rotation: 0.0,
        },
        src: Some(SafePath::new("photos/grad.png").unwrap()),
        crop: Crop {
            zoom: 1.5,
            pan_x: 0.3,
            pan_y: -0.2,
        },
    });
    els.push(Element::Photo {
        id: "b".into(),
        rect: Rect {
            x: 82.0,
            y: 14.0,
            w: 55.0,
            h: 40.0,
            rotation: -8.0,
        },
        src: Some(SafePath::new("photos/grad.png").unwrap()),
        crop: Crop::default(),
    });
    els.push(Element::Text {
        id: "t".into(),
        rect: Rect {
            x: 6.0,
            y: 66.0,
            w: 140.0,
            h: 30.0,
            rotation: 0.0,
        },
        text: "Golden test\nPrintNest".into(),
        font: "sans".into(),
        size_pt: 24.0,
        colour: "#203040".into(),
        align: Align::Center,
    });
    p
}

fn gradient() -> Arc<Photo> {
    let (w, h) = (96u32, 64u32);
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, 160, 255]);
        }
    }
    Arc::new(Photo {
        width: w,
        height: h,
        rgba,
    })
}

#[test]
fn matches_the_golden_image() {
    let cat = Catalogue::builtin().unwrap();
    let prod = cat.product("print-6x4").unwrap();
    let photos = HashMap::from([("photos/grad.png".to_owned(), gradient())]);
    let r = render_page(&design(&cat), prod, 0, photos, 50.0, Area::Artwork, &|| {
        false
    })
    .unwrap();
    let path = golden_path();
    if std::env::var_os("PRINTNEST_UPDATE_GOLDEN").is_some() {
        image::save_buffer(
            &path,
            &r.rgb,
            r.width,
            r.height,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    }
    let golden = image::open(&path).expect("golden image exists").to_rgb8();
    assert_eq!((golden.width(), golden.height()), (r.width, r.height));
    // Pixels differing by more than a little (SIMD/rounding differences
    // between CPUs are tiny); at most 0.1% may differ (SPEC section 10).
    let differing = golden
        .as_raw()
        .chunks_exact(3)
        .zip(r.rgb.chunks_exact(3))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 8))
        .count();
    let total = (r.width * r.height) as usize;
    assert!(
        differing * 1000 <= total,
        "{differing} of {total} pixels differ from {}",
        path.display()
    );
}
