//! Clipboard history: text, links, images and files (memory only, nothing is saved to disk).
use std::borrow::Cow;
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use arboard::{Clipboard, ImageData};
use image::{DynamicImage, ImageFormat, RgbaImage};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel, Weak};
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;

use crate::{ClipItem, MainWindow};

const MAX_IMAGES: usize = 8;
const MAX_PIXELS: usize = 64 * 1024 * 1024;

enum Payload {
    Text(String),
    Image(Vec<u8>),
    Files(Vec<String>),
}

struct Clip {
    payload: Payload,
    /// 0 text, 1 link, 2 image, 3 files
    kind: i32,
    title: String,
    sub: String,
    meta: String,
    pinned: bool,
    born: Instant,
    /// small preview (width, height, RGBA bytes)
    thumb: Option<(u32, u32, Vec<u8>)>,
    /// quick fingerprint of an image, so the same image is not stored twice
    fp: u64,
}

struct Row {
    id: i32,
    kind: i32,
    title: String,
    sub: String,
    meta: String,
    age: String,
    pinned: bool,
    thumb: Option<(u32, u32, Vec<u8>)>,
}

static HISTORY: OnceLock<Mutex<Vec<Clip>>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(true);
static MAX_ITEMS: AtomicUsize = AtomicUsize::new(20);
/// 0 all, 1 text, 2 links, 3 images, 4 files, 5 snippets
static FILTER: AtomicI32 = AtomicI32::new(0);
/// Clipboard number of a change we made ourselves (copying a clip back).
static SKIP_SEQ: AtomicU32 = AtomicU32::new(0);

fn history() -> &'static Mutex<Vec<Clip>> {
    HISTORY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Settings: keep history on/off and how many items.
pub fn configure(enabled: bool, max: usize) {
    ENABLED.store(enabled, Ordering::Relaxed);
    MAX_ITEMS.store(max.max(1), Ordering::Relaxed);
}

fn age_text(born: Instant) -> String {
    let s = born.elapsed().as_secs();
    if s < 60 {
        "now".to_string()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(max).collect()
}

fn is_link(t: &str) -> bool {
    if t.chars().any(char::is_whitespace) || t.len() > 400 {
        return false;
    }
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.")
}

fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or("").trim_start_matches("www.").to_string()
}

fn matches(clip: &Clip, filter: i32) -> bool {
    match filter {
        1 => clip.kind == 0,
        2 => clip.kind == 1,
        3 => clip.kind == 2,
        4 => clip.kind == 3,
        5 => clip.pinned,
        _ => true,
    }
}

fn same(a: &Clip, b: &Clip) -> bool {
    match (&a.payload, &b.payload) {
        (Payload::Text(x), Payload::Text(y)) => x == y,
        (Payload::Files(x), Payload::Files(y)) => x == y,
        (Payload::Image(_), Payload::Image(_)) => a.fp == b.fp && a.fp != 0,
        _ => false,
    }
}

/// Sends the (filtered) history to the UI. Returns false when the UI is gone.
fn publish(ui: &Weak<MainWindow>) -> bool {
    let filter = FILTER.load(Ordering::Relaxed);
    let rows: Vec<Row> = match history().lock() {
        Ok(h) => h
            .iter()
            .enumerate()
            .filter(|(_, c)| matches(c, filter))
            .map(|(i, c)| Row {
                id: i as i32,
                kind: c.kind,
                title: c.title.clone(),
                sub: c.sub.clone(),
                meta: c.meta.clone(),
                age: age_text(c.born),
                pinned: c.pinned,
                thumb: c.thumb.clone(),
            })
            .collect(),
        Err(_) => return true,
    };
    ui.upgrade_in_event_loop(move |ui| {
        let items: Vec<ClipItem> = rows
            .into_iter()
            .map(|r| {
                let (thumb, has_thumb) = match r.thumb {
                    Some((w, h, bytes)) if bytes.len() == (w * h * 4) as usize => {
                        let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
                        buffer.make_mut_bytes().copy_from_slice(&bytes);
                        (Image::from_rgba8(buffer), true)
                    }
                    _ => (Image::default(), false),
                };
                ClipItem {
                    id: r.id,
                    kind: r.kind,
                    title: r.title.into(),
                    sub: r.sub.into(),
                    meta: r.meta.into(),
                    age: r.age.into(),
                    pinned: r.pinned,
                    thumb,
                    has_thumb,
                }
            })
            .collect();
        ui.set_clips(ModelRc::new(VecModel::from(items)));
    })
    .is_ok()
}

