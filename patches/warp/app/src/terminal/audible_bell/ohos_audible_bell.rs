// §15.3 / §3.11 — OHOS 终端响铃实现
//
// 使用 OH_AudioRenderer NDK API 播放 PCM 正弦波缓冲。
// OHOS 5.0+ (API 12+) 使用 builder 模式 + 回调驱动模型：
//   OH_AudioStreamBuilder_Create → Set* → SetWriteDataCallback → GenerateRenderer
//   → Start → (回调填充音频数据) → Stop → Release
//
// 不依赖外部声音文件，与各桌面平台行为一致（macOS NSBeep、Linux X11 bell）。
//
// ⚠️ ohaudio API 通过 dlopen/dlsym 在运行时动态加载，而非编译期 #[link]，
// 避免设备 libohaudio.so 与 NDK 版本不一致时整个 libwarp.so 加载失败。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{bail, Result};

// dlopen/dlsym FFI（不依赖 libc crate，直接声明）
extern "C" {
    fn dlopen(filename: *const i8, flags: i32) -> *mut std::ffi::c_void;
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
    fn dlclose(handle: *mut std::ffi::c_void) -> i32;
}
const RTLD_LAZY: i32 = 1;
const RTLD_LOCAL: i32 = 0;

// ── OH_AudioRenderer NDK API 类型 ────────────────────────────────────

type OHAudioRenderer = std::ffi::c_void;
type OHAudioStreamBuilder = std::ffi::c_void;
type OHAudioData = std::ffi::c_void;

type OHAudioRendererOnWriteData = unsafe extern "C" fn(
    *mut OHAudioRenderer, *mut std::ffi::c_void, *mut OHAudioData,
) -> i32;

#[repr(C)]
#[derive(Copy, Clone)]
struct OHAudioRendererCallbacks {
    on_write_data: Option<OHAudioRendererOnWriteData>,
}

#[repr(C)]
struct OHAudioDataInner {
    format: i32,
    buf: *mut u8,
    size: i32,
    offset: i32,
}

struct AudioWriteState {
    pcm_data: Vec<f32>,
    write_pos: usize,
    complete: Arc<AtomicBool>,
}

struct StateGuard(*mut AudioWriteState);
impl StateGuard {
    fn take(&mut self) -> *mut AudioWriteState {
        let ptr = self.0;
        self.0 = std::ptr::null_mut();
        ptr
    }
}
impl Drop for StateGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { drop(Box::from_raw(self.0)); }
        }
    }
}

// ── 音频参数常量 ──────────────────────────────────────────────────────

const STREAM_TYPE_RENDERER: i32 = 1;
const SAMPLE_RATE_48000: i32 = 48000;
const CHANNEL_COUNT_MONO: i32 = 1;
const SAMPLE_FORMAT_F32: i32 = 0;
const LATENCY_MODE_NORMAL: i32 = 0;
const CONTENT_TYPE_MUSIC: i32 = 2;
const STREAM_USAGE_MEDIA: i32 = 1;
const RENDERER_FLAGS_NONE: i32 = 0;

// ── 动态加载的 ohaudio API ────────────────────────────────────────────

/// 从 `*mut c_void` 到函数指针的转换（union 字段访问需 unsafe）。
union FnPtr<T: Copy> {
    ptr: *mut std::ffi::c_void,
    func: T,
}

