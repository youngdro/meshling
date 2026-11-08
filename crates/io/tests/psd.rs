//! PSD reading, against documents built by a minimal writer below (no binary fixtures in the
//! repo), plus an opt-in check against a real See-through export.

#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use meshling_geom::Size;
use meshling_io::psd::premultiply;
use meshling_io::{PsdError, read_psd};
use meshling_model::Blend;

use meshling_io::psd_writer::{Spec, write_psd};

#[test]
fn reads_offsets_order_and_premultiplies() {
    let bytes = write_psd(
        64,
        32,
        &[
            Spec::pixel("back", 2, 3, 10, 6, |_, _| [200, 100, 50, 255]),
            Spec::pixel("front", 40, 20, 4, 2, |x, _| [255, 0, 0, if x < 2 { 128 } else { 255 }]),
        ],
    );
    let doc = read_psd(&bytes).unwrap();
    assert_eq!(doc.size, Size::new(64, 32));
    let names: Vec<_> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["back", "front"], "bottom to top");
    let front = &doc.layers[1];
    assert_eq!((front.left, front.top), (40, 20));
    assert_eq!(front.pixels.size, Size::new(4, 2));
    assert_eq!(front.pixels.get(0, 0), [128, 0, 0, 128]);
    assert_eq!(front.pixels.get(3, 1), [255, 0, 0, 255]);
    assert_eq!(doc.layers[0].pixels.get(9, 5), [200, 100, 50, 255]);
    assert!(doc.composite.is_some());
}

#[test]
fn rle_and_raw_channels_decode_the_same() {
    let art =
        |x: u32, y: u32| [(x * 9) as u8, (y * 17) as u8, if x % 7 < 4 { 30 } else { 31 }, if y == 0 { 0 } else { 255 }];
    let raw = read_psd(&write_psd(300, 20, &[Spec::pixel("a", 0, 0, 300, 20, art)])).unwrap();
    let rle = read_psd(&write_psd(300, 20, &[Spec { rle: true, ..Spec::pixel("a", 0, 0, 300, 20, art) }])).unwrap();
    assert_eq!(raw.layers, rle.layers);
    // Row 0 is transparent, so the layer is trimmed to start one row lower.
    assert_eq!((rle.layers[0].top, rle.layers[0].pixels.size), (1, Size::new(300, 19)));
}

#[test]
fn trims_transparent_margins_and_skips_empty_layers() {
    let doc = read_psd(&write_psd(
        16,
        16,
        &[
            Spec::pixel("empty", 0, 0, 8, 8, |_, _| [9, 9, 9, 0]),
            Spec::pixel("dot", 4, 4, 6, 6, |x, y| if (x, y) == (2, 3) { [1, 2, 3, 255] } else { [0; 4] }),
            Spec::pixel("tiny", 10, 1, 2, 3, |_, _| [5, 5, 5, 255]),
        ],
    ))
    .unwrap();
    assert_eq!(doc.empty_layers, ["empty"]);
    let dot = &doc.layers[0];
    assert_eq!((dot.left, dot.top, dot.pixels.size), (6, 7, Size::new(1, 1)));
    let tiny = &doc.layers[1];
    assert_eq!((tiny.left, tiny.top, tiny.pixels.size), (10, 1, Size::new(2, 3)));
}

#[test]
fn reads_groups_visibility_blend_opacity_and_unicode_names() {
    let solid = |_, _| [10, 20, 30, 255];
    let doc = read_psd(&write_psd(
        8,
        8,
        &[
            Spec::pixel("body", 0, 0, 2, 2, solid),
            Spec::folder("</Layer group>", 3),
            Spec::folder("</Layer group>", 3),
            Spec { blend: b"mul ", opacity: 128, ..Spec::pixel("眼影", 0, 0, 2, 2, solid) },
            Spec { hidden: true, ..Spec::folder("eye_L", 1) },
            Spec { blend: b"lddg", ..Spec::pixel("glow", 0, 0, 2, 2, solid) },
            Spec { blend: b"over", hidden: true, ..Spec::pixel("odd", 0, 0, 2, 2, solid) },
            Spec::folder("head", 2),
            Spec::pixel("top", 0, 0, 2, 2, solid),
        ],
    ))
    .unwrap();
    let summary: Vec<_> =
        doc.layers.iter().map(|l| (l.name.as_str(), l.groups.join("/"), l.blend, l.visible)).collect();
    assert_eq!(
        summary,
        [
            ("body", String::new(), Blend::Normal, true),
            ("眼影", "head/eye_L".into(), Blend::Multiply, true),
            ("glow", "head".into(), Blend::Add, true),
            ("odd", "head".into(), Blend::Normal, false),
            ("top", String::new(), Blend::Normal, true),
        ]
    );
    assert_eq!(doc.layers[1].opacity, 128);
    assert!(doc.layers[3].blend_approximated && !doc.layers[2].blend_approximated);
    let groups: Vec<_> = doc.groups.iter().map(|g| (g.path.join("/"), g.visible)).collect();
    assert_eq!(groups, [("head".into(), true), ("head/eye_L".into(), false)]);
}

