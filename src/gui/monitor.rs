// Watches the desktop for anything that affects the overlay and wakes the GUI only when something visible changes.
// Everything here is driven by window messages and system events, so the thread sleeps while nothing happens.
use crate::config::get_config;
use crate::gui::{gui_placement, gui_util};
use crate::power;
use eframe::egui;
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Shell::{ABM_NEW, ABN_FULLSCREENAPP, APPBARDATA, SHAppBarMessage};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EVENT_SYSTEM_FOREGROUND, GetCursorPos, GetMessageW,
    HSHELL_WINDOWCREATED, HSHELL_WINDOWDESTROYED, HSHELL_WINDOWREPLACED, KillTimer, MSG, PostMessageW,
    RegisterClassW, RegisterShellHookWindow, RegisterWindowMessageW, SetTimer, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_APP, WM_DISPLAYCHANGE, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_POPUP,
};

// Measuring the taskbar costs explorer ~25ms of CPU, so after anything that can move its icons, re-measure on a
// backing-off schedule that restarts while the icons are still animating into place.
const TASKBAR_BURST_DELAYS_MS: [u32; 4] = [100, 250, 500, 1_000];
// Catches layout changes no event reports, such as pinning an icon or a tray icon appearing.
const TASKBAR_SAFETY_INTERVAL_MS: u32 = 30_000;
// Catches the taskbar being raised above the overlay without a foreground change.
const ZORDER_CHECK_INTERVAL_MS: u32 = 1_000;
// The taskbar raises itself slightly after it becomes the foreground window, so check the z-order again after a moment.
const FOREGROUND_RECHECK_DELAY_MS: u32 = 250;
// Poll the cursor quickly only while it is over the taskbar's row, where it can reach the buttons.
const HOVER_INTERVAL_NEAR_MS: u32 = 33;
const HOVER_INTERVAL_FAR_MS: u32 = 250;
const HOVER_NEAR_MARGIN: i32 = 40;

const TIMER_TASKBAR_BURST: usize = 1;
const TIMER_TASKBAR_SAFETY: usize = 2;
const TIMER_ZORDER: usize = 3;
const TIMER_FOREGROUND_RECHECK: usize = 4;
const TIMER_HOVER: usize = 5;
const TIMER_TRIM_MEMORY: usize = 6;

// Long enough for the window, fonts and first taskbar measurements to have settled.
const TRIM_MEMORY_DELAY_MS: u32 = 15_000;

const WM_APPBAR_CALLBACK: u32 = WM_APP + 1;
const WM_FOREGROUND_CHANGED: u32 = WM_APP + 2;

// What the GUI needs to draw the overlay.
#[derive(Clone)]
pub struct OverlayState {
    pub placement: ([f32; 2], [f32; 2]),
    pub fullscreen: bool,
    pub dark_mode: bool,
    pub mouse_passthrough: bool,
}

struct Shared {
    state: OverlayState,
    interactive_rects: Vec<egui::Rect>,
    pixels_per_point: f32,
}

static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();

fn shared() -> &'static Mutex<Shared> {
    SHARED.get_or_init(|| {
        let placement = get_config()
            .map(|config| (config.position, config.size))
            .unwrap_or(([0.0, 0.0], [gui_placement::TASKBAR_WIDGET_WIDTH, 48.0]));
        Mutex::new(Shared {
            state: OverlayState {
                placement,
                fullscreen: false,
                dark_mode: gui_util::is_dark_mode().unwrap_or(false),
                mouse_passthrough: true,
            },
            interactive_rects: Vec::new(),
            pixels_per_point: 1.0,
        })
    })
}

pub fn overlay_state() -> OverlayState {
    shared().lock().unwrap().state.clone()
}

// Called by the GUI after each frame so hover detection knows where the buttons are.
pub fn set_interactive_rects(rects: Vec<egui::Rect>, pixels_per_point: f32) {
    let mut shared = shared().lock().unwrap();
    shared.interactive_rects = rects;
    shared.pixels_per_point = pixels_per_point;
}

