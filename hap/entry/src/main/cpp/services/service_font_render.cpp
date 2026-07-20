// §2.2.10 — OHOS 系统字体渲染封装
// dlopen 运行时加载 libnative_drawing.so，避免直接链接的命名空间限制。
// Rust 侧 dlsym("ohos_render_glyph_native") 调用，失败回退自有 rasterizer。
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <cstdio>
#include <dlfcn.h>

// OH_Drawing_RunBuffer 的精确结构（4 个指针字段）
typedef struct { uint16_t* glyphs; float* pos; char* txt; uint32_t* cls; } RunBuf;

// 函数指针类型
typedef void* (*F_MSC)(const void*, size_t, int);
typedef void  (*F_MSD)(void*);
typedef void* (*F_CTFS)(void*, int);
typedef void  (*F_DT)(void*);
typedef void* (*F_CF)();
typedef void  (*F_DF)(void*);
typedef void  (*F_FST)(void*, void*);
typedef void  (*F_FSTS)(void*, float);
typedef void  (*F_FSH)(void*, int);
typedef void  (*F_FSS)(void*, int);
typedef void* (*F_CB)();
typedef void  (*F_DB)(void*);
typedef void  (*F_BB)(void*, int, int, void*);
typedef void* (*F_BGP)(void*);
typedef void* (*F_CC)();
typedef void  (*F_DC)(void*);
typedef void  (*F_CBN)(void*, void*);
typedef void  (*F_CCL)(void*, uint32_t);
typedef void  (*F_CDT)(void*, void*, float, float);
typedef void* (*F_CTB)();
typedef void  (*F_DTB)(void*);
typedef const RunBuf* (*F_ARP)(void*, void*, int, void*);
typedef void* (*F_TBM)(void*);
typedef void  (*F_DT2)(void*);

static struct { int ok;
    F_MSC MSC; F_MSD MSD; F_CTFS CTFS; F_DT DT; F_CF CF; F_DF DF;
    F_FST FST; F_FSTS FSTS; F_FSH FSH; F_FSS FSS;
    F_CB CB; F_DB DB; F_BB BB; F_BGP BGP;
    F_CC CC; F_DC DC; F_CBN CBN; F_CCL CCL; F_CDT CDT;
    F_CTB CTB; F_DTB DTB; F_ARP ARP; F_TBM TBM; F_DT2 DT2;
} G = {0};

#define L(h, n) G.n = (F_##n)dlsym(h, "OH_Drawing_" #n)
static int init() {
    if (G.ok) return 1;
    void* h = dlopen("libnative_drawing.so", RTLD_NOW | RTLD_GLOBAL);
    if (!h) return 0;
    L(h, MSC); L(h, MSD); L(h, CTFS); L(h, DT);
    L(h, CF); L(h, DF); L(h, FST); L(h, FSTS);
    L(h, FSH); L(h, FSS); L(h, CB); L(h, DB);
    L(h, BB); L(h, BGP); L(h, CC); L(h, DC);
    L(h, CBN); L(h, CCL); L(h, CDT);
    L(h, CTB); L(h, DTB); L(h, ARP); L(h, TBM); L(h, DT2);
    G.ok = G.MSC && G.CTFS && G.CF && G.CB && G.CC && G.CTB && G.ARP && G.TBM && G.CDT && G.BGP;
    if (!G.ok) { dlclose(h); G.ok = 0; return 0; }
    return 1;
}