#[test]
fn rejects_what_it_cannot_read() {
    assert_eq!(read_psd(b"GIF89a"), Err(PsdError::NotPsd));
    let mut psb = write_psd(4, 4, &[]);
    psb[5] = 2;
    assert!(matches!(read_psd(&psb), Err(PsdError::Unsupported(_))));
    let mut sixteen = write_psd(4, 4, &[]);
    sixteen[23] = 16;
    assert!(matches!(read_psd(&sixteen), Err(PsdError::Unsupported(_))));
    let full = write_psd(16, 16, &[Spec::pixel("a", 0, 0, 16, 16, |_, _| [1, 2, 3, 255])]);
    for cut in [30, 60, 100, 200, 600, 1000] {
        assert!(read_psd(&full[..cut]).is_err(), "truncated at {cut}");
    }
}

#[test]
fn premultiply_rounds_to_nearest() {
    assert_eq!(premultiply(255, 128), 128);
    assert_eq!(premultiply(1, 128), 1);
    assert_eq!(premultiply(200, 0), 0);
    assert_eq!(premultiply(77, 255), 77);
}

/// A real See-through export. Its licence is unclear, so it is not in the repo; run with
/// `MESHLING_SAMPLE_PSD=/path/to/seethrough_output.psd cargo test -p meshling-io -- --ignored`.
#[test]
#[ignore = "needs MESHLING_SAMPLE_PSD"]
fn reads_a_real_see_through_export() {
    let path = std::env::var("MESHLING_SAMPLE_PSD").expect("set MESHLING_SAMPLE_PSD");
    let doc = read_psd(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(doc.layers.len(), 16);
    assert_eq!(doc.layers.first().map(|l| l.name.as_str()), Some("back hair"));
    assert!(doc.layers.iter().all(|l| l.visible && l.groups.is_empty()));
    let nose = doc.layers.iter().find(|l| l.name == "nose").unwrap();
    assert_eq!(nose.pixels.size, Size::new(2, 3));

    // Re-composite (Normal, over transparent) and compare with the stored merged image, which
    // See-through writes as straight colour with alpha dropped: premultiply it by our alpha.
    let composite = doc.composite.as_ref().unwrap();
    let (w, h) = (doc.size.width as i32, doc.size.height as i32);
    let mut out = vec![[0f32; 4]; (w * h) as usize];
    for l in &doc.layers {
        for y in 0..l.pixels.size.height {
            for x in 0..l.pixels.size.width {
                let (cx, cy) = (l.left + x as i32, l.top + y as i32);
                if cx < 0 || cy < 0 || cx >= w || cy >= h {
                    continue;
                }
                let s = l.pixels.get(x, y).map(|c| f32::from(c) / 255.0);
                let d = &mut out[(cy * w + cx) as usize];
                for k in 0..4 {
                    d[k] = s[k] + d[k] * (1.0 - s[3]);
                }
            }
        }
    }
    let mut sum = 0.0;
    for (i, px) in out.iter().enumerate() {
        for k in 0..3 {
            let ours = px[k] * 255.0;
            let theirs = f32::from(composite.rgba[i * 4 + k]) * px[3];
            sum += f64::from((ours - theirs).abs());
        }
    }
    let mean = sum / (out.len() * 3) as f64;
    assert!(mean < 0.5, "mean difference from the stored composite: {mean}/255");
}

#[test]
fn corrupted_files_error_instead_of_panicking() {
    let solid = |x: u32, y: u32| [x as u8, y as u8, 7, if (x + y).is_multiple_of(5) { 0 } else { 255 }];
    let good = write_psd(
        24,
        24,
        &[
            Spec { rle: true, ..Spec::pixel("a", -3, 2, 20, 9, solid) },
            Spec::folder("</Layer group>", 3),
            Spec::pixel("b", 5, 5, 7, 7, solid),
            Spec::folder("g", 1),
        ],
    );
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..3000 {
        let mut bytes = good.clone();
        for _ in 0..1 + next() % 4 {
            let i = (next() % bytes.len() as u64) as usize;
            bytes[i] = next() as u8;
        }
        let _ = read_psd(&bytes);
    }
}
