//! A small reader for the parts of Photoshop documents that matter to a rig: 8-bit RGB(A)
//! layers with their names, offsets, opacity, blend mode, visibility and group path, plus the
//! document's own merged image (used to check a re-composite).
//!
//! Supported: PSD version 1, 8 bits per channel, RGB colour mode, raw or RLE (PackBits)
//! channel data. Anything else is rejected with a [`PsdError`] rather than misread. Layer masks
//! and adjustment layers are skipped.

use meshling_geom::Size;
use meshling_model::Blend;
use meshling_plan::Pixels;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PsdError {
    #[error("not a Photoshop document")]
    NotPsd,
    #[error("unsupported PSD: {0}")]
    Unsupported(String),
    #[error("truncated or malformed PSD: {0}")]
    Malformed(String),
}

/// One pixel layer, cropped to the bounding box of its ink (alpha > 0).
#[derive(Clone, Debug, PartialEq)]
pub struct PsdLayer {
    pub name: String,
    /// Names of the groups containing the layer, outermost first.
    pub groups: Vec<String>,
    /// Canvas position of `pixels`' top-left corner. May be negative or past the canvas.
    pub left: i32,
    pub top: i32,
    /// Premultiplied RGBA8.
    pub pixels: Pixels,
    pub opacity: u8,
    pub blend: Blend,
    /// The four-character Photoshop blend key, e.g. `norm`, `mul `, `lddg`.
    pub blend_key: String,
    /// True when the blend mode has no equivalent here and was read as Normal.
    pub blend_approximated: bool,
    /// The layer's own visibility flag (a hidden group hides it too).
    pub visible: bool,
}

/// A group (folder) of layers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdGroup {
    /// Names of the groups containing this one, outermost first, then its own name.
    pub path: Vec<String>,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PsdDocument {
    pub size: Size,
    /// Pixel layers bottom to top (back to front). Layers with no ink are left out.
    pub layers: Vec<PsdLayer>,
    /// Groups from the top of the layer panel down; a group comes before the groups inside it.
    pub groups: Vec<PsdGroup>,
    /// Names of pixel layers that had no ink and were skipped.
    pub empty_layers: Vec<String>,
    /// The merged image Photoshop stored (premultiplied), when the file has a readable one.
    pub composite: Option<Pixels>,
}

/// Parse a PSD file.
pub fn read_psd(bytes: &[u8]) -> Result<PsdDocument, PsdError> {
    let mut r = Reader { bytes, pos: 0 };
    if r.take(4).map_err(|_| PsdError::NotPsd)? != b"8BPS" {
        return Err(PsdError::NotPsd);
    }
    match r.u16()? {
        1 => {}
        2 => return Err(PsdError::Unsupported("large document format (PSB)".into())),
        v => return Err(PsdError::Unsupported(format!("version {v}"))),
    }
    r.skip(6)?;
    let channels = r.u16()?;
    let height = r.u32()?;
    let width = r.u32()?;
    let depth = r.u16()?;
    let mode = r.u16()?;
    if depth != 8 {
        return Err(PsdError::Unsupported(format!("{depth} bits per channel (only 8 is supported)")));
    }
    if mode != 3 {
        return Err(PsdError::Unsupported(format!("colour mode {mode} (only RGB is supported)")));
    }
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(PsdError::Malformed(format!("canvas {width}×{height}")));
    }
    let size = Size::new(width, height);

    let colour_data = r.u32()? as usize;
    r.skip(colour_data)?;
    let resources = r.u32()? as usize;
    r.skip(resources)?;

    let section_len = r.u32()? as usize;
    let section_end = r.pos.checked_add(section_len).ok_or_else(|| malformed("layer section length"))?;
    let mut records = Vec::new();
    if section_len >= 4 {
        let info_len = r.u32()? as usize;
        if info_len >= 2 {
            let count = r.i16()?.unsigned_abs() as usize;
            for _ in 0..count {
                records.push(read_record(&mut r)?);
            }
            // Channel image data follows the records, layer by layer, channel by channel.
            for rec in &mut records {
                read_channels(&mut r, rec)?;
            }
        }
    }

    // Records run bottom to top. Walking them top down, a folder record opens a group and a
    // "bounding section divider" closes it.
    let mut doc =
        PsdDocument { size, layers: Vec::new(), groups: Vec::new(), empty_layers: Vec::new(), composite: None };
    let mut path: Vec<String> = Vec::new();
    let mut top_down = Vec::new();
    for rec in records.into_iter().rev() {
        match rec.section {
            Some(1 | 2) => {
                path.push(rec.name.clone());
                doc.groups.push(PsdGroup { path: path.clone(), visible: rec.visible });
            }
            Some(3) => {
                path.pop();
            }
            _ => {
                let name = rec.name.clone();
                match rec.into_layer(path.clone()) {
                    Some(layer) => top_down.push(layer),
                    None => doc.empty_layers.push(name),
                }
            }
        }
    }
    top_down.reverse();
    doc.layers = top_down;
    doc.empty_layers.reverse();

    // The merged image follows the layer and mask section.
    if section_end <= bytes.len() {
        let mut m = Reader { bytes, pos: section_end };
        doc.composite = read_composite(&mut m, size, channels).ok();
    }
    Ok(doc)
}