struct Monitor {
    ctx: egui::Context,
    hwnd: HWND,
    shell_hook_msg: u32,
    taskbar_created_msg: u32,
    // Index into TASKBAR_BURST_DELAYS_MS of the pending measurement, if a burst is running.
    taskbar_burst_step: Option<usize>,
    hover_interval: u32,
    over_interactive: bool,
}

static MONITOR_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    static MONITOR: RefCell<Option<Monitor>> = const { RefCell::new(None) };
}

// Ignore tiny floating point differences so we only move the viewport when the taskbar layout meaningfully changes.
fn placement_has_changed(old: ([f32; 2], [f32; 2]), new: ([f32; 2], [f32; 2])) -> bool {
    const EPSILON: f32 = 0.5;

    (old.0[0] - new.0[0]).abs() > EPSILON
        || (old.0[1] - new.0[1]).abs() > EPSILON
        || (old.1[0] - new.1[0]).abs() > EPSILON
        || (old.1[1] - new.1[1]).abs() > EPSILON
}

impl Monitor {
    fn schedule_taskbar_burst_step(&mut self, step: usize) {
        self.taskbar_burst_step = Some(step);
        unsafe { SetTimer(Some(self.hwnd), TIMER_TASKBAR_BURST, TASKBAR_BURST_DELAYS_MS[step], None) };
    }

    fn start_taskbar_burst(&mut self) {
        self.schedule_taskbar_burst_step(0);
    }

    fn on_taskbar_burst_tick(&mut self) {
        let Some(step) = self.taskbar_burst_step else {
            let _ = unsafe { KillTimer(Some(self.hwnd), TIMER_TASKBAR_BURST) };
            return;
        };

        if self.measure_taskbar() {
            // Icons may still be sliding into place, so keep watching closely.
            self.schedule_taskbar_burst_step(0);
        } else if step + 1 < TASKBAR_BURST_DELAYS_MS.len() {
            self.schedule_taskbar_burst_step(step + 1);
        } else {
            self.taskbar_burst_step = None;
            let _ = unsafe { KillTimer(Some(self.hwnd), TIMER_TASKBAR_BURST) };
        }
    }

    fn on_taskbar_safety_tick(&mut self) {
        if self.taskbar_burst_step.is_none() && self.measure_taskbar() {
            self.start_taskbar_burst();
        }
    }

    // Re-measures the taskbar gap and wakes the GUI if the overlay needs to move. Returns true if it moved.
    fn measure_taskbar(&mut self) -> bool {
        let Some(placement) =
            gui_placement::get_taskbar_overlay_placement().or_else(gui_placement::get_taskbar_position_and_size)
        else {
            return false;
        };

        let changed = {
            let mut shared = shared().lock().unwrap();
            let changed = placement_has_changed(shared.state.placement, placement);
            if changed {
                shared.state.placement = placement;
            }
            changed
        };

        if changed {
            self.ctx.request_repaint();
        }
        changed
    }

    fn check_fullscreen(&mut self) {
        let fullscreen = gui_util::is_foreground_fullscreen();
        let changed = {
            let mut shared = shared().lock().unwrap();
            let changed = shared.state.fullscreen != fullscreen;
            shared.state.fullscreen = fullscreen;
            changed
        };

        if changed {
            if fullscreen {
                self.over_interactive = false;
            } else {
                self.ensure_on_top();
            }
            self.update_passthrough();
            self.ctx.request_repaint();
        }
    }

    fn is_fullscreen(&self) -> bool {
        shared().lock().unwrap().state.fullscreen
    }

    fn check_dark_mode(&mut self) {
        let Ok(dark_mode) = gui_util::is_dark_mode() else {
            return;
        };
        let changed = {
            let mut shared = shared().lock().unwrap();
            let changed = shared.state.dark_mode != dark_mode;
            shared.state.dark_mode = dark_mode;
            changed
        };
        if changed {
            self.ctx.request_repaint();
        }
    }

    // Keeps the overlay above the taskbar, re-pinning only when the taskbar got above it.
    fn ensure_on_top(&mut self) {
        if self.is_fullscreen() {
            return;
        }
        if gui_util::overlay_below_taskbar() {
            gui_util::pin_overlay_topmost();
        }
    }

