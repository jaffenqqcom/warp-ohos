//! The OHOS event loop.
//!
//! HarmonyOS reports input, lifecycle and vsync callbacks on the ArkTS (UI)
//! thread, and the XComponent surface belongs to that thread. Warp's event loop
//! must not run there: [`crate::platform::ohos::App::run`] blocks until the
//! application terminates, so running it on the UI thread would freeze the
//! ability. The two are therefore split:
//!
//! * [`register`] installs the `openharmony-ability` run-loop handler on the UI
//!   thread. The handler only translates one callback into an owned [`AppEvent`]
//!   and pushes it onto the queue, so it never blocks.
//! * A dedicated `warp-main` thread drains that queue and runs warp's loop,
//!   including all rendering.

use std::cell::Cell;
use std::collections::HashSet;
use std::mem::ManuallyDrop;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SendError};
use std::time::{Duration, Instant};

use openharmony_ability::ime::KeyboardStatus;
use openharmony_ability::xcomponent::{
    Action, KeyCode, KeyEventData, RawWindow, TouchEvent, TouchEventData,
};
use openharmony_ability_plugin_filedropin::{FileDropEventData, set_filedropin_callback};
use openharmony_ability::{
    AxisEventData, AxisToolType, ColorMode, DeviceModifiers, Event as PlatformEvent, ImeEvent,
    InputEvent, MouseAction, MouseButton, MouseEventData, OpenHarmonyApp,
};

use super::keycodes::{self, Modifiers};
use super::windowing::PENDING_REDRAW;
use crate::event::{Event as WindowEvent, KeyEventDetails, KeyState, ModifiersState};
use crate::geometry::vector::Vector2F;
use crate::keymap::Keystroke;
use crate::platform::app::{
    AppCallbackDispatcher, ApproveTerminateResult, TerminationRequestSource, TerminationResult,
};
use crate::platform::{self, TerminationMode};
use crate::{AppContext, WindowId};

/// Set once a frame has reached the renderer. Frames are otherwise only
/// reported at `debug` level, which the logger filters out on-device, so this
/// makes "is anything being drawn at all" answerable from a hilog capture.
static FIRST_FRAME_REPORTED: AtomicBool = AtomicBool::new(false);

/// Whether the ability's window currently holds focus.
///
/// The platform reports focus through several overlapping callbacks, and each
/// keyboard request crosses to ArkTS and blocks until it answers, so the edge is
/// tracked to ask for the keyboard only when focus is actually gained.
static WINDOW_FOCUSED: AtomicBool = AtomicBool::new(false);

/// A repeat press of the same button within this window counts as one more
/// click. The value matches the winit back-end so that double-click behaviour
/// (selecting a word) feels the same on both.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);

/// One mouse-wheel notch, in ArkUI axis units.
const AXIS_WHEEL_UNIT: f64 = 120.0;

/// Lines scrolled per wheel notch, matching the winit back-end's line delta.
const WHEEL_LINES_PER_NOTCH: f64 = 3.0;

/// Bit positions of `ArkUI_ModifierKeyName`, carried by mouse events.
const MODIFIER_CTRL: u64 = 1 << 0;
const MODIFIER_SHIFT: u64 = 1 << 1;
const MODIFIER_ALT: u64 = 1 << 2;

/// Button bits of `MouseEventData::button_mask`.
const MOUSE_BUTTON_LEFT: u32 = 0x01;
const MOUSE_BUTTON_RIGHT: u32 = 0x02;
const MOUSE_BUTTON_MIDDLE: u32 = 0x04;

/// Movement past this many logical pixels turns a touch into a scroll or a
/// drag rather than a tap, matching the winit back-end.
const TOUCH_SLOP: f32 = 18.0;

/// A touch held at least this long becomes a right click.
const LONG_PRESS_DURATION: Duration = Duration::from_millis(500);

/// A platform callback translated into owned data, so that it can cross the
/// thread boundary between the ability thread and the warp main thread.
pub(super) enum AppEvent {
    /// Run the wrapped task on the main thread.
    RunTask(ManuallyDrop<async_task::Runnable>),
    /// Run a synchronous callback on the main thread.
    RunCallback(Box<dyn FnOnce(&mut AppContext) + Send + Sync>),
    /// Close a window.
    CloseWindow(WindowId),
    /// Active window changed.
    ActiveWindowChanged(Option<WindowId>),
    /// Exit the event loop, terminating the application.
    Terminate(TerminationMode),
    /// A registered global shortcut fired.
    GlobalShortcutTriggered(Keystroke),

    /// The XComponent surface exists; the window may create its GPU resources.
    SurfaceCreated {
        window: RawWindow,
        /// Surface size in physical pixels.
        size: Vector2F,
    },
    /// The XComponent surface is gone; the window must drop its GPU resources.
    SurfaceDestroyed,
    /// The surface was resized, in physical pixels.
    Resized(Vector2F),
    /// A vsync frame was delivered for the XComponent.
    Frame { time_stamp: i64 },
    /// The system light/dark mode changed.
    ColorModeChanged(ColorMode),
    /// The application gained (`true`) or lost (`false`) focus.
    FocusChanged(bool),
    /// The window gained focus; bind the soft keyboard to the focused editor.
    OpenImeRequested,
    /// An input event to dispatch to the active window.
    Input(WindowEvent),
    /// Files are being dragged over the window, at the given position.
    FileDrag { location: Vector2F },
    /// Files were dropped onto the window: their URIs, plus the drop position.
    FileDrop { uris: Vec<String>, location: Vector2F },
}

#[derive(Clone)]
pub(super) struct EventSender {
    sender: mpsc::Sender<AppEvent>,
}

impl EventSender {
    pub(super) fn send(&self, event: AppEvent) -> Result<(), SendError<AppEvent>> {
        self.sender.send(event)
    }
}

