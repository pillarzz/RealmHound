//! Narrow Linux capture process protocol. Only this executable needs CAP_NET_RAW.
//! Stdout contains framed packets; stdin carries no commands and closes with the parent.

use super::sniffer::MAX_CAPTURE_FRAME;
use super::{
    packet_channel, CaptureError, CaptureHandle, NetworkInterface, PacketFormat, PacketReceiver,
    RawPacket, Sniffer, SnifferConfig, DEFAULT_QUEUE_CAPACITY,
};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_BUFFER_SIZE: i32 = 64 * 1024;
const MAX_BUFFER_SIZE: i32 = 256 * 1024 * 1024;
const MAX_TIMEOUT_MS: i32 = 60_000;

fn capture_interface_name(override_name: Option<String>) -> String {
    override_name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "any".into())
}

fn bounded(value: &str, flag: &str, min: i32, max: i32) -> Result<i32, String> {
    let parsed = value
        .parse::<i32>()
        .map_err(|_| format!("{flag} expects an integer, got {value:?}"))?;
    if !(min..=max).contains(&parsed) {
        return Err(format!(
            "{flag} must be between {min} and {max}, got {parsed}"
        ));
    }
    Ok(parsed)
}

/// Capture settings the GUI hands over at spawn; a manual launch keeps the defaults.
fn parse_config(args: impl Iterator<Item = OsString>) -> Result<SnifferConfig, String> {
    let mut config = SnifferConfig::default();
    let mut args = args.map(|arg| arg.to_string_lossy().into_owned());
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} expects a value"));
        match arg.as_str() {
            "--filter" => {
                let filter = value()?;
                if filter.trim().is_empty() {
                    return Err("--filter must not be empty".into());
                }
                config.filter = Some(filter);
            }
            "--buffer-size" => {
                config.buffer_size =
                    bounded(&value()?, "--buffer-size", MIN_BUFFER_SIZE, MAX_BUFFER_SIZE)?;
            }
            "--timeout-ms" => {
                config.timeout_ms = bounded(&value()?, "--timeout-ms", 0, MAX_TIMEOUT_MS)?;
            }
            "--promiscuous" => config.promiscuous = true,
            other => return Err(format!("unknown capture argument: {other}")),
        }
    }
    Ok(config)
}

fn helper_args(config: &SnifferConfig) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--buffer-size"),
        OsString::from(config.buffer_size.to_string()),
        OsString::from("--timeout-ms"),
        OsString::from(config.timeout_ms.to_string()),
    ];
    if let Some(filter) = &config.filter {
        args.push(OsString::from("--filter"));
        args.push(OsString::from(filter));
    }
    if config.promiscuous {
        args.push(OsString::from("--promiscuous"));
    }
    args
}

fn report_startup_error(stdout: &mut impl Write, message: &str) -> std::io::Result<()> {
    let bytes = message.as_bytes();
    let len = bytes.len().min(u16::MAX as usize);
    stdout.write_all(&[1])?;
    stdout.write_all(&(len as u16).to_be_bytes())?;
    stdout.write_all(&bytes[..len])?;
    stdout.flush()
}

