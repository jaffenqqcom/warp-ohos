# OHOS 终端正文白色过亮，按设置说明文字标准调暗

关联：[[ohos-debug-lessons]]

- 日期：2026-10-06
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：终端默认前景色（`color_index::FOREGROUND`）。只影响 OHOS；非 OHOS 平台编译期整块剔除，前景色不变。
- 状态：已实现（源码改动尚未提交）。

## 问题描述

OHOS 上终端正文的默认白色偏亮、长时间阅读刺眼。需要把它调暗到接近设置项**说明文字**的白色观感，
即"白 @80% 不透明度"的目标亮度——80 是 OHOS 自己选定的值，与主题 `text_sub` 的 60 刻意不同，
同时不能影响终端配色表里的其它颜色。

## 问题表现

- 终端正文纯白（默认前景 `#FFFFFF`）比设置页说明文字亮，中文与英文正文都偏刺眼。
- 仅正文默认前景偏亮；ANSI 调色板里的彩色、以及加粗色等不受此观感问题影响（本次也不改它们）。

## 问题原因

根因不是"颜色算错了"，而是**终端 grid 的下游会覆写 alpha**，导致不能像普通 UI 文本那样简单地给前景色加透明度。

### 因果链

1. 上游 `app/src/terminal/color.rs` 的 `List::fill_named` 直接把 `colors.primary.foreground`（默认白）
   写入 `color_index::FOREGROUND`，没有任何调暗。

2. 终端正文由 `app/src/terminal/grid_renderer.rs` 绘制。其中的 `paint_line` 在写 glyph 前景色时执行
   `foreground_color.a = alpha;`——**用绘制 alpha 覆写了 cell 前景色自带的 alpha**。

3. 因此若只是在 `FOREGROUND` 上挂一个 80% 的 alpha，到了绘制阶段会被 paint alpha 覆盖掉，
   透明度等于没加，正文仍是纯白。

4. 要让"白 @80%"这种亮度真正落到屏幕上，必须**在进入 grid 之前就把半透明白对着终端背景预合成成不透明色**，
   这样 alpha 通道不再承载信息、被覆写也无所谓。

## 解决方案

关键洞察：终端前景的透明度会在这条渲染链路里被抹掉，所以"半透明白"的正确表达是**预合成后的不透明灰**，
而不是带 alpha 的白。

### 实现

- `app/src/terminal/color.rs` 的 `List::fill_named` 里，在原赋值之后插入 `#[cfg(target_env = "ohos")]` 门控块，
  就地把默认前景调暗并覆盖 `color_index::FOREGROUND`。门控块内：
  - 常量 `DESCRIPTION_TEXT_OPACITY_PERCENT = 80`：以设置页说明文字的白色观感为参考基准，
    但 80 是 OHOS 刻意采取的取值，与主题 `text_sub` 的 60 不同（属 OHOS 覆盖，不镜像主题）。
  - 用 `colors.primary.background.blend(&coloru_with_opacity(colors.primary.foreground, DESCRIPTION_TEXT_OPACITY_PERCENT))`
    把前景按 80% 叠到终端背景上，返回值 alpha 为不透明（背景不透明时 blend 结果即不透明）。
  - 当前默认主题下预合成结果为 `#CDCDCD`。
  - 块内以 `use` 引入 `Blend` 与 `coloru_with_opacity`，非 OHOS 平台不产生未使用 import。
  - 原有那一行 `self[color_index::FOREGROUND] = colors.primary.foreground;` **一行未动**。

### 备选方案与取舍

- **直接给前景 alpha 赋值 80%**：如上所述会被 `paint_line` 覆写，无效。排除。
- **改 `grid_renderer.rs` 不再覆写 alpha**：该行为同时服务于 ANSI 调色板、暗色字符等，
  改动共享渲染核心风险大、会波及其它平台。排除。
- **改主题里的 `primary.foreground`**：会连带影响配色表其它派生（`dim_foreground`、加粗前景等）与其它 UI，
  超出"只调暗终端正文"的范围。排除。

## 修改文件

- `app/src/terminal/color.rs` — 在 `List::fill_named` 的 `FOREGROUND` 赋值后插入 `#[cfg(target_env = "ohos")]` 门控块，就地把默认前景以说明文字白色观感为参考、用 OHOS 自定的 80%（非 `text_sub` 的 60）对着终端背景预合成后覆盖；已有赋值行未改。

## 门控与跨平台约束

- `color.rs` 的路径**不含 `ohos`**，属受保护文件。改动为「一个 `#[cfg(target_env = "ohos")]` 块」，
  已有代码一行未动；非 OHOS 平台编译期整块剔除，前景色完全不变。
- 调暗逻辑就地内联在门控块内，不新增 `ohos` 模块，`app/src/terminal/mod.rs` 无需改动。
- `grid_renderer.rs` 等共享渲染代码未改，其它平台的终端渲染不受影响。

## 验证

**目视对照**：在 OHOS 设备上打开 Warp，对照同一个设置页里的说明文字与终端正文的白色亮度，
两者观感应一致（终端正文应为约 `#CDCDCD` 的灰白，而非纯白）。

**代码侧自证**：
- 编译 OHOS 目标（debug）确认无警告，`color_index::FOREGROUND` 在 OHOS 下为预合成色。
- 非 OHOS 目标 `cargo check` 确认覆盖块被剔除，`FOREGROUND` 仍等于 `colors.primary.foreground`。
- 确认预合成结果的 alpha 为不透明，避免仍被 `paint_line` 的 alpha 覆写影响。