    fn update_passthrough(&mut self) {
        let passthrough = self.is_fullscreen() || !self.over_interactive;
        let changed = {
            let mut shared = shared().lock().unwrap();
            let changed = shared.state.mouse_passthrough != passthrough;
            shared.state.mouse_passthrough = passthrough;
            changed
        };
        // The GUI applies it, since viewport commands sent from outside a frame can be dropped.
        if changed {
            self.ctx.request_repaint();
        }
    }

    // Uses the OS cursor position so hover detection works even while mouse passthrough is active.
    fn on_hover_tick(&mut self) {
        let mut cursor = POINT::default();
        let window_rect = gui_util::overlay_window_rect();
        let mut near = false;

        if let (Some(window_rect), Ok(())) = (window_rect, unsafe { GetCursorPos(&mut cursor) }) {
            near = cursor.y >= window_rect.top - HOVER_NEAR_MARGIN && cursor.y <= window_rect.bottom + HOVER_NEAR_MARGIN;

            if !self.is_fullscreen() {
                let over_interactive = near && {
                    let shared = shared().lock().unwrap();
                    let local = egui::pos2(
                        (cursor.x - window_rect.left) as f32 / shared.pixels_per_point,
                        (cursor.y - window_rect.top) as f32 / shared.pixels_per_point,
                    );
                    shared.interactive_rects.iter().any(|rect| rect.contains(local))
                };

                if over_interactive != self.over_interactive {
                    self.over_interactive = over_interactive;
                    if over_interactive {
                        gui_util::pin_overlay_topmost();
                    }
                    self.update_passthrough();
                }

                // Clicking the taskbar raises it above the overlay, so watch the z-order closely while the cursor is on it.
                if near && gui_util::overlay_below_taskbar() {
                    gui_util::pin_overlay_topmost();
                }
            }
        }

        let interval = if near { HOVER_INTERVAL_NEAR_MS } else { HOVER_INTERVAL_FAR_MS };
        if interval != self.hover_interval {
            self.hover_interval = interval;
            unsafe { SetTimer(Some(self.hwnd), TIMER_HOVER, interval, None) };
        }
    }

    fn on_foreground_changed(&mut self) {
        self.check_fullscreen();
        self.ensure_on_top();
        unsafe { SetTimer(Some(self.hwnd), TIMER_FOREGROUND_RECHECK, FOREGROUND_RECHECK_DELAY_MS, None) };
    }

    fn register_appbar(&self) {
        // Registering as an app bar without claiming any space makes the shell report fullscreen apps to us.
        let mut data = APPBARDATA {
            cbSize: size_of::<APPBARDATA>() as u32,
            hWnd: self.hwnd,
            uCallbackMessage: WM_APPBAR_CALLBACK,
            ..Default::default()
        };
        unsafe { SHAppBarMessage(ABM_NEW, &mut data) };
    }

    fn handle_message(&mut self, msg: u32, wparam: WPARAM) {
        match msg {
            WM_TIMER => match wparam.0 {
                TIMER_TASKBAR_BURST => self.on_taskbar_burst_tick(),
                TIMER_TASKBAR_SAFETY => self.on_taskbar_safety_tick(),
                TIMER_ZORDER => {
                    // Cheap enough to double as a backstop for missed fullscreen notifications.
                    self.check_fullscreen();
                    self.ensure_on_top();
                }
                TIMER_FOREGROUND_RECHECK => {
                    let _ = unsafe { KillTimer(Some(self.hwnd), TIMER_FOREGROUND_RECHECK) };
                    self.check_fullscreen();
                    self.ensure_on_top();
                }
                TIMER_HOVER => self.on_hover_tick(),
                TIMER_TRIM_MEMORY => {
                    let _ = unsafe { KillTimer(Some(self.hwnd), TIMER_TRIM_MEMORY) };
                    power::trim_working_set();
                }
                _ => {}
            },
            WM_FOREGROUND_CHANGED => self.on_foreground_changed(),
            WM_APPBAR_CALLBACK if wparam.0 as u32 == ABN_FULLSCREENAPP => self.check_fullscreen(),
            WM_SETTINGCHANGE => {
                self.check_dark_mode();
                self.start_taskbar_burst();
            }
            WM_DISPLAYCHANGE => self.start_taskbar_burst(),
            _ if msg == self.shell_hook_msg => {
                let code = wparam.0 as u32;
                if code == HSHELL_WINDOWCREATED || code == HSHELL_WINDOWDESTROYED || code == HSHELL_WINDOWREPLACED {
                    self.start_taskbar_burst();
                }
            }
            _ if msg == self.taskbar_created_msg => {
                // Explorer restarted, so the app bar registration was lost along with the old taskbar.
                self.register_appbar();
                self.start_taskbar_burst();
            }
            _ => {}
        }
    }

