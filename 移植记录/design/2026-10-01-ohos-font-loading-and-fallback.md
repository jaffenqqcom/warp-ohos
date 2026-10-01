# OHOS 字体加载与回退查找：配置驱动排序 + 覆盖裁剪 + 磁盘缓存

关联：[[ohos-debug-lessons]]、[[warp鸿蒙移植分析]] 11.8、[[project_ohos_font_gap]]

- 日期：2026-10-01
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：`crates/warpui/src/platform/ohos/` 的字体子系统（系统字体枚举、缺字回退排序、Unicode 覆盖裁剪、覆盖磁盘缓存）与 `crates/warpui/src/windowing/winit/fonts.rs` 的 OHOS loader 接线
- 状态：已落地，装机验证通过（`238 families / 294 faces`，无 panic；二次启动命中缓存，`50ms total / 2ms coverage`）

## 问题描述

OHOS 版 warp 的字体枚举是自研的（设备无 fontconfig、无 freetype）。最初 `platform/ohos/fonts.rs::fallback_font_faces` 的做法是：**把"主族之外的全部 face"按族名字典序摊成一条回退链，emoji/symbol 族垫底**。这条链既不看请求本身的字重/倾斜，也不看设备自带的"语言→族"回退配置，更没有 Unicode 覆盖裁剪。于是缺字回退（尤其中文）落到哪个字体，全凭族名的字母序碰运气，且链长达全部 face。

用户要求把 OHOS 的字体加载（含裁剪）与查找逻辑改造成和 Linux 一致：Linux 走 fontconfig 的 `FcFontSetSort(trim=true)`，得到的是"按 pattern（族名/字重/倾斜）贴近度排序 + 按 Unicode 覆盖裁剪"的候选集；OHOS 没有 fontconfig，就自己写一个等价物。

本记录就是这套"新方案"的实现说明：数据源、排序、裁剪、缓存，以及它被消费的路径。

## 问题表现

- **中文被比例字体抢走**：旧链按族名字典序，`HarmonyOS Sans`(H) 排在等宽 CJK 族 `Maple Mono NF CN`(M) 之前，中文落到比例字体，终端里 CJK 宽度对不齐。
- **一度出现"中文变斜体"**：中间方案引入了"等宽优先 + 族内 `(is_italic, |weight-400|)` 排序"，按此中文应落到 `Maple Mono NF CN`（本机唯一覆盖 U+4E2D 的等宽族，共 16 个 face）。但装机 HAP 的构建时间早于该排序源码 8 分钟，链里跑的还是旧二进制；用户字体目录 readdir 首个文件恰是 `MapleMono-NF-CN-MediumItalic.ttf`，于是中文命中斜体 face。**根因是打包陈旧，不是算法。**
- **启动被拖慢**：为支持裁剪需要先算每条 face 的 Unicode 覆盖。初版同时枚举 BMP（cmap format 4）与全码表（format 12）子表，CJK 字体被重复枚举，`fontdb` 扫描实测 **4619ms**。

## 问题原因

分两层：直接原因与设计原因。

**设计原因——旧回退链缺少 fontconfig 语义。** 旧实现等价于"主族之外全部 face，按族名字典序"，缺少 `FcFontSetSort(trim=true)` 的两个关键动作：

- 按 pattern（请求族名 / 字重 / 倾斜）的**贴近度排序**；
- 按 Unicode 覆盖**裁剪**掉"已被前面 face 覆盖"的项。

后果是候选顺序无意义、链过长，中文的归属由族名字母序偶然决定。

**OHOS 无 fontconfig，且 NDK 匹配接口不可用。** 设备既无 `libfontconfig.so`，也无 `/etc/fonts`；它把字体匹配声明在一个私有 JSON（`/system/etc/fontconfig.json`）里，由图形栈（skia）消费。NDK 的 `OH_Drawing_FontMgr`（`MatchFamilyStyle` / `MatchFamilyStyleCharacter`，语义上等价 `FcFontMatch`）只返回不透明的 `OH_Drawing_Typeface*`，**取不到文件路径/索引**，而 warp 需要 path + index 自行加载 → 无法直接用，只能自己实现匹配。

**"中文变斜体"的直接原因——stale 产物。** 装机 HAP 早于族内排序源码 8 分钟构建，与算法无关。判据：**HAP 构建时间须晚于 `platform/ohos/fonts.rs` 的 mtime**。

**扫描耗时原因——cmap 重复枚举。** format 12/13 是全码表，已包含 format 4 的全部 BMP 映射；同时枚举两遍是纯粹的浪费。

