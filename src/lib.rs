//! Rust QR code **image** generator matching the exact styling of
//! [nimiq/qr-creator](https://nimiq.github.io/qr-creator/demo/).
//!
//! Unlike crates that only return the QR matrix, this crate rasterizes the
//! rounded-module artwork (connected blobs + gap fillers) to an image.
//!
//! ```json
//! {
//!   "text": "Hello world of QR codes!",
//!   "radius": "0.5",
//!   "ecLevel": "H",
//!   "fill": "#0064a8",
//!   "background": null,
//!   "size": "250"
//! }
//! ```
//!
//! ```rust
//! use qr_creator::Options;
//! let opts: Options = serde_json::from_value(serde_json::json!({
//!     "text": "Hello world of QR codes!",
//!     "radius": 0.5,
//!     "ecLevel": "H",
//!     "fill": "#0064a8",
//!     "background": null,
//!     "size": 250
//! })).unwrap();
//! let img = opts.render_image().unwrap();
//! img.save(std::env::temp_dir().join("qr.png")).unwrap();
//! ```

use qrcode::{EcLevel, QrCode};
use serde::Deserialize;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

/// Cubic approximation constant for a quarter circle.
const KAPPA: f32 = 0.5522847498307936;

/// QR error-correction level, same labels as the JS demo (`L/M/Q/H`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    L,
    M,
    Q,
    H,
}

impl Level {
    fn to_qrcode(self) -> EcLevel {
        match self {
            Level::L => EcLevel::L,
            Level::M => EcLevel::M,
            Level::Q => EcLevel::Q,
            Level::H => EcLevel::H,
        }
    }
}

fn deserialize_level<'de, D>(d: D) -> Result<Level, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    match s.to_ascii_uppercase().as_str() {
        "L" => Ok(Level::L),
        "M" => Ok(Level::M),
        "Q" => Ok(Level::Q),
        "H" => Ok(Level::H),
        other => Err(serde::de::Error::unknown_variant(
            other,
            &["L", "M", "Q", "H"],
        )),
    }
}

fn deserialize_float<'de, D>(d: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    struct V;
    impl Visitor<'_> for V {
        type Value = f32;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a number or numeric string 0..0.5")
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<f32, E> {
            Ok(v as f32)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<f32, E> {
            Ok(v as f32)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<f32, E> {
            Ok(v as f32)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<f32, E> {
            v.parse::<f32>().map_err(E::custom)
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<f32, E> {
            self.visit_str(&v)
        }
    }
    d.deserialize_any(V)
}

fn deserialize_size<'de, D>(d: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    struct V;
    impl Visitor<'_> for V {
        type Value = u32;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a positive integer or numeric string")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
            u32::try_from(v).map_err(|_| E::custom("size out of range"))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
            u32::try_from(v).map_err(|_| E::custom("size must be positive"))
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<u32, E> {
            if v < 1.0 {
                return Err(E::custom("size must be positive"));
            }
            Ok(v.round() as u32)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
            v.parse::<f32>()
                .map_err(E::custom)
                .map(|f| f.round() as u32)
                .and_then(|n| {
                    if n == 0 {
                        Err(E::custom("size must be positive"))
                    } else {
                        Ok(n)
                    }
                })
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<u32, E> {
            self.visit_str(&v)
        }
    }
    d.deserialize_any(V)
}

fn deserialize_opt_color<'de, D>(d: D) -> Result<Option<image::Rgba<u8>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            if s.trim().is_empty() || s.eq_ignore_ascii_case("null") {
                return Ok(None);
            }
            parse_color(&s).map(Some).map_err(serde::de::Error::custom)
        }
    }
}

fn deserialize_color<'de, D>(d: D) -> Result<image::Rgba<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    parse_color(&s).map_err(serde::de::Error::custom)
}