fn wait_for_parent_disconnect(mut input: impl Read, disconnected: &AtomicBool) {
    let mut buffer = [0u8; 16];
    loop {
        match input.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    disconnected.store(true, Ordering::Release);
}

fn read_startup_response(input: &mut impl Read) -> std::io::Result<Option<String>> {
    let mut status = [0];
    input.read_exact(&mut status)?;
    if status[0] == 0 {
        return Ok(None);
    }

    let mut len = [0; 2];
    input.read_exact(&mut len)?;
    let mut message = vec![0; u16::from_be_bytes(len) as usize];
    input.read_exact(&mut message)?;
    Ok(Some(String::from_utf8_lossy(&message).into_owned()))
}

fn kill_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn await_startup(
    mut output: ChildStdout,
    child: &mut Child,
    timeout: Duration,
) -> Result<ChildStdout, CaptureError> {
    let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
    let startup_reader = std::thread::spawn(move || {
        let response = read_startup_response(&mut output);
        let _ = startup_tx.send((output, response));
    });
    let output = match startup_rx.recv_timeout(timeout) {
        Ok((output, Ok(None))) => output,
        Ok((_output, Ok(Some(message)))) => {
            kill_and_wait(child);
            let _ = startup_reader.join();
            return Err(CaptureError::CaptureError(message));
        }
        Ok((_output, Err(error))) => {
            kill_and_wait(child);
            let _ = startup_reader.join();
            return Err(CaptureError::CaptureError(format!(
                "Capture helper exited during startup: {error}"
            )));
        }
        Err(RecvTimeoutError::Timeout) => {
            kill_and_wait(child);
            return Err(CaptureError::CaptureError(format!(
                "Capture helper did not respond within {} seconds",
                timeout.as_secs_f64()
            )));
        }
        Err(RecvTimeoutError::Disconnected) => {
            kill_and_wait(child);
            let _ = startup_reader.join();
            return Err(CaptureError::CaptureError(
                "Capture helper startup reader stopped unexpectedly".into(),
            ));
        }
    };
    let _ = startup_reader.join();
    Ok(output)
}

/// Entry point for the separately installed, capability-bearing executable.
pub fn run() -> Result<(), String> {
    let parent_disconnected = Arc::new(AtomicBool::new(false));
    let disconnect_signal = parent_disconnected.clone();
    std::thread::spawn(move || {
        wait_for_parent_disconnect(std::io::stdin(), &disconnect_signal);
    });

    let config = match parse_config(std::env::args_os().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            let _ = report_startup_error(&mut std::io::stdout().lock(), &message);
            return Err(message);
        }
    };

    let name = capture_interface_name(std::env::var("REALMHOUND_CAPTURE_INTERFACE").ok());
    let iface = NetworkInterface {
        name,
        description: None,
        addresses: Vec::new(),
    };
    let mut stdout = std::io::stdout().lock();
    let mut sniffer = match Sniffer::new(&iface, config) {
        Ok(sniffer) => sniffer,
        Err(error) => {
            let message = error.to_string();
            let _ = report_startup_error(&mut stdout, &message);
            return Err(message);
        }
    };
    stdout.write_all(&[0]).map_err(|e| e.to_string())?;
    stdout.flush().map_err(|e| e.to_string())?;
    let mut oversized_dropped = 0u64;
    while !parent_disconnected.load(Ordering::Acquire) {
        match sniffer.next_packet() {
            Ok(Some(packet)) => {
                if packet.data.len() > MAX_CAPTURE_FRAME {
                    oversized_dropped += 1;
                    eprintln!(
                        "oversized capture frames dropped: {oversized_dropped} total (latest was {} bytes; maximum is {MAX_CAPTURE_FRAME})",
                        packet.data.len()
                    );
                    continue;
                }
                let secs = packet.timestamp.timestamp();
                let nanos = packet.timestamp.timestamp_subsec_nanos();
                stdout
                    .write_all(&(packet.data.len() as u32).to_be_bytes())
                    .map_err(|e| e.to_string())?;
                stdout
                    .write_all(&[packet.packet_format.code()])
                    .map_err(|e| e.to_string())?;
                stdout
                    .write_all(&secs.to_be_bytes())
                    .map_err(|e| e.to_string())?;
                stdout
                    .write_all(&nanos.to_be_bytes())
                    .map_err(|e| e.to_string())?;
                stdout.write_all(&packet.data).map_err(|e| e.to_string())?;
            }
            Ok(None) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn read_packet(input: &mut impl Read) -> std::io::Result<RawPacket> {
    let mut header = [0u8; 17];
    input.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
    if len > MAX_CAPTURE_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "capture frame too large",
        ));
    }
    let format = PacketFormat::from_code(header[4]).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid capture format")
    })?;
    let secs = i64::from_be_bytes(header[5..13].try_into().unwrap());
    let nanos = u32::from_be_bytes(header[13..17].try_into().unwrap());
    let timestamp = chrono::DateTime::from_timestamp(secs, nanos).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid capture timestamp")
    })?;
    let mut data = vec![0; len];
    input.read_exact(&mut data)?;
    Ok(RawPacket {
        timestamp,
        packet_format: format,
        #[cfg(feature = "latency-diagnostics")]
        enqueued_at: None,
        data,
    })
}

/// Spawn the helper beside the GUI and bridge its stdout into the existing queue.
pub fn start(
    config: &SnifferConfig,
) -> Result<
    (
        CaptureHandle,
        PacketReceiver,
        tokio::sync::oneshot::Receiver<String>,
    ),
    CaptureError,
