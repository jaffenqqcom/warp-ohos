// §2.4.4 — 字体系统：鸿蒙字体加载
//
// load_all_system_fonts() 扫描 /system/fonts/ 目录查找系统字体。
// 使用鸿蒙字体 NDK API 或回退到目录扫描方式。
//
// 鸿蒙系统字体目录：
//   /system/fonts/           — 系统预装字体
//   /data/fonts/             — 用户安装字体（如果存在）
//
// OhosFontDB 使用 fontdb crate 管理字体存储和查询。

use std::any::Any;
use std::collections::HashMap;
use std::ffi::CString;
use std::ops::Range;
use std::thread;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::sync::OnceLock;

extern "C" {
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::os::raw::c_char) -> *mut std::ffi::c_void;
}

use anyhow::{anyhow, Result};
use cosmic_text::{Align, Attrs, AttrsList, BidiParagraphs, LayoutLine, ShapeLine, Shaping, Wrap};
use fontdb::{Database, Family, ID as FontDbId, Query, Source};
use futures::future::BoxFuture;
use futures::FutureExt;
use owned_ttf_parser::{Face as TtfFace, GlyphId as TtfGlyphId, OutlineBuilder, Rect as TtfRect};
use pathfinder_geometry::rect::RectI;
use pathfinder_geometry::vector::{Vector2F, Vector2I, vec2i};
use vec1::{vec1, Vec1};

use warpui_core::fonts::canvas::{Canvas, RasterFormat};
use warpui_core::fonts::{
    FamilyId, FontId, GlyphId, Metrics, Properties, RasterizedGlyph, SubpixelAlignment, Weight,
};
use warpui_core::platform::{FontDB, LineStyle, LoadedSystemFonts, TextLayoutSystem};
use warpui_core::rendering::GlyphConfig;
use warpui_core::text_layout::{
    ClipConfig, Glyph as TextGlyph, Line, Run, StyleAndFont, TextAlignment, TextFrame, TextStyle,
};

/// 将 fontdb::Source 提取为 Vec<u8> 字节缓冲。
fn source_to_bytes(source: &Source, cache: Option<&Mutex<HashMap<PathBuf, Vec<u8>>>>) -> Option<Vec<u8>> {
    // 快速路径：从缓存中获取已读入内存的字体数据
    if let Source::File(path) = source {
        if let Some(cache) = cache {
            let mut guard = cache.lock().unwrap_or_else(ignore_poison);
            if let Some(cached) = guard.get(path) {
                return Some(cached.clone());
            }
            if let Ok(data) = std::fs::read(path) {
                guard.insert(path.clone(), data.clone());
                return Some(data);
            }
            return None;
        }
    }
    // 原始路径：Binary/SharedFile 或未提供缓存
    match source {
        Source::Binary(data) => {
            let bytes: &[u8] = data.as_ref().as_ref();
            Some(bytes.to_vec())
        }
        Source::File(path) => std::fs::read(path).ok(),
        Source::SharedFile(_, data) => {
            let bytes: &[u8] = data.as_ref().as_ref();
            Some(bytes.to_vec())
        }
    }
}

/// 从中毒的 Mutex 中恢复数据，避免 panic 传播。
fn ignore_poison<T>(e: PoisonError<T>) -> T {
    e.into_inner()
}

/// 从 TTF/OTF 字体文件的二进制数据中提取表标签（如 glyf, CFF, COLR, SVG 等），
/// 用于诊断字体轮廓格式不受 owned_ttf_parser 支持的原因。
///
/// 解析 OpenType 文件目录：offset=12，每个记录 16 字节（tag+checksum+offset+length）。
fn detect_ttf_table_tags(data: &[u8]) -> String {
    if data.len() < 12 {
        return "invalid".into();
    }
    let num_tables = u16::from_be_bytes([data[4], data[5]]);
    let dir_offset: usize = 12;
    let mut tags = Vec::new();
    for i in 0..num_tables as usize {
        let off = dir_offset + i * 16;
        if off + 4 > data.len() {
            break;
        }
        let tag_bytes = &data[off..off + 4];
        let tag_str = std::str::from_utf8(tag_bytes).unwrap_or("????");
        tags.push(tag_str);
    }
    tags.join(", ")
}

/// 扫描目录查找字体文件，返回找到的路径列表。
fn scan_font_dir(dir: &str) -> Vec<PathBuf> {
    let mut fonts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("ttf") | Some("otf") | Some("ttc") => fonts.push(path),
                _ => {}
            }
        }
    }
    fonts
}

/// 快速验证字体文件能否被 owned_ttf_parser 正常解析并提取字形轮廓。
///
/// 读取字体文件并用 'm' 字形测试：
/// - 标准 TTF 字体应有 `glyph_bounding_box()` 返回 Some（glyf 表）
/// - CFF 字体需要 `outline_glyph()` 能提取到线段
///
/// 跳过那些两者都失败的字体（如 OHOS 的 Noto Sans），确保后续
/// OhosFontDB 和 cosmic_text 只加载 owned_ttf_parser 可渲染的字体。
fn is_font_usable(path: &PathBuf) -> bool {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            log::warn!("is_font_usable: read failed for '{fname}': {e}");
            return false;
        }
    };
    let face = match TtfFace::parse(&data, 0) {
        Ok(f) => f,
        Err(e) => {
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            log::warn!("is_font_usable: parse failed for '{fname}': {e:?}");
            return false;
        }
    };
    // 先尝试 Latin 字符 'm'，如果不存在则尝试 CJK 字符 '一'（U+4E00），
    // 确保纯 CJK 字体（如 HYQiHeiL3.ttf 汉仪旗黑）也能通过轮廓检查。
    let test_char = if face.glyph_index('m').is_some() { 'm' } else { '\u{4E00}' };
    if let Some(gid) = face.glyph_index(test_char) {
        // 标准 TTF 有 glyf 表，直接返回 bounding_box
        if face.glyph_bounding_box(gid).is_some() {
            return true;
        }
        // CFF/PostScript 回退：尝试提取轮廓线段
        let mut collector = OutlineCollector::new();
        if face.outline_glyph(gid, &mut collector).is_some() {
            return !collector.segments.is_empty();
        }
        // 两者都失败 → 不可用，输出诊断信息
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let table_tags = detect_ttf_table_tags(&data);
        log::warn!(
            "is_font_usable: font '{fname}' ({} bytes) has NO usable outlines. \
             glyph_bounding_box=None, outline_glyph=None. \
             Detected tables: [{table_tags}]. \
             This font will be skipped for glyph rasterization.",
            data.len(),
        );
        return false;
    }
    let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    let table_tags = detect_ttf_table_tags(&data);
    log::warn!(
        "is_font_usable: font '{fname}' ({} bytes) - glyph_index('{test_char}') returned None. \
         Detected tables: [{table_tags}]. Skipping.",
        data.len(),
    );
    false
}

/// 收集到的系统字体信息。
pub struct OhosSystemFonts {
    /// 所有找到的系统字体文件路径。
    font_paths: Vec<PathBuf>,
}

impl OhosSystemFonts {
    fn new() -> Self {
        let mut font_paths = Vec::new();
        font_paths.extend(scan_font_dir("/system/fonts/"));
        font_paths.extend(scan_font_dir("/data/fonts/"));
        let font_count = font_paths.len();
        Self { font_paths }
    }

    pub fn font_paths(&self) -> &[PathBuf] {
        &self.font_paths
    }
}