/// Parse `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` (with or without `#`).
pub fn parse_color(s: &str) -> Result<image::Rgba<u8>, String> {
    let h = s.trim().strip_prefix('#').unwrap_or(s.trim());
    let bytes = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|e| e.to_string());
    let nib = |i: usize| {
        u8::from_str_radix(&h[i..i + 1], 16)
            .map(|v| v * 17)
            .map_err(|e| e.to_string())
    };
    match h.len() {
        3 => Ok(image::Rgba([nib(0)?, nib(1)?, nib(2)?, 255])),
        4 => Ok(image::Rgba([nib(0)?, nib(1)?, nib(2)?, nib(3)?])),
        6 => Ok(image::Rgba([bytes(0)?, bytes(2)?, bytes(4)?, 255])),
        8 => Ok(image::Rgba([bytes(0)?, bytes(2)?, bytes(4)?, bytes(6)?])),
        _ => Err(format!(
            "invalid color '{s}': expected #rgb, #rgba, #rrggbb or #rrggbbaa"
        )),
    }
}

/// Generator options. Field names mirror the JS demo (`ecLevel`, ...).
#[derive(Debug, Clone, Deserialize)]
pub struct Options {
    /// Text / payload to encode.
    #[serde(default = "default_text")]
    pub text: String,
    /// Module corner radius as fraction of module size, clamped to `0..=0.5`.
    /// Accepts number or numeric string (demo sends `"0.5"`).
    #[serde(default = "default_radius", deserialize_with = "deserialize_float")]
    pub radius: f32,
    /// Error correction level.
    #[serde(
        rename = "ecLevel",
        default = "default_level",
        deserialize_with = "deserialize_level"
    )]
    pub ec_level: Level,
    /// Foreground color.
    #[serde(default = "default_fill", deserialize_with = "deserialize_color")]
    pub fill: image::Rgba<u8>,
    /// Background color, `null` = transparent.
    #[serde(default, deserialize_with = "deserialize_opt_color")]
    pub background: Option<image::Rgba<u8>>,
    /// Output image width/height in px. Accepts number or numeric string.
    #[serde(default = "default_size", deserialize_with = "deserialize_size")]
    pub size: u32,
    /// Quiet-zone width in modules. Demo uses `0`.
    #[serde(default)]
    pub quiet: u32,
}

fn default_text() -> String {
    "no text".to_string()
}
fn default_radius() -> f32 {
    0.5
}
fn default_level() -> Level {
    Level::L
}
fn default_fill() -> image::Rgba<u8> {
    image::Rgba([0, 0, 0, 255])
}
fn default_size() -> u32 {
    200
}

impl Default for Options {
    fn default() -> Self {
        Self {
            text: default_text(),
            radius: 0.5,
            ec_level: Level::L,
            fill: default_fill(),
            background: None,
            size: 200,
            quiet: 0,
        }
    }
}

impl Options {
    /// Build from the demo's JSON (`radius`/`size` may be strings, `background` may be null).
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// QR matrix width (without quiet zone).
    pub fn matrix_width(&self) -> Result<usize, String> {
        let code = QrCode::with_error_correction_level(
            self.text.as_bytes(),
            self.ec_level.to_qrcode(),
        )
        .map_err(|e| e.to_string())?;
        Ok(code.width())
    }

    /// Render to an `image::RgbaImage` of `size x size`.
    pub fn render_image(&self) -> Result<image::RgbaImage, String> {
        let pix = self.render_pixmap()?;
        let mut img = image::RgbaImage::new(pix.width(), pix.height());
        for (x, y, px) in img.enumerate_pixels_mut() {
            let p = pix.pixel(x, y).ok_or("pixmap bounds")?;
            *px = image::Rgba([p.red(), p.green(), p.blue(), p.alpha()]);
        }
        Ok(img)
    }

