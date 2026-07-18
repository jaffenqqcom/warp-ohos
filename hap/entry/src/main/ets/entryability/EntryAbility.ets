// EntryAbility — 鸿蒙 Ability 生命周期入口
// 直接继承 UIAbility，不经过 NativeAbility（避免基类初始化失败）
import { AbilityConstant, UIAbility, Want } from '@kit.AbilityKit';
import { hilog } from '@kit.PerformanceAnalysisKit';
import { window } from '@kit.ArkUI';

const DOMAIN: number = 0x0001;
const TAG: string = 'EntryAbility';

export default class EntryAbility extends UIAbility {
  async onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): Promise<void> {
    hilog.info(DOMAIN, TAG, 'Ability onCreate');
  }

  async onWindowStageCreate(windowStage: window.WindowStage): Promise<void> {
    try {
      await windowStage.loadContent('pages/Index');
      hilog.info(DOMAIN, TAG, 'Succeeded in loading the content.');
    } catch (err) {
      hilog.error(DOMAIN, TAG, 'Failed to load the content. Cause: %{public}s', JSON.stringify(err));
    }
  }
}