impl LoadedSystemFonts for OhosSystemFonts {
    fn as_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

/// Ohos 平台的 FontDB 实现。
///
/// 使用 fontdb::Database 管理字体存储和查询。
/// 通过 OhosSystemFonts 扫描系统字体目录。
pub struct OhosFontDB {
    db: Mutex<Database>,
    /// 记录哪些 fontdb::ID 对应哪个 FamilyId（每个 family 可能有多字重）。
    family_fonts: Mutex<HashMap<FamilyId, Vec<FontDbId>>>,
    next_family_id: Mutex<u32>,
    /// fontdb::ID → FontId 映射。fontdb::ID 的 inner 字段是私有的，
    /// 无法从 usize 构造，因此不能直接使用 FontId(usize) 作为索引。
    font_id_map: Mutex<HashMap<FontDbId, FontId>>,
    /// FontId → fontdb::ID 反向映射，用于根据 FontId 查找对应 face。
    font_id_rev_map: Mutex<HashMap<FontId, FontDbId>>,
    /// 单调递增的 FontId 计数器。
    next_font_id: AtomicUsize,
    /// 回退字体搜索结果缓存：char → 支持该字符的 FontId 列表
    /// 回退字体搜索结果缓存：char → 支持该字符的 FontId 列表
    fallback_cache: Mutex<HashMap<char, Vec<FontId>>>,
    /// 热点回退缓存：Unicode 区块 → 上次成功匹配的 FontId（类 Linux 回退记忆）
    hot_fallback: Mutex<HashMap<u8, FontId>>,
    /// 预排序的回退字体列表（类 Linux fontconfig 方案）：按主字体 FontId 索引，
    /// 返回所有候选回退字体（无衬线优先，等宽优先）。不逐字符扫描。
    fallback_list: Mutex<HashMap<FontId, Vec<FontId>>>,
    /// cosmic-text 文本布局引擎（惰性初始化）。
    font_system: Mutex<Option<cosmic_text::FontSystem>>,
    /// 系统字体路径，用于初始化 FontSystem。
    font_paths: Vec<PathBuf>,
    /// cosmic-text fontdb::ID → OhosFontDB FontId 缓存。
    cosmic_font_cache: Mutex<HashMap<FontDbId, FontId>>,
    /// 字体文件数据缓存：首次读取后复用，避免反复磁盘 I/O。
    font_data_cache: Mutex<HashMap<PathBuf, Vec<u8>>>,
    /// 通过 load_from_bytes 注册的打包字体数据（Hack、Roboto 等），
    /// 用于同步注入 cosmic_text::FontSystem 使其也能识别这些字体。
    extra_font_data: Mutex<Vec<(String, Vec<Vec<u8>>)>>,
    /// 运行时区块缓存：block_number → 该区块已发现的字体列表（MRU 顺序）。
    /// 最近匹配成功的字体排最前，连续字符大概率在同一字体内命中。
    block_cache: Mutex<HashMap<u8, Vec<FontId>>>,
    /// 按优先级排序的去重字体列表（供 block_cache 构建时使用）。
    sorted_fonts: OnceLock<Vec<FontId>>,
}

impl OhosFontDB {
    pub fn new() -> Self {
        let mut db = Database::new();
        let system_fonts = OhosSystemFonts::new();
        let font_paths = system_fonts.font_paths().to_vec();
        // 使用 OhosSystemFonts 扫描并加载系统字体
        for path in &font_paths {
            // 跳过已知图标/符号字体（这些字体只有极少数字形，选择它们会导致
            // 大部分文字渲染失败）。OHOS 设备上有 FTSymbol、FTToken 等图标字体。
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname.contains("Symbol") || fname.contains("Token") || fname.contains("Math") {
                continue;
            }
            db.load_font_file(path);
        }
        let face_count = db.faces().count();
        Self {
            db: Mutex::new(db),
            family_fonts: Mutex::new(HashMap::new()),
            next_family_id: Mutex::new(0),
            font_id_map: Mutex::new(HashMap::new()),
            font_id_rev_map: Mutex::new(HashMap::new()),
            next_font_id: AtomicUsize::new(1),
            font_system: Mutex::new(None),
            font_paths,
            fallback_cache: Mutex::new(HashMap::new()),
            hot_fallback: Mutex::new(HashMap::new()),            fallback_list: Mutex::new(HashMap::new()),
            cosmic_font_cache: Mutex::new(HashMap::new()),
            font_data_cache: Mutex::new(HashMap::new()),
            extra_font_data: Mutex::new(Vec::new()),
            block_cache: Mutex::new(HashMap::new()),
            sorted_fonts: OnceLock::new(),
        }
    }

    /// 将 fontdb::ID 列表注册为一个 FamilyId，返回分配的 FamilyId。
    fn register_family(&self, ids: Vec<FontDbId>) -> FamilyId {
        if ids.is_empty() {
            return FamilyId(0);
        }
        let mut next = self.next_family_id.lock().unwrap_or_else(ignore_poison);
        let family_id = FamilyId(*next as usize);
        *next += 1;
        self.family_fonts.lock().unwrap_or_else(ignore_poison).insert(family_id, ids);
        family_id
    }

    /// 获取或创建 fontdb::ID → FontId 映射。
    fn get_or_create_font_id(&self, fontdb_id: FontDbId) -> FontId {
        let mut map = self.font_id_map.lock().unwrap_or_else(ignore_poison);
        if let Some(&fid) = map.get(&fontdb_id) {
            return fid;
        }
        let fid = FontId(self.next_font_id.fetch_add(1, Ordering::Relaxed));
        map.insert(fontdb_id, fid);
        self.font_id_rev_map.lock().unwrap_or_else(ignore_poison).insert(fid, fontdb_id);
        fid
    }

    /// 根据 FontId 查找对应的 fontdb::ID。
    fn resolve_fontdb_id(&self, font_id: FontId) -> Option<FontDbId> {
        self.font_id_rev_map.lock().unwrap_or_else(ignore_poison).get(&font_id).copied()
    }

    /// 获取字体原始数据（Vec<u8>），用于后续 ttf_parser 解析。
    fn get_font_data(&self, font_id: FontId) -> Option<Vec<u8>> {
        let db = self.db.lock().unwrap_or_else(ignore_poison);
        let fontdb_id = self.resolve_fontdb_id(font_id)?;
        let face = db.face(fontdb_id)?;
        source_to_bytes(&face.source, Some(&self.font_data_cache))
    }

