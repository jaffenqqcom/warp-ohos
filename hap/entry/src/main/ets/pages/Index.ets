import { DefaultXComponent } from '@ohos-rs/ability';
import warpModule from 'libwarp.so';
import common from '@ohos.app.ability.common';
import { hilog } from '@kit.PerformanceAnalysisKit';

const DOMAIN = 0x0001;
const TAG = 'Index';

// ArkUI MouseEvent action 值（ts-universal-mouse-key.md）
const MOUSE_PRESS = 1;
const MOUSE_RELEASE = 2;

// 触摸点数常量
const SINGLE_TOUCH = 1;
const TWO_FINGER = 2;

// 自定义触摸事件类型（无标准 ArkTS 枚举值）
const TOUCH_TYPE_TWO_FINGER_DOWN = -1;

// 按键动作
const KEY_ACTION_DOWN = 0;
const KEY_ACTION_UP = 1;

// 修饰键位掩码（与 winit OHOS backend 一致: bit0=Ctrl, bit1=Shift, bit2=Alt, bit4=CapsLock）
const MOD_CTRL = 0x01;
const MOD_SHIFT = 0x02;
const MOD_ALT = 0x04;
const MOD_CAPSLOCK = 0x10;

@Entry
@Component
struct Index {
  private lastTouchX: number = 0;
  private lastTouchY: number = 0;
  private mousePressed: boolean = false;
  // 当前触摸序列是否由鼠标按下引发。用于在 onTouch(Up) 中跳过
  // 向 NDK 发送 TouchEvent(Ended)，避免 convert_touch_ended 的
  // 长按检测误触右键菜单。
  private mouseWasActive: boolean = false;

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
      if (event.touches.length === SINGLE_TOUCH) {
        if (event.type === TouchType.Down) {
          this.lastTouchX = event.touches[0].x;
          this.lastTouchY = event.touches[0].y;
          // 纯触摸开始时重置 mouseWasActive（如果鼠标未按下）
          if (!this.mousePressed) {
            this.mouseWasActive = false;
          }
          warpModule.onTouchEvent(event.type, event.touches[0].x, event.touches[0].y);
        } else if (event.type === TouchType.Up) {
          // 由鼠标引起的触摸序列跳过 onTouchEvent(UP)，避免
          // convert_touch_ended 长按检测误触右键菜单。
          if (!this.mouseWasActive) {
            warpModule.onTouchEvent(event.type, event.touches[0].x, event.touches[0].y);
          }
        } else if (event.type === TouchType.Move && !this.mousePressed) {
          // 鼠标按下时不将触摸移动转为滚轮事件，避免干扰鼠标选词。
          // 纯净触摸（手指/触控板非按压状态）正常转换为滚轮滚动。
          let dx = event.touches[0].x - this.lastTouchX;
          let dy = event.touches[0].y - this.lastTouchY;
          this.lastTouchX = event.touches[0].x;
          this.lastTouchY = event.touches[0].y;
          warpModule.onWheelEvent(dx, dy);
        }
      } else if (event.touches.length === TWO_FINGER && event.type === TouchType.Down) {
        warpModule.onTouchEvent(TOUCH_TYPE_TWO_FINGER_DOWN, event.touches[0].x, event.touches[0].y);
      }
    })
    .onKeyEvent((event: KeyEvent) => {
      let modState = 0;
      if (event.getModifierKeyState!(['ctrl'])) modState |= MOD_CTRL;
      if (event.getModifierKeyState!(['shift'])) modState |= MOD_SHIFT;
      if (event.getModifierKeyState!(['alt'])) modState |= MOD_ALT;
      if (event.isCapsLockOn) modState |= MOD_CAPSLOCK;
      let keyAction = event.type === KeyType.Down ? KEY_ACTION_DOWN : KEY_ACTION_UP;
      warpModule.onKeyEvent(event.keyCode, keyAction, modState);
    })
    .onMouse((event: MouseEvent) => {
      if (event.action === MOUSE_PRESS) {
        this.mousePressed = true;
        this.mouseWasActive = true;
      }
      if (event.action === MOUSE_RELEASE) {
        this.mousePressed = false;
      }
      warpModule.onMouseEvent(event.action, event.button, event.x, event.y);
    })
  }
}
