//! Pictures, GIFs and videos for the media widget.
use std::time::{Duration, Instant};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

pub enum Player {
    Still,
    Gif(Gif),
    Video(crate::video::Video, Key),
}

/// Which plain background colour to make see-through (0 none, 1 white, 2 black, 3 green).
#[derive(Clone, Copy)]
pub struct Key {
    pub mode: u8,
    pub tol: i32,
}

impl Key {
    pub fn new(name: &str, tol: i32) -> Key {
        let mode = match name {
            "White" => 1,
            "Black" => 2,
            "Green" => 3,
            _ => 0,
        };
        Key { mode, tol: tol.clamp(0, 100) }
    }
}

/// Makes the chosen colour (and colours close to it) see-through. `px` holds R,G,B,A bytes.
fn remove_colour(px: &mut [u8], key: Key) {
    if key.mode == 0 {
        return;
    }
    let thr = key.tol * 2;
    let feather = 24;
    for p in px.chunks_exact_mut(4) {
        let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
        let dist = match key.mode {
            1 => (255 - r).max(255 - g).max(255 - b),
            2 => r.max(g).max(b),
            _ => 255 - (g - r.max(b)).clamp(0, 255),
        };
        if dist <= thr {
            p[3] = 0;
        } else if dist < thr + feather {
            let a = (dist - thr) * 255 / feather;
            p[3] = (p[3] as i32 * a / 255) as u8;
        }
    }
}

/// Some videos come with a thin bright-green strip on the right and bottom (unused picture memory).
/// Cuts it off.
fn trim_green(data: Vec<u8>, w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let pad = |d: &[u8], x: usize, y: usize| {
        let i = (y * w + x) * 4;
        d[i] <= 24 && d[i + 2] <= 24 && d[i + 1] >= 90 && d[i + 1] <= 180
    };
    let mut cw = w;
    while w - cw < 16 && cw > w / 2 && (0..h).all(|y| pad(&data, cw - 1, y)) {
        cw -= 1;
    }
    let mut ch = h;
    while h - ch < 16 && ch > h / 2 && (0..cw).all(|x| pad(&data, x, ch - 1)) {
        ch -= 1;
    }
    if cw == w && ch == h {
        return (data, w, h);
    }
    let mut out = Vec::with_capacity(cw * ch * 4);
    for y in 0..ch {
        out.extend_from_slice(&data[y * w * 4..(y * w + cw) * 4]);
    }
    (out, cw, ch)
}

pub struct Gif {
    frames: Vec<(SharedPixelBuffer<Rgba8Pixel>, u64)>,
    idx: usize,
    next: Instant,
}

fn to_image(img: &image::RgbaImage) -> Image {
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height());
    Image::from_rgba8(buf)
}

fn ext_of(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn is_video(path: &str) -> bool {
    matches!(ext_of(path).as_str(), "mp4" | "m4v" | "mov" | "wmv" | "avi" | "mkv" | "webm")
}

/// Opens a file. Gives back the player and the first picture to show.
pub fn load(path: &str, flip: bool, key: Key) -> Result<(Player, Option<Image>), String> {
    if path.trim().is_empty() {
        return Err("Choose a picture, GIF or MP4 in settings".to_string());
    }
    if !std::path::Path::new(path).exists() {
        return Err("File not found. Check the path in settings".to_string());
    }
    if is_video(path) {
        let v = crate::video::Video::open(path, flip)?;
        return Ok((Player::Video(v, key), None));
    }
    if ext_of(path) == "gif" {
        if let Ok(g) = load_gif(path, key) {
            let first = Image::from_rgba8(g.frames[0].0.clone());
            return Ok((Player::Gif(g), Some(first)));
        }
    }
    let img = image::open(path).map_err(|e| format!("Could not open this file: {e}"))?;
    let img = if img.width() > 1920 || img.height() > 1920 { img.resize(1920, 1920, image::imageops::FilterType::Triangle) } else { img };
    let mut rgba = img.to_rgba8();
    remove_colour(&mut rgba, key);
    Ok((Player::Still, Some(to_image(&rgba))))
}

fn load_gif(path: &str, key: Key) -> Result<Gif, String> {
    use image::AnimationDecoder;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let mut frames = Vec::new();
    for frame in dec.into_frames() {
        let frame = frame.map_err(|e| e.to_string())?;
        let (n, d) = frame.delay().numer_denom_ms();
        let ms = if d == 0 { 100 } else { (n / d) as u64 };
        let ms = if ms < 20 { 100 } else { ms };
        let mut buf = frame.into_buffer();
        let (w, h) = buf.dimensions();
        if w.max(h) > 720 {
            let k = 720.0 / w.max(h) as f32;
            let nw = ((w as f32 * k) as u32).max(1);
            let nh = ((h as f32 * k) as u32).max(1);
            buf = image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Triangle);
        }
        remove_colour(&mut buf, key);
        frames.push((
            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(buf.as_raw(), buf.width(), buf.height()),
            ms,
        ));
        if frames.len() >= 400 {
            break;
        }
    }
    if frames.is_empty() {
        return Err("empty gif".to_string());
    }
    Ok(Gif { frames, idx: 0, next: Instant::now() })
}

impl Player {
    /// Called often. Returns a new picture when the frame changed.
    pub fn tick(&mut self, paused: bool) -> Option<Image> {
        match self {
            Player::Still => None,
            Player::Gif(g) => {
                let now = Instant::now();
                if paused {
                    g.next = now + Duration::from_millis(g.frames[g.idx].1);
                    return None;
                }
                if now >= g.next {
                    g.idx = (g.idx + 1) % g.frames.len();
                    g.next = now + Duration::from_millis(g.frames[g.idx].1);
                    return Some(Image::from_rgba8(g.frames[g.idx].0.clone()));
                }
                None
            }
            Player::Video(v, key) => {
                v.set_paused(paused);
                let key = *key;
                v.take_frame().map(|f| {
                    let (mut data, w, h) = trim_green(f.rgba, f.w as usize, f.h as usize);
                    remove_colour(&mut data, key);
                    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&data, w as u32, h as u32);
                    Image::from_rgba8(buf)
                })
            }
        }
    }

    pub fn error(&self) -> Option<String> {
        match self {
            Player::Video(v, _) => v.error(),
            _ => None,
        }
    }
}