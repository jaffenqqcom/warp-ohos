use openharmony_ability_derive::ability;
use openharmony_ability_plugin_cursor::CursorBridgePlugin;
use openharmony_ability_plugin_filedropin::FileDropInBridgePlugin;
use openharmony_ability_plugin_filepicker::FilePickerBridgePlugin;
use openharmony_ability_plugin_ime::ImeBridgePlugin;
use openharmony_ability_plugin_openbysys::OpenBySysBridgePlugin;
use openharmony_ability_plugin_url::UrlBridgePlugin;
use warp_core::AppId;
use warp_core::channel::{Channel, ChannelConfig, ChannelState, OzConfig, WarpServerConfig};

/// Directory a private HNP is installed into (`hnpPackages` entries with type
/// `private`), and the only place the app may exec local tools from.
const HNP_PRIVATE_BIN_DIR: &str = "/data/app/bin";

/// Terminal child shell: the `hitshell` bridge from the private `hitshell.hnp`.
/// It either hands the session to `hitdaemon`, which runs outside the sandbox
/// and therefore with the system's own permissions, or replaces itself with the
/// bundled zsh when `hitdaemon` is not running.
const TERMINAL_SHELL_PATH: &str = "/data/app/bin/hitshell";

/// The bundled zsh, in its private `zsh.hnp` install (see the porting analysis,
/// section 11.9). The terminal is no longer started with it directly, but its
/// package is where the terminfo database lives, so the terminfo lookup below
/// resolves from this path rather than from [`TERMINAL_SHELL_PATH`].
const ZSH_SHELL_PATH: &str = "/data/app/bin/zsh";

/// Environment variable warp reads to pin the terminal child shell. Without it
/// warp falls back to its hardcoded `/bin/zsh` -> `/bin/bash` -> `/bin/fish`
/// discovery, none of which exist under the OHOS sandbox.
const WARP_SHELL_PATH_ENV: &str = "WARP_SHELL_PATH";

/// Directory the terminal shell's temporary files are rooted in, as the
/// application sees it.
///
/// zsh resolves here-documents through `$TMPPREFIX`, whose documented default
/// is `/tmp/zsh`, and warp's bootstrap script opens one. The compiled-in
/// default names a path the sandbox does not have, so zsh aborted the whole
/// bootstrap with "can't create temp file for here-document: no such file or
/// directory". The sandbox's cache directory is writable and meant for exactly
/// this kind of transient data.
const SANDBOX_CACHE_DIR: &str = "/data/storage/el2/base/cache";

/// File-name prefix used below [`SANDBOX_CACHE_DIR`] for `$TMPPREFIX`. zsh
/// appends its own random suffix, and the directory part has to exist.
const SANDBOX_TMP_PREFIX_NAME: &str = "zsh";

/// Environment variable the terminal shell reads to locate its terminfo
/// database.
const TERMINFO_ENV: &str = "TERMINFO";

/// Terminfo database location inside an HNP install, relative to the root that
/// also holds the shell's `bin` directory.
const TERMINFO_SUBDIR: &str = "share/terminfo";

/// NAPI entry the HAP calls to start Warp.
#[ability]
pub fn launch_app(app: openharmony_ability::OpenHarmonyApp) {
    // `warp::run()` installs the global logger, so `log::` records emitted
    // before that point are dropped and hilog only shows them through
    // `direct_hilog`. Both calls are kept deliberately: a device log that shows
    // the direct line but not the `log::` one confirms the redirect is not live
    // yet, and a log with neither says the hilog link itself is broken.
    log::info!(
        "launch_app: entry, base_path={:?}, pref_path={:?}",
        app.base_path(),
        app.pref_path()
    );
    warp_logging::direct_hilog(&format!(
        "launch_app: entry, base_path={:?}, pref_path={:?}",
        app.base_path(),
        app.pref_path()
    ));
    // Register the bridge plugins before the window starts rendering: the ArkTS
    // host installs a plugin factory only when the native module declares the
    // matching plugin ID, so this is what makes these bridges exist at all.
    register_bridge_plugins(&app);
    // Hand the app to the platform layer: it registers the ability's callback
    // handler and then starts warp's blocking event loop on a dedicated thread,
    // because this thread must stay free to service platform callbacks.
    warpui::platform::ohos::set_global_app(app.clone());
    // Two slots have to be filled, because the bridge plugins read a different
    // one than the platform layer. `plugin-ime` turns every keystroke into an
    // `InputEvent` through `openharmony_ability::global_app()`, and its slot is
    // distinct from warpui's even though both are thread-local on this same
    // thread. Filling only warpui's leaves the plugin's slot empty, and each
    // keystroke is then dropped with "global OpenHarmonyApp unavailable".
    openharmony_ability::set_global_app(app.clone());
    warp_logging::direct_hilog(
        "launch_app: the app is stored in the warpui and openharmony-ability slots",
    );
    let spawn_result = warpui::platform::ohos::spawn(move || {
        prepare_process_environment(&app);
        install_channel_state();
        match warp::run() {
            Ok(()) => log::info!("launch_app: warp::run returned normally"),
            // The alternate form prints the whole context chain, not just the
            // outermost message: the terminal server's spawn failure is wrapped
            // several layers deep, and the outer message alone does not say why.
            Err(err) => log::error!("launch_app: warp::run failed: {err:#}"),
        }
    });
    if let Err(err) = spawn_result {
        log::error!("launch_app: failed to start warp: {err:#}");
    }
}