pub(super) struct EventReceiver {
    receiver: Receiver<AppEvent>,
    /// Kept alive so that the signal handler installed by [`run`] can reach the
    /// loop, and so that `run` can drop the sender when the loop ends.
    sender: EventSender,
}

impl EventReceiver {
    pub(super) fn sender(&self) -> &EventSender {
        &self.sender
    }
}

pub(super) fn channel() -> (EventSender, EventReceiver) {
    let (sender, receiver) = mpsc::channel();
    let event_sender = EventSender { sender };
    let event_receiver = EventReceiver {
        receiver,
        sender: event_sender.clone(),
    };
    (event_sender, event_receiver)
}

/// Installs the `openharmony-ability` run-loop handler.
///
/// Must be called on the ability (UI) thread: the run-loop handler is stored in
/// a single-threaded cell that the framework also drives from that thread.
pub(super) fn register(app: &OpenHarmonyApp, sender: EventSender) {
    register_file_drop_handler(sender.clone());
    let mut translator = Translator::new(app.clone(), sender);
    log::info!("ohos::event_loop::register: installing the ability run-loop handler");
    app.run_loop(move |event| {
        translator.handle(event);
    });
}

/// Connects the ArkTS file-drop plugin to warp's input stream.
///
/// `openharmony-ability` has no native drag-and-drop channel, so the ArkTS
/// plugin watches for drops on a hit-test-transparent overlay above the
/// XComponent and pushes each event over the bridge. Every push arrives on the
/// ability thread -- the thread this runs on -- which is what lets the handler
/// reach the single-threaded slot the plugin keeps.
///
/// Only the events that carry a position are forwarded: warp anchors a drag at
/// a window position, and the positionless `drag-enter` sample is superseded by
/// the `drag-move` that follows it.
fn register_file_drop_handler(sender: EventSender) {
    log::info!("ohos::event_loop::register_file_drop_handler: installing the file-drop handler");
    set_filedropin_callback(Box::new(move |event| {
        let app_event = match event {
            FileDropEventData::Enter => {
                log::debug!("ohos::event_loop::file_drop: the drag entered the window");
                return;
            }
            FileDropEventData::Move {
                position_x,
                position_y,
            } => {
                log::debug!(
                    "ohos::event_loop::file_drop: the drag moved to ({position_x}, {position_y})"
                );
                AppEvent::FileDrag {
                    location: drag_position(position_x, position_y),
                }
            }
            FileDropEventData::Drop {
                files,
                position_x,
                position_y,
            } => {
                log::info!(
                    "ohos::event_loop::file_drop: {} file(s) dropped at ({position_x}, \
                     {position_y})",
                    files.len()
                );
                AppEvent::FileDrop {
                    uris: files,
                    location: drag_position(position_x, position_y),
                }
            }
        };
        if sender.send(app_event).is_err() {
            log::debug!("ohos::event_loop::file_drop: the event loop is gone");
        }
    }));
}

/// Converts a drag position into warpui's logical coordinates.
///
/// The plugin reports window points (vp), and the scaled density warpui uses as
/// its backing scale factor is the physical-pixel-to-vp ratio, so a window
/// point is already the unit warpui lays out in. The input callbacks differ:
/// those hand over physical pixels, which is why [`Translator::device_position`]
/// scales them down.
fn drag_position(position_x: f64, position_y: f64) -> Vector2F {
    Vector2F::new(position_x as f32, position_y as f32)
}

/// Translates platform callbacks into [`AppEvent`]s.
///
/// Owned by the run-loop handler and therefore only ever touched by the UI
/// thread, which is why the key modifier state can live here unguarded.
struct Translator {
    app: OpenHarmonyApp,
    sender: EventSender,
    modifiers: Modifiers,
    /// Whether the left button is currently held, so that motion is reported as
    /// a drag rather than a plain move.
    left_button_pressed: bool,
    /// The previous button press, used to count single, double and triple
    /// clicks.
    last_press: Option<ButtonPress>,
    /// The touchscreen gesture in progress.
    touch: TouchGesture,
    /// Input channels that are known to be unimplemented. Tracked so the
    /// warning is emitted once per channel instead of once per event.
    reported_gaps: HashSet<&'static str>,
}

/// The press that a later press of the same button is measured against.
#[derive(Clone, Copy)]
struct ButtonPress {
    button: MouseButton,
    at: Instant,
    click_count: u32,
}

/// The touchscreen gesture in progress.
///
/// A finger landing on the screen is first a possible tap; only once it
/// travels past [`TOUCH_SLOP`] does it become a scroll (one finger) or a
/// selection drag (the second press of a multi-click), which is how the winit
/// back-end classifies the same input.
#[derive(Clone, Copy)]
enum TouchGesture {
    Idle,
    /// A touch that has not been classified yet.
    Pending {
        start: Vector2F,
        started_at: Instant,
        click_count: u32,
    },
    /// A single finger drag scrolling the content.
    Scroll { last: Vector2F },
    /// A drag extending a multi-click selection.
    Select,
}

impl Translator {
    fn new(app: OpenHarmonyApp, sender: EventSender) -> Self {
        Self {
            app,
            sender,
            modifiers: Modifiers::default(),
            left_button_pressed: false,
            last_press: None,
            touch: TouchGesture::Idle,
            reported_gaps: HashSet::new(),
        }
    }