/// A different group (All, Text, Links, ...) was chosen.
pub fn set_filter(ui: &Weak<MainWindow>, filter: i32) {
    FILTER.store(filter.clamp(0, 5), Ordering::Relaxed);
    publish(ui);
}

/// Put the clip with this number back on the clipboard.
pub fn copy(id: usize) -> bool {
    enum Out {
        Text(String),
        Image(Vec<u8>),
    }
    let out = match history().lock() {
        Ok(h) => h.get(id).map(|c| match &c.payload {
            Payload::Text(t) => Out::Text(t.clone()),
            Payload::Files(f) => Out::Text(f.join("\n")),
            Payload::Image(png) => Out::Image(png.clone()),
        }),
        Err(_) => None,
    };
    let Some(out) = out else { return false };
    let Ok(mut cb) = Clipboard::new() else { return false };
    let ok = match out {
        Out::Text(t) => cb.set_text(t).is_ok(),
        Out::Image(png) => match image::load_from_memory(&png) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                cb.set_image(ImageData { width: w as usize, height: h as usize, bytes: Cow::Owned(rgba.into_raw()) })
                    .is_ok()
            }
            Err(_) => false,
        },
    };
    SKIP_SEQ.store(unsafe { GetClipboardSequenceNumber() }, Ordering::Relaxed);
    ok
}

/// Removes one clip.
pub fn remove(ui: &Weak<MainWindow>, id: usize) {
    if let Ok(mut h) = history().lock() {
        if id < h.len() {
            h.remove(id);
        }
    }
    publish(ui);
}

/// Pins a clip as a snippet (or unpins it). Snippets stay when old clips are dropped.
pub fn toggle_pin(ui: &Weak<MainWindow>, id: usize) {
    if let Ok(mut h) = history().lock() {
        if let Some(c) = h.get_mut(id) {
            c.pinned = !c.pinned;
        }
    }
    publish(ui);
}

/// Removes all clips except snippets (in the Snippets group it removes those too).
pub fn clear(ui: &Weak<MainWindow>) {
    let all = FILTER.load(Ordering::Relaxed) == 5;
    if let Ok(mut h) = history().lock() {
        if all {
            h.clear();
        } else {
            h.retain(|c| c.pinned);
        }
    }
    publish(ui);
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string())
}