    /// 检查指定字体是否包含某字符（直接引用缓存数据，避免克隆字体数据）。
    fn font_has_glyph(&self, font_id: FontId, ch: char) -> Option<bool> {
        let db = self.db.lock().unwrap_or_else(ignore_poison);
        let fontdb_id = self.resolve_fontdb_id(font_id)?;
        let face = db.face(fontdb_id)?;
        match &face.source {
            Source::Binary(data) => {
                let bytes: &[u8] = data.as_ref().as_ref();
                let ttf = TtfFace::parse(bytes, 0).ok()?;
                Some(ttf.glyph_index(ch).is_some())
            }
            Source::File(path) => {
                let p = path.clone(); // 仅克隆路径字符串，不克隆字体数据
                drop(db);
                let mut cache = self.font_data_cache.lock().unwrap_or_else(ignore_poison);
                let data: &[u8] = match cache.get(&p) {
                    Some(d) => d.as_ref(),
                    None => {
                        // 首次访问：缓存未命中，从磁盘读取
                        let bytes = std::fs::read(&p).ok()?;
                        cache.insert(p.clone(), bytes);
                        // 获取刚插入数据的引用 —— 需要再次 get
                        cache.get(&p)?.as_ref()
                    }
                };
                let ttf = TtfFace::parse(data, 0).ok()?;
                Some(ttf.glyph_index(ch).is_some())
            }
            Source::SharedFile(_, data) => {
                let bytes: &[u8] = data.as_ref().as_ref();
                let ttf = TtfFace::parse(bytes, 0).ok()?;
                Some(ttf.glyph_index(ch).is_some())
            }
        }
    }
}

impl FontDB for OhosFontDB {
    fn load_from_bytes(&mut self, name: &str, bytes: Vec<Vec<u8>>) -> Result<FamilyId> {
        // 保存一份字体数据副本供 cosmic_text::FontSystem 使用，
        // 这样打包字体（Hack、Roboto）在文本布局时也能被 cosmic_text 识别。
        {
            let cosmic_copy: Vec<Vec<u8>> = bytes.iter().map(|b| b.clone()).collect();
            self.extra_font_data
                .lock()
                .unwrap_or_else(ignore_poison)
                .push((name.to_string(), cosmic_copy));
        }

        let mut db = self.db.lock().unwrap_or_else(ignore_poison);
        let mut all_ids = Vec::new();
        for data in bytes {
            let source = Source::Binary(std::sync::Arc::new(data));
            let ids = db.load_font_source(source);
            all_ids.extend(ids);
        }
        if all_ids.is_empty() {
            return Ok(FamilyId(0));
        }
        // 尝试按 name 匹配 family
        let family_ids: Vec<FontDbId> = all_ids
            .iter()
            .filter(|id| {
                db.face(**id)
                    .and_then(|f| f.families.first())
                    .map(|(fname, _)| fname.as_str() == name || name.is_empty())
                    .unwrap_or(false)
            })
            .copied()
            .collect();
        let ids_to_register = if family_ids.is_empty() {
            all_ids
        } else {
            family_ids
        };
        let family_id = self.register_family(ids_to_register);

        // 如果 cosmic_text::FontSystem 已经初始化，立即注入新加载的字体数据，
        // 确保后续 layout_text/layout_line 能识别此字体族（如 Hack、Roboto）。
        drop(db); // 先释放 db 锁，避免与 font_system 锁产生冲突
        if let Ok(mut fs_guard) = self.font_system.lock() {
            if let Some(ref mut fs) = *fs_guard {
                let cosmic_db = fs.db_mut();
                if let Ok(extra) = self.extra_font_data.lock() {
                    if let Some((_, data_vec)) = extra.last() {
                        for data in data_vec {
                            cosmic_db.load_font_source(
                                Source::Binary(std::sync::Arc::new(data.clone())),
                            );
                        }
                    }
                }
                // injected into cosmic_text FontSystem
            }
        }

        Ok(family_id)
    }

    #[cfg(not(target_family = "wasm"))]
    fn load_from_system(&mut self, font_family: &str) -> Result<FamilyId> {
        let db = self.db.lock().unwrap_or_else(ignore_poison);
        // 尝试按 family name 查找已有字体
        let query = Query {
            families: &[Family::Name(font_family.into())],
            ..Default::default()
        };
        if let Some(id) = db.query(&query) {
            let existing = vec![id];
            let matched: Vec<FontDbId> = db
                .faces()
                .filter(|f| {
                    f.families
                        .iter()
                        .any(|(fam_name, _)| fam_name == font_family)
                })
                .map(|f| f.id)
                .collect();
            let ids = if matched.is_empty() { existing } else { matched };
            return Ok(self.register_family(ids));
        }
        log::warn!("OhosFontDB::load_from_system: font family '{font_family}' not found");
        Ok(FamilyId(0))
    }

    #[cfg(not(target_family = "wasm"))]
    fn load_all_system_fonts(&self) -> BoxFuture<'static, Box<dyn LoadedSystemFonts>> {
        let system_fonts = OhosSystemFonts::new();
        futures::future::ready(Box::new(system_fonts) as Box<dyn LoadedSystemFonts>).boxed()
    }

    #[cfg(not(target_family = "wasm"))]
    fn process_loaded_system_fonts(
        &mut self,
        loaded_system_fonts: Box<dyn LoadedSystemFonts>,
    ) -> Vec<(Option<FamilyId>, warpui_core::fonts::FontInfo)> {
        let ohos_fonts: Box<OhosSystemFonts> = loaded_system_fonts
            .as_any()
            .downcast()
            .expect("OhosFontDB: failed to downcast LoadedSystemFonts to OhosSystemFonts");

        // 先加载所有字体文件到 db，应用与 OhosFontDB::new() 一致的过滤规则
        // 跳过图标/符号字体和 owned_ttf_parser 无法解析的字体。
        let paths: Vec<PathBuf> = ohos_fonts.font_paths().to_vec();
        {
            let mut db = self.db.lock().unwrap_or_else(ignore_poison);
            for path in &paths {
                let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if fname.contains("Symbol") || fname.contains("Token") || fname.contains("Math") {
                    continue;
                }
                let _ = db.load_font_file(path);
            }
        }

        // 再从 db 中遍历 face 获取信息
        let db = self.db.lock().unwrap_or_else(ignore_poison);
        let mut results = Vec::new();
        for face in db.faces() {
            let family_name = face
                .families
                .first()
                .map(|(name, _)| name.clone())
                .unwrap_or_default();
            // 如果 fontdb 的 monospaced 标记未正确设置，回退按字体名匹配
            let lower = family_name.to_lowercase();
            let mut is_mono = face.monospaced || {
                lower.contains("mono") || lower.contains("hack") || lower.contains("code")
                    || lower.contains("console") || lower.contains("fixed")
            };
            // 如果仍未检测为等宽，通过 'm' 的 advance / upem ≈ 0.6 来判定
            if !is_mono {
                if let Some(data) = source_to_bytes(&face.source, Some(&self.font_data_cache)) {
                    if let Ok(ttf_face) = TtfFace::parse(&data, 0) {
                        let upem = ttf_face.units_per_em() as f32;
                        if let Some(gid) = ttf_face.glyph_index('m') {
                            if let Some(advance) = ttf_face.glyph_hor_advance(gid) {
                                let advance_ratio = advance as f32 / upem;
                                if (advance_ratio - 0.6).abs() < 0.15 {
                                    is_mono = true;
                                }
                            }
                        }
                    }
                }
            }
            let font_info = warpui_core::fonts::FontInfo {
                family_name,
                is_monospace: is_mono,
            };
            // 注册此 face 所属 family（此处简化为每个 face 单独注册）
            let family_id = self.register_family(vec![face.id]);
            results.push((Some(family_id), font_info));
        }
        results
    }

    fn family_id_for_name(&self, name: &str) -> Option<FamilyId> {
        let id = {
            let db = self.db.lock().unwrap_or_else(ignore_poison);
            let query = Query {
                families: &[Family::Name(name.into())],
                ..Default::default()
            };
            db.query(&query)?
        };
        let mut family_map = self.family_fonts.lock().unwrap_or_else(ignore_poison);
        for (fid, ids) in family_map.iter() {
            if ids.contains(&id) {
                return Some(*fid);
            }
        }
        let mut next = self.next_family_id.lock().unwrap_or_else(ignore_poison);
        let family_id = FamilyId(*next as usize);
        *next += 1;
        drop(next);
        family_map.insert(family_id, vec![id]);
        Some(family_id)
    }

    fn load_family_name_from_id(&self, id: FamilyId) -> Option<String> {
        let db = self.db.lock().unwrap_or_else(ignore_poison);
        let font_ids = self.family_fonts.lock().unwrap_or_else(ignore_poison);
        if let Some(ids) = font_ids.get(&id) {
            if let Some(first) = ids.first() {
                return db.face(*first).and_then(|f| {
                    f.families.first().map(|(name, _)| name.clone())
                });
            }
        }
        None
    }