    fn handle(&mut self, event: PlatformEvent<'_>) {
        log::debug!("ohos::event_loop::Translator::handle: {}", event.as_str());
        match event {
            PlatformEvent::SurfaceCreate => {
                // The XComponent callback registry is thread-local and the
                // per-frame callback is delivered on this, the UI, thread, so
                // the arm has to happen here: arming it from the warp main
                // thread would register a callback this thread never sees.
                log::info!(
                    "ohos::event_loop::Translator::handle: arming the XComponent frame callback"
                );
                PENDING_REDRAW.store(true, Ordering::Release);
                self.app.enable_frame_callback();
                // A window that was already focused before its surface appeared
                // never sees another focus edge, and the first request may have
                // been issued before the bridge session existed. Asking again
                // here costs one idempotent bridge call.
                if WINDOW_FOCUSED.load(Ordering::Acquire) {
                    log::info!(
                        "ohos::event_loop::Translator::handle: the surface appeared while the \
                         window was focused, requesting the soft keyboard"
                    );
                    self.send(AppEvent::OpenImeRequested);
                }
                self.push_surface_created();
            }
            PlatformEvent::SurfaceDestroy => {
                self.send(AppEvent::SurfaceDestroyed);
            }
            PlatformEvent::WindowResize(size) => {
                self.resize(Vector2F::new(size.width as f32, size.height as f32));
            }
            PlatformEvent::ContentRectChange(content_rect) => {
                let rect = content_rect.rect;
                self.resize(Vector2F::new(rect.width as f32, rect.height as f32));
            }
            PlatformEvent::WindowRedraw(interval) => {
                // Park the per-vsync callbacks once the pending demand has been
                // consumed, so an idle window stops waking this thread every
                // frame; the waker arms them again when a frame is wanted.
                if !PENDING_REDRAW.swap(false, Ordering::AcqRel) {
                    log::debug!("ohos::event_loop::Translator::handle: parking the frame callback");
                    self.app.disable_frame_callback();
                }
                self.send(AppEvent::Frame {
                    time_stamp: interval.time_stamp,
                });
            }
            PlatformEvent::ConfigChanged(config) => {
                log::info!(
                    "ohos::event_loop::Translator::handle: color mode changed to {:?}",
                    config.color_mode
                );
                self.send(AppEvent::ColorModeChanged(config.color_mode));
            }
            PlatformEvent::Start | PlatformEvent::GainedFocus | PlatformEvent::Resume(_) => {
                self.modifiers.clear_held();
                // Hopping to the warp main thread is required, not just tidy:
                // binding the IME blocks until ArkTS answers, and ArkTS answers
                // on this, the UI, thread -- so waiting here would deadlock.
                if !WINDOW_FOCUSED.swap(true, Ordering::AcqRel) {
                    log::info!(
                        "ohos::event_loop::Translator::handle: the window gained focus, requesting \
                         the soft keyboard"
                    );
                    self.send(AppEvent::OpenImeRequested);
                }
                self.send(AppEvent::FocusChanged(true));
            }
            PlatformEvent::LostFocus | PlatformEvent::Pause | PlatformEvent::Stop => {
                self.modifiers.clear_held();
                WINDOW_FOCUSED.store(false, Ordering::Release);
                self.send(AppEvent::FocusChanged(false));
            }
            PlatformEvent::VisibilityChanged(visible) => {
                log::debug!(
                    "ohos::event_loop::Translator::handle: the window became {}",
                    if visible { "visible" } else { "hidden" }
                );
                // A titlebar minimize/hide on 2in1 fires no windowStageEvent at
                // all -- no Stop/LostFocus on the way out, and symmetrically no
                // Resume/GainedFocus on the way back -- so `WINDOW_FOCUSED` stays
                // set and the focus edge above never asks for the keyboard again.
                // This callback is the only signal for that transition, so the
                // keyboard is requested here as well. Re-binding an already bound
                // session is idempotent, so a restore that does report focus only
                // repeats the request.
                if visible {
                    log::info!(
                        "ohos::event_loop::Translator::handle: the window is visible again, \
                         requesting the soft keyboard"
                    );
                    self.send(AppEvent::OpenImeRequested);
                }
            }
            PlatformEvent::Destroy | PlatformEvent::WindowDestroy => {
                log::info!("ohos::event_loop::Translator::handle: the ability was destroyed");
                self.send(AppEvent::Terminate(TerminationMode::ForceTerminate));
            }
            PlatformEvent::Input(input) => self.handle_input(input),
            PlatformEvent::SaveState(_) => {
                // Warp persists its own state, so the ability save-state payload
                // has no producer here.
                self.report_gap("ability save-state callbacks");
            }
            PlatformEvent::KeyboardEvent(height) => {
                log::debug!(
                    "ohos::event_loop::Translator::handle: soft keyboard height changed to {height}"
                );
                self.report_gap("soft keyboard height changes");
            }
            PlatformEvent::AvoidAreaChange(_) => {
                log::debug!(
                    "ohos::event_loop::Translator::handle: avoid-area geometry changed; the \
                     content rect event carries the resulting size"
                );
            }
            PlatformEvent::LowMemory => {
                log::warn!("ohos::event_loop::Translator::handle: the system is low on memory");
            }
            PlatformEvent::Create | PlatformEvent::WindowCreate => {}
            PlatformEvent::UserEvent => {
                // The waker runs on this, the UI, thread. Arm the frame callback
                // when a frame is wanted again, which is what restarts the
                // per-vsync callbacks after an idle window stopped them.
                if PENDING_REDRAW.load(Ordering::Acquire) {
                    log::debug!(
                        "ohos::event_loop::Translator::handle: arming the frame callback on wake"
                    );
                    self.app.enable_frame_callback();
                }
            }
        }
    }

    fn handle_input(&mut self, input: InputEvent) {
        match input {
            InputEvent::KeyEvent(key) => self.handle_key(&key),
            InputEvent::MouseEvent(mouse) => self.handle_mouse(&mouse),
            InputEvent::AxisEvent(axis) => self.handle_axis(&axis),
            InputEvent::ImeEvent(ime) => self.handle_ime(&ime),
            InputEvent::TouchEvent(touch) => self.handle_touch(&touch),
            InputEvent::HoverEvent(_) => self.report_gap("pointer hover"),
        }
    }

