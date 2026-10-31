use super::Font;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
#[derive(Default)]
struct Store {
    path: Option<String>,
    fonts: HashMap<String, Option<Arc<Font>>>,
    warnings: HashSet<String>,
    bytes: usize,
}
fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}
pub fn font_path() -> String {
    store().lock().unwrap_or_else(PoisonError::into_inner).path.clone().unwrap_or_else(|| std::env::var("CADCRAFT_FONTPATH").unwrap_or_default())
}
pub fn set_font_path(path: &str) {
    let mut s = store().lock().unwrap_or_else(PoisonError::into_inner);
    s.path = Some(path.into());
    s.fonts.clear();
    s.bytes = 0;
    s.warnings.clear();
}
pub(super) fn warn(message: &str) {
    let mut s = store().lock().unwrap_or_else(PoisonError::into_inner);
    if s.warnings.len() < 256 && s.warnings.insert(message.into()) {
        eprintln!("CADCraft: {message}");
    }
}
fn candidate(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.is_file() {
        return Some(p);
    }
    std::fs::read_dir(dir)
        .ok()?
        .take(16384)
        .filter_map(std::result::Result::ok)
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name) && e.path().is_file())
        .map(|e| e.path())
}
pub fn find(name: &str) -> Option<Arc<Font>> {
    if !name.to_ascii_lowercase().ends_with(".shx") {
        return None;
    }
    let paths = font_path();
    let key = format!("{paths}\n{name}");
    if let Some(f) = store().lock().unwrap_or_else(PoisonError::into_inner).fonts.get(&key) {
        return f.clone();
    }
    let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let direct = Path::new(name);
    let path = if direct.is_file() { Some(direct.to_owned()) } else { std::env::split_paths(&paths).find_map(|dir| candidate(&dir, basename)) };
    let found = path.and_then(|p| {
        let mut bytes = Vec::new();
        std::fs::File::open(p).ok()?.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
        match Font::parse(&bytes) {
            Ok(f) => Some(Arc::new(f)),
            Err(e) => {
                warn(&format!("invalid SHX {name}: {e}"));
                None
            }
        }
    });
    if found.is_none() {
        warn(&format!("SHX {name} unavailable; using fallback"));
    }
    let mut s = store().lock().unwrap_or_else(PoisonError::into_inner);
    let size = found.as_ref().map_or(0, |font| font.shapes.values().map(Vec::len).sum::<usize>());
    if s.fonts.len() >= 64 || s.bytes.saturating_add(size) > 64 * 1024 * 1024 {
        s.fonts.clear();
        s.bytes = 0;
    }
    s.bytes += size;
    s.fonts.insert(key, found.clone());
    found
}