extern "C" {
int32_t ohos_render_glyph_native(
    const uint8_t* fd, int32_t fdl, uint32_t gid, float fs,
    uint8_t** out, int32_t* ow, int32_t* oh)
{
    *out = nullptr; *ow = 0; *oh = 0;
    if (!init()) return -1;

    auto* s = G.MSC(fd, fdl, 1);
    if (!s) return -1;
    auto* tf = G.CTFS(s, 0);
    if (!tf) { G.MSD(s); return -1; }
    auto* f = G.CF();
    G.FST(f, tf); G.FSTS(f, fs); G.FSH(f, 2); G.FSS(f, 1);

    int cs = (int)(fs * 2) + 32;
    auto* bm = G.CB(); G.BB(bm, cs, cs, nullptr);
    auto* cv = G.CC(); G.CBN(cv, bm); G.CCL(cv, 0);

    auto* bl = G.CTB();
    if (!bl) { G.DC(cv); G.DB(bm); G.DF(f); G.DT(tf); G.MSD(s); return -1; }
    auto* rb = G.ARP(bl, f, 1, nullptr);
    if (!rb || !rb->glyphs) { G.DTB(bl); G.DC(cv); G.DB(bm); G.DF(f); G.DT(tf); G.MSD(s); return -1; }
    const_cast<uint16_t*>(rb->glyphs)[0] = (uint16_t)gid;
    const_cast<float*>(rb->pos)[0] = 0;
    const_cast<float*>(rb->pos)[1] = fs * 0.9f;

    auto* tb = G.TBM(bl); G.DTB(bl);
    if (!tb) { G.DC(cv); G.DB(bm); G.DF(f); G.DT(tf); G.MSD(s); return -1; }
    G.CDT(cv, tb, 0, 0); G.DT2(tb);

    auto* px = G.BGP(bm);
    if (!px) { G.DC(cv); G.DB(bm); G.DF(f); G.DT(tf); G.MSD(s); return -1; }

    auto* src = (const uint8_t*)px;
    int t=cs, b=0, l=cs, r=0;
    for (int y = 0; y < cs; y++) for (int x = 0; x < cs; x++)
        if (src[(y * cs + x) * 4 + 3] > 0) { if (x < l) l=x; if (x > r) r=x; if (y < t) t=y; if (y > b) b=y; }
    int w = (r>l)?(r-l+1):0, h = (b>t)?(b-t+1):0;
    if (w<=0||h<=0) { G.DC(cv); G.DB(bm); G.DF(f); G.DT(tf); G.MSD(s); return 0; }

    auto* r2 = (uint8_t*)malloc((size_t)(w * h * 4));
    if (!r2) { G.DC(cv); G.DB(bm); G.DF(f); G.DT(tf); G.MSD(s); return -1; }
    for (int y = t; y <= b; y++)
        memcpy(r2 + (y-t)*w*4, src + (y*cs+l)*4, (size_t)(w*4));

    *out = r2; *ow = w; *oh = h;
    G.DC(cv); G.DB(bm); G.DF(f); G.DT(tf); G.MSD(s);
    return 0;
}
} // extern "C"

// ── FontMgr / FontDescriptor API（dlopen 加载 libnative_drawing.so） ──────────

// FontDescriptor 结构体（需与 NDK 定义一致）
typedef struct {
    char* path;
    char* postScriptName;
    char* fullName;
    char* fontFamily;
    char* fontSubfamily;
    int weight;
    int width;
    int italic;
    int monoSpace;
    int symbolic;
} FontDesc;

// FontMgr 函数指针
typedef void* (*F_FM_Create)();
typedef void  (*F_FM_Destroy)(void*);
typedef int   (*F_FM_GetFamilyCount)(void*);
typedef char* (*F_FM_GetFamilyName)(void*, int);
typedef void  (*F_FM_DestroyFamilyName)(char*);
typedef void* (*F_FM_MatchFamily)(void*, const char*);
typedef void* (*F_FM_MatchFamilyStyle)(void*, const char*, void*);
typedef void* (*F_FM_MatchFamilyStyleChar)(void*, const char*, void*, const char**, int, int);
typedef void* (*F_MFD)(void*, size_t*);
typedef void  (*F_DFD)(void*, size_t);
typedef void* (*F_CD)();
typedef void  (*F_DD)(void*);
typedef void* (*F_TS)(void*, int);
typedef int   (*F_TSC)(void*);
typedef void  (*F_TSStyle)(void*, int, int*, int*, void**);
typedef void* (*F_TSCreateType)(void*, int);
typedef void* (*F_FS)(int, int, int);

static struct {
    int ok;
    void* handle;
    F_FM_Create FM_Create;
    F_FM_Destroy FM_Destroy;
    F_FM_GetFamilyCount FM_GetFamilyCount;
    F_FM_GetFamilyName FM_GetFamilyName;
    F_FM_DestroyFamilyName FM_DestroyFamilyName;
    F_FM_MatchFamilyStyleChar FM_MatchFamilyStyleChar;
    F_MFD MFD;  // MatchFontDescriptors
    F_DFD DFD;  // DestroyFontDescriptors
    F_CD CD;    // CreateFontDescriptor
    F_DD DD;    // DestroyFontDescriptor
} FM = {0};

