//! Launch the explicitly configured converter or the macOS app's bundled exporter.

use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::AepProgress;

fn bundled_exporter(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" || app.extension()? != "app" {
        return None;
    }
    Some(contents.join("Resources/aep_ir_export.py"))
}

pub(super) fn convert(path: &str, progress: &AepProgress) -> io::Result<Vec<u8>> {
    let script = std::env::var_os("EFFECTCRAFT_AEP_EXPORTER")
        .map(PathBuf::from)
        .or_else(|| {
            if !cfg!(target_os = "macos") {
                return None;
            }
            bundled_exporter(&std::env::current_exe().ok()?).filter(|p| p.is_file())
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "AEP exporter script missing; set EFFECTCRAFT_AEP_EXPORTER"))?;
    let python = std::env::var_os("EFFECTCRAFT_AEP_PYTHON").unwrap_or_else(|| "python3".into());
    let mut command = Command::new(python);
    command.arg(script).arg(path).arg("--stdout").arg("--progress").env("PYTHONIOENCODING", "utf-8");
    capture(&mut command, 512 * 1024 * 1024, progress)
}

/// A converter progress line on stderr (`--progress`): `@progress DONE TOTAL MESSAGE`.
fn progress_line(line: &[u8]) -> Option<(u64, u64, String)> {
    let text = String::from_utf8_lossy(line.strip_prefix(b"@progress ")?);
    let mut parts = text.trim_end().splitn(3, ' ');
    let done = parts.next()?.parse().ok()?;
    let total = parts.next()?.parse().ok()?;
    Some((done, total, parts.next().unwrap_or_default().to_string()))
}

fn capture(command: &mut Command, limit: u64, progress: &AepProgress) -> io::Result<Vec<u8>> {
    // Converted projects never enter shared temporary storage. Drain diagnostics concurrently
    // so a noisy converter cannot fill its stderr pipe and deadlock the stdout reader; progress
    // lines are reported from there, and `progress` returning false stops the converter.
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("AEP converter has no output or diagnostic pipe"));
    };
    let child = Mutex::new(child);
    let kill = || {
        let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
    };
    let cancelled = AtomicBool::new(false);
    let outcome = std::thread::scope(|scope| {
        let diagnostics = std::thread::Builder::new().name("aep-diagnostics".into()).spawn_scoped(scope, || -> io::Result<Vec<u8>> {
            let (mut stderr, mut line, mut bytes) = (BufReader::new(stderr), Vec::new(), Vec::new());
            loop {
                line.clear();
                if (&mut stderr).take(4096).read_until(b'\n', &mut line)? == 0 {
                    return Ok(bytes);
                }
                if let Some((done, total, message)) = progress_line(&line) {
                    if !progress(done, total, &message) && !cancelled.swap(true, Ordering::Relaxed) {
                        kill();
                    }
                } else {
                    let room = (64 * 1024usize).saturating_sub(bytes.len());
                    bytes.extend_from_slice(&line[..line.len().min(room)]);
                }
            }
        });
        let diagnostics = match diagnostics {
            Ok(thread) => thread,
            Err(error) => {
                kill();
                return Err(error);
            }
        };
        let read = (|| {
            let mut bytes = Vec::new();
            stdout.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                return Err(io::Error::other(format!("AEP conversion exceeds {limit} bytes")));
            }
            Ok(bytes)
        })();
        if read.is_err() {
            kill();
        }
        Ok((read, diagnostics.join()))
    });
    let status = child.into_inner().unwrap_or_else(|e| e.into_inner()).wait();
    let (read, diagnostics) = outcome?;
    let diagnostics = diagnostics.map_err(|_| io::Error::other("AEP diagnostic reader failed"))?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "AEP conversion cancelled"));
    }
    let bytes = read?;
    if !status?.success() {
        return Err(io::Error::other(format!(
            "AEP export failed (set EFFECTCRAFT_AEP_PYTHON to an interpreter with py-aep installed): {}",
            String::from_utf8_lossy(&diagnostics?).trim()
        )));
    }
    diagnostics?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_app_bundles_supply_an_automatic_exporter() {
        assert_eq!(
            bundled_exporter(Path::new("/Applications/EffectCraft.app/Contents/MacOS/effectcraft")),
            Some(PathBuf::from("/Applications/EffectCraft.app/Contents/Resources/aep_ir_export.py"))
        );
        for exe in [
            "/Downloads/bin/effectcraft",
            "/Downloads/MacOS/effectcraft",
            "/Downloads/Contents/MacOS/effectcraft",
            "/Downloads/Fake/Contents/MacOS/effectcraft",
        ] {
            assert_eq!(bundled_exporter(Path::new(exe)), None, "{exe}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn converter_output_is_bounded_and_failures_keep_diagnostics() {
        let run = |script: &str, limit| capture(Command::new("sh").args(["-c", script]), limit, &|_, _, _| true);
        assert_eq!(run("printf '{\"items\":[]}'", 12).unwrap(), b"{\"items\":[]}");
        assert_eq!(run("printf 'é π'", 5).unwrap(), "é π".as_bytes());
        assert!(run("printf '12345'", 4).unwrap_err().to_string().contains("exceeds"));
        assert!(run("printf 'bad project' >&2; exit 1", 32).unwrap_err().to_string().contains("bad project"));
        assert_eq!(run("head -c 100000 /dev/zero >&2; printf ok", 2).unwrap(), b"ok");
    }

    #[cfg(unix)]
    #[test]
    fn converter_progress_is_reported_apart_from_diagnostics_and_can_stop_it() {
        let seen = Mutex::new(vec![]);
        let report = |done, total, message: &str| {
            seen.lock().unwrap().push((done, total, message.to_string()));
            true
        };
        let script = "printf '@progress 1 4 Reading compositions (1/2)\nbad project\n' >&2; exit 1";
        let error = capture(Command::new("sh").args(["-c", script]), 32, &report).unwrap_err().to_string();
        assert!(error.contains("bad project") && !error.contains("@progress"), "{error}");
        assert_eq!(*seen.lock().unwrap(), [(1, 4, "Reading compositions (1/2)".to_string())]);

        let started = std::time::Instant::now();
        let script = "printf '@progress 1 4 Reading\n' >&2; exec sleep 30";
        let error = capture(Command::new("sh").args(["-c", script]), 32, &|_, _, _| false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed().as_secs() < 10);
    }
}