    fn select_font(&self, family_id: FamilyId, properties: Properties) -> FontId {
        let db = self.db.lock().unwrap_or_else(ignore_poison);
        let font_ids = self.family_fonts.lock().unwrap_or_else(ignore_poison);
        if let Some(ids) = font_ids.get(&family_id) {
            let weight = match properties.weight {
                Weight::Thin => fontdb::Weight::THIN,
                Weight::ExtraLight => fontdb::Weight::EXTRA_LIGHT,
                Weight::Light => fontdb::Weight::LIGHT,
                Weight::Normal => fontdb::Weight::NORMAL,
                Weight::Medium => fontdb::Weight::MEDIUM,
                Weight::Semibold => fontdb::Weight::SEMIBOLD,
                Weight::Bold => fontdb::Weight::BOLD,
                Weight::ExtraBold => fontdb::Weight::EXTRA_BOLD,
                Weight::Black => fontdb::Weight::BLACK,
            };
            // 在已注册字体内查找最佳匹配
            if let Some(best) = ids.iter().find(|id| {
                db.face(**id).map_or(false, |face| {
                    (face.weight.0 as i32 - weight.0 as i32).abs() < 100
                })
            }) {
                return self.get_or_create_font_id(*best);
            }
            return self.get_or_create_font_id(ids[0]);
        }
        FontId(0)
    }

    fn fallback_fonts(&self, ch: char, font_id: FontId) -> Vec<FontId> {
        let block = ((ch as u32) >> 8) as u8;

        // 查运行时区块缓存（MRU 顺序：最近匹配的排最前）
        let cached = self.block_cache.lock()
            .unwrap_or_else(ignore_poison)
            .get(&block).cloned();

        let sorted = self.sorted_fonts.get().cloned().unwrap_or_default();

        if let Some(mut list) = cached {
            // 依次检查缓存的字体（从最近匹配的开始）
            for i in 0..list.len() {
                let cfid = list[i];
                if cfid != font_id && self.font_has_glyph(cfid, ch) == Some(true) {
                    if i > 0 {
                        // MRU 提升：移到最前面
                        list.remove(i);
                        list.insert(0, cfid);
                        self.block_cache.lock().unwrap_or_else(ignore_poison)
                            .insert(block, list);
                    }
                    return vec![cfid];
                }
            }
            // 缓存中所有字体都没有 → 从 sorted_fonts 补充，跳过已缓存的
            let cached_set: std::collections::HashSet<FontId> =
                list.iter().copied().collect();
            for &fid in &sorted {
                if cached_set.contains(&fid) || fid == font_id { continue; }
                if self.font_has_glyph(fid, ch) == Some(true) {
                    let mut new_list = vec![fid];
                    new_list.extend(list);
                    self.block_cache.lock().unwrap_or_else(ignore_poison)
                        .insert(block, new_list);
                    return vec![fid];
                }
            }
            return Vec::new();
        }

        // 首次遇到该区块：沿 sorted_fonts 找到第一个匹配
        for &fid in &sorted {
            if fid == font_id { continue; }
            if self.font_has_glyph(fid, ch) == Some(true) {
                self.block_cache.lock().unwrap_or_else(ignore_poison)
                    .insert(block, vec![fid]);
                return vec![fid];
            }
        }
        Vec::new()
    }

    fn font_metrics(&self, font_id: FontId) -> Metrics {
        let data = match self.get_font_data(font_id) {
            Some(d) => d,
            None => {
                return Metrics {
                    units_per_em: 2048,
                    ascent: 1901,
                    descent: -483,
                    line_gap: 0,
                };
            }
        };
        match TtfFace::parse(&data, 0) {
            Ok(face) => Metrics {
                units_per_em: face.units_per_em() as u32,
                ascent: face.ascender(),
                descent: face.descender(),
                line_gap: face.line_gap(),
            },
            Err(_) => Metrics {
                units_per_em: 2048,
                ascent: 1901,
                descent: -483,
                line_gap: 0,
            },
        }
    }

    fn glyph_advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Vector2I> {
        let data = self
            .get_font_data(font_id)
            .ok_or_else(|| anyhow!("glyph_advance: font not found for font_id={}", font_id.0))?;
        let ttf_face = TtfFace::parse(&data, 0)
            .map_err(|_| anyhow!("glyph_advance: failed to parse font data"))?;
        let ttf_gid = TtfGlyphId(glyph_id as u16);
        let h_advance = ttf_face.glyph_hor_advance(ttf_gid).unwrap_or(0);
        Ok(vec2i(h_advance.into(), 0))
    }

    fn glyph_raster_bounds(
        &self,
        font_id: FontId,
        size: f32,
        glyph_id: GlyphId,
        scale: Vector2F,
        _glyph_config: &GlyphConfig,
    ) -> Result<RectI> {
        let typographic = self.glyph_typographic_bounds(font_id, glyph_id)?;
        let typo = typographic.to_f32();
        let effective_size = size * scale.x();
        let data = self
            .get_font_data(font_id)
            .ok_or_else(|| anyhow!("glyph_raster_bounds: font not found for font_id={font_id:?}"))?;
        let ttf_face = TtfFace::parse(&data, 0)
            .map_err(|_| anyhow!("glyph_raster_bounds: failed to parse font data for font_id={font_id:?}"))?;
        let upem = ttf_face.units_per_em() as u32;
        let em_scale = effective_size / upem as f32;
        // TTF 坐标是 y-up：y_min=底部（基线），y_max=顶部（基线上方）。
        // Screen 坐标是 y-down：负 y = 上方。
        // bitmap 的第 0 行对应 glyph 顶部（y_max），所以 bitmap 左上角
        // 需放在基线上方，即 screen_y = baseline_y - y_max * em_scale。
        // typo.origin = (x_min, y_min)，typo.size = (width, y_max - y_min)。
        // 所以 y_max = typo.origin.y + typo.size.y。
        let scaled_origin = vec2i(
            (typo.origin().x() * em_scale).floor() as i32,
            (-(typo.origin().y() + typo.size().y()) * em_scale).floor() as i32,
        );
        let scaled_size = vec2i(
            (typo.size().x() * em_scale).ceil() as i32,
            (typo.size().y() * em_scale).ceil() as i32,
        );
        Ok(RectI::new(scaled_origin, scaled_size))
    }

    fn glyph_typographic_bounds(
        &self,
        font_id: FontId,
        glyph_id: GlyphId,
    ) -> Result<RectI> {
        let data = self
            .get_font_data(font_id)
            .ok_or_else(|| anyhow!("glyph_typographic_bounds: font not found for font_id={}", font_id.0))?;
        let ttf_face = TtfFace::parse(&data, 0)
            .map_err(|_| anyhow!("glyph_typographic_bounds: failed to parse font data"))?;
        let ttf_gid = TtfGlyphId(glyph_id as u16);
        // 优先使用 glyf 表的边界框（TrueType 轮廓）。
        // CFF（PostScript）字体无 glyf 表，此时回退到从 outline 线段计算。
        let bbox = match ttf_face.glyph_bounding_box(ttf_gid) {
            Some(bbox) => bbox,
            None => {
                let mut collector = OutlineCollector::new();
                if ttf_face.outline_glyph(ttf_gid, &mut collector).is_some() {
                    bbox_from_outline_segments(&collector.segments).ok_or_else(|| {
                        anyhow!("no bounding box for glyph {} (CFF fallback failed)", glyph_id)
                    })?
                } else {
                    // 诊断：识别失败的字体
                    let fontdb_id = self.resolve_fontdb_id(font_id);
                    let family_name: String = fontdb_id.and_then(|fid| {
                        self.db.lock().ok().and_then(|db| {
                            db.face(fid).and_then(|f| {
                                f.families.first().map(|(n, _)| n.clone())
                            })
                        })
                    }).unwrap_or_else(|| "unknown".into());
                    // 空格/控制字符无声轮廓是正常行为，不应阻断图层。
                    // 返回零尺寸边界框让 glyph_cache.rs:116 静默跳过此 glyph。
                    return Ok(RectI::new(vec2i(0, 0), vec2i(0, 0)));
                }
            }
        };
        Ok(RectI::new(
            vec2i(bbox.x_min.into(), bbox.y_min.into()),
            vec2i(bbox.width().into(), bbox.height().into()),
        ))
    }

/// 尝试用 OHOS 系统渲染引擎（libnative_drawing.so）渲染字形。
/// 成功返回 (pixels, width, height)，失败返回 None（回退到自有 rasterizer）。
#[allow(dead_code)]