> {
    let exe = std::env::current_exe().map_err(|e| CaptureError::CaptureError(e.to_string()))?;
    let helper = exe.with_file_name("realmhound-capture");
    let mut child = Command::new(&helper)
        .args(helper_args(config))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CaptureError::CaptureError(format!("Cannot launch {}: {e}. Install the Linux capture helper beside RealmHound and run the one-time setup.", helper.display())))?;
    let output = child.stdout.take().unwrap();
    let errors = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(errors).lines().map_while(Result::ok) {
            tracing::warn!("Linux capture helper: {line}");
        }
    });
    let mut output = await_startup(output, &mut child, STARTUP_TIMEOUT)?;
    let child = Arc::new(Mutex::new(child));
    let handle = CaptureHandle::with_linux_child(child.clone());
    let (tx, rx) = packet_channel(DEFAULT_QUEUE_CAPACITY);
    let (detected_tx, detected_rx) = tokio::sync::oneshot::channel();
    let _ = detected_tx.send(capture_interface_name(
        std::env::var("REALMHOUND_CAPTURE_INTERFACE").ok(),
    ));
    std::thread::spawn(move || {
        while let Ok(packet) = read_packet(&mut output) {
            if tx.send(packet).is_err() {
                break;
            }
        }
        if let Ok(mut child) = child.lock() {
            kill_and_wait(&mut child);
        }
        tracing::debug!("Linux capture helper stopped");
    });
    Ok((handle, rx, detected_rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_parent_pipe_signals_disconnect() {
        let disconnected = AtomicBool::new(false);

        wait_for_parent_disconnect(std::io::empty(), &disconnected);

        assert!(disconnected.load(Ordering::Acquire));
    }

    #[test]
    fn reads_ready_startup_response() {
        assert_eq!(read_startup_response(&mut [0].as_slice()).unwrap(), None);
    }

    #[test]
    fn reads_error_startup_response() {
        let mut response = [1, 0, 4, b'o', b'o', b'p', b's'].as_slice();

        assert_eq!(
            read_startup_response(&mut response).unwrap(),
            Some("oops".into())
        );
    }

    #[test]
    fn rejects_truncated_startup_response() {
        let error = read_startup_response(&mut [1, 0, 4, b'o'].as_slice()).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn startup_timeout_kills_and_reaps_child() {
        let mut child = Command::new("sleep")
            .arg("60")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let output = child.stdout.take().unwrap();

        let error = await_startup(output, &mut child, Duration::from_millis(50)).unwrap_err();

        assert!(error.to_string().contains("did not respond"));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn startup_read_error_reaps_child() {
        let mut child = Command::new("true").stdout(Stdio::piped()).spawn().unwrap();
        let output = child.stdout.take().unwrap();

        let error = await_startup(output, &mut child, Duration::from_secs(1)).unwrap_err();

        assert!(error.to_string().contains("exited during startup"));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn frame_round_trip() {
        let packet = RawPacket {
            timestamp: chrono::Utc::now(),
            packet_format: PacketFormat::LinuxSll2,
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
            data: vec![1, 2, 3],
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(packet.data.len() as u32).to_be_bytes());
        bytes.push(packet.packet_format.code());
        bytes.extend_from_slice(&packet.timestamp.timestamp().to_be_bytes());
        bytes.extend_from_slice(&packet.timestamp.timestamp_subsec_nanos().to_be_bytes());
        bytes.extend_from_slice(&packet.data);
        let decoded = read_packet(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded.data, packet.data);
        assert_eq!(decoded.packet_format, PacketFormat::LinuxSll2);
    }

    #[test]
    fn accepts_frame_larger_than_maximum_ip_packet() {
        let data = vec![0; usize::from(u16::MAX) + 20];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.push(PacketFormat::LinuxSll2.code());
        bytes.extend_from_slice(&0i64.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&data);

        let decoded = read_packet(&mut bytes.as_slice()).unwrap();
        assert_eq!(decoded.data.len(), data.len());
    }

    #[test]
    fn rejects_frame_larger_than_capture_limit() {
        let mut header = [0u8; 17];
        header[..4].copy_from_slice(&((MAX_CAPTURE_FRAME + 1) as u32).to_be_bytes());

        let error = read_packet(&mut header.as_slice()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn defaults_to_any_with_manual_override() {
        assert_eq!(capture_interface_name(None), "any");
        assert_eq!(capture_interface_name(Some("eth0".into())), "eth0");
        assert_eq!(capture_interface_name(Some(" ".into())), "any");
    }

    #[test]
    fn helper_args_round_trip_through_parser() {
        let config = SnifferConfig {
            timeout_ms: 250,
            buffer_size: 1 << 20,
            promiscuous: true,
            filter: Some("tcp port 2050 and ip".into()),
        };

        assert_eq!(
            parse_config(helper_args(&config).into_iter()).unwrap(),
            config
        );
    }

    #[test]
    fn missing_args_keep_defaults() {
        assert_eq!(
            parse_config(std::iter::empty()).unwrap(),
            SnifferConfig::default()
        );
    }

    #[test]
    fn rejects_unknown_flags_and_out_of_range_values() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();

        for values in [
            vec!["--unknown"],
            vec!["--filter"],
            vec!["--filter", ""],
            vec!["--buffer-size", "1024"],
            vec!["--buffer-size", "huge"],
            vec!["--timeout-ms", "60001"],
        ] {
            let parsed = parse_config(args(&values).into_iter());
            assert!(parsed.is_err(), "{values:?} should be rejected");
        }
    }
}
