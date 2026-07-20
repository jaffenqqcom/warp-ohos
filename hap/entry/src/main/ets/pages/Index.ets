import { DefaultXComponent } from '@ohos-rs/ability';
import warpModule from 'libwarp.so';
import common from '@ohos.app.ability.common';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0001;
const TAG = 'Index';

@Entry
@Component
struct Index {
  private lastTouchX: number = 0;
  private lastTouchY: number = 0;

  async aboutToAppear(): Promise<void> {
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      // 传 AbilityContext 给 C++ NAPI 侧存为 napi_ref，供 openUrl 使用
      warpModule.setAbilityContext(context);
      warpModule.registerServiceBridge();
      hilog.info(DOMAIN, TAG, 'aboutToAppear: calling startWarpMain');
      warpModule.startWarpMain(
        context.filesDir,
        context.cacheDir,
        context.tempDir,
        context.filesDir
      );
      hilog.info(DOMAIN, TAG, 'startWarpMain returned successfully');
    } catch (err) {
      hilog.error(DOMAIN, TAG, 'aboutToAppear 失败');
    }
  }

  // 拦截系统返回键（2in1 设备上 Escape 被映射为返回），阻止窗口最小化。
  // Escape 键仍会通过 onKeyEvent 转发到 Rust 侧。
  onBackPress(): boolean {
    return true;
  }

  build() {
    Stack() {
      DefaultXComponent({ moduleName: 'warp' })
    }
    .width('100%')
    .height('100%')
    .focusable(true)
    .defaultFocus(true)
    .onTouch((event: TouchEvent) => {
      if (event.touches.length === 1) {
        if (event.type === TouchType.Down) {
          this.lastTouchX = event.touches[0].x;
          this.lastTouchY = event.touches[0].y;
          warpModule.onTouchEvent(event.type, event.touches[0].x, event.touches[0].y);
        } else if (event.type === TouchType.Up) {
          warpModule.onTouchEvent(event.type, event.touches[0].x, event.touches[0].y);
        } else if (event.type === TouchType.Move) {
          let dx = event.touches[0].x - this.lastTouchX;
          let dy = event.touches[0].y - this.lastTouchY;
          this.lastTouchX = event.touches[0].x;
          this.lastTouchY = event.touches[0].y;
          warpModule.onWheelEvent(dx, dy);
        }
      } else if (event.touches.length === 2 && event.type === TouchType.Down) {
        warpModule.onTouchEvent(-1, event.touches[0].x, event.touches[0].y);
      }
    })
    .onKeyEvent((event: KeyEvent) => {
      let modState = 0;
      if (event.getModifierKeyState!(['ctrl'])) modState |= 0x01;
      if (event.getModifierKeyState!(['shift'])) modState |= 0x02;
      if (event.getModifierKeyState!(['alt'])) modState |= 0x04;
      if (event.isCapsLockOn) modState |= 0x10;
      warpModule.onKeyEvent(event.keyCode, event.type === KeyType.Down ? 0 : 1, modState);
    })
    .onMouse((event: MouseEvent) => {
      warpModule.onMouseEvent(event.action, event.button, event.x, event.y);
    })
  }
}