    fn rasterize_glyph(
        &self,
        font_id: FontId,
        size: f32,
        glyph_id: GlyphId,
        scale: Vector2F,
        _subpixel_alignment: SubpixelAlignment,
        _glyph_config: &GlyphConfig,
        _format: RasterFormat,
    ) -> Result<RasterizedGlyph> {
        let _t0 = std::time::Instant::now();
        let effective_size = size * scale.x();

        let data = self
            .get_font_data(font_id)
            .ok_or_else(|| anyhow!("rasterize_glyph: font not found"))?;

        // 尝试系统渲染引擎（libnative_drawing.so），支持 hinting + 子像素 AA
        if let Some((pixels, w, h)) = try_native_rasterize(&data, glyph_id, effective_size) {
            let elapsed = _t0.elapsed();
            if elapsed.as_millis() > 50 {
                log::info!("FONT_DIAG: native_rasterize {}ms glyph_id={} font_id={:?} size={}",
                    elapsed.as_millis(), glyph_id, font_id, effective_size);
            }
            return Ok(RasterizedGlyph {
                canvas: Canvas {
                    pixels,
                    size: vec2i(w, h),
                    row_stride: (w * 4) as usize,
                    format: RasterFormat::Rgba32,
                },
                is_emoji: false,
            });
        }

        let ttf_face = TtfFace::parse(&data, 0)
            .map_err(|_| anyhow!("rasterize_glyph: failed to parse font"))?;
        let upem = ttf_face.units_per_em() as u32;
        let ttf_gid = TtfGlyphId(glyph_id as u16);

        let mut collector = OutlineCollector::new();
        if ttf_face.outline_glyph(ttf_gid, &mut collector).is_none() {
            log::warn!("rasterize_glyph: no outline for font_id={font_id:?} glyph={glyph_id} size={effective_size}");
            return Ok(RasterizedGlyph {
                canvas: Canvas {
                    pixels: vec![],
                    size: vec2i(0, 0),
                    row_stride: 0,
                    format: RasterFormat::A8,
                },
                is_emoji: false,
            });
        }

        let bbox = match ttf_face.glyph_bounding_box(ttf_gid) {
            Some(bbox) => bbox,
            None => {
                // CFF 字体回退：从已收集的 outline 线段计算边界框。
                // collector.segments 在 outline_glyph 成功后有值。
                bbox_from_outline_segments(&collector.segments).ok_or_else(|| {
                    anyhow!("rasterize_glyph: no bounding box for glyph {} (CFF fallback failed)", glyph_id)
                })?
            }
        };

        let scale_f = effective_size / upem as f32;
        let canvas_w = (((bbox.x_max - bbox.x_min) as f32) * scale_f).ceil() as i32;
        let canvas_h = (((bbox.y_max - bbox.y_min) as f32) * scale_f).ceil() as i32;


        if canvas_w <= 0 || canvas_h <= 0 {
            return Ok(RasterizedGlyph {
                canvas: Canvas {
                    pixels: vec![],
                    size: vec2i(0, 0),
                    row_stride: 0,
                    format: RasterFormat::A8,
                },
                is_emoji: false,
            });
        }

        let pixels = rasterize_outline(&collector.segments, &bbox, scale_f, canvas_w, canvas_h);

        // 将 A8 alpha 展开为 RGBA32。
        // 着色器取 R 通道（tex_color.r）做对比增强，所以覆盖度必须放在 R 通道。
        // A 通道用于 ALPHA_BLENDING。
        // 不能用 [255,255,255,a] 会导致 R 通道始终为 1.0 → 渲染为实心色块。
        let rgba: Vec<u8> = pixels.iter().flat_map(|&a| vec![a, a, a, a]).collect();

        let elapsed = _t0.elapsed();
        if elapsed.as_millis() > 50 {
            log::info!("FONT_DIAG: rasterize_glyph {}ms glyph_id={} font_id={:?} size={}", elapsed.as_millis(), glyph_id, font_id, effective_size);
        }
        Ok(RasterizedGlyph {
            canvas: Canvas {
                pixels: rgba,
                size: vec2i(canvas_w, canvas_h),
                row_stride: canvas_w as usize * 4,
                format: RasterFormat::Rgba32,
            },
            is_emoji: false,
        })
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let data = self.get_font_data(font_id)?;
        let ttf_face = TtfFace::parse(&data, 0).ok()?;
        if let Some(gid) = ttf_face.glyph_index(ch) {
            // 记录热点回退：该字体的 Unicode 区块成功匹配此字符
            let block = ((ch as u32) >> 8) as u8;
            if let Ok(mut hf) = self.hot_fallback.lock() {
                hf.insert(block, font_id);
            }
            Some(gid.0.into())
        } else {
            None
        }
    }

    fn text_layout_system(&self) -> &dyn TextLayoutSystem {
        self
    }
}

/// 尝试用 OHOS 系统渲染引擎（libnative_drawing.so）渲染字形。
/// 成功返回 (pixels, width, height)，失败返回 None（回退到自有 rasterizer）。
#[allow(dead_code)]
fn try_native_rasterize(font_data: &[u8], glyph_id: GlyphId, size: f32) -> Option<(Vec<u8>, i32, i32)> {
    type NativeRenderFn = unsafe extern "C" fn(
        *const u8, i32, u32, f32,
        *mut *mut u8, *mut i32, *mut i32,
    ) -> i32;

    extern "C" {
        fn free(p: *mut std::ffi::c_void);
    }

    static FN: OnceLock<Option<NativeRenderFn>> = OnceLock::new();
    let f = FN.get_or_init(|| {
        let cname = CString::new("ohos_render_glyph_native").ok()?;
        let ptr = unsafe { dlsym(std::ptr::null_mut(), cname.as_ptr().cast()) };
        if ptr.is_null() {
            log::info!("try_native_rasterize: ohos_render_glyph_native not found, using fallback rasterizer");
            None
        } else {
            Some(unsafe { std::mem::transmute::<*mut std::ffi::c_void, NativeRenderFn>(ptr) })
        }
    });

    let func = f.as_ref()?;
    let mut out_data: *mut u8 = std::ptr::null_mut();
    let mut out_w: i32 = 0;
    let mut out_h: i32 = 0;

    let ret = unsafe {
        func(
            font_data.as_ptr(),
            font_data.len() as i32,
            glyph_id as u32,
            size,
            &mut out_data,
            &mut out_w,
            &mut out_h,
        )
    };

    if ret != 0 || out_data.is_null() || out_w <= 0 || out_h <= 0 {
        if !out_data.is_null() {
            unsafe { free(out_data as *mut std::ffi::c_void); }
        }
        return None;
    }

    let len = (out_w * out_h * 4) as usize;
    let pixels = unsafe { std::slice::from_raw_parts(out_data, len).to_vec() };
    unsafe { free(out_data as *mut std::ffi::c_void); }

    log::info!("try_native_rasterize: rendered glyph_id={} size={} → {}x{}", glyph_id, size, out_w, out_h);
    Some((pixels, out_w, out_h))
}

