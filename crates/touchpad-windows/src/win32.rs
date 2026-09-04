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

use touchpad_core::MouseButton;

use crate::{
    WindowsError, WindowsOutputApi, WindowsTouchpadDevice, PRECISION_TOUCHPAD_USAGE,
    PRECISION_TOUCHPAD_USAGE_PAGE,
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