/// Largest canvas or layer side accepted (Photoshop's own PSD limit).
const MAX_SIDE: u32 = 30_000;

fn malformed(what: &str) -> PsdError {
    PsdError::Malformed(what.to_string())
}

struct Channel {
    id: i16,
    len: usize,
}

struct Record {
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    channels: Vec<Channel>,
    blend_key: [u8; 4],
    opacity: u8,
    visible: bool,
    name: String,
    /// Section divider type from `lsct`: 1/2 open/closed folder, 3 end of folder.
    section: Option<u32>,
    /// Decoded R, G, B, A planes (`None` when the channel is absent).
    planes: [Option<Vec<u8>>; 4],
}

impl Record {
    fn size(&self) -> (usize, usize) {
        let w = (i64::from(self.right) - i64::from(self.left)).max(0) as usize;
        let h = (i64::from(self.bottom) - i64::from(self.top)).max(0) as usize;
        (w, h)
    }

    /// Premultiply and crop to the ink. `None` for a layer with no ink.
    fn into_layer(self, groups: Vec<String>) -> Option<PsdLayer> {
        let (w, h) = self.size();
        if w == 0 || h == 0 {
            return None;
        }
        let [r, g, b, a] = &self.planes;
        // Every colour plane must have been decoded (its size then matches the bounds, backed by
        // real data); this also keeps a corrupt record from allocating a canvas of nothing.
        if r.is_none() || g.is_none() || b.is_none() {
            return None;
        }
        let plane =
            |p: &Option<Vec<u8>>, i: usize, default: u8| p.as_ref().and_then(|v| v.get(i).copied()).unwrap_or(default);
        let alpha = |i: usize| plane(a, i, 255);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if alpha(y * w + x) > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (cw, ch) = (x1 - x0, y1 - y0);
        let mut rgba = Vec::with_capacity(cw * ch * 4);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * w + x;
                let al = alpha(i);
                for c in [plane(r, i, 0), plane(g, i, 0), plane(b, i, 0)] {
                    rgba.push(premultiply(c, al));
                }
                rgba.push(al);
            }
        }
        let pixels = Pixels::new(Size::new(cw as u32, ch as u32), rgba)?;
        let (blend, blend_approximated) = match &self.blend_key {
            b"norm" | b"pass" => (Blend::Normal, false),
            b"mul " => (Blend::Multiply, false),
            b"lddg" => (Blend::Add, false),
            _ => (Blend::Normal, true),
        };
        Some(PsdLayer {
            name: self.name,
            groups,
            left: self.left + x0 as i32,
            top: self.top + y0 as i32,
            pixels,
            opacity: self.opacity,
            blend,
            blend_key: String::from_utf8_lossy(&self.blend_key).into_owned(),
            blend_approximated,
            visible: self.visible,
        })
    }
}

/// Straight 8-bit channel value to premultiplied, rounded to nearest.
pub fn premultiply(c: u8, a: u8) -> u8 {
    ((u32::from(c) * u32::from(a) + 127) / 255) as u8
}

fn read_record(r: &mut Reader) -> Result<Record, PsdError> {
    let top = r.i32()?;
    let left = r.i32()?;
    let bottom = r.i32()?;
    let right = r.i32()?;
    let (h, w) = (i64::from(bottom) - i64::from(top), i64::from(right) - i64::from(left));
    if h < 0 || w < 0 || h > i64::from(MAX_SIDE) || w > i64::from(MAX_SIDE) {
        return Err(malformed("layer bounds"));
    }
    let n = r.u16()? as usize;
    if n > 56 {
        return Err(malformed("channel count"));
    }
    let mut channels = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.i16()?;
        let len = r.u32()? as usize;
        channels.push(Channel { id, len });
    }
    if r.take(4)? != b"8BIM" {
        return Err(malformed("blend mode signature"));
    }
    let mut blend_key = [0u8; 4];
    blend_key.copy_from_slice(r.take(4)?);
    let opacity = r.u8()?;
    let _clipping = r.u8()?;
    let flags = r.u8()?;
    let _filler = r.u8()?;
    let extra = r.u32()? as usize;
    let extra_end = r.pos.checked_add(extra).ok_or_else(|| malformed("layer extra data"))?;
    let mask = r.u32()? as usize;
    r.skip(mask)?;
    let ranges = r.u32()? as usize;
    r.skip(ranges)?;
    let name_len = r.u8()? as usize;
    let mut name: String = r.take(name_len)?.iter().map(|&b| char::from(b)).collect();
    r.skip((4 - (name_len + 1) % 4) % 4)?;
    let mut section = None;
    while r.pos + 12 <= extra_end {
        let sig = r.take(4)?;
        if sig != b"8BIM" && sig != b"8B64" {
            break;
        }
        let mut key = [0u8; 4];
        key.copy_from_slice(r.take(4)?);
        let len = r.u32()? as usize;
        let body_end = r.pos.checked_add(len).ok_or_else(|| malformed("additional layer info"))?;
        if body_end > extra_end {
            return Err(malformed("additional layer info past its layer"));
        }
        match &key {
            b"luni" if len >= 4 => {
                let count = r.u32()? as usize;
                let units: Vec<u16> = (0..count.min((len - 4) / 2)).map(|_| r.u16()).collect::<Result<_, _>>()?;
                name = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
            }
            b"lsct" | b"lsdk" if len >= 4 => section = Some(r.u32()?),
            _ => {}
        }
        r.pos = body_end;
    }
    if extra_end > r.bytes.len() {
        return Err(malformed("layer record past end of file"));
    }
    r.pos = extra_end;
    Ok(Record {
        top,
        left,
        bottom,
        right,
        channels,
        blend_key,
        opacity,
        visible: flags & 0x02 == 0,
        name,
        section,
        planes: [None, None, None, None],
    })
}