impl OhosFontDB {
    /// 获取或初始化 cosmic-text FontSystem（惰性加载）。
    fn with_font_system<T>(&self, f: impl FnOnce(&mut cosmic_text::FontSystem) -> T) -> T {
        let mut guard = self.font_system.lock().unwrap_or_else(ignore_poison);
        if guard.is_none() {
            let _t_init = std::time::Instant::now();
            let mut fs = cosmic_text::FontSystem::new_with_locale_and_db(
                "en".into(),
                fontdb::Database::new(),
            );
            let db = fs.db_mut();

            // 先加载打包字体（Hack、Roboto 等），确保它们在 fontdb 中优先。
            // 系统字体后加载，不会覆盖已注册的打包字体。
            let extra_guard = self.extra_font_data.lock().unwrap_or_else(ignore_poison);
            for (family_name, data_vec) in extra_guard.iter() {
                for data in data_vec {
                    db.load_font_source(Source::Binary(std::sync::Arc::new(data.clone())));
                }
            }

            // TTF 字体优先加载，确保 cosmic_text 回退时优先找到 upem=2048 的字体。
            // CFF/OTF 字体后加载（upem=1000），避免 CFF/TTF 混用导致字形高度不一致。
            let (ttf_paths, otf_paths): (Vec<_>, Vec<_>) = self.font_paths.iter()
                .partition(|p| p.extension().and_then(|e| e.to_str()) == Some("ttf"));
            for path in ttf_paths.iter().chain(otf_paths.iter()) {
                let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if fname.contains("Symbol") || fname.contains("Token") || fname.contains("Math") {
                    continue;
                }
                db.load_font_file(path);
            }
            drop(extra_guard);

            // Log all available face info for diagnosis
            if fs.db().faces().count() == 0 {
                log::error!("OhosFontDB: NO font faces loaded! Text will not render.");
            }
            log::info!("FONT_DIAG: FontSystem init took {}ms, loaded {} font files",
                _t_init.elapsed().as_millis(),
                ttf_paths.len() + otf_paths.len());
            // 构建回退字体排序列表（类 Linux fontconfig 方案）
            self.build_fallback_list();
            *guard = Some(fs);
        }
        f(guard.as_mut().unwrap())
    }

    /// 将 cosmic-text 的 fontdb::ID 转换为 OhosFontDB 的 FontId。
    fn cosmic_to_local_font_id(&self, cosmic_id: FontDbId) -> Option<FontId> {
        // 查缓存
        {
            let cache = self.cosmic_font_cache.lock().unwrap_or_else(ignore_poison);
            if let Some(&fid) = cache.get(&cosmic_id) {
                return Some(fid);
            }
        }

        // 从 FontSystem 的 db 获取字体信息
        let (family_name, weight, style) = self.with_font_system(
            |fs: &mut cosmic_text::FontSystem| -> Option<(String, fontdb::Weight, fontdb::Style)> {
                let face = fs.db().face(cosmic_id)?;
                let family = face.families.first()?.0.clone();
                Some((family, face.weight, face.style))
            },
        )?;

        // 在 OhosFontDB 中查找匹配的字体。
        // 优先选择 Binary 来源的字体（从 ASSETS 加载的打包字体，如 Hack），
        // 避免匹配到同名的系统字体（字形数据不同导致"音符"效应）。
        let db_guard = self.db.lock().unwrap_or_else(ignore_poison);
        let binary_match = db_guard.faces().filter(|f| {
            let db_family = f.families.first().map(|(n, _)| n.as_str()).unwrap_or("");
            db_family == family_name && f.weight == weight && f.style == style
                && matches!(f.source, fontdb::Source::Binary(_))
        }).next();
        let match_face = binary_match.or_else(|| {
            db_guard.faces().filter(|f| {
                let db_family = f.families.first().map(|(n, _)| n.as_str()).unwrap_or("");
                db_family == family_name && f.weight == weight && f.style == style
            }).next()
        });
        if let Some(db_face) = match_face {
            let font_id = self.get_or_create_font_id(db_face.id);
            self.cosmic_font_cache.lock().unwrap_or_else(ignore_poison).insert(cosmic_id, font_id);

            // 诊断：检测 CFF (upem=1000) 与 TTF (upem=2048) 混用情况
            if let Some(data) = source_to_bytes(&db_face.source, None) {
                if let Ok(ttf_face) = TtfFace::parse(&data, 0) {
                    let upem = ttf_face.units_per_em();
                    if upem != 2048 {
                        log::warn!(
                            "FONT_UPM: cosmic_id={:?} font_id={:?} family='{}' upem={} (expected 2048). \
                             Glyphs from this font may render at different heights when mixed with TTF fonts.",
                            cosmic_id, font_id, family_name, upem,
                        );
                    }
                }
            }

            return Some(font_id);
        }

        log::warn!(
            "cosmic_to_local_font_id: NO MATCH for family='{family_name}' weight={weight:?} style={style:?}. \
             OhosFontDB has {} faces",
            db_guard.faces().count(),
        );
        None
    }

    /// 根据 FamilyId 获取字体族名称。
    fn family_id_to_name(&self, family_id: FamilyId) -> Option<String> {
        let db = self.db.lock().unwrap_or_else(ignore_poison);
        let ids = self.family_fonts.lock().unwrap_or_else(ignore_poison);
        let font_ids = ids.get(&family_id)?;
        let first_id = *font_ids.first()?;
        let face = db.face(first_id)?;
        face.families.first().map(|(n, _)| n.clone())
    }

    /// 将 warp Weight 转换为 cosmic-text Weight 数值。
    fn weight_to_cosmic(weight: Weight) -> cosmic_text::Weight {
        match weight {
            Weight::Thin => cosmic_text::Weight(100),
            Weight::ExtraLight => cosmic_text::Weight(200),
            Weight::Light => cosmic_text::Weight(300),
            Weight::Normal => cosmic_text::Weight(400),
            Weight::Medium => cosmic_text::Weight(500),
            Weight::Semibold => cosmic_text::Weight(600),
            Weight::Bold => cosmic_text::Weight(700),
            Weight::ExtraBold => cosmic_text::Weight(800),
            Weight::Black => cosmic_text::Weight(900),
        }
    }

    /// 从 style_runs 构建 cosmic-text AttrsList。
    fn build_attrs_list(&self, text: &str, style_runs: &[(Range<usize>, StyleAndFont)]) -> AttrsList {
        let mut attrs_list = AttrsList::new(Attrs::new());
        for (range, style_and_font) in style_runs {
            let start = range.start.min(text.len());
            let end = range.end.min(text.len());
            if start >= end {
                continue;
            }
            let family_name = self.family_id_to_name(style_and_font.font_family);
            attrs_list.add_span(start..end, Attrs {
                color_opt: None,
                family: match family_name {
                    Some(ref name) => cosmic_text::Family::Name(name.as_str()),
                    None => cosmic_text::Family::Monospace,
                },
                stretch: Default::default(),
                style: cosmic_text::Style::Normal,
                weight: Self::weight_to_cosmic(style_and_font.properties.weight),
                metadata: 0,
                cache_key_flags: cosmic_text::CacheKeyFlags::empty(),
                metrics_opt: None,
            });
        }
        attrs_list
    }

