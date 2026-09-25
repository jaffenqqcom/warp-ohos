import 'libcore.so';
import { BridgePlugin, LazyPlugin, NativeAbility } from '@ohos-rs/ability';
import { CursorPlugin } from '@ohos-rs/ability-plugin-cursor';
import { FilePickerPlugin } from '@ohos-rs/ability-plugin-filepicker';
import { OpenBySysPlugin } from '@ohos-rs/ability-plugin-openbysys';
import { PinchPlugin } from '@ohos-rs/ability-plugin-pinch';
import { FileDropInPlugin } from '@ohos-rs/ability-plugin-filedropin';
import { FileLaunchPlugin } from '@ohos-rs/ability-plugin-filelaunch';
import { ImePlugin } from '@ohos-rs/ability-plugin-ime';
import { UrlPlugin } from '@ohos-rs/ability-plugin-url';
import { Want, AbilityConstant, ConfigurationConstant } from '@kit.AbilityKit';
import hilog from '@ohos.hilog';

// EntryAbility：鸿蒙 Ability 生命周期入口。
// moduleName 与 native 库名一致（libcore.so），NativeAbility 基类负责把窗口/surface/IME/
// 生命周期事件经 NAPI 路由到 Rust 侧（crates/entry_ohos 的 #[ability] launch_app 入口）。
// loadMode='sync'：同步加载 native 模块，避免默认页动态 import 导致黑屏（无错误日志）。
// bridgePlugins：注册 ArkTS 侧能力插件，与 Rust 侧 app.register_plugin 声明一一对应。
// 单一共享的 fileLaunch 实例：onCreate/onNewWant 触发后需引用它，且插件安装前可能先收到 want。
//
// 数据根目录由基类交给 native 侧：`createInitContext` 用 `context.filesDir` 同时充当
// basePath 与 prefPath。上游 ability 已从 `AbilityInitContext` 移除 `home_directory`，
// ArkTS 无从把用户挑的目录交给 Rust，故本文件不再有选目录流程（见 12.2 第 7 步）。
const fileLaunch = new FileLaunchPlugin();

const DOMAIN: number = 0x0001;
const TAG: string = 'EntryAbility';

export default class EntryAbility extends NativeAbility {
  public moduleName: string = 'core';
  public loadMode: 'sync' | 'async' = 'sync';
  public bridgePlugins: LazyPlugin[] = [
    new LazyPlugin((): BridgePlugin => new CursorPlugin()),
    new LazyPlugin((): BridgePlugin => new FilePickerPlugin()),
    new LazyPlugin((): BridgePlugin => new OpenBySysPlugin()),
    new LazyPlugin((): BridgePlugin => new PinchPlugin()),
    new LazyPlugin((): BridgePlugin => new FileDropInPlugin()),
    new LazyPlugin((): BridgePlugin => fileLaunch),
    new LazyPlugin((): BridgePlugin => new ImePlugin()),
    new LazyPlugin((): BridgePlugin => new UrlPlugin()),
  ];

  // 冷启动：系统带 want（含要打开的文件 URI）启动。先让基类完成会话初始化，再把 want
  // 交给 fileLaunch 处理（若插件尚未 onInstall，会在内部暂存）。
  onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): void {
    super.onCreate(want, launchParam);
    // Declare that the app follows the system dark/light mode. Without this the configuration
    // forwarded to Rust only ever reports "not set", so the theme never tracks the system.
    try {
      this.context.setColorMode(ConfigurationConstant.ColorMode.COLOR_MODE_NOT_SET);
    } catch (error) {
      hilog.error(DOMAIN, TAG, 'setColorMode(COLOR_MODE_NOT_SET) failed: %{public}s', String(error));
    }
    fileLaunch.openFromWant(want);
  }

  // 已运行时再次收到打开请求（如文件管理器“打开方式”选 hiwarp）。
  onNewWant(want: Want, launchParam: AbilityConstant.LaunchParam): void {
    super.onNewWant(want, launchParam);
    fileLaunch.openFromWant(want);
  }
}