#define FM_LOAD(n) FM.n = (decltype(FM.n))dlsym(FM.handle, "OH_Drawing_" #n)
static int fm_init() {
    if (FM.ok) return 1;
    FM.handle = dlopen("libnative_drawing.so", RTLD_NOW);
    if (!FM.handle) return 0;
    FM_LOAD(FM_Create); FM_LOAD(FM_Destroy);
    FM_LOAD(FM_GetFamilyCount); FM_LOAD(FM_GetFamilyName);
    FM_LOAD(FM_DestroyFamilyName); FM_LOAD(FM_MatchFamilyStyleChar);
    FM_LOAD(MFD); FM_LOAD(DFD); FM_LOAD(CD); FM_LOAD(DD);
    FM.ok = FM.FM_Create && FM.FM_Destroy && FM.FM_GetFamilyCount
         && FM.FM_GetFamilyName && FM.FM_MatchFamilyStyleChar;
    if (!FM.ok) { dlclose(FM.handle); FM.handle = nullptr; return 0; }
    return 1;
}

// ── 导出函数 ───────────────────────────────────────────────────────────

extern "C" {

/// 获取系统字体描述符列表，返回 JSON 格式字符串（Rust 端解析）。
/// 格式: [{"path":"...","family":"...","mono":0|1},...]
/// 调用者 free() 返回值。
char* ohos_get_system_fonts() {
    if (!fm_init()) return nullptr;
    size_t num = 0;
    // 尝试 MatchFontDescriptors（API 18+）
    FontDesc* descs = nullptr;
    if (FM.MFD) {
        auto* tmpl = FM.CD ? FM.CD() : nullptr;
        descs = (FontDesc*)FM.MFD(tmpl, &num);
        if (tmpl) FM.DD(tmpl);
    }
    // 如果 MatchFontDescriptors 不可用，回退到 FontMgr 枚举
    if (!descs || num == 0) {
        auto* mgr = FM.FM_Create();
        if (!mgr) return nullptr;
        num = FM.FM_GetFamilyCount(mgr);
        // 构建 JSON 字符串（只有族名，没有路径）
        size_t buf_size = 256 * num + 32;
        char* buf = (char*)malloc(buf_size);
        if (!buf) { FM.FM_Destroy(mgr); return nullptr; }
        size_t pos = 0;
        pos += snprintf(buf + pos, buf_size - pos, "[");
        for (size_t i = 0; i < num && pos < buf_size - 8; i++) {
            char* name = FM.FM_GetFamilyName(mgr, (int)i);
            if (i > 0) buf[pos++] = ',';
            pos += snprintf(buf + pos, buf_size - pos,
                "{\"path\":\"\",\"family\":\"%s\",\"mono\":0}", name ? name : "");
            if (name) FM.FM_DestroyFamilyName(name);
        }
        pos += snprintf(buf + pos, buf_size - pos, "]");
        FM.FM_Destroy(mgr);
        return buf;
    }
    // MatchFontDescriptors 成功，构建 JSON
    size_t buf_size = 512 * num + 32;
    char* buf = (char*)malloc(buf_size);
    if (!buf) { FM.DFD(descs, num); return nullptr; }
    size_t pos = 0;
    pos += snprintf(buf + pos, buf_size - pos, "[");
    for (size_t i = 0; i < num && pos < buf_size - 16; i++) {
        auto& d = descs[i];
        if (i > 0) buf[pos++] = ',';
        pos += snprintf(buf + pos, buf_size - pos,
            "{\"path\":\"%s\",\"family\":\"%s\",\"mono\":%d}",
            d.path ? d.path : "",
            d.fontFamily ? d.fontFamily : "",
            d.monoSpace ? 1 : 0);
    }
    pos += snprintf(buf + pos, buf_size - pos, "]");
    FM.DFD(descs, num);
    return buf;
}

/// 检查指定字体族是否包含指定 Unicode 字符。
/// @param fontFamily 字体族名
/// @param codepoint Unicode 码点
/// @return 1 支持，0 不支持或出错
int ohos_font_has_glyph(const char* fontFamily, int codepoint) {
    auto* mgr = fm_init() ? FM.FM_Create() : nullptr;
    if (!mgr) return 0;
    auto style = (void*)0;  // default style
    auto* tf = FM.FM_MatchFamilyStyleChar(mgr, fontFamily, style, nullptr, 0, codepoint);
    int result = (tf != nullptr) ? 1 : 0;
    if (tf) { /* Typeface 没有 destroy API？用 FontMgrDestroy 时自动释放 */ }
    FM.FM_Destroy(mgr);
    return result;
}

} // extern "C"