    /// 将 cosmic-text LayoutLine 转换为 warp 的 Line。
    fn cosmic_line_to_line(
        &self,
        layout_line: &LayoutLine,
        font_size: f32,
        line_height_ratio: f32,
        baseline_ratio: f32,
        clip_config: Option<ClipConfig>,
    ) -> Line {
        if layout_line.glyphs.is_empty() {
            return Line::empty(font_size, line_height_ratio, 0);
        }

        // 按 font_id 分组成 Run
        let mut runs: Vec<Run> = Vec::new();
        let mut current_run_glyphs: Vec<(FontDbId, &cosmic_text::LayoutGlyph)> = Vec::new();

        for glyph in &layout_line.glyphs {
            if let Some(&(last_cosmic_id, _)) = current_run_glyphs.last() {
                if last_cosmic_id != glyph.font_id {
                    // 完成当前 run
                    if let Some(run) = self.flush_run(&current_run_glyphs) {
                        runs.push(run);
                    }
                    current_run_glyphs.clear();
                }
            }
            current_run_glyphs.push((glyph.font_id, glyph));
        }
        if !current_run_glyphs.is_empty() {
            if let Some(run) = self.flush_run(&current_run_glyphs) {
                runs.push(run);
            }
        }

        Line {
            width: layout_line.w,
            trailing_whitespace_width: 0.0,
            runs,
            font_size,
            line_height_ratio,
            baseline_ratio,
            ascent: layout_line.max_ascent,
            descent: layout_line.max_descent,
            clip_config,
            caret_positions: Vec::new(),
            chars_with_missing_glyphs: Vec::new(),
        }
    }

    /// 将一组同字体的 glyph 转为 Run。
    fn flush_run(&self, glyphs: &[(FontDbId, &cosmic_text::LayoutGlyph)]) -> Option<Run> {
        let (cosmic_id, _) = glyphs.first()?;
        let font_id = self.cosmic_to_local_font_id(*cosmic_id)?;

        let text_glyphs: Vec<TextGlyph> = glyphs.iter().map(|(_, g)| TextGlyph {
            id: g.glyph_id as GlyphId,
            position_along_baseline: Vector2F::new(g.x, g.y),
            index: g.start,
            width: g.w,
        }).collect();

        let width = text_glyphs.iter().map(|g| g.width).sum();

        Some(Run {
            font_id,
            glyphs: text_glyphs,
            styles: TextStyle::default(),
            width,
        })
    }

    /// 在后台线程预扫描常用特殊字符，填充 fallback 缓存。
    /// 一次遍历字体列表同时检查所有字符，避免每字符重复扫 258 字体。
    /// 注意：块状字符排在最前，确保 Hermes logo 等常见场景第一时间命中。
    /// 构建回退字体列表（类 Linux fontconfig 方案）。
    /// 遍历 fontdb 中所有 face，按优先级排序后缓存。
    fn build_fallback_list(&self) {
        let db = self.db.lock().unwrap_or_else(ignore_poison);

        // 第一步：为所有 face 创建 FontId，收集 (FontDbId, 族名)
        let all_faces: Vec<(FontDbId, String)> = db.faces().filter_map(|face| {
            let family = face.families.first().map(|(n, _)| n.clone()).unwrap_or_default();
            self.get_or_create_font_id(face.id); // 确保 FontId 已注册
            Some((face.id, family))
        }).collect();
        drop(db);

        // 第二步：按族名去重，构建候选列表（每个族只取第一个 face）
        let seen: std::cell::RefCell<std::collections::HashSet<String>> =
            std::cell::RefCell::new(std::collections::HashSet::new());
        let mut candidates: Vec<(FontDbId, String)> = all_faces.iter()
            .filter(|(_, family)| seen.borrow_mut().insert(family.clone()))
            .cloned()
            .collect();

        // 按优先级排序
        const SENTINELS: [&str; 7] = ["mono", "hack", "code", "sans", "hei", "deng", "cjk"];
        candidates.sort_by(|(_, a), (_, b)| {
            let al = a.to_lowercase(); let bl = b.to_lowercase();
            let a_sc = SENTINELS.iter().position(|k| al.contains(k)).unwrap_or(7);
            let b_sc = SENTINELS.iter().position(|k| bl.contains(k)).unwrap_or(7);
            a_sc.cmp(&b_sc).then(al.cmp(&bl))
        });

        // 第三步：获取 FontId
        let id_map = self.font_id_map.lock().unwrap_or_else(ignore_poison);
        let sorted: Vec<FontId> = candidates.iter()
            .filter_map(|(fid, _)| id_map.get(fid).copied())
            .collect();
        let all_ids: Vec<FontId> = all_faces.iter()
            .filter_map(|(fid, _)| id_map.get(fid).copied())
            .collect();
        drop(id_map);

        // 为 **所有** font_id 创建映射（key = 所有 face 的 FontId），
        // 确保 cosmic_text 使用任何字重的字体时都能查到 fallback 列表。
        let map: HashMap<FontId, Vec<FontId>> = all_ids.iter().map(|&pid| {
            let list: Vec<FontId> = sorted.iter().filter(|&&id| id != pid).copied().collect();
            (pid, list)
        }).collect();
        let n_keys = map.len();
        *self.fallback_list.lock().unwrap_or_else(ignore_poison) = map;
        log::info!("FONT_DIAG: built fallback list ({} keys from {} faces, {} sorted families)",
            n_keys, all_faces.len(), sorted.len());
        // 保存排序后的字体列表，供运行时区块缓存使用
        _ = self.sorted_fonts.set(sorted);
    }
}

/// CJK Unified Ideographs and related Unicode blocks
const CJK_RANGES: &[std::ops::RangeInclusive<char>] = &[
    '\u{4E00}'..='\u{9FFF}',   // CJK Unified Ideographs
    '\u{3400}'..='\u{4DBF}',   // CJK Unified Ideographs Extension A
    '\u{2E80}'..='\u{2EFF}',   // CJK Radicals Supplement
    '\u{3000}'..='\u{303F}',   // CJK Symbols and Punctuation
    '\u{3040}'..='\u{309F}',   // Hiragana
    '\u{30A0}'..='\u{30FF}',   // Katakana
    '\u{FF00}'..='\u{FFEF}',   // Fullwidth Forms
    '\u{FE30}'..='\u{FE4F}',   // CJK Compatibility Forms
];

fn is_cjk_char(ch: char) -> bool {
    CJK_RANGES.iter().any(|range| range.contains(&ch))
}

/// Fallback 缓存中用于共享 CJK 结果的哨兵键（见 fallback_fonts）
const CJK_SENTINEL: char = '\u{4E00}';