struct AudioApi {
    #[allow(dead_code)]
    lib: *mut std::ffi::c_void,
    create: extern "C" fn(*mut *mut OHAudioStreamBuilder, i32) -> i32,
    destroy: extern "C" fn(*mut OHAudioStreamBuilder) -> i32,
    set_sampling_rate: extern "C" fn(*mut OHAudioStreamBuilder, i32) -> i32,
    set_channel_count: extern "C" fn(*mut OHAudioStreamBuilder, i32) -> i32,
    set_sample_format: extern "C" fn(*mut OHAudioStreamBuilder, i32) -> i32,
    set_latency_mode: extern "C" fn(*mut OHAudioStreamBuilder, i32) -> i32,
    set_renderer_info: extern "C" fn(*mut OHAudioStreamBuilder, i32, i32, i32) -> i32,
    set_write_data_callback: extern "C" fn(
        *mut OHAudioStreamBuilder, OHAudioRendererCallbacks, *mut std::ffi::c_void,
    ) -> i32,
    generate_renderer: extern "C" fn(*mut OHAudioStreamBuilder, *mut *mut OHAudioRenderer) -> i32,
    start: extern "C" fn(*mut OHAudioRenderer) -> i32,
    stop: extern "C" fn(*mut OHAudioRenderer) -> i32,
    release: extern "C" fn(*mut OHAudioRenderer) -> i32,
}

unsafe impl Send for AudioApi {}
unsafe impl Sync for AudioApi {}

static AUDIO_API: OnceLock<Result<AudioApi, ()>> = OnceLock::new();

fn audio_api() -> Option<&'static AudioApi> {
    let result = AUDIO_API.get_or_init(|| {
        let lib = unsafe { dlopen(c"libohaudio.so".as_ptr() as *const i8, RTLD_LAZY | RTLD_LOCAL) };
        if lib.is_null() {
            log::warn!("ohos_audible_bell: libohaudio.so not found, audible bell disabled");
            return Err(());
        }

        macro_rules! load_fn {
            ($n:literal) => {{
                let s = std::ffi::CString::new($n).unwrap();
                let p = unsafe { dlsym(lib, s.as_ptr() as *const i8) };
                if p.is_null() {
                    log::warn!("ohos_audible_bell: {}. not found", $n);
                    unsafe { dlclose(lib) };
                    return Err(());
                }
                p
            }};
        }

        macro_rules! fn_init {
            ($n:literal) => { unsafe { FnPtr { ptr: load_fn!($n) }.func } };
        }

        Ok(AudioApi {
            lib,
            create: fn_init!("OH_AudioStreamBuilder_Create"),
            destroy: fn_init!("OH_AudioStreamBuilder_Destroy"),
            set_sampling_rate: fn_init!("OH_AudioStreamBuilder_SetSamplingRate"),
            set_channel_count: fn_init!("OH_AudioStreamBuilder_SetChannelCount"),
            set_sample_format: fn_init!("OH_AudioStreamBuilder_SetSampleFormat"),
            set_latency_mode: fn_init!("OH_AudioStreamBuilder_SetLatencyMode"),
            set_renderer_info: fn_init!("OH_AudioStreamBuilder_SetRendererInfo"),
            set_write_data_callback: fn_init!("OH_AudioStreamBuilder_SetWriteDataCallback"),
            generate_renderer: fn_init!("OH_AudioStreamBuilder_GenerateRenderer"),
            start: fn_init!("OH_AudioRenderer_Start"),
            stop: fn_init!("OH_AudioRenderer_Stop"),
            release: fn_init!("OH_AudioRenderer_Release"),
        })
    });

    result.as_ref().ok()
}

// ── AudibleBell ──────────────────────────────────────────────────────

pub(super) struct AudibleBell;

impl AudibleBell {
    pub fn new() -> Self {
        Self
    }

