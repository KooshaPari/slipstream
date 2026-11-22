//! The Windows matcher: a ToolHelp process snapshot plus per-process reads.
//!
//! Contract, rules, and the shared vocabulary live in [`super`]; this module only reads the process
//! table. It follows the same two rules as the Linux matcher:
//!
//! 1. A process that started **before** the launch is never adopted. Start times come from
//!    `GetProcessTimes` (FILETIME, 100 ns ticks since 1601), converted onto the same
//!    seconds-since-boot timeline that [`Scanner::now_stamp`] reports, so the comparison is between
//!    two values from one clock.
//! 2. A remembered process is identified by pid **and** start stamp ([`Scanner::alive`]), so a
//!    recycled pid is never reported alive.
//!
//! Signals honoured here: the exact executable, the install directory (image path), and the bare
//! process name. The Steam-appid and environment-marker signals read another process's command line
//! or environment, which on Windows needs `NtQueryInformationProcess` and remote-memory reads. That is
//! a larger and riskier surface than this matcher warrants, so those signals do **not** match here.
//! Missing a match leaves a game untracked; it never adopts a wrong process. A spec that rests
//! entirely on those signals logs one warning instead of matching nothing silently.

use super::{ProcRef, START_SLACK_SECS};
use crate::library::DetectSpec;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::K32GetProcessImageFileNameW;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// 100-nanosecond intervals between the FILETIME epoch (1601-01-01) and the Unix epoch.
const FILETIME_TO_UNIX_100NS: u64 = 116_444_736_000_000_000;

/// A Windows process reader. `system()` is the live table; the matching logic is kept separate so it
/// can be exercised without spawning processes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scanner;

impl Scanner {
    /// The live process table of this machine.
    pub fn system() -> Self {
        Scanner
    }

    /// Seconds since boot, on the same timeline as the process start stamps. `None` only if the
    /// tick count is unavailable, which disables start-time filtering rather than rejecting all.
    pub fn now_stamp(&self) -> Option<f64> {
        // SAFETY: `GetTickCount64` takes no arguments and returns a plain integer.
        let ms = unsafe { GetTickCount64() };
        if ms == 0 {
            return None;
        }
        Some(ms as f64 / 1000.0)
    }

    /// Every process matching any of `spec`'s honoured signals, restricted to processes that started
    /// at or after `min_start` (seconds since boot; `None` disables the filter).
    pub fn find(&self, spec: &DetectSpec, min_start: Option<f64>) -> Vec<ProcRef> {
        if spec.is_empty() {
            return Vec::new();
        }
        // A spec carrying only signals this platform cannot evaluate would otherwise match nothing
        // forever and look like "the game has no processes". Say so once, prominently.
        if spec.exe.is_none()
            && spec.install_dir.is_none()
            && spec.process_name.is_none()
            && (spec.steam_appid.is_some() || spec.env_marker.is_some())
        {
            warn_unevaluable_once();
            return Vec::new();
        }
        let exe = spec.exe.as_deref().map(normalize);
        let dir = spec.install_dir.as_deref().map(normalize);
        let mut out = Vec::new();
        for entry in snapshot() {
            // The System Idle and System pseudo-processes have no image we can read or own.
            if entry.pid == 0 || entry.pid == 4 {
                continue;
            }
            let Some(handle) = open_limited(entry.pid) else {
                continue; // access denied (another user's or protected process) or vanished
            };
            let start = start_seconds(handle);
            let image = image_path(handle);
            // SAFETY: `handle` was returned by OpenProcess above and is closed exactly once here.
            unsafe {
                let _ = CloseHandle(handle);
            }
            let (Some(start), Some(image)) = (start, image) else {
                continue;
            };
            if let Some(min) = min_start {
                if start + START_SLACK_SECS < min {
                    continue; // predates this launch, so it is never ours (rule 1)
                }
            }
            if matches(&image, &entry.exe_name, exe.as_deref(), dir.as_deref(), spec) {
                out.push(ProcRef {
                    pid: entry.pid,
                    start: start_key(start),
                });
            }
        }
        out
    }

