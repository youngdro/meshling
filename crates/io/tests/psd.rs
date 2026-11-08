//! PSD reading, against documents built by a minimal writer below (no binary fixtures in the
//! repo), plus an opt-in check against a real See-through export.

#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use meshling_geom::Size;
use meshling_io::psd::premultiply;
use meshling_io::{PsdError, read_psd};
use meshling_model::Blend;

/// A layer for the writer: straight RGBA rows, `w × h` at `(left, top)`.
struct Spec {
    name: &'static str,
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    opacity: u8,
    blend: &'static [u8; 4],
    hidden: bool,
    section: Option<u32>,
    rle: bool,
}

impl Spec {
    fn pixel(name: &'static str, left: i32, top: i32, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Self {
        let rgba = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| f(x, y)).collect();
        Self { name, left, top, w, h, rgba, opacity: 255, blend: b"norm", hidden: false, section: None, rle: false }
    }

    fn folder(name: &'static str, kind: u32) -> Self {
        Self { section: Some(kind), ..Self::pixel(name, 0, 0, 0, 0, |_, _| [0; 4]) }
    }
}

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// PackBits: runs of 3+ equal bytes as repeats, everything else as literals.
fn pack_bits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        let mut run = 1;
        while i + run < row.len() && row[i + run] == row[i] && run < 128 {
            run += 1;
        }
        if run >= 3 {
            out.push((1 - run as i16) as u8);
            out.push(row[i]);
            i += run;
        } else {
            let start = i;
            while i < row.len() && i - start < 128 {
                if i + 2 < row.len() && row[i] == row[i + 1] && row[i] == row[i + 2] {
                    break;
                }
                i += 1;
            }
            out.push((i - start - 1) as u8);
            out.extend_from_slice(&row[start..i]);
        }
    }
    out
}

/// Encode one channel plane with its compression tag.
fn channel(plane: &[u8], w: usize, rle: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if !rle || w == 0 {
        be16(&mut out, 0);
        out.extend_from_slice(plane);
        return out;
    }
    be16(&mut out, 1);
    let rows: Vec<Vec<u8>> = plane.chunks(w).map(pack_bits).collect();
    for r in &rows {
        be16(&mut out, r.len() as u16);
    }
    for r in rows {
        out.extend(r);
    }
    out
}

/// A PSD with `layers` listed bottom to top.
fn write_psd(width: u32, height: u32, layers: &[Spec]) -> Vec<u8> {
    let mut out = b"8BPS".to_vec();
    be16(&mut out, 1);
    out.extend([0; 6]);
    be16(&mut out, 4);
    be32(&mut out, height);
    be32(&mut out, width);
    be16(&mut out, 8);
    be16(&mut out, 3);
    be32(&mut out, 0); // colour mode data
    be32(&mut out, 0); // image resources

    let mut records = Vec::new();
    let mut data = Vec::new();
    be16(&mut records, (-(layers.len() as i16)) as u16);
    for l in layers {
        let n = (l.w * l.h) as usize;
        let plane = |c: usize| (0..n).map(|i| l.rgba[i * 4 + c]).collect::<Vec<u8>>();
        let chans: Vec<(i16, Vec<u8>)> =
            [(-1, 3), (0, 0), (1, 1), (2, 2)].map(|(id, c)| (id, channel(&plane(c), l.w as usize, l.rle))).into();
        for v in [l.top, l.left, l.top + l.h as i32, l.left + l.w as i32] {
            be32(&mut records, v as u32);
        }
        be16(&mut records, 4);
        for (id, bytes) in &chans {
            be16(&mut records, *id as u16);
            be32(&mut records, bytes.len() as u32);
            data.extend_from_slice(bytes);
        }
        records.extend_from_slice(b"8BIM");
        records.extend_from_slice(l.blend);
        records.extend([l.opacity, 0, if l.hidden { 2 } else { 0 }, 0]);
        let mut extra = Vec::new();
        be32(&mut extra, 0); // mask
        be32(&mut extra, 0); // blending ranges
        let ascii: Vec<u8> = l.name.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }).collect();
        extra.push(ascii.len() as u8);
        extra.extend(&ascii);
        while extra.len() % 4 != 0 {
            extra.push(0);
        }
        let units: Vec<u16> = l.name.encode_utf16().collect();
        extra.extend_from_slice(b"8BIMluni");
        be32(&mut extra, 4 + 2 * units.len() as u32 + (units.len() % 2 * 2) as u32);
        be32(&mut extra, units.len() as u32);
        for u in &units {
            be16(&mut extra, *u);
        }
        if units.len() % 2 == 1 {
            be16(&mut extra, 0);
        }
        if let Some(kind) = l.section {
            extra.extend_from_slice(b"8BIMlsct");
            be32(&mut extra, 4);
            be32(&mut extra, kind);
        }
        be32(&mut records, extra.len() as u32);
        records.extend(extra);
    }
    let mut info = records;
    info.extend(data);
    if info.len() % 2 == 1 {
        info.push(0);
    }
    let mut section = Vec::new();
    be32(&mut section, info.len() as u32);
    section.extend(info);
    be32(&mut section, 0); // global mask info
    be32(&mut out, section.len() as u32);
    out.extend(section);

    // Merged image: raw, all black and transparent.
    be16(&mut out, 0);
    out.extend(std::iter::repeat_n(0, (width * height * 4) as usize));
    out
}

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
    let solid = |x: u32, y: u32| [x as u8, y as u8, 7, if (x + y) % 5 == 0 { 0 } else { 255 }];
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