    pub fn ring(&self) -> Result<()> {
        log::debug!("ohos_audible_bell::ring: attempting playback");
        let api = audio_api().ok_or_else(|| {
            log::warn!("ohos_audible_bell: ohaudio API not available");
            anyhow::anyhow!("ohos_audible_bell: ohaudio API not available")
        })?;

        let sample_rate = 48000;
        let duration_sec = 0.1;
        let frequency = 1000.0;
        let num_samples = (sample_rate as f64 * duration_sec) as usize;

        let pcm_buffer: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f64 / sample_rate as f64;
                let envelope = if t < 0.010 { t / 0.010 }
                    else if t > duration_sec - 0.020 { (duration_sec - t) / 0.020 }
                    else { 1.0 };
                (2.0 * std::f64::consts::PI * frequency * t).sin() as f32 * envelope as f32 * 0.5
            })
            .collect();

        let complete = Arc::new(AtomicBool::new(false));
        let state = Box::into_raw(Box::new(AudioWriteState {
            pcm_data: pcm_buffer,
            write_pos: 0,
            complete: complete.clone(),
        }));
        let mut state_guard = StateGuard(state);

        unsafe extern "C" fn on_audio_write_data(
            _renderer: *mut OHAudioRenderer,
            user_data: *mut std::ffi::c_void,
            audio_data: *mut OHAudioData,
        ) -> i32 {
            if user_data.is_null() || audio_data.is_null() {
                return 0;
            }
            let data = &*(audio_data as *const OHAudioDataInner);
            let state = &mut *(user_data as *mut AudioWriteState);

            let dst_slice = std::slice::from_raw_parts_mut(data.buf as *mut f32, data.size as usize / 4);
            let remaining = state.pcm_data.len() - state.write_pos;
            let samples_to_copy = dst_slice.len().min(remaining);

            if samples_to_copy > 0 {
                dst_slice[..samples_to_copy].copy_from_slice(
                    &state.pcm_data[state.write_pos..state.write_pos + samples_to_copy],
                );
                state.write_pos += samples_to_copy;
            }
            if samples_to_copy < dst_slice.len() {
                dst_slice[samples_to_copy..].fill(0.0f32);
            }
            if state.write_pos >= state.pcm_data.len() {
                state.complete.store(true, Ordering::SeqCst);
            }
            0
        }

        let callbacks = OHAudioRendererCallbacks {
            on_write_data: Some(on_audio_write_data),
        };

        unsafe {
            let mut builder: *mut OHAudioStreamBuilder = std::ptr::null_mut();
            let mut rc = (api.create)(&mut builder, STREAM_TYPE_RENDERER);
            if rc != 0 || builder.is_null() {
                let _ = Box::from_raw(state_guard.take());
                bail!("OH_AudioStreamBuilder_Create failed: {rc}");
            }

            rc = (api.set_sampling_rate)(builder, SAMPLE_RATE_48000);
            rc |= (api.set_channel_count)(builder, CHANNEL_COUNT_MONO);
            rc |= (api.set_sample_format)(builder, SAMPLE_FORMAT_F32);
            rc |= (api.set_latency_mode)(builder, LATENCY_MODE_NORMAL);
            rc |= (api.set_renderer_info)(builder, CONTENT_TYPE_MUSIC, STREAM_USAGE_MEDIA, RENDERER_FLAGS_NONE);
            if rc != 0 {
                (api.destroy)(builder);
                let _ = Box::from_raw(state_guard.take());
                bail!("OH_AudioStreamBuilder_Set* failed: {rc}");
            }

            rc = (api.set_write_data_callback)(builder, callbacks, state as *mut std::ffi::c_void);
            if rc != 0 {
                (api.destroy)(builder);
                let _ = Box::from_raw(state_guard.take());
                bail!("OH_AudioStreamBuilder_SetWriteDataCallback failed: {rc}");
            }

            let mut renderer: *mut OHAudioRenderer = std::ptr::null_mut();
            rc = (api.generate_renderer)(builder, &mut renderer);
            (api.destroy)(builder);
            if rc != 0 || renderer.is_null() {
                let _ = Box::from_raw(state_guard.take());
                bail!("OH_AudioStreamBuilder_GenerateRenderer failed: {rc}");
            }

            rc = (api.start)(renderer);
            if rc != 0 {
                let _ = Box::from_raw(state_guard.take());
                (api.release)(renderer);
                bail!("OH_AudioRenderer_Start failed: {rc}");
            }

            let start = std::time::Instant::now();
            while !complete.load(Ordering::SeqCst) {
                if start.elapsed() > std::time::Duration::from_secs(1) {
                    log::warn!("AudibleBell: audio playback timed out, stopping");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            (api.stop)(renderer);
            let _ = Box::from_raw(state_guard.take());
            (api.release)(renderer);
        }
        Ok(())
    }
}
