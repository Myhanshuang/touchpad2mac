//! Minimal Win32 FFI for the Windows platform boundary.
//!
//! The FFI is intentionally tiny and uses only ABI-stable C layouts from
//! `winuser.h`. New synthetic-touchpad APIs are probed by export name instead
//! of linked directly, so an older Windows 11 build can report the feature as
//! unavailable rather than failing process startup.

#![allow(unsafe_code)]

use std::ffi::{c_void, CString};
use std::mem::size_of;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use touchpad_core::MouseButton;

use crate::{
    WindowsCaptureSummary, WindowsError, WindowsOutputApi, WindowsRawHidReport,
    WindowsTouchpadDevice, PRECISION_TOUCHPAD_USAGE, PRECISION_TOUCHPAD_USAGE_PAGE,
};

type Handle = *mut c_void;
type HModule = *mut c_void;

const RIM_TYPEHID: u32 = 2;
const RIDI_DEVICENAME: u32 = 0x2000_0007;
const RIDI_DEVICEINFO: u32 = 0x2000_000b;
const UINT_ERROR: u32 = u32::MAX;

const INPUT_MOUSE: u32 = 0;
const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
const MOUSEEVENTF_XDOWN: u32 = 0x0080;
const MOUSEEVENTF_XUP: u32 = 0x0100;
const MOUSEEVENTF_WHEEL: u32 = 0x0800;
const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;
const RIDEV_INPUTSINK: u32 = 0x0000_0100;
const RID_INPUT: u32 = 0x1000_0003;
const WM_INPUT: u32 = 0x00ff;
const PM_REMOVE: u32 = 0x0001;

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInputDeviceList {
    h_device: Handle,
    dw_type: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RidDeviceInfoHid {
    dw_vendor_id: u32,
    dw_product_id: u32,
    dw_version_number: u32,
    us_usage_page: u16,
    us_usage: u16,
}

#[repr(C)]
union RidDeviceInfoUnion {
    hid: RidDeviceInfoHid,
    alignment: [u64; 3],
}

#[repr(C)]
struct RidDeviceInfo {
    cb_size: u32,
    dw_type: u32,
    data: RidDeviceInfoUnion,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MouseInput {
    dx: i32,
    dy: i32,
    mouse_data: u32,
    dw_flags: u32,
    time: u32,
    dw_extra_info: usize,
}

#[repr(C)]
union InputUnion {
    mi: MouseInput,
}

#[repr(C)]
struct Input {
    input_type: u32,
    data: InputUnion,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInputDevice {
    usage_page: u16,
    usage: u16,
    flags: u32,
    target: Handle,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInputHeaderLayout {
    input_type: u32,
    size: u32,
    device: Handle,
    w_param: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawHidPrefix {
    size_hid: u32,
    count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Message {
    hwnd: Handle,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    point: Point,
    private: u32,
}

impl Default for Message {
    fn default() -> Self {
        Self {
            hwnd: null_mut(),
            message: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            point: Point::default(),
            private: 0,
        }
    }
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetRawInputDeviceList(
        raw_input_device_list: *mut RawInputDeviceList,
        num_devices: *mut u32,
        size: u32,
    ) -> u32;
    fn GetRawInputDeviceInfoW(
        device: Handle,
        command: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> u32;
    fn SendInput(count: u32, inputs: *const Input, size: i32) -> u32;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: *const c_void,
    ) -> Handle;
    fn DestroyWindow(window: Handle) -> i32;
    fn RegisterRawInputDevices(devices: *const RawInputDevice, count: u32, size: u32) -> i32;
    fn GetRawInputData(
        raw_input: Handle,
        command: u32,
        data: *mut c_void,
        size: *mut u32,
        header_size: u32,
    ) -> u32;
    fn PeekMessageW(
        message: *mut Message,
        window: Handle,
        min_message: u32,
        max_message: u32,
        remove: u32,
    ) -> i32;
    fn TranslateMessage(message: *const Message) -> i32;
    fn DispatchMessageW(message: *const Message) -> isize;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(module_name: *const u16) -> HModule;
    fn GetProcAddress(module: HModule, proc_name: *const u8) -> *mut c_void;
}

/// Real support-probe implementation marker.
pub(crate) struct Win32SupportApi;

/// Real `SendInput` implementation marker.
pub(crate) struct Win32OutputApi;

pub(crate) fn enumerate_touchpads() -> Result<Vec<WindowsTouchpadDevice>, WindowsError> {
    let mut count = 0u32;
    // SAFETY: null buffer is the documented size-query form. `count` points
    // to writable storage and `size` matches `RAWINPUTDEVICELIST`.
    let status = unsafe {
        GetRawInputDeviceList(
            null_mut(),
            &mut count,
            size_of::<RawInputDeviceList>() as u32,
        )
    };
    if status == UINT_ERROR {
        return Err(WindowsError::last_os_error("GetRawInputDeviceList(size)"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut list = vec![
        RawInputDeviceList {
            h_device: null_mut(),
            dw_type: 0,
        };
        count as usize
    ];
    let mut actual = count;
    // SAFETY: `list` has `count` writable entries and the ABI size matches.
    let status = unsafe {
        GetRawInputDeviceList(
            list.as_mut_ptr(),
            &mut actual,
            size_of::<RawInputDeviceList>() as u32,
        )
    };
    if status == UINT_ERROR {
        return Err(WindowsError::last_os_error("GetRawInputDeviceList(data)"));
    }
    list.truncate(actual as usize);

    let mut out = Vec::new();
    for item in list {
        if item.dw_type != RIM_TYPEHID || item.h_device.is_null() {
            continue;
        }
        let mut info = RidDeviceInfo {
            cb_size: size_of::<RidDeviceInfo>() as u32,
            dw_type: 0,
            data: RidDeviceInfoUnion { alignment: [0; 3] },
        };
        let mut info_size = size_of::<RidDeviceInfo>() as u32;
        // SAFETY: `info` is a correctly sized writable RID_DEVICE_INFO and
        // `item.h_device` came from GetRawInputDeviceList.
        let status = unsafe {
            GetRawInputDeviceInfoW(
                item.h_device,
                RIDI_DEVICEINFO,
                (&mut info as *mut RidDeviceInfo).cast(),
                &mut info_size,
            )
        };
        if status == UINT_ERROR || info.dw_type != RIM_TYPEHID {
            continue;
        }
        // SAFETY: dwType == RIM_TYPEHID selects the `hid` union member.
        let hid = unsafe { info.data.hid };
        if hid.us_usage_page != PRECISION_TOUCHPAD_USAGE_PAGE
            || hid.us_usage != PRECISION_TOUCHPAD_USAGE
        {
            continue;
        }
        out.push(WindowsTouchpadDevice {
            device_name: raw_input_device_name(item.h_device)
                .unwrap_or_else(|_| "<unavailable raw-input path>".to_string()),
            vendor_id: hid.dw_vendor_id,
            product_id: hid.dw_product_id,
            version_number: hid.dw_version_number,
            usage_page: hid.us_usage_page,
            usage: hid.us_usage,
        });
    }
    out.sort_by(|a, b| a.device_name.cmp(&b.device_name));
    Ok(out)
}

fn raw_input_device_name(device: Handle) -> Result<String, WindowsError> {
    let mut chars = 0u32;
    // SAFETY: null data requests the required UTF-16 character count.
    let status = unsafe { GetRawInputDeviceInfoW(device, RIDI_DEVICENAME, null_mut(), &mut chars) };
    if status == UINT_ERROR {
        return Err(WindowsError::last_os_error(
            "GetRawInputDeviceInfoW(name-size)",
        ));
    }
    let mut buffer = vec![0u16; chars.saturating_add(1) as usize];
    let mut writable = chars;
    // SAFETY: the buffer holds at least the character count returned by the
    // size query and `writable` names that capacity to user32.
    let status = unsafe {
        GetRawInputDeviceInfoW(
            device,
            RIDI_DEVICENAME,
            buffer.as_mut_ptr().cast(),
            &mut writable,
        )
    };
    if status == UINT_ERROR {
        return Err(WindowsError::last_os_error(
            "GetRawInputDeviceInfoW(name-data)",
        ));
    }
    let len = buffer
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(writable as usize);
    Ok(String::from_utf16_lossy(&buffer[..len]))
}

pub(crate) fn synthetic_touchpad_exports_available() -> bool {
    let user32: Vec<u16> = "user32.dll\0".encode_utf16().collect();
    // SAFETY: `user32` is NUL-terminated and remains alive for the call.
    let module = unsafe { GetModuleHandleW(user32.as_ptr()) };
    if module.is_null() {
        return false;
    }
    [
        "CreateSyntheticPointerDevice2",
        "InjectSyntheticPointerInput",
        "InjectTouchpadAction",
        "DestroySyntheticPointerDevice",
    ]
    .iter()
    .all(|name| {
        let name = CString::new(*name).expect("static export names contain no NUL");
        // SAFETY: `module` is a loaded user32 module and the C string is
        // NUL-terminated for the duration of the call.
        !unsafe { GetProcAddress(module, name.as_ptr().cast()) }.is_null()
    })
}

pub(crate) fn capture_precision_touchpad_raw_input(
    duration: Duration,
    on_report: &mut dyn FnMut(WindowsRawHidReport) -> Result<(), WindowsError>,
) -> Result<WindowsCaptureSummary, WindowsError> {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let message_only_parent = (-3isize) as Handle;
    // SAFETY: `STATIC` is a built-in class. The window is message-only and
    // never shown; all pointer arguments remain valid for this call.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            null_mut(),
            0,
            0,
            0,
            0,
            0,
            message_only_parent,
            null_mut(),
            null_mut(),
            null_mut(),
        )
    };
    if window.is_null() {
        return Err(WindowsError::last_os_error("CreateWindowExW(message-only)"));
    }

    struct WindowGuard(Handle);
    impl Drop for WindowGuard {
        fn drop(&mut self) {
            // SAFETY: handle was created in this scope and remains owned here.
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    let _window_guard = WindowGuard(window);

    let registration = RawInputDevice {
        usage_page: PRECISION_TOUCHPAD_USAGE_PAGE,
        usage: PRECISION_TOUCHPAD_USAGE,
        flags: RIDEV_INPUTSINK,
        target: window,
    };
    // SAFETY: one valid RAWINPUTDEVICE layout and a live target HWND.
    let registered =
        unsafe { RegisterRawInputDevices(&registration, 1, size_of::<RawInputDevice>() as u32) };
    if registered == 0 {
        return Err(WindowsError::last_os_error("RegisterRawInputDevices(PTP)"));
    }

    let deadline = Instant::now()
        .checked_add(duration)
        .ok_or_else(|| WindowsError::Unsupported("capture duration overflow".to_string()))?;
    let mut summary = WindowsCaptureSummary::default();
    while Instant::now() < deadline {
        let mut saw_message = false;
        loop {
            let mut message = Message::default();
            // SAFETY: message is writable and `window` remains alive.
            let available = unsafe { PeekMessageW(&mut message, window, 0, 0, PM_REMOVE) };
            if available == 0 {
                break;
            }
            saw_message = true;
            if message.message == WM_INPUT {
                let reports = read_raw_hid(message.l_param as Handle)?;
                if !reports.is_empty() {
                    summary.raw_input_messages = summary.raw_input_messages.saturating_add(1);
                }
                for report in reports {
                    summary.hid_reports = summary.hid_reports.saturating_add(1);
                    summary.hid_bytes = summary.hid_bytes.saturating_add(report.bytes.len() as u64);
                    on_report(report)?;
                }
            }
            // Let the built-in window procedure perform normal message
            // cleanup after we have copied the raw input payload.
            unsafe {
                let _ = TranslateMessage(&message);
                let _ = DispatchMessageW(&message);
            }
        }
        if !saw_message {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(summary)
}

fn read_raw_hid(raw_input: Handle) -> Result<Vec<WindowsRawHidReport>, WindowsError> {
    let header_size = size_of::<RawInputHeaderLayout>() as u32;
    let mut bytes = 0u32;
    // SAFETY: null data is the documented size-query form.
    let query =
        unsafe { GetRawInputData(raw_input, RID_INPUT, null_mut(), &mut bytes, header_size) };
    if query == UINT_ERROR {
        return Err(WindowsError::last_os_error("GetRawInputData(size)"));
    }
    if bytes < header_size + size_of::<RawHidPrefix>() as u32 {
        return Err(WindowsError::Decode(format!(
            "RAWINPUT payload is too small: {bytes} bytes"
        )));
    }

    let mut buffer = vec![0u8; bytes as usize];
    let mut writable = bytes;
    // SAFETY: buffer has `writable` bytes and header_size matches the native
    // RAWINPUTHEADER layout represented above.
    let copied = unsafe {
        GetRawInputData(
            raw_input,
            RID_INPUT,
            buffer.as_mut_ptr().cast(),
            &mut writable,
            header_size,
        )
    };
    if copied == UINT_ERROR {
        return Err(WindowsError::last_os_error("GetRawInputData(data)"));
    }
    let minimum = size_of::<RawInputHeaderLayout>() + size_of::<RawHidPrefix>();
    if (copied as usize) < minimum {
        return Err(WindowsError::Decode(
            "RAWINPUT copy was shorter than header + RAWHID prefix".to_string(),
        ));
    }

    // SAFETY: fixed-prefix sizes are checked above; unaligned reads avoid any
    // dependence on Vec<u8>'s alignment.
    let header =
        unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<RawInputHeaderLayout>()) };
    if header.input_type != RIM_TYPEHID {
        return Ok(Vec::new());
    }
    let hid_offset = size_of::<RawInputHeaderLayout>();
    let hid =
        unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(hid_offset).cast::<RawHidPrefix>()) };
    if hid.size_hid == 0 || hid.count == 0 {
        return Ok(Vec::new());
    }
    let report_size = hid.size_hid as usize;
    let payload_len = report_size
        .checked_mul(hid.count as usize)
        .ok_or_else(|| WindowsError::Decode("RAWHID payload length overflow".to_string()))?;
    let raw_offset = hid_offset + size_of::<RawHidPrefix>();
    let end = raw_offset
        .checked_add(payload_len)
        .ok_or_else(|| WindowsError::Decode("RAWHID payload offset overflow".to_string()))?;
    if end > copied as usize || end > buffer.len() {
        return Err(WindowsError::Decode(format!(
            "RAWHID advertises {payload_len} bytes beyond copied RAWINPUT payload"
        )));
    }

    let mut reports = Vec::with_capacity(hid.count as usize);
    for index in 0..hid.count {
        let start = raw_offset + index as usize * report_size;
        reports.push(WindowsRawHidReport {
            device_handle: header.device as usize,
            batch_index: index,
            bytes: buffer[start..start + report_size].to_vec(),
        });
    }
    Ok(reports)
}

impl WindowsOutputApi for Win32OutputApi {
    fn relative_move(&mut self, dx: i32, dy: i32) -> Result<(), WindowsError> {
        send_mouse(MouseInput {
            dx,
            dy,
            dw_flags: MOUSEEVENTF_MOVE,
            ..MouseInput::default()
        })
    }

    fn button(&mut self, button: MouseButton, down: bool) -> Result<(), WindowsError> {
        let (flags, mouse_data) = match (button, down) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Other(1), true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::Other(1), false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::Other(2), true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::Other(2), false) => (MOUSEEVENTF_XUP, XBUTTON2),
            (MouseButton::Other(code), _) => {
                return Err(WindowsError::Unsupported(format!(
                    "mouse button Other({code}); only XBUTTON1/2 are mapped"
                )))
            }
            _ => {
                return Err(WindowsError::Unsupported(
                    "unknown mouse button variant".to_string(),
                ))
            }
        };
        send_mouse(MouseInput {
            mouse_data,
            dw_flags: flags,
            ..MouseInput::default()
        })
    }

    fn wheel(&mut self, horizontal: bool, delta: i32) -> Result<(), WindowsError> {
        send_mouse(MouseInput {
            mouse_data: delta as u32,
            dw_flags: if horizontal {
                MOUSEEVENTF_HWHEEL
            } else {
                MOUSEEVENTF_WHEEL
            },
            ..MouseInput::default()
        })
    }
}

fn send_mouse(mouse: MouseInput) -> Result<(), WindowsError> {
    let input = Input {
        input_type: INPUT_MOUSE,
        data: InputUnion { mi: mouse },
    };
    // SAFETY: `input` is a valid one-element INPUT array whose union member
    // matches INPUT_MOUSE; user32 copies it during the call.
    let inserted = unsafe { SendInput(1, &input, size_of::<Input>() as i32) };
    if inserted == 1 {
        Ok(())
    } else {
        Err(WindowsError::last_os_error("SendInput"))
    }
}
