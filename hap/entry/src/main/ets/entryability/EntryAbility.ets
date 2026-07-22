// EntryAbility：鸿蒙 Ability 生命周期入口
//
// 继承 NativeAbility（来自 @ohos-rs/ability），由基类自动处理生命周期事件路由。
// moduleName 设为 'warp'（对应 libwarp.so 的 NAPI 模块名）。
// defaultPage 设为 false（基类默认 true），避免 NativeAbility.onWindowStageCreate
// 尝试 loadContentByName('NativeAbility') 加载 MainPage 失败。

import { NativeAbility } from '@ohos-rs/ability';
import { AbilityConstant, ConfigurationConstant, Want, UIAbility } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';

const DOMAIN = 0x0001;
const TAG = 'EntryAbility';

export default class EntryAbility extends NativeAbility {
  // libwarp.so 已修复符号加载问题（DT_NEEDED + RPATH 解析 libnative_window.so），
  // 可由 MMG 直接加载，不再需要 libentry.so 的 dlopen 方案
  moduleName: string = 'warp';
  defaultPage: boolean = false;

  async onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): Promise<void> {
    await super.onCreate(want, launchParam);
    try {
      // 类型断言到 UIAbility 绕过跨 HAR 类型链解析
      (this as UIAbility).context.getApplicationContext()
        .setColorMode(ConfigurationConstant.ColorMode.COLOR_MODE_NOT_SET);
    } catch (err) {
      hilog.error(DOMAIN, TAG, 'Failed to set colorMode. Cause: %{public}s', JSON.stringify(err));
    }
    hilog.info(DOMAIN, TAG, 'Ability onCreate');
  }

  async onWindowStageCreate(windowStage: window.WindowStage): Promise<void> {
    await super.onWindowStageCreate(windowStage);
    try {
      await windowStage.loadContent('pages/Index');
      hilog.info(DOMAIN, TAG, 'Succeeded in loading the content.');
    } catch (err) {
      hilog.error(DOMAIN, TAG, 'Failed to load the content. Cause: %{public}s', JSON.stringify(err));
    }
  }
}
