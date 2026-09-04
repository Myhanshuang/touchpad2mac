//! Windows hardware bring-up commands.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::env::CommandEnv;
use crate::exit::CommandFailure;

/// Runs a bounded read-only Precision Touchpad Raw Input capture and writes
/// one JSON object per HID report. No keyboard reports are registered, no
/// synthetic input is emitted, and Windows continues to own native touchpad
/// behavior throughout the capture.
pub fn run_capture(
    env: &mut CommandEnv<'_>,
    output: &Path,
    seconds: u32,
) -> Result<(), CommandFailure> {
    let touchpads = touchpad_windows::enumerate_touchpads().map_err(windows_failure)?;
    if touchpads.is_empty() {
        return Err(CommandFailure::Unexpected(
            "Windows Raw Input did not enumerate a Precision Touchpad (usage page 0x0D, usage 0x05)"
                .to_string(),
        ));
    }

    let file = File::create(output).map_err(|error| {
        CommandFailure::Unexpected(format!(
            "could not create Windows capture {}: {error}",
            output.display()
        ))
    })?;
    let mut writer = BufWriter::new(file);
    let device_json: Vec<_> = touchpads
        .iter()
        .map(|device| {
            serde_json::json!({
                "path": device.device_name,
                "vendor_id": device.vendor_id,
                "product_id": device.product_id,
                "version_number": device.version_number,
                "usage_page": device.usage_page,
                "usage": device.usage,
            })
        })
        .collect();
    write_json_line(
        &mut writer,
        &serde_json::json!({
            "type": "header",
            "schema_version": 1,
            "capture_seconds": seconds,
            "privacy": "contains raw touchpad HID reports (touch positions may be encoded); contains no keyboard registration or key codes",
            "devices": device_json,
        }),
    )?;
    writer.flush().map_err(io_failure)?;

    writeln!(
        env.err,
        "Windows PTP capture: {} device(s), {} second(s), output={} (read-only; native Windows touchpad handling remains enabled)",
        touchpads.len(),
        seconds,
        output.display()
    )
    .map_err(io_failure)?;

    let started = Instant::now();
    let summary = touchpad_windows::capture_precision_touchpad_raw_input(
        Duration::from_secs(u64::from(seconds)),
        |report| {
            let record = serde_json::json!({
                "type": "hid-report",
                "elapsed_ns": u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                "device_handle": format!("0x{:x}", report.device_handle),
                "batch_index": report.batch_index,
                "length": report.bytes.len(),
                "hex": hex(&report.bytes),
            });
            write_json_line(&mut writer, &record)
                .and_then(|()| writer.flush().map_err(io_failure))
                .map_err(|error| touchpad_windows::WindowsError::Pipeline(error.to_string()))
        },
    )
    .map_err(windows_failure)?;

    write_json_line(
        &mut writer,
        &serde_json::json!({
            "type": "summary",
            "raw_input_messages": summary.raw_input_messages,
            "hid_reports": summary.hid_reports,
            "hid_bytes": summary.hid_bytes,
        }),
    )?;
    writer.flush().map_err(io_failure)?;

    writeln!(
        env.out,
        "captured {} HID report(s), {} byte(s) from {} WM_INPUT message(s)",
        summary.hid_reports, summary.hid_bytes, summary.raw_input_messages
    )
    .map_err(io_failure)?;
    Ok(())
}

fn write_json_line(
    writer: &mut dyn Write,
    value: &serde_json::Value,
) -> Result<(), CommandFailure> {
    serde_json::to_writer(&mut *writer, value).map_err(|error| {
        CommandFailure::Unexpected(format!("could not encode Windows capture JSON: {error}"))
    })?;
    writer.write_all(b"\n").map_err(io_failure)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn windows_failure(error: touchpad_windows::WindowsError) -> CommandFailure {
    CommandFailure::Unexpected(error.to_string())
}

fn io_failure(error: std::io::Error) -> CommandFailure {
    CommandFailure::Unexpected(format!("Windows capture I/O failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_stable_lowercase_and_lossless_length() {
        assert_eq!(hex(&[0x00, 0x0f, 0x10, 0xab, 0xff]), "000f10abff");
    }
}