### 消费路径（为什么改这一个函数就够）

缺字回退的实际决策点在 `crates/warpui_core/src/fonts.rs::system_font_fallback`：它**按序**遍历平台返回的候选列表，取第一个含该字符字形的字体。链路为

```
system_font_fallback
  -> platform.fallback_fonts(ch, font)              // winit/fonts.rs::load_fallback_fonts
    -> loader::fallback_fonts(family, properties)   // #[cfg(ohos)] mod loader
      -> platform/ohos/fonts.rs::fallback_font_faces // ← 本方案改造点
```

OHOS 分支的 `fallback_fonts(character, font_id)` **不按字符过滤**，整条链对所有字符相同。因此"某字符最终落到哪个字体"完全由 `fallback_font_faces` 的**顺序 + 裁剪**决定——改这里即可。

## 解决方案

关键洞察：**把设备的 `fontconfig.json` 当作数据源，复刻 `FcFontSetSort`**；用 `fontdb` 已解析的 face 元数据（weight/style）做 pattern 排序，用 cmap 覆盖做 trim，再把覆盖落盘缓存。

### 1. 数据源：读设备私有配置（新增 `platform/ohos/fontconfig.rs`）

`/system/etc/fontconfig.json` 是干净 JSON，两类数据：

- `generic`：通用名 → 具体族（如 `monospace`、`serif`）。
- `fallback`：语言/脚本 → 族链（如 `zh-Hans` → `HarmonyOS Sans SC`）。

解析落在 `fontconfig.rs`，`OnceLock` 一次性加载；读不到/解析失败返回 `None`，调用方退回启发式。两个实测坑：

- **坑1**：`fallback` 条目里混着 `font-variations`，其值是**数组**。用 `HashMap<String, String>` 会整份解析失败（`invalid type: sequence, expected a string`）→ 必须用 `serde_json::Value` 容错，只保留 `Value::String`。
- **坑2**：同目录的 `fontconfig_ohos.json` 带 C 注释，**非标准 JSON，不能当它解析**。

对外只暴露 `is_loaded()` / `generic_alias_of(name)` / `fallback_family_chain()`。

### 2. 回退排序（`fonts.rs::ordered_fallback_faces`）

族顺序 = **请求族（经 generic alias 归一）→ 配置 fallback 链 → 其余族**（emoji/symbol 单独收集、垫到最后）。`push_family` 小写去重、排除主族。

族内 face 顺序按**本次请求**排序，取代旧实现的固定 `|weight-400|`：

```rust
let want_italic = matches!(properties.style, Style::Italic);
let want_weight = weight_value(properties.weight);
family_faces.sort_by_key(|face| {
    (face.is_italic != want_italic, face.weight.abs_diff(want_weight))
});
```

无配置时退回"等宽族优先"的启发式（旧的等宽优先方案降级为兜底），并打 `warn`。

### 3. 覆盖裁剪（`fonts.rs::trim_fallback_faces`）

以主族的覆盖并集为初始 `covered`，按序保留"提供新增覆盖"的 face，被完全覆盖的 face 剔除：

```rust
if !face.coverage.is_empty() && coverage_is_subset(&face.coverage, &covered) {
    continue; // 已被前面的 face 覆盖，裁掉
}
covered = coverage_union(&covered, &face.coverage);
kept.push(face);
```

空覆盖的 face **保留**——覆盖未知 ≠ 无覆盖，丢掉可能留窟窿。装机实测 `294 candidates → 154 faces kept`。

### 4. 覆盖来源（`fonts.rs::unicode_codepoints` / `face_coverage`）

只读"全码表"子表（cmap format 12 `SegmentedCoverage` / 13 `ManyToOneRangeMappings`）；没有才遍历全部 Unicode 子表。扫描 **4619ms → 270ms**（其中 coverage 257ms）。

### 5. 覆盖磁盘缓存（新增 `platform/ohos/fontcache.rs`）

覆盖计算是首次启动的大头（257ms），落盘复用：

- 位置：`$HOME/.cache/warp/font-coverage.bin`。
- 键：`(path, index)`；值附带**文件身份** `(mtime_secs, mtime_nanos, size)`。
- 头部带 magic `WFC1` + `FORMAT_VERSION`，格式变更即丢弃旧缓存。
- 每次运行**重写**，只写本次见到的 face → 系统字体增/删/升级都会被自然感知，消失的字体被剔除。
- 实测二次启动：`294 cached faces available` / `unchanged, 294 faces reused`，扫描 `50ms total, 2ms coverage`。