fn folder_name(p: &str) -> String {
    Path::new(p)
        .parent()
        .and_then(|d| d.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn clip_from_files(files: Vec<String>) -> Clip {
    let title = if files.len() == 1 { file_name(&files[0]) } else { format!("{} files", files.len()) };
    let sub = files.iter().take(3).map(|f| file_name(f)).collect::<Vec<_>>().join(", ");
    let meta = folder_name(&files[0]);
    Clip {
        payload: Payload::Files(files),
        kind: 3,
        title,
        sub,
        meta,
        pinned: false,
        born: Instant::now(),
        thumb: None,
        fp: 0,
    }
}

fn clip_from_text(text: String) -> Clip {
    if is_link(&text) {
        Clip {
            kind: 1,
            title: host_of(&text),
            sub: one_line(&text, 160),
            meta: "Link".to_string(),
            payload: Payload::Text(text),
            pinned: false,
            born: Instant::now(),
            thumb: None,
            fp: 0,
        }
    } else {
        let words = text.split_whitespace().count();
        Clip {
            kind: 0,
            title: one_line(&text, 180),
            sub: String::new(),
            meta: format!("{} words", words),
            payload: Payload::Text(text),
            pinned: false,
            born: Instant::now(),
            thumb: None,
            fp: 0,
        }
    }
}

fn clip_from_image(img: ImageData<'static>) -> Option<Clip> {
    let (w, h) = (img.width as u32, img.height as u32);
    if w == 0 || h == 0 || (w as usize) * (h as usize) > MAX_PIXELS {
        return None;
    }
    let bytes = img.bytes.into_owned();

    // quick fingerprint from a sample of the pixels
    let mut fp: u64 = 0xcbf29ce484222325 ^ bytes.len() as u64;
    let step = (bytes.len() / 4096).max(1);
    let mut i = 0;
    while i < bytes.len() {
        fp ^= bytes[i] as u64;
        fp = fp.wrapping_mul(0x100000001b3);
        i += step;
    }

    let rgba = RgbaImage::from_raw(w, h, bytes)?;
    let dynimg = DynamicImage::ImageRgba8(rgba);
    let small = dynimg.thumbnail(260, 170).to_rgba8();
    let (tw, th) = small.dimensions();
    let mut png: Vec<u8> = Vec::new();
    dynimg.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).ok()?;

    Some(Clip {
        payload: Payload::Image(png),
        kind: 2,
        title: "Image".to_string(),
        sub: String::new(),
        meta: format!("{} × {}", w, h),
        pinned: false,
        born: Instant::now(),
        thumb: Some((tw, th, small.into_raw())),
        fp: fp.max(1),
    })
}

/// Reads whatever is on the clipboard right now.
fn capture(cb: &mut Clipboard) -> Option<Clip> {
    if let Ok(files) = cb.get().file_list() {
        let list: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
        if !list.is_empty() {
            return Some(clip_from_files(list));
        }
    }
    if let Ok(img) = cb.get_image() {
        if let Some(c) = clip_from_image(img) {
            return Some(c);
        }
    }
    if let Ok(text) = cb.get_text() {
        let t = text.trim().to_string();
        if !t.is_empty() {
            return Some(clip_from_text(t));
        }
    }
    None
}

/// Short text for the pop-up.
fn preview(c: &Clip) -> String {
    match c.kind {
        2 => format!("Image {}", c.meta),
        3 => c.title.clone(),
        1 => c.sub.chars().take(60).collect(),
        _ => c.title.chars().take(60).collect(),
    }
}

/// Adds a clip on top, removes duplicates and old items.
fn add(clip: Clip) {
    let max = MAX_ITEMS.load(Ordering::Relaxed);
    if let Ok(mut h) = history().lock() {
        let mut clip = clip;
        if let Some(pos) = h.iter().position(|c| same(c, &clip)) {
            let old = h.remove(pos);
            clip.pinned = old.pinned;
        }
        h.insert(0, clip);

        // at most MAX_IMAGES pictures
        let mut seen = 0;
        let mut i = 0;
        while i < h.len() {
            if h[i].kind == 2 {
                seen += 1;
                if seen > MAX_IMAGES && !h[i].pinned {
                    h.remove(i);
                    continue;
                }
            }
            i += 1;
        }
        // drop the oldest clips that are not snippets
        while h.len() > max {
            match h.iter().rposition(|c| !c.pinned) {
                Some(p) => {
                    h.remove(p);
                }
                None => break,
            }
        }
    }
}

/// Watches the clipboard. It is only opened when Windows says it changed, so other apps are never blocked.
pub fn spawn_watcher(ui: Weak<MainWindow>) {
    thread::spawn(move || {
        let mut last_seq: u32 = 0;
        let mut first = true;
        let mut last_publish = Instant::now();
        loop {
            if ENABLED.load(Ordering::Relaxed) {
                let seq = unsafe { GetClipboardSequenceNumber() };
                if seq != last_seq {
                    if let Ok(mut cb) = Clipboard::new() {
                        last_seq = seq;
                        if seq != SKIP_SEQ.load(Ordering::Relaxed) {
                            if let Some(clip) = capture(&mut cb) {
                                let text = preview(&clip);
                                add(clip);
                                if !publish(&ui) {
                                    return;
                                }
                                last_publish = Instant::now();
                                // pop-up for every new copy (but not for what was already there at start-up)
                                if !first {
                                    let sent = ui.upgrade_in_event_loop(move |ui| {
                                        crate::ticker::toast(&ui, "Clipboard", "Copied", &text);
                                    });
                                    if sent.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        first = false;
                    }
                } else if last_publish.elapsed() >= Duration::from_secs(30) {
                    // keep the "now / 5m / 1h" labels fresh
                    if !publish(&ui) {
                        return;
                    }
                    last_publish = Instant::now();
                }
            }
            thread::sleep(Duration::from_millis(700));
        }
    });
}