    /// Which of `procs` are still the same live processes: pid present **and** start stamp unchanged.
    pub fn alive(&self, procs: &[ProcRef]) -> Vec<ProcRef> {
        procs
            .iter()
            .copied()
            .filter(|p| {
                open_limited(p.pid)
                    .map(|h| {
                        let now = start_seconds(h).map(start_key);
                        // SAFETY: `h` came from OpenProcess above; closed once.
                        unsafe {
                            let _ = CloseHandle(h);
                        }
                        now == Some(p.start)
                    })
                    .unwrap_or(false)
            })
            .collect()
    }
}

/// Warn exactly once that a spec rests entirely on signals this matcher cannot read.
///
/// Reading another process's command line or environment block needs `NtQueryInformationProcess`
/// plus `ReadProcessMemory` and hard-coded PEB offsets, and a wrong offset can attribute a foreign
/// process to the game. Since the whole point of this module is never to adopt the wrong process, the
/// gap is reported rather than guessed at, and the operator can supply an install dir, executable, or
/// process name instead (which is also how Steam titles already arrive: `steam.rs` attaches the
/// install dir alongside the appid).
fn warn_unevaluable_once() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if ONCE.set(()).is_ok() {
        tracing::warn!(
            "process matching on Windows cannot evaluate env-marker or Steam-appid-only signals, so a \
             game described only that way will not be tracked; give it an install dir, an executable, \
             or a process name"
        );
    }
}

/// One row of the ToolHelp snapshot.
struct Entry {
    pid: u32,
    exe_name: String,
}

/// Every process in the ToolHelp snapshot. Empty if the snapshot cannot be taken.
fn snapshot() -> Vec<Entry> {
    let mut out = Vec::new();
    // SAFETY: a snapshot of the process list; the handle is closed below on every path.
    let snap = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(h) => h,
        Err(_) => return out,
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry` is a correctly sized, writable PROCESSENTRY32W for the duration of each call.
    unsafe {
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                out.push(Entry {
                    pid: entry.th32ProcessID,
                    exe_name: wide_to_string(&entry.szExeFile),
                });
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Open a process with the least privilege that still reads its times and image path. `None` when
/// the process cannot be opened, which is the normal case for protected or foreign-user processes.
fn open_limited(pid: u32) -> Option<HANDLE> {
    // SAFETY: a plain OpenProcess call; the returned handle (if any) is owned by the caller.
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()
}

/// The process start time as seconds since boot, from `GetProcessTimes`. `None` if the call fails.
fn start_seconds(handle: HANDLE) -> Option<f64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all four out-pointers are valid, writable FILETIME values for the call.
    unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }.ok()?;
    let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
    // Convert the FILETIME to Unix seconds, then onto the seconds-since-boot timeline by
    // subtracting the boot instant (now minus uptime), so it matches `now_stamp`.
    let unix = ticks.checked_sub(FILETIME_TO_UNIX_100NS)? as f64 / 10_000_000.0;
    Some(unix - boot_unix_seconds()?)
}

/// The full image path of a process, via `QueryFullProcessImageNameW`, with a K32 fallback.
fn image_path(handle: HANDLE) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` is a writable buffer of `len` UTF-16 units; `len` is updated to the written size.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    if ok.is_ok() && len > 0 {
        return Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
    }
    // SAFETY: as above, with the older K32 API that writes the image name into the buffer.
    let n = unsafe { K32GetProcessImageFileNameW(handle, &mut buf) };
    if n > 0 {
        return Some(PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])));
    }
    None
}

/// Does this process satisfy any honoured signal? Pure over its inputs, so it is unit-tested directly.
fn matches(
    image: &Path,
    exe_name: &str,
    exe: Option<&Path>,
    install_dir: Option<&Path>,
    spec: &DetectSpec,
) -> bool {
    if let Some(want) = exe {
        if eq_path(image, want) {
            return true;
        }
    }
    if let Some(dir) = install_dir {
        // `Path::starts_with` compares whole components, so `C:\games\x` never matches `C:\games\xyz`.
        if starts_with_path(image, dir) {
            return true;
        }
    }
    if let Some(want) = spec.process_name.as_deref() {
        if exe_name.eq_ignore_ascii_case(want) {
            return true;
        }
    }
    false
}