    /// Translates a mouse callback.
    ///
    /// Motion is reported as a drag while the left button is held, mirroring
    /// the winit back-end, where the button state is tracked for the same
    /// reason. Right and middle clicks carry the modifier state as two
    /// booleans because warp's event model does not take a full
    /// [`ModifiersState`] for them.
    fn handle_mouse(&mut self, data: &MouseEventData) {
        let position = self.device_position(data.x, data.y);
        log::debug!(
            "ohos::event_loop::Translator::handle_mouse: action={:?} button={:?} mask={:#x} \
             position={position:?}",
            data.action,
            data.button,
            data.button_mask
        );
        let modifiers = modifiers_from_key_mask(data.modifiers);
        match data.action {
            MouseAction::Move => {
                if self.left_button_pressed {
                    self.send(AppEvent::Input(WindowEvent::LeftMouseDragged {
                        position,
                        modifiers,
                    }));
                } else {
                    self.send(AppEvent::Input(WindowEvent::MouseMoved {
                        position,
                        cmd: modifiers.cmd,
                        shift: modifiers.shift,
                        is_synthetic: false,
                    }));
                }
            }
            MouseAction::Press => {
                let click_count = self.click_count_for_press(data.button);
                let event = match data.button {
                    MouseButton::LeftButton => {
                        self.left_button_pressed = true;
                        WindowEvent::LeftMouseDown {
                            position,
                            modifiers,
                            click_count,
                            is_first_mouse: false,
                        }
                    }
                    MouseButton::RightButton => WindowEvent::RightMouseDown {
                        position,
                        cmd: modifiers.cmd,
                        shift: modifiers.shift,
                        click_count,
                    },
                    MouseButton::MiddleButton => WindowEvent::MiddleMouseDown {
                        position,
                        cmd: modifiers.cmd,
                        shift: modifiers.shift,
                        click_count,
                    },
                    MouseButton::BackButton => WindowEvent::BackMouseDown {
                        position,
                        cmd: modifiers.cmd,
                        shift: modifiers.shift,
                        click_count,
                    },
                    MouseButton::ForwardButton => WindowEvent::ForwardMouseDown {
                        position,
                        cmd: modifiers.cmd,
                        shift: modifiers.shift,
                        click_count,
                    },
                    MouseButton::NoneButton => {
                        log::warn!(
                            "ohos::event_loop::Translator::handle_mouse: the platform reported a \
                             press without a button, dropping it"
                        );
                        return;
                    }
                };
                self.send(AppEvent::Input(event));
            }
            MouseAction::Release => {
                if data.button == MouseButton::LeftButton {
                    self.left_button_pressed = false;
                    self.send(AppEvent::Input(WindowEvent::LeftMouseUp {
                        position,
                        modifiers,
                    }));
                } else {
                    log::debug!(
                        "ohos::event_loop::Translator::handle_mouse: no warpui event for a {:?} \
                         release",
                        data.button
                    );
                }
            }
            MouseAction::Cancel => {
                // The platform withdrew the gesture, so the button can no
                // longer be considered held and a later move must not be
                // reported as a drag.
                log::warn!(
                    "ohos::event_loop::Translator::handle_mouse: the platform cancelled the mouse \
                     gesture, clearing the held left button"
                );
                self.left_button_pressed = false;
            }
            MouseAction::None => {
                log::debug!(
                    "ohos::event_loop::Translator::handle_mouse: dropping an event with no action"
                );
            }
        }
    }

    /// Counts presses of the same button into single, double and triple clicks.
    ///
    /// The count resets when the button changes or when the previous press is
    /// older than [`MULTI_CLICK_INTERVAL`].
    fn click_count_for_press(&mut self, button: MouseButton) -> u32 {
        let now = Instant::now();
        let click_count = self
            .last_press
            .filter(|previous| {
                previous.button == button
                    && now.duration_since(previous.at) <= MULTI_CLICK_INTERVAL
            })
            .map(|previous| previous.click_count + 1)
            .unwrap_or(1);
        self.last_press = Some(ButtonPress {
            button,
            at: now,
            click_count,
        });
        log::debug!(
            "ohos::event_loop::Translator::click_count_for_press: button={button:?} \
             click_count={click_count}"
        );
        click_count
    }

    /// Translates a scroll callback.
    ///
    /// A wheel reports discrete notches in axis units and becomes a
    /// line-based delta; a touchpad reports pixels and becomes a precise
    /// pixel delta. Shift swaps the axes, matching the winit back-end.
    fn handle_axis(&mut self, data: &AxisEventData) {
        let position = self.device_position(data.x, data.y);
        let mut horizontal = data.scroll_horizontal;
        let mut vertical = data.scroll_vertical;
        if data.modifiers.shift {
            std::mem::swap(&mut horizontal, &mut vertical);
        }
        let (delta, precise) = match data.tool_type {
            AxisToolType::Mouse => (
                Vector2F::new(
                    -(horizontal / AXIS_WHEEL_UNIT * WHEEL_LINES_PER_NOTCH) as f32,
                    -(vertical / AXIS_WHEEL_UNIT * WHEEL_LINES_PER_NOTCH) as f32,
                ),
                false,
            ),
            AxisToolType::Touchpad => (
                Vector2F::new(-horizontal as f32, -vertical as f32),
                true,
            ),
        };
        log::debug!(
            "ohos::event_loop::Translator::handle_axis: tool={:?} phase={:?} delta={delta:?} \
             precise={precise} position={position:?}",
            data.tool_type,
            data.scroll_phase
        );
        self.send(AppEvent::Input(WindowEvent::ScrollWheel {
            position,
            delta,
            precise,
            modifiers: modifiers_from_device(data.modifiers),
        }));
    }

