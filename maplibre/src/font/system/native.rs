//! Native OS font discovery and lazy materialization backed by `fontdb`.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use harfrust::Font as HbFont;
use read_fonts::FontRef;
use skrifa::MetadataProvider;

use super::{inspect_face_metadata, parse_font_stack_entry, FontStyle, FontSystem, LoadedFace};

pub(in crate::font) struct NativeFontDb {
    db: fontdb::Database,
    loaded_by_db_id: HashMap<fontdb::ID, u64>,
    shared_files: HashMap<PathBuf, Arc<Vec<u8>>>,
    codepoint_cache: HashMap<(String, u32), Option<u64>>,
}

impl FontSystem {
    pub(super) fn ensure_native_db(&mut self) {
        if self.native.is_some() {
            return;
        }
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        #[cfg(target_os = "macos")]
        load_macos_mobile_asset_fonts(&mut db);
        #[cfg(target_os = "visionos")]
        db.load_fonts_dir("/System/Library/Fonts");
        #[cfg(target_os = "android")]
        db.load_fonts_dir("/system/fonts");
        self.native = Some(NativeFontDb {
            db,
            loaded_by_db_id: HashMap::new(),
            shared_files: HashMap::new(),
            codepoint_cache: HashMap::new(),
        });
    }

    pub(super) fn resolve_native_face(
        &mut self,
        font_stack: &str,
        ch: char,
    ) -> Option<(u64, Arc<LoadedFace>)> {
        self.ensure_native_db();
        let cache_key = (font_stack.to_owned(), u32::from(ch));
        if let Some(&cached) = self
            .native
            .as_ref()
            .and_then(|n| n.codepoint_cache.get(&cache_key))
        {
            let id = cached?;
            return self.faces.get(&id).map(|f| (id, Arc::clone(f)));
        }

        let found_db_id = self.find_matching_db_face(font_stack, ch);
        let loaded_id = found_db_id.and_then(|db_id| self.materialize_db_face(db_id));
        if let Some(native) = self.native.as_mut() {
            native.codepoint_cache.insert(cache_key, loaded_id);
        }
        let id = loaded_id?;
        self.faces.get(&id).map(|f| (id, Arc::clone(f)))
    }

    fn find_matching_db_face(&self, font_stack: &str, ch: char) -> Option<fontdb::ID> {
        let native = self.native.as_ref()?;
        let db = &native.db;
        let face_has_char = |id: fontdb::ID| -> bool {
            if db.face(id).is_some_and(|info| {
                info.post_script_name.contains("LastResort")
                    || info
                        .families
                        .iter()
                        .any(|(name, _)| name.contains("LastResort"))
            }) {
                return false;
            }
            db.with_face_data(id, |data, idx| {
                FontRef::from_index(data, idx)
                    .ok()
                    .and_then(|f| f.charmap().map(ch))
                    .is_some_and(|gid| gid != skrifa::GlyphId::NOTDEF)
            })
            .unwrap_or(false)
        };

        let (_, primary_weight, primary_style) =
            parse_font_stack_entry(font_stack.split(',').next().unwrap_or(font_stack));
        let primary_db_style = match primary_style {
            FontStyle::Normal => fontdb::Style::Normal,
            FontStyle::Italic => fontdb::Style::Italic,
            FontStyle::Oblique => fontdb::Style::Oblique,
        };

        for raw_entry in font_stack.split(',') {
            let (family, weight, style) = parse_font_stack_entry(raw_entry);
            let db_style = match style {
                FontStyle::Normal => fontdb::Style::Normal,
                FontStyle::Italic => fontdb::Style::Italic,
                FontStyle::Oblique => fontdb::Style::Oblique,
            };
            let lower = family.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "sans-serif" | "system-ui" | "serif" | "monospace"
            ) {
                for fallback_name in preferred_system_fallbacks(ch) {
                    let query = fontdb::Query {
                        families: &[fontdb::Family::Name(fallback_name)],
                        weight: fontdb::Weight(weight.0),
                        stretch: fontdb::Stretch::Normal,
                        style: db_style,
                    };
                    if let Some(id) = db.query(&query).filter(|&id| face_has_char(id)) {
                        return Some(id);
                    }
                }
            }
            let fam = match lower.as_str() {
                "sans-serif" | "system-ui" => fontdb::Family::SansSerif,
                "serif" => fontdb::Family::Serif,
                "monospace" => fontdb::Family::Monospace,
                _ => fontdb::Family::Name(&family),
            };
            let query = fontdb::Query {
                families: &[fam],
                weight: fontdb::Weight(weight.0),
                stretch: fontdb::Stretch::Normal,
                style: db_style,
            };
            if let Some(id) = db.query(&query).filter(|&id| face_has_char(id)) {
                return Some(id);
            }
        }