    /// Render to PNG bytes.
    pub fn render_png(&self) -> Result<Vec<u8>, String> {
        let img = self.render_image()?;
        let mut buf = Vec::new();
        let enc = image::codecs::png::PngEncoder::new(&mut buf);
        use image::ImageEncoder;
        enc.write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    fn render_pixmap(&self) -> Result<Pixmap, String> {
        let size = self.size.max(1);
        let code = QrCode::with_error_correction_level(
            self.text.as_bytes(),
            self.ec_level.to_qrcode(),
        )
        .map_err(|e| e.to_string())?;
        let n = code.width() as u32;
        let quiet = self.quiet;
        let total = n + 2 * quiet;
        if total == 0 {
            return Err("empty matrix".into());
        }
        let module = size as f32 / total as f32;
        let radius_px =
            (self.radius.clamp(0.0, 0.5) * module).floor().max(0.0);

        let is_dark = |row: i32, col: i32| -> bool {
            let mr = row - quiet as i32;
            let mc = col - quiet as i32;
            if mr < 0 || mc < 0 || mr >= n as i32 || mc >= n as i32 {
                return false;
            }
            // qrcode crate indexes (x=col, y=row)
            code[(mc as usize, mr as usize)] == qrcode::Color::Dark
        };

        let mut pix = Pixmap::new(size, size).ok_or("invalid size")?;
        if let Some(bg) = self.background {
            pix.fill(tiny_skia::Color::from_rgba8(
                bg[0], bg[1], bg[2], bg[3],
            ));
        }

        let mut pb = PathBuilder::new();
        let total_i = total as i32;
        for row in 0..total_i {
            for col in 0..total_i {
                // JS: x = round(left + col*module), edges rounded to int px.
                let x0 = (col as f32 * module).round();
                let x1 = ((col + 1) as f32 * module).round();
                let y0 = (row as f32 * module).round();
                let y1 = ((row + 1) as f32 * module).round();
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let dark = is_dark(row, col);
                if dark {
                    let n_ = is_dark(row - 1, col);
                    let s = is_dark(row + 1, col);
                    let w = is_dark(row, col - 1);
                    let e = is_dark(row, col + 1);
                    push_dark_cell(
                        &mut pb,
                        x0,
                        y0,
                        x1,
                        y1,
                        radius_px,
                        !n_ && !w,
                        !n_ && !e,
                        !s && !e,
                        !s && !w,
                    );
                } else {
                    let n_ = is_dark(row - 1, col);
                    let s = is_dark(row + 1, col);
                    let w = is_dark(row, col - 1);
                    let e = is_dark(row, col + 1);
                    if n_ && w && is_dark(row - 1, col - 1) {
                        push_gap_tl(&mut pb, x0, y0, radius_px);
                    }
                    if n_ && e && is_dark(row - 1, col + 1) {
                        push_gap_tr(&mut pb, x1, y0, radius_px);
                    }
                    if s && e && is_dark(row + 1, col + 1) {
                        push_gap_br(&mut pb, x1, y1, radius_px);
                    }
                    if s && w && is_dark(row + 1, col - 1) {
                        push_gap_bl(&mut pb, x0, y1, radius_px);
                    }
                }
            }
        }
        let path = pb.finish().ok_or("empty path")?;
        let mut paint = Paint::default();
        paint.set_color_rgba8(self.fill[0], self.fill[1], self.fill[2], self.fill[3]);
        paint.anti_alias = true;
        pix.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        Ok(pix)
    }
}

/// Dark module with selectively rounded outer corners.
/// Flags: tl/tr/br/bl = round this corner.
#[allow(clippy::too_many_arguments)]
fn push_dark_cell(
    pb: &mut PathBuilder,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    r: f32,
    tl: bool,
    tr: bool,
    br: bool,
    bl: bool,
) {
    let w = x1 - x0;
    let h = y1 - y0;
    let r = r.min(w / 2.0).min(h / 2.0);
    if r < 0.25 {
        pb.move_to(x0, y0);
        pb.line_to(x1, y0);
        pb.line_to(x1, y1);
        pb.line_to(x0, y1);
        pb.close();
        return;
    }
    let k = KAPPA * r;
    pb.move_to(x0 + if tl { r } else { 0.0 }, y0);
    // top edge -> TR
    pb.line_to(x1 - if tr { r } else { 0.0 }, y0);
    if tr {
        pb.cubic_to(x1 - r + k, y0, x1, y0 + r - k, x1, y0 + r);
    }
    // right edge -> BR
    pb.line_to(x1, y1 - if br { r } else { 0.0 });
    if br {
        pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    }
    // bottom edge -> BL
    pb.line_to(x0 + if bl { r } else { 0.0 }, y1);
    if bl {
        pb.cubic_to(x0 + r - k, y1, x0, y1 - r + k, x0, y1 - r);
    }
    // left edge -> TL
    pb.line_to(x0, y0 + if tl { r } else { 0.0 });
    if tl {
        pb.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    }
    pb.close();
}

/// Concave inner-corner fillets for light cells.
///
/// Canvas `arcTo(cx, cy, ...)` puts the arc centre *inside* the light cell
/// (e.g. TL: `(x0+r, y0+r)`), so the arc dips *toward* the dark junction
/// corner — a concave fillet joining the two neighbouring dark edges.
/// The cubic controls therefore step back toward the corner along each edge.
fn push_gap_tl(pb: &mut PathBuilder, x0: f32, y0: f32, r: f32) {
    if r < 0.25 {
        return;
    }
    let k = KAPPA * r;
    pb.move_to(x0 + r, y0);
    pb.line_to(x0, y0);
    pb.line_to(x0, y0 + r);
    pb.cubic_to(x0, y0 + r - k, x0 + r - k, y0, x0 + r, y0);
    pb.close();
}

fn push_gap_tr(pb: &mut PathBuilder, x1: f32, y0: f32, r: f32) {
    if r < 0.25 {
        return;
    }
    let k = KAPPA * r;
    pb.move_to(x1 - r, y0);
    pb.line_to(x1, y0);
    pb.line_to(x1, y0 + r);
    pb.cubic_to(x1, y0 + r - k, x1 - r + k, y0, x1 - r, y0);
    pb.close();
}

fn push_gap_br(pb: &mut PathBuilder, x1: f32, y1: f32, r: f32) {
    if r < 0.25 {
        return;
    }
    let k = KAPPA * r;
    pb.move_to(x1 - r, y1);
    pb.line_to(x1, y1);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.close();
}

fn push_gap_bl(pb: &mut PathBuilder, x0: f32, y1: f32, r: f32) {
    if r < 0.25 {
        return;
    }
    let k = KAPPA * r;
    pb.move_to(x0 + r, y1);
    pb.line_to(x0, y1);
    pb.line_to(x0, y1 - r);
    pb.cubic_to(x0, y1 - r + k, x0 + r - k, y1, x0 + r, y1);
    pb.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_demo_json_with_string_numbers_and_null() {
        let o: Options = serde_json::from_value(serde_json::json!({
            "text": "Hello world of QR codes!",
            "radius": "0.5",
            "ecLevel": "H",
            "fill": "#0064a8",
            "background": null,
            "size": "250"
        }))
        .unwrap();
        assert_eq!(o.text, "Hello world of QR codes!");
        assert!((o.radius - 0.5).abs() < 1e-6);
        assert_eq!(o.ec_level, Level::H);
        assert_eq!(o.fill, image::Rgba([0x00, 0x64, 0xa8, 255]));
        assert_eq!(o.background, None);
        assert_eq!(o.size, 250);
    }

    #[test]
    fn renders_demo_image_size_and_colors() {
        let o: Options = serde_json::from_value(serde_json::json!({
            "text": "Hello world of QR codes!",
            "radius": 0.5,
            "ecLevel": "H",
            "fill": "#0064a8",
            "background": null,
            "size": 250
        }))
        .unwrap();
        let img = o.render_image().unwrap();
        assert_eq!(img.dimensions(), (250, 250));
        // Finder pattern outer ring is dark, center of top-left finder is dark.
        // Just assert image is non-empty and has transparent + blue pixels.
        let mut blue = 0u32;
        let mut transparent = 0u32;
        for p in img.pixels() {
            if p[3] == 0 {
                transparent += 1;
            }
            if p[0] == 0x00 && p[1] == 0x64 && p[2] == 0xa8 && p[3] == 255 {
                blue += 1;
            }
        }
        assert!(blue > 5000, "expected blue modules, got {blue}");
        assert!(transparent > 5000, "expected transparent bg, got {transparent}");
        // Finder top-left corner pixel (quiet=0 => module 0,0 is dark finder border)
        assert_eq!(img.get_pixel(2, 2)[3], 255);
    }

    #[test]
    fn radius_zero_gives_sharp_rectangles() {
        let mk = |radius: f32| {
            let o = Options {
                text: "Hello".into(),
                radius,
                ec_level: Level::M,
                fill: image::Rgba([0, 0, 0, 255]),
                background: Some(image::Rgba([255, 255, 255, 255])),
                size: 200,
                quiet: 0,
            };
            o.render_image().unwrap()
        };
        let sharp = mk(0.0);
        let round = mk(0.5);
        assert_ne!(sharp.as_raw(), round.as_raw());
    }

    #[test]
    fn internal_corners_are_concave_fillets() {
        // A convex (backwards) filler would blob dark ink into the light
        // cell: the point at half-radius diagonally from the junction
        // corner would be dark. The correct concave fillet leaves it light.
        // Use a large size so the sample sits well clear of AA edges.
        let o = Options {
            text: "Hello world of QR codes!".into(),
            radius: 0.5,
            ec_level: Level::H,
            fill: image::Rgba([0, 0, 0, 255]),
            background: Some(image::Rgba([255, 255, 255, 255])),
            size: 800,
            quiet: 0,
        };
        let img = o.render_image().unwrap();
        let code = QrCode::with_error_correction_level(
            o.text.as_bytes(),
            o.ec_level.to_qrcode(),
        )
        .unwrap();
        let n = code.width() as i32;
        let is_dark = |row: i32, col: i32| -> bool {
            if row < 0 || col < 0 || row >= n || col >= n {
                return false;
            }
            code[(col as usize, row as usize)] == qrcode::Color::Dark
        };
        let module = o.size as f32 / n as f32;
        let r = (0.5 * module).floor();
        assert!(r >= 8.0, "need a large radius for a stable sample");
        let half = (r / 2.0).floor() as u32;
        let mut counts = [0u32; 4];
        for row in 0..n {
            for col in 0..n {
                if is_dark(row, col) {
                    continue;
                }
                let x0 = (col as f32 * module).round() as u32;
                let y0 = (row as f32 * module).round() as u32;
                let x1 = ((col + 1) as f32 * module).round() as u32;
                let y1 = ((row + 1) as f32 * module).round() as u32;
                let probe =
                    |px: u32, py: u32| -> image::Rgba<u8> { *img.get_pixel(px, py) };
                let is_light = |p: image::Rgba<u8>| {
                    p[0] > 200 && p[1] > 200 && p[2] > 200
                };
                if is_dark(row - 1, col)
                    && is_dark(row, col - 1)
                    && is_dark(row - 1, col - 1)
                {
                    counts[0] += 1;
                    assert!(
                        is_light(probe(x0 + half, y0 + half)),
                        "TL inner corner blobs into light cell at ({row},{col})"
                    );
                }
                if is_dark(row - 1, col)
                    && is_dark(row, col + 1)
                    && is_dark(row - 1, col + 1)
                {
                    counts[1] += 1;
                    assert!(
                        is_light(probe(x1 - half - 1, y0 + half)),
                        "TR inner corner blobs into light cell at ({row},{col})"
                    );
                }
                if is_dark(row + 1, col)
                    && is_dark(row, col + 1)
                    && is_dark(row + 1, col + 1)
                {
                    counts[2] += 1;
                    assert!(
                        is_light(probe(x1 - half - 1, y1 - half - 1)),
                        "BR inner corner blobs into light cell at ({row},{col})"
                    );
                }
                if is_dark(row + 1, col)
                    && is_dark(row, col - 1)
                    && is_dark(row + 1, col - 1)
                {
                    counts[3] += 1;
                    assert!(
                        is_light(probe(x0 + half, y1 - half - 1)),
                        "BL inner corner blobs into light cell at ({row},{col})"
                    );
                }
            }
        }
        assert!(
            counts.iter().all(|&c| c > 0),
            "expected all four inner-corner orientations, got {counts:?}"
        );
    }

    #[test]
    fn decodes_with_standard_decoder() {
        // Round-trip through a real decoder to prove validity.
        let o: Options = serde_json::from_value(serde_json::json!({
            "text": "Hello world of QR codes!",
            "radius": 0.5,
            "ecLevel": "H",
            "fill": "#000000",
            "background": "#ffffff",
            "size": 400
        }))
        .unwrap();
        let img = o.render_image().unwrap();
        let gray = image::DynamicImage::ImageRgba8(img).to_luma8();
        let mut prepared =
            rqrr::PreparedImage::prepare(gray);
        let grids = prepared.detect_grids();
        assert_eq!(grids.len(), 1);
        let (_, content) = grids[0].decode().unwrap();
        assert_eq!(content, "Hello world of QR codes!");
    }
}