    /// Translates a soft-keyboard callback.
    ///
    /// Committed text becomes [`WindowEvent::TypedCharacters`], the same event
    /// the winit back-end produces for an IME commit. The editing keys have no
    /// text payload, so they are delivered as ordinary key presses and the
    /// focused view applies its own semantics: in a terminal, Backspace must
    /// delete through the PTY rather than inside a composition buffer.
    fn handle_ime(&mut self, event: &ImeEvent) {
        log::debug!("ohos::event_loop::Translator::handle_ime: {event:?}");
        match event {
            ImeEvent::TextInputEvent(data) => {
                if data.text.is_empty() {
                    log::debug!(
                        "ohos::event_loop::Translator::handle_ime: dropping an empty text commit"
                    );
                    return;
                }
                self.send(AppEvent::Input(WindowEvent::TypedCharacters {
                    chars: data.text.clone(),
                }));
            }
            ImeEvent::BackspaceEvent(len) => {
                log::debug!(
                    "ohos::event_loop::Translator::handle_ime: backspace of {len} character(s)"
                );
                self.send_key_press("backspace");
            }
            ImeEvent::EnterEvent(len) => {
                log::debug!("ohos::event_loop::Translator::handle_ime: enter, key={len}");
                self.send_key_press("enter");
            }
            ImeEvent::DeleteRightEvent(len) => {
                log::debug!(
                    "ohos::event_loop::Translator::handle_ime: forward delete of {len} character(s)"
                );
                self.send_key_press("delete");
            }
            ImeEvent::ImeStatusEvent(status) => {
                log::info!(
                    "ohos::event_loop::Translator::handle_ime: the soft keyboard is now {status:?}"
                );
                super::delegate::set_ime_open(matches!(status, KeyboardStatus::Show));
            }
        }
    }

    /// Delivers a modifier-free key press for an IME editing key.
    fn send_key_press(&self, key: &str) {
        log::info!("ohos::event_loop::Translator::send_key_press: key={key}");
        // An IME reports Enter by name only, so the CR byte the pty needs has to
        // be filled in here, matching the winit back-end.
        let chars = match key.to_lowercase().as_str() {
            "enter" => "\r".to_string(),
            _ => String::new(),
        };
        self.send(AppEvent::Input(WindowEvent::KeyDown {
            keystroke: Keystroke {
                ctrl: false,
                alt: false,
                shift: false,
                cmd: false,
                meta: false,
                key: key.to_string(),
            },
            chars,
            details: KeyEventDetails::default(),
            is_composing: false,
        }));
    }

    /// Converts a device (physical-pixel) position into the logical pixels
    /// warpui lays out and hit-tests in.
    fn device_position(&self, x: f32, y: f32) -> Vector2F {
        let scale = self.app.scale().max(1.0);
        Vector2F::new(x / scale, y / scale)
    }

    /// Translates a touchscreen callback.
    ///
    /// A touchscreen has no buttons, so the gesture is classified here: a tap
    /// becomes a left click, a hold opens the context menu, a one-finger drag
    /// scrolls, and a drag that extends a multi-click selects. Pointer, touch
    /// and axis input each arrive on their own callback, so no source
    /// filtering is needed.
    fn handle_touch(&mut self, data: &TouchEventData) {
        let position = self.device_position(data.x, data.y);
        // Only pressed points count: the payload always carries a fixed-size
        // point array, so its length says nothing about how many fingers are
        // down.
        let point_count = data
            .touch_points
            .iter()
            .filter(|point| point.is_pressed)
            .count();
        log::debug!(
            "ohos::event_loop::Translator::handle_touch: type={:?} points={point_count} \
             reported_points={} position={position:?}",
            data.event_type,
            data.num_points
        );
        match data.event_type {
            TouchEvent::Down => self.touch_down(position, point_count),
            TouchEvent::Move => self.touch_move(position, point_count),
            TouchEvent::Up => self.touch_up(position),
            TouchEvent::Cancel => self.touch_cancel(),
            TouchEvent::Unknown => {
                log::debug!(
                    "ohos::event_loop::Translator::handle_touch: dropping a touch with an unknown \
                     type"
                );
            }
        }
    }

    /// Starts a gesture. A second finger down cannot be a tap.
    fn touch_down(&mut self, position: Vector2F, point_count: usize) {
        if point_count > 1 {
            log::debug!(
                "ohos::event_loop::Translator::touch_down: {point_count} fingers down, not a tap"
            );
            self.touch = TouchGesture::Idle;
            return;
        }
        let click_count = self.click_count_for_press(MouseButton::LeftButton);
        log::info!(
            "ohos::event_loop::Translator::touch_down: position={position:?} \
             click_count={click_count}"
        );
        self.touch = TouchGesture::Pending {
            start: position,
            started_at: Instant::now(),
            click_count,
        };
        self.send(AppEvent::Input(WindowEvent::LeftMouseDown {
            position,
            modifiers: ModifiersState::default(),
            click_count,
            is_first_mouse: false,
        }));
    }