        for fallback_name in preferred_system_fallbacks(ch) {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(fallback_name)],
                weight: fontdb::Weight(primary_weight.0),
                stretch: fontdb::Stretch::Normal,
                style: primary_db_style,
            };
            if let Some(id) = db.query(&query).filter(|&id| face_has_char(id)) {
                return Some(id);
            }
        }

        db.faces().map(|info| info.id).find(|&id| face_has_char(id))
    }

    fn materialize_db_face(&mut self, db_id: fontdb::ID) -> Option<u64> {
        if let Some(&existing) = self
            .native
            .as_ref()
            .and_then(|n| n.loaded_by_db_id.get(&db_id))
        {
            return Some(existing);
        }
        let (bytes, face_index) = {
            let native = self.native.as_mut()?;
            let (source, index) = native.db.face_source(db_id)?;
            let arc_bytes = match source {
                fontdb::Source::Binary(bin) => Arc::new((*bin).as_ref().to_vec()),
                fontdb::Source::File(path) => {
                    if let Some(shared) = native.shared_files.get(&path) {
                        Arc::clone(shared)
                    } else {
                        let data = std::fs::read(&path).ok()?;
                        let arc = Arc::new(data);
                        native.shared_files.insert(path, Arc::clone(&arc));
                        arc
                    }
                }
                _ => {
                    let copied = native.db.with_face_data(db_id, |slice, _| slice.to_vec())?;
                    Arc::new(copied)
                }
            };
            (arc_bytes, index)
        };

        let font_ref = FontRef::from_index(bytes.as_slice(), face_index).ok()?;
        let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::clone(&bytes) as _;
        let hb_font = HbFont::new(blob, face_index)?;
        let (families, weight, style) = inspect_face_metadata(&font_ref);
        self.next_id += 1;
        let id = self.next_id;
        self.faces.insert(
            id,
            Arc::new(LoadedFace {
                id,
                bytes,
                face_index,
                hb_font,
                families,
                weight,
                style,
                unicode_ranges: Vec::new(),
            }),
        );
        if let Some(native) = self.native.as_mut() {
            native.loaded_by_db_id.insert(db_id, id);
        }
        Some(id)
    }
}

fn preferred_system_fallbacks(ch: char) -> &'static [&'static str] {
    let cp = u32::from(ch);
    if matches!(cp, 0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF65..=0xFF9F | 0x1B000..=0x1B16F) {
        &[
            "Hiragino Sans",
            "Hiragino Kaku Gothic ProN",
            "PingFang SC",
            "Noto Sans CJK JP",
            "Yu Gothic",
            "Arial Unicode MS",
        ]
    } else if matches!(cp, 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF) {
        &[
            "Apple SD Gothic Neo",
            "Noto Sans CJK KR",
            "Malgun Gothic",
            "PingFang SC",
            "Arial Unicode MS",
        ]
    } else if super::super::cjk::is_local_cjk_codepoint(ch) {
        &[
            "PingFang SC",
            "Hiragino Sans",
            "Hiragino Sans GB",
            "Apple SD Gothic Neo",
            "Heiti SC",
            "Songti SC",
            "STSong",
            "Kaiti SC",
            "Noto Sans CJK SC",
            "Noto Sans CJK JP",
            "Noto Sans CJK KR",
            "WenQuanYi Micro Hei",
            "Microsoft YaHei",
            "Yu Gothic",
            "Malgun Gothic",
            "SimSun-ExtB",
            "Arial Unicode MS",
        ]
    } else {
        &[
            "SF Pro",
            "Helvetica Neue",
            "Arial",
            "Kohinoor Devanagari",
            "Devanagari Sangam MN",
            "Geeza Pro",
            "Thonburi",
            "Noto Sans",
            "Noto Sans Georgian",
            "Noto Sans Devanagari",
            "Noto Sans Arabic",
            "DejaVu Sans",
            "Segoe UI",
            "Nirmala UI",
            "Arial Unicode MS",
        ]
    }
}

#[cfg(target_os = "macos")]
fn load_macos_mobile_asset_fonts(db: &mut fontdb::Database) {
    let Ok(entries) = std::fs::read_dir("/System/Library/AssetsV2") else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("com_apple_MobileAsset_Font"))
        {
            db.load_fonts_dir(path);
        }
    }
}