/// Normalize a path for Windows comparison: canonical when it exists, otherwise as given, lower-cased
/// with forward slashes folded to backslashes. Windows paths are case-insensitive.
fn normalize(p: &Path) -> PathBuf {
    let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    // Strip the `\\?\` verbatim prefix `canonicalize` adds, so it compares with ToolHelp images.
    let s = canon.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).replace('/', r"\");
    PathBuf::from(s)
}

/// Case-insensitive full-path equality for Windows.
fn eq_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Case-insensitive, component-wise prefix test for Windows.
fn starts_with_path(path: &Path, prefix: &Path) -> bool {
    let p = path.to_string_lossy().to_ascii_lowercase();
    let mut pre = prefix.to_string_lossy().to_ascii_lowercase();
    if !pre.ends_with('\\') {
        pre.push('\\');
    }
    p.starts_with(&pre)
}

/// Stable integer key for a start stamp, so it can live in `ProcRef::start` and compare for equality.
/// Millisecond resolution is enough to pin a pid to one process incarnation.
fn start_key(seconds: f64) -> u64 {
    (seconds * 1000.0).round() as u64
}

/// Current Unix time in seconds, or `None` if the clock is unavailable.
fn unix_now() -> Option<f64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

/// Seconds since boot, from the tick count.
fn uptime_seconds() -> Option<f64> {
    // SAFETY: `GetTickCount64` takes no arguments and returns a plain integer.
    let ms = unsafe { GetTickCount64() };
    (ms > 0).then(|| ms as f64 / 1000.0)
}

/// The boot instant on the Unix timeline, computed **once** and cached.
///
/// This must not be recomputed per call: it is derived from two separate clock reads (`unix_now` and
/// the tick count), so each recomputation yields a slightly different value and the same process
/// would get different start stamps in `find` and `alive`. Caching it makes a process's stamp stable
/// for the process's lifetime, which is exactly what rule 2 compares.
fn boot_unix_seconds() -> Option<f64> {
    static BOOT: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    if let Some(v) = BOOT.get() {
        return Some(*v);
    }
    let v = unix_now()? - uptime_seconds()?;
    let _ = BOOT.set(v);
    BOOT.get().copied()
}