无变更时跳过写盘：`computed == 0 && fresh.len() == loaded.len()`。

### 6. 接线

- `crates/warpui/src/windowing/winit/fonts.rs` 的 `#[cfg(ohos)] mod loader`：`fallback_fonts(family_name, properties)` 把 `properties` 透传进 `fallback_font_faces`（原为 `_properties`，签名带一个用不上的参数）。
- `crates/warpui/src/platform/ohos/mod.rs`：注册 `mod fontcache;` 与 `mod fontconfig;`。
- `crates/warpui/Cargo.toml`：OHOS target 依赖加 `serde_json.workspace = true`。

## 修改文件

- `crates/warpui/src/platform/ohos/fonts.rs` — 回退查找重写：`fallback_font_faces` 改为「配置驱动排序 + 覆盖裁剪」；新增 `ordered_fallback_faces` / `push_family` / `trim_fallback_faces` / `weight_value` / `face_coverage` / `unicode_codepoints` / `full_repertoire_subtable` / `coverage_ranges` / `coverage_is_subset` / `coverage_union`；`ScannedFontFace` 增 `weight` / `is_italic` / `coverage`；扫描循环接入 `CoverageCache`；`reported_font_dirs()` 保留（先前的 NDK 目录扩充）。
- `crates/warpui/src/platform/ohos/fontconfig.rs` — **新增**：解析 `/system/etc/fontconfig.json`，提供通用名 alias 与 fallback 族链。
- `crates/warpui/src/platform/ohos/fontcache.rs` — **新增**：Unicode 覆盖的磁盘缓存（`CoverageCache`、`file_identity`、`encode`/`decode`）。
- `crates/warpui/src/platform/ohos/mod.rs` — 注册 `fontcache`、`fontconfig` 两个模块。
- `crates/warpui/src/windowing/winit/fonts.rs` — OHOS loader 把 `properties` 透传进 `fallback_font_faces`。
- `crates/warpui/Cargo.toml` — OHOS target 依赖新增 `serde_json`。
- `Cargo.lock` — 随之更新（warpui 依赖 serde_json）。

## 排查过程中的误判（诚实记录）

- **把"中文变斜体"当成算法 bug 追**：真因是装机 HAP 陈旧（早于族内排序源码 8 分钟）。教训：报字体问题先核对**HAP 构建时间 vs 源文件 mtime**。
- **以为 `fontconfig.json` 可以直接 `HashMap<String, String>` 反序列化**：被 `font-variations`（数组值）整份打挂，改为 `serde_json::Value` 容错。
- **以为只扫目录就够**：`OH_Drawing_GetFontPathsByType(ALL)` 报出的用户字体不在硬编码目录里，必须把报告目录并入扫描（`reported_font_dirs`）；且 API 报的只是目录扫描的**子集**，不能替换扫描，只能取并集。
- **以为可以只读 format 4 BMP**：会漏 BMP 外的中文（CJK 扩展区），必须以全码表 format 12/13 为准。
- **正常路径打了 `warn` 日志**：用户明确要求删除正常路径日志，正常路径一律 `info`/`debug`（release 静默），`warn` 只留给异常。

## 已知缺口与后续

- **format 12 不完整时会低估覆盖**：极少数损坏字体可能有 format 12 但只覆盖补充平面，此时只读它就会丢掉 BMP 覆盖，导致该 face 被误裁。当前按"12/13 是全码表"的规范假设处理；若要更稳，可退化为"12/13 与 4 取并集"（首次多花约 100ms，之后走缓存）。
- **缓存写入非原子**：`fs::File::create` 先截断再写，中途崩溃会留下损坏文件；下次加载 `decode` 失败即丢弃并重算，能自愈，只是浪费一次扫描。可改为临时文件 + rename。
- **`decode` 边界**：`entry_count` / `range_count` 已加"乘以最小单元不超过剩余字节"的校验，防止损坏的长度字段触发无界分配。
- **空覆盖 face 一律保留**：可能让回退链略长于 fontconfig 的裁剪结果。
- **族名去重大小写不敏感**：仅大小写不同的同名族会合并为一个，取先到者。

## 与设计文档 11.8 的关系

`移植记录/design/warp鸿蒙移植分析.md` 的 11.8 节给出的是**落地计划**——"基座 = warp 自带 cosmic-text 实现 + 新写一个 OHOS 字体加载器"。本记录是其后续：加载器在**回退匹配语义**上对齐 Linux fontconfig 的最终实现（配置驱动 + 覆盖裁剪 + 覆盖缓存）。