impl TextLayoutSystem for OhosFontDB {
    fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        clip_config: ClipConfig,
    ) -> Line {
        let _t_layout = std::time::Instant::now();
        if text.is_empty() {
            return Line::empty(line_style.font_size, line_style.line_height_ratio, 0);
        }

        // 只处理第一段（layout_line 是单行布局）
        let first_para = BidiParagraphs::new(text).next().unwrap_or(text);

        let attrs_list = self.build_attrs_list(first_para, style_runs);

        let maybe_ll = self.with_font_system(|fs| {
            let tab_width = line_style.fixed_width_tab_size.unwrap_or(4) as u16;
            let shaped = ShapeLine::new(fs, first_para, &attrs_list, Shaping::Advanced, tab_width);
            let layouts = shaped.layout(
                line_style.font_size,
                Some(max_width),
                Wrap::None,
                Some(Align::Left),
                None,
                None,
            );
            layouts.into_iter().next()
        });

        let Some(ll) = maybe_ll else {
            return Line::empty(line_style.font_size, line_style.line_height_ratio, 0);
        };

        let result = self.cosmic_line_to_line(
            &ll,
            line_style.font_size,
            line_style.line_height_ratio,
            line_style.baseline_ratio,
            Some(clip_config),
        );
        let elapsed = _t_layout.elapsed();
        if elapsed.as_millis() > 10 {
            log::info!("FONT_DIAG: layout_line {}ms for text='{}'", elapsed.as_millis(), text.chars().take(20).collect::<String>());
        }
        result
    }

    fn layout_text(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        max_height: f32,
        alignment: TextAlignment,
        first_line_head_indent: Option<f32>,
    ) -> TextFrame {
        if text.is_empty() {
            return TextFrame::empty(line_style.font_size, line_style.line_height_ratio);
        }

        let attrs_list = self.build_attrs_list(text, style_runs);

        let align = match alignment {
            TextAlignment::Left => Align::Left,
            TextAlignment::Center => Align::Center,
            TextAlignment::Right => Align::Right,
        };

        let layouts: Vec<LayoutLine> = self.with_font_system(|fs| {
            let mut all_lines = Vec::new();
            for paragraph in BidiParagraphs::new(text) {
                let tab_width = line_style.fixed_width_tab_size.unwrap_or(4) as u16;
                let shaped = ShapeLine::new(fs, paragraph, &attrs_list, Shaping::Advanced, tab_width);
                let lines = shaped.layout(
                    line_style.font_size,
                    Some(max_width),
                    Wrap::WordOrGlyph,
                    Some(align),
                    first_line_head_indent,
                    None,
                );
                all_lines.extend(lines);
            }
            all_lines
        });

        if layouts.is_empty() {
            return TextFrame::empty(line_style.font_size, line_style.line_height_ratio);
        }

        let max_w = layouts.iter().map(|l| l.w).fold(0.0f32, f32::max);

        // 按 max_height 裁切
        let mut total_height = 0.0f32;
        let lines: Vec<Line> = layouts.into_iter().take_while(|ll| {
            let line_h = (ll.max_ascent + ll.max_descent).abs();
            if total_height + line_h > max_height && total_height > 0.0 {
                false
            } else {
                total_height += line_h;
                true
            }
        }).map(|ll| {
            self.cosmic_line_to_line(
                &ll,
                line_style.font_size,
                line_style.line_height_ratio,
                line_style.baseline_ratio,
                None,
            )
        }).collect();

        match Vec1::try_from_vec(lines) {
            Ok(vec) => TextFrame::new(vec, max_w, alignment),
            Err(_) => TextFrame::empty(line_style.font_size, line_style.line_height_ratio),
        }
    }
}

// ── TrueType 字形轮廓收集与光栅化 ──────────────────────────────────────────

/// 收集 TrueType 字形轮廓线段，实现 OutlineBuilder trait。
struct OutlineCollector {
    segments: Vec<(f32, f32, f32, f32)>,
    last_x: f32,
    last_y: f32,
    start_x: f32,
    start_y: f32,
}

impl OutlineCollector {
    fn new() -> Self {
        Self {
            segments: Vec::new(),
            last_x: 0.0,
            last_y: 0.0,
            start_x: 0.0,
            start_y: 0.0,
        }
    }
}

impl OutlineBuilder for OutlineCollector {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last_x = x;
        self.last_y = y;
        self.start_x = x;
        self.start_y = y;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push((self.last_x, self.last_y, x, y));
        self.last_x = x;
        self.last_y = y;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let n = 8;
        let mut prev_x = self.last_x;
        let mut prev_y = self.last_y;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let qx = mt * mt * self.last_x + 2.0 * mt * t * x1 + t * t * x;
            let qy = mt * mt * self.last_y + 2.0 * mt * t * y1 + t * t * y;
            self.segments.push((prev_x, prev_y, qx, qy));
            prev_x = qx;
            prev_y = qy;
        }
        self.last_x = x;
        self.last_y = y;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let n = 12;
        let mut prev_x = self.last_x;
        let mut prev_y = self.last_y;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let cx = mt * mt * mt * self.last_x
                + 3.0 * mt * mt * t * x1
                + 3.0 * mt * t * t * x2
                + t * t * t * x;
            let cy = mt * mt * mt * self.last_y
                + 3.0 * mt * mt * t * y1
                + 3.0 * mt * t * t * y2
                + t * t * t * y;
            self.segments.push((prev_x, prev_y, cx, cy));
            prev_x = cx;
            prev_y = cy;
        }
        self.last_x = x;
        self.last_y = y;
    }

    fn close(&mut self) {
        if (self.last_x - self.start_x).abs() > 0.001
            || (self.last_y - self.start_y).abs() > 0.001
        {
            self.segments
                .push((self.last_x, self.last_y, self.start_x, self.start_y));
        }
        self.last_x = self.start_x;
        self.last_y = self.start_y;
    }
}

/// 将 TrueType 字形轮廓线段光栅化为 A8 alpha 缓冲。
/// 从 OutlineBuilder 收集的线段计算 glyph 边界框（font units）。
///
/// 回退路径：CFF（PostScript）字体没有 TrueType glyf 表，
/// `Face::glyph_bounding_box()` 因此返回 None。本函数从 `outline_glyph()`
/// 已收集的线段（坐标同为 font units）计算边界框，作为备用。
fn bbox_from_outline_segments(segments: &[(f32, f32, f32, f32)]) -> Option<owned_ttf_parser::Rect> {
    if segments.is_empty() {
        return None;
    }
    let mut x_min = f32::MAX;
    let mut y_min = f32::MAX;
    let mut x_max = f32::MIN;
    let mut y_max = f32::MIN;
    for &(x1, y1, x2, y2) in segments {
        if x1 < x_min { x_min = x1; }
        if y1 < y_min { y_min = y1; }
        if x2 < x_min { x_min = x2; }
        if y2 < y_min { y_min = y2; }
        if x1 > x_max { x_max = x1; }
        if y1 > y_max { y_max = y1; }
        if x2 > x_max { x_max = x2; }
        if y2 > y_max { y_max = y2; }
    }
    // 线段坐标 font units 为 f32，夹紧到 i16 标准范围（TTF font units 范围）
    let clamp = |v: f32| -> i16 {
        if v < i16::MIN as f32 { i16::MIN }
        else if v > i16::MAX as f32 { i16::MAX }
        else { v as i16 }
    };
    Some(owned_ttf_parser::Rect {
        x_min: clamp(x_min),
        y_min: clamp(y_min),
        x_max: clamp(x_max),
        y_max: clamp(y_max),
    })
}

/// 使用扫描线算法 + even-odd 填充规则。
fn rasterize_outline(
    segments: &[(f32, f32, f32, f32)],
    bbox: &TtfRect,
    scale: f32,
    canvas_width: i32,
    canvas_height: i32,
) -> Vec<u8> {
    let x_min = bbox.x_min as f32;
    let y_max = bbox.y_max as f32;
    let mut pixels = vec![0u8; (canvas_width * canvas_height) as usize];

    for py in 0..canvas_height {
        let font_y = y_max - (py as f32 + 0.5) / scale;

        let mut xs: Vec<f32> = Vec::new();
        for &(fx0, fy0, fx1, fy1) in segments {
            if (fy1 - fy0).abs() < 0.001 {
                continue;
            }
            let (y_lo, y_hi) = if fy0 < fy1 { (fy0, fy1) } else { (fy1, fy0) };
            if font_y > y_lo && font_y <= y_hi {
                let t = (font_y - fy0) / (fy1 - fy0);
                let fx = fx0 + t * (fx1 - fx0);
                let px = (fx - x_min) * scale;
                xs.push(px);
            }
        }

        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            let x_start = xs[i].ceil() as i32;
            let x_end = xs[i + 1].floor() as i32;
            for px in x_start.max(0)..x_end.min(canvas_width) {
                pixels[(py * canvas_width + px) as usize] = 255;
            }
            i += 2;
        }
    }

    pixels
}