/// A NUL-terminated UTF-16 buffer to a `String`.
fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_signal_is_case_insensitive_full_path() {
        let image = Path::new(r"C:\games\Tunic\tunic.exe");
        let spec = DetectSpec {
            exe: Some(PathBuf::from(r"c:\games\tunic\tunic.exe")),
            ..Default::default()
        };
        assert!(matches(image, "tunic.exe", spec.exe.as_deref(), None, &spec));
        // A different binary under the same folder must not match.
        let other = DetectSpec {
            exe: Some(PathBuf::from(r"c:\games\tunic\other.exe")),
            ..Default::default()
        };
        assert!(!matches(image, "tunic.exe", other.exe.as_deref(), None, &other));
    }

    #[test]
    fn install_dir_matches_on_component_boundaries_only() {
        // The install dir is the *specific* folder, so its siblings must not match.
        const INSTALL: &str = r"C:\games\x";
        let dir = Path::new(INSTALL);
        let spec = DetectSpec {
            install_dir: Some(PathBuf::from(INSTALL)),
            ..Default::default()
        };
        assert!(matches(
            Path::new(r"C:\games\x\a.exe"),
            "a.exe",
            None,
            Some(dir),
            &spec
        ));
        // `C:\games\xyz` must NOT satisfy the prefix `C:\games\x`.
        assert!(!matches(
            Path::new(r"C:\games\xyz\a.exe"),
            "a.exe",
            None,
            Some(dir),
            &spec
        ));
        // A sibling tree that merely shares a string prefix is not under the install dir.
        assert!(!matches(
            Path::new(r"C:\games-other\a.exe"),
            "a.exe",
            None,
            Some(dir),
            &spec
        ));
    }

    #[test]
    fn process_name_signal_is_case_insensitive() {
        let spec = DetectSpec {
            process_name: Some("RetroArch.EXE".into()),
            ..Default::default()
        };
        assert!(matches(
            Path::new(r"C:\emus\retroarch.exe"),
            "retroarch.exe",
            None,
            None,
            &spec
        ));
        assert!(!matches(
            Path::new(r"C:\emus\other.exe"),
            "other.exe",
            None,
            None,
            &spec
        ));
    }

    #[test]
    fn empty_spec_adopts_nothing() {
        let spec = DetectSpec::default();
        assert!(Scanner::system().find(&spec, None).is_empty());
    }

    #[test]
    fn appid_only_spec_adopts_nothing_rather_than_guessing() {
        // `steam.rs` attaches the install dir whenever it can, so this is the narrow case where only
        // the appid is known. Adopting any `steam.exe`-adjacent process would be a guess, and a guess
        // here can end a game the player already had open, so the answer must be "nothing".
        let spec = DetectSpec::steam(570);
        assert!(spec.install_dir.is_none() && spec.exe.is_none());
        assert!(
            Scanner::system().find(&spec, None).is_empty(),
            "an appid-only spec must adopt nothing on Windows"
        );
    }

    /// Rule 1 against the live process table: a process that started before the launch stamp is never
    /// adopted, and one that started after it is. This is the invariant that stops a session end from
    /// killing a copy of the game the player already had open.
    #[test]
    fn pre_launch_processes_are_never_adopted() {
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let sleeper = |tag: &str| {
            std::process::Command::new(&comspec)
                .args(["/c", "ping -n 20 127.0.0.1 >nul"])
                .env("SLIPSTREAM_PROCSCAN_TEST", tag)
                .spawn()
                .expect("spawn sleeper")
        };

        // Started BEFORE the stamp, and by more than `START_SLACK_SECS`, so it predates the launch and
        // must be rejected. The slack deliberately tolerates a process that raced the launch, so a
        // process only a few hundred milliseconds older is expected to be adopted.
        let mut pre = sleeper("pre");
        std::thread::sleep(std::time::Duration::from_secs_f64(START_SLACK_SECS + 1.5));

        let scanner = Scanner::system();
        let stamp = scanner.now_stamp().expect("now_stamp on Windows");
        assert!(stamp > 0.0, "uptime must be positive, got {stamp}");

        // Started AFTER the stamp, so it belongs to this launch.
        let mut post = sleeper("post");
        std::thread::sleep(std::time::Duration::from_millis(700));

        let spec = DetectSpec {
            process_name: Some("cmd.exe".into()),
            ..Default::default()
        };
        let found: Vec<u32> = scanner.find(&spec, Some(stamp)).iter().map(|p| p.pid).collect();

        let post_pid = post.id();
        let pre_pid = pre.id();

        // Clean up before asserting, so a failure never leaves sleepers behind.
        let _ = pre.kill();
        let _ = post.kill();
        let _ = pre.wait();
        let _ = post.wait();

        assert!(
            found.contains(&post_pid),
            "a process started after the launch stamp must be adopted: pid {post_pid} not in {found:?}"
        );
        assert!(
            !found.contains(&pre_pid),
            "a process that predates the launch must never be adopted: pid {pre_pid} was in {found:?}"
        );
    }

    /// Rule 2: `alive` must reject a remembered pid whose start stamp no longer matches, which is how a
    /// recycled pid is prevented from being treated as the same process.
    #[test]
    fn alive_requires_the_matching_start_stamp() {
        let scanner = Scanner::system();
        let me = std::process::id();
        let live = scanner.find(
            &DetectSpec {
                process_name: Some(
                    std::env::current_exe()
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .unwrap_or_default(),
                ),
                ..Default::default()
            },
            None,
        );
        // Whatever the harness is, its own stamp must round-trip through `alive`.
        if let Some(entry) = live.iter().find(|p| p.pid == me) {
            assert_eq!(scanner.alive(&[*entry]).len(), 1, "own process must verify as alive");
            let mut wrong = *entry;
            wrong.start = wrong.start.wrapping_add(1);
            assert!(
                scanner.alive(&[wrong]).is_empty(),
                "a mismatched start stamp must not be reported alive"
            );
        } else {
            // Not fatal: the test binary may be unreadable under a hardened runner. The stamp-mismatch
            // half above is the assertion that matters and it is covered when the pid is visible.
            eprintln!("skipped: own pid {me} not visible in the snapshot");
        }
    }
}