    /// Classifies a moving touch on its first move past [`TOUCH_SLOP`], then
    /// continues whichever gesture it became.
    fn touch_move(&mut self, position: Vector2F, point_count: usize) {
        match self.touch {
            TouchGesture::Idle => {
                log::debug!(
                    "ohos::event_loop::Translator::touch_move: dropping a move with no touch in \
                     progress"
                );
            }
            TouchGesture::Pending {
                start,
                click_count,
                ..
            } => {
                if point_count > 1 {
                    // The gesture changed shape, so it is no longer the tap
                    // that the press announced.
                    log::info!(
                        "ohos::event_loop::Translator::touch_move: {point_count} fingers, \
                         withdrawing the pending tap"
                    );
                    self.touch = TouchGesture::Idle;
                    self.send(AppEvent::Input(WindowEvent::LeftMouseUp {
                        position,
                        modifiers: ModifiersState::default(),
                    }));
                    return;
                }
                let dx = position.x() - start.x();
                let dy = position.y() - start.y();
                if dx * dx + dy * dy <= TOUCH_SLOP * TOUCH_SLOP {
                    return;
                }
                if click_count >= 2 {
                    log::info!(
                        "ohos::event_loop::Translator::touch_move: a drag after {click_count} \
                         clicks selects from {start:?}"
                    );
                    self.touch = TouchGesture::Select;
                    self.send(AppEvent::Input(WindowEvent::LeftMouseDragged {
                        position,
                        modifiers: ModifiersState::default(),
                    }));
                } else {
                    log::info!(
                        "ohos::event_loop::Translator::touch_move: a one-finger drag scrolls \
                         from {start:?}"
                    );
                    self.touch = TouchGesture::Scroll { last: position };
                    self.send(AppEvent::Input(WindowEvent::ScrollWheel {
                        position,
                        delta: position - start,
                        precise: true,
                        modifiers: ModifiersState::default(),
                    }));
                }
            }
            TouchGesture::Scroll { last } => {
                let delta = position - last;
                self.touch = TouchGesture::Scroll { last: position };
                self.send(AppEvent::Input(WindowEvent::ScrollWheel {
                    position,
                    delta,
                    precise: true,
                    modifiers: ModifiersState::default(),
                }));
            }
            TouchGesture::Select => {
                self.send(AppEvent::Input(WindowEvent::LeftMouseDragged {
                    position,
                    modifiers: ModifiersState::default(),
                }));
            }
        }
    }

    /// Ends a gesture: a short pending touch completes the tap, a long one
    /// opens the context menu, and a scroll simply stops.
    fn touch_up(&mut self, position: Vector2F) {
        match self.touch {
            TouchGesture::Pending {
                started_at,
                click_count,
                ..
            } => {
                let held = started_at.elapsed();
                if held >= LONG_PRESS_DURATION {
                    log::info!(
                        "ohos::event_loop::Translator::touch_up: a hold of {held:?} opens the \
                         context menu"
                    );
                    self.send(AppEvent::Input(WindowEvent::RightMouseDown {
                        position,
                        cmd: false,
                        shift: false,
                        click_count,
                    }));
                } else {
                    self.send(AppEvent::Input(WindowEvent::LeftMouseUp {
                        position,
                        modifiers: ModifiersState::default(),
                    }));
                }
            }
            TouchGesture::Select => {
                self.send(AppEvent::Input(WindowEvent::LeftMouseUp {
                    position,
                    modifiers: ModifiersState::default(),
                }));
            }
            TouchGesture::Scroll { .. } => {
                // Scrolling has no button to release, so nothing follows.
                log::debug!(
                    "ohos::event_loop::Translator::touch_up: ending a scroll without an event"
                );
            }
            TouchGesture::Idle => {
                log::debug!(
                    "ohos::event_loop::Translator::touch_up: dropping an up with no touch in \
                     progress"
                );
            }
        }
        self.touch = TouchGesture::Idle;
    }

    /// Drops a gesture the platform withdrew. Nothing is emitted, because the
    /// system has already taken the touch over.
    fn touch_cancel(&mut self) {
        if !matches!(self.touch, TouchGesture::Idle) {
            log::info!(
                "ohos::event_loop::Translator::touch_cancel: the platform withdrew the touch"
            );
        }
        self.touch = TouchGesture::Idle;
    }

    /// Translates a key callback.
    ///
    /// Warp's event model only carries key presses; modifier state is tracked
    /// here so that the keystroke built for a press reflects the keys held at
    /// that moment.
    fn handle_key(&mut self, event: &KeyEventData) {
        let is_modifier = self.modifiers.update(event);
        if is_modifier {
            let Some(key_code) = modifier_keycode(event.code) else {
                log::debug!(
                    "ohos::event_loop::Translator::handle_key: lock key {:?} toggled, no keystroke",
                    event.code
                );
                return;
            };
            let state = match event.action {
                Action::Down => KeyState::Pressed,
                Action::Up => KeyState::Released,
                Action::Unknown => return,
            };
            self.send(AppEvent::Input(WindowEvent::ModifierKeyChanged {
                key_code,
                state,
            }));
            return;
        }

        if event.action != Action::Down {
            return;
        }

        let Some(keystroke) = keycodes::key_event_to_keystroke(event, &self.modifiers) else {
            log::debug!(
                "ohos::event_loop::Translator::handle_key: {:?} produced no keystroke",
                event.code
            );
            return;
        };
        let chars = keycodes::key_event_to_chars(event, &self.modifiers).unwrap_or_default();
        let details = KeyEventDetails {
            left_alt: event.code == KeyCode::AltLeft,
            right_alt: event.code == KeyCode::AltRight,
            key_without_modifiers: keycodes::key_without_modifiers(event),
        };
        log::debug!(
            "ohos::event_loop::Translator::handle_key: code={:?} keystroke={keystroke:?} chars={chars:?}",
            event.code
        );
        self.send(AppEvent::Input(WindowEvent::KeyDown {
            keystroke,
            chars,
            details,
            is_composing: false,
        }));
    }

    fn push_surface_created(&mut self) {
        let Some(window) = self.app.native_window() else {
            log::warn!(
                "ohos::event_loop::Translator::push_surface_created: the ability reported a \
                 surface without a native window"
            );
            return;
        };
        let rect = self.app.content_rect();
        let size = Vector2F::new(rect.width as f32, rect.height as f32);
        log::info!(
            "ohos::event_loop::Translator::push_surface_created: surface size={size:?} scale={}",
            self.app.scale()
        );
        self.send(AppEvent::SurfaceCreated { window, size });
    }

