//! The `ltshot` protocol: the frozen frame as an image the screen page can show.
use super::capture::Frame;
use tauri::http::{header, Response, StatusCode};

const FILE_HEADER: usize = 14;
const V4_HEADER: usize = 108;
const LCS_SRGB: u32 = 0x7352_4742;
const BI_BITFIELDS: u32 = 3;

/// A 32-bit top-down BMP. The alpha mask is 0, so the decoder treats every pixel as opaque.
pub fn encode(frame: &Frame) -> Vec<u8> {
    let pixels = frame.bgra.len();
    let offset = FILE_HEADER + V4_HEADER;
    let mut out = Vec::with_capacity(offset + pixels);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((offset + pixels) as u32).to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&(offset as u32).to_le_bytes());

    out.extend_from_slice(&(V4_HEADER as u32).to_le_bytes());
    out.extend_from_slice(&(frame.width as i32).to_le_bytes());
    out.extend_from_slice(&(-(frame.height as i32)).to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&32_u16.to_le_bytes());
    out.extend_from_slice(&BI_BITFIELDS.to_le_bytes());
    out.extend_from_slice(&(pixels as u32).to_le_bytes());
    // 72 DPI in pixels per meter, then no palette.
    for value in [2835_u32, 2835, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    // Red, green, blue and alpha masks of a BGRA pixel read as a little-endian u32.
    for mask in [0x00FF_0000_u32, 0x0000_FF00, 0x0000_00FF, 0] {
        out.extend_from_slice(&mask.to_le_bytes());
    }
    out.extend_from_slice(&LCS_SRGB.to_le_bytes());
    // Endpoints and gamma, unused with sRGB.
    out.resize(offset, 0);
    out.extend_from_slice(&frame.bgra);
    out
}

/// `/<id>` of the current frame, else 404.
pub fn response(frame: Option<&Frame>, path: &str) -> Response<Vec<u8>> {
    let id = path.trim_start_matches('/').parse::<u64>().ok();
    let builder = Response::builder().header(header::CACHE_CONTROL, "no-store");
    match frame.filter(|frame| Some(frame.id) == id) {
        Some(frame) => builder
            .header(header::CONTENT_TYPE, "image/bmp")
            .body(encode(frame)),
        None => builder.status(StatusCode::NOT_FOUND).body(Vec::new()),
    }
    .expect("static headers are valid")
}
