use cfg_aliases::cfg_aliases;

fn main() {
    cfg_aliases! {
        macos: { target_os = "macos" },
        ohos: { target_env = "ohos" },
        noop: { not(any(macos, windows, ohos)) },
    }
}