/// Registers the ArkTS-facing bridge plugin facades this application owns.
///
/// The ArkTS host's `configurePlugins` walks the declarations the native module
/// publishes and installs only the factories it finds a declaration for, so a
/// plugin never registered here is never installed, and every call through it
/// fails with "Bridge plugin '<id>' is not installed". These keep the ArkTS
/// `bridgePlugins` list in `EntryAbility` and this module in step.
fn register_bridge_plugins(app: &openharmony_ability::OpenHarmonyApp) {
    register_bridge_plugin(app, CursorBridgePlugin);
    register_bridge_plugin(app, FileDropInBridgePlugin);
    register_bridge_plugin(app, FilePickerBridgePlugin);
    register_bridge_plugin(app, ImeBridgePlugin);
    register_bridge_plugin(app, OpenBySysBridgePlugin);
    register_bridge_plugin(app, UrlBridgePlugin);
}

/// Registers one facade, reporting the outcome through both the logger and
/// hilog. The `log::` records are dropped until `warp::run()` installs the
/// logger, and registration happens before that, so the direct hilog line is the
/// one that actually reaches the device log.
fn register_bridge_plugin<P: openharmony_ability::BridgePlugin>(
    app: &openharmony_ability::OpenHarmonyApp,
    plugin: P,
) {
    match app.register_plugin(plugin) {
        Ok(()) => {
            log::info!("register_bridge_plugin: {} is registered", P::ID);
            warp_logging::direct_hilog(&format!(
                "register_bridge_plugin: {} is registered",
                P::ID
            ));
        }
        Err(err) => {
            log::error!("register_bridge_plugin: registering {} failed: {err}", P::ID);
            warp_logging::direct_hilog(&format!(
                "register_bridge_plugin: registering {} failed: {err}",
                P::ID
            ));
        }
    }
}

/// Pins the process environment that warp's terminal shell discovery and the
/// local HNP tools read.
fn prepare_process_environment(app: &openharmony_ability::OpenHarmonyApp) {
    let base_path = app.base_path().filter(|path| !path.is_empty());

    // Edition 2024 makes `set_var` unsafe: mutating the process environment is
    // only sound while no other thread reads it. Every call below happens on the
    // launch thread before `warp::run()` spawns anything.
    unsafe {
        std::env::set_var(WARP_SHELL_PATH_ENV, TERMINAL_SHELL_PATH);
    }
    log::info!("prepare_process_environment: {WARP_SHELL_PATH_ENV}={TERMINAL_SHELL_PATH}");

    // `openharmony-ability` no longer carries a user-chosen home directory: it
    // was removed from `AbilityInitContext` upstream, so the ArkTS host has no
    // way to hand one over. The sandbox files directory reported as `basePath`
    // is the only root left, and stands in for `HOME`.
    match base_path {
        Some(home) => {
            unsafe {
                std::env::set_var("HOME", &home);
            }
            log::info!("prepare_process_environment: HOME={home}");
        }
        None => log::warn!("prepare_process_environment: no base path to use as HOME"),
    }

    // `TMPDIR` alone does not move zsh's scratch files: appspawn already points
    // it at the sandbox cache and the here-document still failed, because zsh
    // resolves them through `$TMPPREFIX`, whose compiled-in default is
    // `/tmp/zsh` -- and the sandbox has no `/tmp`. Pin the prefix explicitly;
    // see `SANDBOX_CACHE_DIR`.
    let inherited_tmp_prefix = std::env::var("TMPPREFIX").ok();
    let tmp_prefix = std::path::Path::new(SANDBOX_CACHE_DIR).join(SANDBOX_TMP_PREFIX_NAME);
    match std::fs::create_dir_all(SANDBOX_CACHE_DIR) {
        Ok(()) => unsafe {
            std::env::set_var("TMPPREFIX", &tmp_prefix);
        },
        Err(err) => warp_logging::direct_hilog(&format!(
            "prepare_process_environment: could not create {SANDBOX_CACHE_DIR}: {err}"
        )),
    }
    warp_logging::direct_hilog(&format!(
        "prepare_process_environment: TMPPREFIX={} (inherited {inherited_tmp_prefix:?})",
        tmp_prefix.display()
    ));

    append_path_entry(HNP_PRIVATE_BIN_DIR);
    point_shell_at_bundled_terminfo();

    // Everything above runs before `warp::run()` installs the logger, so the
    // resolved environment is reported through hilog directly.
    // The terminal server re-executes this process, so the path that spawn has
    // to reach is worth reporting next to the environment: when the spawn
    // fails, this says whether the target was resolvable at all.
    warp_logging::direct_hilog(&format!(
        "prepare_process_environment: ready, {WARP_SHELL_PATH_ENV}={:?}, HOME={:?}, PATH={:?}, \
         exe={:?}",
        std::env::var(WARP_SHELL_PATH_ENV),
        std::env::var("HOME"),
        std::env::var("PATH"),
        std::env::current_exe()
    ));
}