    fn resize(&mut self, size: Vector2F) {
        log::info!("ohos::event_loop::Translator::resize: physical size={size:?}");
        self.send(AppEvent::Resized(size));
    }

    fn send(&self, event: AppEvent) {
        if self.sender.send(event).is_err() {
            log::warn!(
                "ohos::event_loop::Translator::send: the warp main thread is no longer running"
            );
        }
    }

    /// Records an input or lifecycle channel that the port does not serve yet.
    fn report_gap(&mut self, channel: &'static str) {
        if self.reported_gaps.insert(channel) {
            log::warn!("ohos::event_loop::Translator: {channel} is not wired up yet");
        }
    }
}

/// Converts the `ArkUI_ModifierKeyName` bitmask that mouse events carry.
///
/// HarmonyOS reports neither the Meta nor the Fn key, so those stay false and
/// `cmd`-bound actions resolve to ctrl, as they do for key events.
fn modifiers_from_key_mask(mask: u64) -> ModifiersState {
    ModifiersState {
        alt: mask & MODIFIER_ALT != 0,
        cmd: false,
        shift: mask & MODIFIER_SHIFT != 0,
        ctrl: mask & MODIFIER_CTRL != 0,
        func: false,
    }
}

/// Converts the modifier state that axis events carry.
fn modifiers_from_device(modifiers: DeviceModifiers) -> ModifiersState {
    ModifiersState {
        alt: modifiers.alt,
        cmd: false,
        shift: modifiers.shift,
        ctrl: modifiers.control,
        func: false,
    }
}

/// Maps a modifier key to the warpui key code, or `None` for lock keys.
fn modifier_keycode(code: KeyCode) -> Option<crate::platform::keyboard::KeyCode> {
    use crate::platform::keyboard::KeyCode as WarpKeyCode;
    Some(match code {
        KeyCode::CtrlLeft => WarpKeyCode::ControlLeft,
        KeyCode::CtrlRight => WarpKeyCode::ControlRight,
        KeyCode::AltLeft => WarpKeyCode::AltLeft,
        KeyCode::AltRight => WarpKeyCode::AltRight,
        KeyCode::ShiftLeft => WarpKeyCode::ShiftLeft,
        KeyCode::ShiftRight => WarpKeyCode::ShiftRight,
        KeyCode::MetaLeft => WarpKeyCode::SuperLeft,
        KeyCode::MetaRight => WarpKeyCode::SuperRight,
        _ => return None,
    })
}

/// Runs warp's event loop until termination.
///
/// Called on the `warp-main` thread, which [`super::spawn`] starts. Every event
/// that needs to touch warp's state is processed here, so the UI thread never
/// blocks on it.
pub(super) fn run(
    mut ui_app: crate::App,
    callbacks: &mut AppCallbackDispatcher,
    init_fn: platform::app::AppInitCallbackFn,
    receiver: EventReceiver,
) -> TerminationResult {
    setup_signal_handler(receiver.sender.clone());

    callbacks.initialize_app(init_fn);

    for event in receiver.receiver.iter() {
        if process_event(event, &mut ui_app, callbacks).is_break() {
            break;
        }
    }

    // Drop the receiver so the Ctrl+C signal handler's channel send will fail,
    // causing it to fall through to `process::exit(130)`. Without this, the send
    // succeeds (since the receiver is still in scope) but nobody is reading from
    // the channel, making Ctrl+C ineffective during shutdown.
    drop(receiver);

    callbacks.app_will_terminate();

    ui_app.termination_result().unwrap_or(Ok(()))
}