fn read_channels(r: &mut Reader, rec: &mut Record) -> Result<(), PsdError> {
    let (w, h) = rec.size();
    for ch in &rec.channels {
        let end = r.pos.checked_add(ch.len).ok_or_else(|| malformed("channel length"))?;
        if end > r.bytes.len() {
            return Err(malformed("channel data past end of file"));
        }
        let slot = match ch.id {
            0..=2 => Some(ch.id as usize),
            -1 => Some(3),
            _ => None, // user and vector masks: skipped by length
        };
        if let Some(slot) = slot
            && w > 0
            && h > 0
        {
            let mut c = Reader { bytes: &r.bytes[..end], pos: r.pos };
            let compression = c.u16()?;
            rec.planes[slot] = Some(read_plane(&mut c, compression, w, h)?);
        }
        r.pos = end;
    }
    Ok(())
}

/// One `w × h` plane after its compression tag.
fn read_plane(r: &mut Reader, compression: u16, w: usize, h: usize) -> Result<Vec<u8>, PsdError> {
    match compression {
        0 => Ok(r.take(w * h)?.to_vec()),
        1 => {
            let counts: Vec<usize> = (0..h).map(|_| r.u16().map(usize::from)).collect::<Result<_, _>>()?;
            let mut out = Vec::new();
            for n in counts {
                unpack_bits(r.take(n)?, w, &mut out)?;
            }
            Ok(out)
        }
        2 | 3 => Err(PsdError::Unsupported("ZIP-compressed channels".into())),
        c => Err(PsdError::Malformed(format!("compression {c}"))),
    }
}

/// Decode one PackBits row of exactly `w` bytes onto `out`.
fn unpack_bits(src: &[u8], w: usize, out: &mut Vec<u8>) -> Result<(), PsdError> {
    let target = out.len() + w;
    let mut i = 0;
    while out.len() < target {
        let Some(&n) = src.get(i) else { return Err(malformed("short RLE row")) };
        i += 1;
        let n = n as i8;
        if n >= 0 {
            let len = n as usize + 1;
            let run = src.get(i..i + len).ok_or_else(|| malformed("RLE literal past row"))?;
            out.extend_from_slice(run);
            i += len;
        } else if n != -128 {
            let &v = src.get(i).ok_or_else(|| malformed("RLE run past row"))?;
            i += 1;
            out.extend(std::iter::repeat_n(v, (1 - i16::from(n)) as usize));
        }
    }
    if out.len() != target {
        return Err(malformed("RLE row longer than the layer"));
    }
    Ok(())
}

fn read_composite(r: &mut Reader, size: Size, channels: u16) -> Result<Pixels, PsdError> {
    let (w, h) = (size.width as usize, size.height as usize);
    let compression = r.u16()?;
    let n = usize::from(channels).min(4);
    let mut planes = Vec::with_capacity(n);
    match compression {
        0 => {
            for _ in 0..n {
                planes.push(r.take(w * h)?.to_vec());
            }
        }
        1 => {
            // All rows' byte counts for every channel come first.
            let counts: Vec<usize> =
                (0..h * usize::from(channels)).map(|_| r.u16().map(usize::from)).collect::<Result<_, _>>()?;
            for c in 0..n {
                let mut plane = Vec::new();
                for &len in &counts[c * h..(c + 1) * h] {
                    unpack_bits(r.take(len)?, w, &mut plane)?;
                }
                planes.push(plane);
            }
        }
        _ => return Err(PsdError::Unsupported("composite compression".into())),
    }
    if planes.len() < 3 {
        return Err(malformed("composite has fewer than 3 channels"));
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        let a = planes.get(3).map_or(255, |p| p[i]);
        for p in &planes[..3] {
            rgba.push(premultiply(p[i], a));
        }
        rgba.push(a);
    }
    Pixels::new(size, rgba).ok_or_else(|| malformed("composite size"))
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], PsdError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| malformed("unexpected end of data"))?;
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<(), PsdError> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Result<u8, PsdError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, PsdError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Result<i16, PsdError> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32, PsdError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&mut self) -> Result<i32, PsdError> {
        Ok(self.u32()? as i32)
    }
}