/// Points the terminal shell at the terminfo database installed beside it.
///
/// The shell resolves its terminal description through `$TERMINFO` first. The
/// HNP build records its own build-host directory in that search path
/// (`/tmp/zshbuild/hnp/zsh/share/terminfo`), which does not exist on device,
/// and the system database under `/usr/share/terminfo` is outside the
/// sandbox's read scope. Without a description the line editor loses cursor
/// addressing and appends each redraw instead of overwriting it, so a
/// submitted command is echoed twice.
fn point_shell_at_bundled_terminfo() {
    let terminfo_dir = std::fs::canonicalize(ZSH_SHELL_PATH)
        .ok()
        .and_then(|shell| {
            shell
                .parent()
                .and_then(std::path::Path::parent)
                .map(|root| root.join(TERMINFO_SUBDIR))
        });
    match terminfo_dir {
        Some(dir) if dir.is_dir() => {
            unsafe {
                std::env::set_var(TERMINFO_ENV, &dir);
            }
            warp_logging::direct_hilog(&format!(
                "prepare_process_environment: {TERMINFO_ENV}={}",
                dir.display()
            ));
        }
        Some(dir) => warp_logging::direct_hilog(&format!(
            "prepare_process_environment: no terminfo database at {}, leaving {TERMINFO_ENV} unset",
            dir.display()
        )),
        None => warp_logging::direct_hilog(&format!(
            "prepare_process_environment: could not resolve {ZSH_SHELL_PATH} for terminfo"
        )),
    }
}

/// Appends `entry` to `PATH` unless it is already present, so the HNP tools
/// (`git`, `zsh`) resolve from a terminal shell.
fn append_path_entry(entry: &str) {
    let current = std::env::var("PATH").unwrap_or_default();
    if current.split(':').any(|existing| existing == entry) {
        log::info!("append_path_entry: {entry} already on PATH");
        return;
    }
    let updated = if current.is_empty() {
        entry.to_string()
    } else {
        format!("{current}:{entry}")
    };
    unsafe {
        std::env::set_var("PATH", &updated);
    }
    log::info!("append_path_entry: PATH={updated}");
}

/// Installs the OSS channel configuration that `warp::run()` requires to be set
/// before it starts.
fn install_channel_state() {
    let mut state = ChannelState::new(
        Channel::Oss,
        ChannelConfig {
            app_id: AppId::new("dev", "warp", "WarpOss"),
            logfile_name: "warp-oss.log".into(),
            server_config: WarpServerConfig::production(),
            oz_config: OzConfig::production(),
            telemetry_config: None,
            crash_reporting_config: None,
            autoupdate_config: None,
            mcp_static_config: None,
        },
    );
    if cfg!(debug_assertions) {
        state = state.with_additional_features(warp_core::features::DEBUG_FLAGS);
    }
    ChannelState::set(state);
    log::info!("install_channel_state: OSS channel state installed");
}