fn process_event(
    event: AppEvent,
    ui_app: &mut crate::App,
    callbacks: &mut AppCallbackDispatcher,
) -> ControlFlow<()> {
    match event {
        AppEvent::RunCallback(callback) => ui_app.update(callback),
        AppEvent::RunTask(task) => {
            let task = ManuallyDrop::into_inner(task);
            task.run();
        }
        AppEvent::Terminate(termination_mode) => {
            let should_terminate = match termination_mode {
                TerminationMode::Cancellable => {
                    matches!(
                        callbacks.should_terminate_app(TerminationRequestSource::User),
                        ApproveTerminateResult::Terminate
                    )
                }
                TerminationMode::ForceTerminate | TerminationMode::ContentTransferred => true,
            };
            if should_terminate {
                return ControlFlow::Break(());
            }
        }
        AppEvent::CloseWindow(window_id) => callbacks.window_will_close(window_id),
        AppEvent::ActiveWindowChanged(window_id) => callbacks.active_window_changed(window_id),
        AppEvent::GlobalShortcutTriggered(keystroke) => {
            callbacks.global_shortcut_triggered(keystroke)
        }
        AppEvent::SurfaceCreated { window, size } => {
            for_each_window(ui_app, |window_handle| {
                super::windowing::downcast_window(window_handle)
                    .attach_surface(window.clone(), size)
            });
        }
        AppEvent::SurfaceDestroyed => {
            for_each_window(ui_app, |window_handle| {
                super::windowing::downcast_window(window_handle).detach_surface()
            });
        }
        AppEvent::Resized(size) => {
            let changed = Cell::new(false);
            for_each_window(ui_app, |window_handle| {
                if super::windowing::downcast_window(window_handle).set_surface_size(size) {
                    changed.set(true);
                }
            });
            if changed.get() {
                notify_window_resized(ui_app, callbacks);
            }
        }
        AppEvent::Frame { time_stamp } => {
            log::debug!("ohos::event_loop::process_event: frame time_stamp={time_stamp}");
            if !FIRST_FRAME_REPORTED.swap(true, Ordering::Relaxed) {
                log::info!("ohos::event_loop::process_event: the first frame reached the renderer");
            }
            let Some(window_id) = active_window_id(callbacks) else {
                log::warn!(
                    "ohos::event_loop::process_event: dropping frame because no window is active"
                );
                return ControlFlow::Continue(());
            };
            let Some(window_handle) = ui_app.read(|ctx| ctx.windows().platform_window(window_id))
            else {
                log::warn!(
                    "ohos::event_loop::process_event: dropping frame because the active window \
                     does not exist"
                );
                return ControlFlow::Continue(());
            };
            let window = super::windowing::downcast_window(window_handle.as_ref());

            let render_result = (|| {
                // Bring the swap chain up to date before building the scene, so
                // that the scene is laid out at the size we are about to render.
                window.update_size_if_needed()?;
                let new_scene = if window.has_scene() {
                    None
                } else {
                    Some(callbacks.for_window(window).build_scene(window))
                };
                callbacks.with_mutable_app_context(|ctx| window.render(new_scene, ctx.font_cache()))
            })();

            match render_result {
                Ok(()) => callbacks.for_window(window).frame_drawn(),
                Err(err) => {
                    log::warn!(
                        "ohos::event_loop::process_event: failed to render frame for window \
                         {window_id:?}: {err:#}"
                    );
                    callbacks.for_window(window).frame_failed_to_draw();
                }
            }
        }
        AppEvent::ColorModeChanged(_color_mode) => callbacks.os_appearance_changed(),
        AppEvent::FocusChanged(true) => callbacks.app_became_active(),
        AppEvent::FocusChanged(false) => callbacks.app_resigned_active(),
        AppEvent::OpenImeRequested => super::delegate::open_ime_async(),
        AppEvent::Input(input) => dispatch_to_active_window(ui_app, callbacks, "input", input),
        AppEvent::FileDrag { location } => dispatch_to_active_window(
            ui_app,
            callbacks,
            "a file drag",
            WindowEvent::DragFiles { location },
        ),
        AppEvent::FileDrop { uris, location } => {
            let paths = super::delegate::local_paths_from_uris(&uris);
            dispatch_to_active_window(
                ui_app,
                callbacks,
                "a file drop",
                WindowEvent::DragAndDropFiles { paths, location },
            );
            // OHOS reports no drag-leave, so the drop is also the end of the
            // drag session; the exit closes the state the drag events opened.
            dispatch_to_active_window(
                ui_app,
                callbacks,
                "a file drop exit",
                WindowEvent::DragFileExit,
            );
        }
    }
    ControlFlow::Continue(())
}

/// Dispatches `event` to the active window.
///
/// Drops the event, with a warning naming `what`, when no window is focused or
/// the active window has gone away.
fn dispatch_to_active_window(
    ui_app: &crate::App,
    callbacks: &mut AppCallbackDispatcher,
    what: &str,
    event: WindowEvent,
) {
    let Some(window_id) = active_window_id(callbacks) else {
        log::warn!("ohos::event_loop::process_event: dropping {what} because no window is active");
        return;
    };
    let Some(window_handle) = ui_app.read(|ctx| ctx.windows().platform_window(window_id)) else {
        log::warn!(
            "ohos::event_loop::process_event: dropping {what} because the active window does not \
             exist"
        );
        return;
    };
    callbacks
        .for_window(window_handle.as_ref())
        .dispatch_event(event);
}

/// The [`WindowId`] of the active window, or `None` when nothing is focused.
fn active_window_id(callbacks: &mut AppCallbackDispatcher) -> Option<WindowId> {
    callbacks.with_mutable_app_context(|ctx| ctx.windows().active_window())
}

/// Tells warp that the active window changed size.
///
/// The window-level callback lets the UI relayout at the new size; the
/// app-level one mirrors the winit back-end, which notifies both.
fn notify_window_resized(ui_app: &crate::App, callbacks: &mut AppCallbackDispatcher) {
    if let Some(window_id) = active_window_id(callbacks)
        && let Some(window_handle) = ui_app.read(|ctx| ctx.windows().platform_window(window_id))
    {
        callbacks
            .for_window(window_handle.as_ref())
            .window_resized(super::windowing::downcast_window(window_handle.as_ref()));
    }
    callbacks.window_resized();
}

/// Runs `apply` on the platform window of every open window.
///
/// The OHOS back-end hosts a single XComponent, so in practice this visits at
/// most one window; it is written as a loop so that the surface events are not
/// silently dropped if the ability ever mounts more than one.
fn for_each_window(ui_app: &crate::App, mut apply: impl FnMut(&dyn platform::Window)) -> bool {
    // `window_ids` borrows the app context, so it is collected before the
    // context is released.
    let window_ids = ui_app.read(|ctx| ctx.window_ids().collect::<Vec<_>>());
    let mut applied = false;
    for window_id in window_ids {
        let Some(window) = ui_app.read(|ctx| ctx.windows().platform_window(window_id)) else {
            continue;
        };
        apply(window.as_ref());
        applied = true;
    }
    if !applied {
        log::warn!("ohos::event_loop::for_each_window: no window is open");
    }
    applied
}

/// Set up a signal handler for Ctrl-C (SIGINT) to gracefully terminate the app.
#[cfg(not(target_family = "wasm"))]
fn setup_signal_handler(sender: EventSender) {
    let result = ctrlc::set_handler(move || {
        log::info!("Received Ctrl-C signal on OHOS, terminating application");
        if sender
            .send(AppEvent::Terminate(TerminationMode::ForceTerminate))
            .is_err()
        {
            log::warn!("Failed to send termination event - event loop may have already stopped");
            std::process::exit(130);
        }
    });

    if let Err(e) = result {
        log::warn!("Failed to set up Ctrl-C handler: {e}");
    }
}

#[cfg(target_family = "wasm")]
fn setup_signal_handler(_sender: EventSender) {
    // Signal handling is unavailable on WASM.
}