    fn handles(&self, msg: u32) -> bool {
        matches!(msg, WM_TIMER | WM_FOREGROUND_CHANGED | WM_APPBAR_CALLBACK | WM_SETTINGCHANGE | WM_DISPLAYCHANGE)
            || msg == self.shell_hook_msg
            || msg == self.taskbar_created_msg
    }
}

unsafe extern "system" fn monitor_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = MONITOR.with(|monitor| {
        // Calls into other windows can let a sent message re-enter here, so retry it later instead of nesting.
        let Ok(mut monitor) = monitor.try_borrow_mut() else {
            if msg == WM_TIMER || msg >= WM_APP {
                let _ = unsafe { PostMessageW(Some(hwnd), msg, wparam, lparam) };
                return true;
            }
            return false;
        };
        let Some(monitor) = monitor.as_mut() else {
            return false;
        };
        if !monitor.handles(msg) {
            return false;
        }
        monitor.handle_message(msg, wparam);
        true
    });

    if handled {
        LRESULT(0)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Handle it through the message queue so it goes through the same re-entrancy guard as everything else.
    let hwnd = HWND(MONITOR_HWND.load(Ordering::Acquire) as *mut _);
    if !hwnd.0.is_null() {
        let _ = unsafe { PostMessageW(Some(hwnd), WM_FOREGROUND_CHANGED, WPARAM(0), LPARAM(0)) };
    }
}

// Starts the monitor thread, which runs for the rest of the process.
pub fn start(ctx: egui::Context) {
    let _ = shared();

    std::thread::spawn(move || unsafe {
        // UI Automation calls block instead of pumping messages in the multithreaded apartment, so handlers never nest.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let instance = GetModuleHandleW(None).unwrap_or_default().into();
        let class = WNDCLASSW {
            lpfnWndProc: Some(monitor_wndproc),
            hInstance: instance,
            lpszClassName: w!("TaskbarMediaMonitor"),
            ..Default::default()
        };
        RegisterClassW(&class);

        // A hidden top-level window rather than a message-only one, since only top-level windows receive broadcasts.
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("TaskbarMediaMonitor"),
            w!("Taskbar Media Monitor"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        ) else {
            eprintln!("Failed to create the desktop monitor window");
            return;
        };

        let monitor = Monitor {
            ctx,
            hwnd,
            shell_hook_msg: RegisterWindowMessageW(w!("SHELLHOOK")),
            taskbar_created_msg: RegisterWindowMessageW(w!("TaskbarCreated")),
            taskbar_burst_step: None,
            hover_interval: HOVER_INTERVAL_FAR_MS,
            over_interactive: false,
        };

        MONITOR_HWND.store(hwnd.0 as isize, Ordering::Release);
        let _ = RegisterShellHookWindow(hwnd);
        monitor.register_appbar();
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        SetTimer(Some(hwnd), TIMER_TASKBAR_SAFETY, TASKBAR_SAFETY_INTERVAL_MS, None);
        SetTimer(Some(hwnd), TIMER_ZORDER, ZORDER_CHECK_INTERVAL_MS, None);
        SetTimer(Some(hwnd), TIMER_HOVER, HOVER_INTERVAL_FAR_MS, None);
        SetTimer(Some(hwnd), TIMER_TRIM_MEMORY, TRIM_MEMORY_DELAY_MS, None);

        MONITOR.with(|cell| *cell.borrow_mut() = Some(monitor));
        MONITOR.with(|cell| {
            if let Some(monitor) = cell.borrow_mut().as_mut() {
                monitor.check_fullscreen();
                monitor.ensure_on_top();
                monitor.start_taskbar_burst();
            }
        });

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            DispatchMessageW(&message);
        }
    });
}
