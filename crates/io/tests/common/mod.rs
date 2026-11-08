//! A minimal PSD writer for tests: 8-bit RGBA layers, raw or RLE, with groups.

#![allow(dead_code, clippy::unwrap_used)]

/// A layer for the writer: straight RGBA rows, `w × h` at `(left, top)`.
pub struct Spec {
    pub name: &'static str,
    pub left: i32,
    pub top: i32,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub opacity: u8,
    pub blend: &'static [u8; 4],
    pub hidden: bool,
    pub section: Option<u32>,
    pub rle: bool,
}

impl Spec {
    pub fn pixel(name: &'static str, left: i32, top: i32, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Self {
        let rgba = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| f(x, y)).collect();
        Self { name, left, top, w, h, rgba, opacity: 255, blend: b"norm", hidden: false, section: None, rle: false }
    }

    pub fn folder(name: &'static str, kind: u32) -> Self {
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
pub fn write_psd(width: u32, height: u32, layers: &[Spec]) -> Vec<u8> {
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
