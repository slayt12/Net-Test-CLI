//! Privilege escalation for the `service` subcommands.
//!
//! Unix: re-execute through `sudo` (then `pkexec`, `doas`) with inherited stdio, so the password
//! prompt and all output appear inline. Windows: `ShellExecuteExW` with the `runas` verb (the
//! UAC prompt). ShellExecute cannot redirect stdio and the elevated console is hidden, so the
//! child is told to append its output to a temp file which the parent prints afterwards.

use std::ffi::OsString;
use std::process::ExitCode;

#[cfg(unix)]
pub fn is_elevated() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// Unix: identical to `reexec_elevated` (the tools need the terminal for their password prompt,
/// so output cannot be captured); the returned text is empty. Exists so callers can be written
/// once for both platforms.
#[cfg(unix)]
pub fn reexec_elevated_captured(args: &[OsString]) -> std::io::Result<(ExitCode, String)> {
    reexec_elevated(args).map(|c| (c, String::new()))
}

#[cfg(unix)]
pub fn reexec_elevated(args: &[OsString]) -> std::io::Result<ExitCode> {
    let exe = std::env::current_exe()?;
    for tool in ["sudo", "pkexec", "doas"] {
        match std::process::Command::new(tool)
            .arg(&exe)
            .args(args)
            .status()
        {
            Ok(status) => {
                let code = status.code().unwrap_or(1).clamp(0, 255) as u8;
                return Ok(ExitCode::from(code));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "needs root and none of sudo, pkexec or doas is installed: rerun as root",
    ))
}

#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: plain Win32 calls with valid out-pointers; the token handle is closed on every path.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(windows)]
pub fn reexec_elevated(args: &[OsString]) -> std::io::Result<ExitCode> {
    let (code, text) = reexec_elevated_captured(args)?;
    print!("{text}");
    Ok(code)
}

/// Run elevated and return the child's captured output instead of printing it (the TUI shows
/// it in a panel; UAC is a separate secure-desktop dialog, so the terminal is untouched).
#[cfg(windows)]
pub fn reexec_elevated_captured(args: &[OsString]) -> std::io::Result<(ExitCode, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NO_CONSOLE, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let exe = std::env::current_exe()?;
    let capture = std::env::temp_dir().join(format!("nettest-svc-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&capture);
    let mut params: Vec<String> = args
        .iter()
        .map(|a| quote_windows_arg(&a.to_string_lossy()))
        .collect();
    params.push("--capture-output".into());
    params.push(quote_windows_arg(&capture.display().to_string()));
    let verb = wide("runas");
    let file = wide(&exe.display().to_string());
    let params_w = wide(&params.join(" "));

    // SAFETY: the struct is fully initialised (zeroed then sized) and the wide strings outlive
    // the call; the process handle is closed after waiting on it.
    let code = unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params_w.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            let err = GetLastError();
            return Err(if err == ERROR_CANCELLED {
                std::io::Error::other("the elevation prompt was declined")
            } else {
                std::io::Error::from_raw_os_error(err as i32)
            });
        }
        if info.hProcess.is_null() {
            return Err(std::io::Error::other("no handle to the elevated process"));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        code
    };
    let text = std::fs::read_to_string(&capture).unwrap_or_default();
    let _ = std::fs::remove_file(&capture);
    Ok((ExitCode::from(code.min(255) as u8), text))
}

#[cfg(not(any(unix, windows)))]
pub fn is_elevated() -> bool {
    true
}

#[cfg(not(any(unix, windows)))]
pub fn reexec_elevated(_args: &[OsString]) -> std::io::Result<ExitCode> {
    Err(std::io::Error::other(
        "elevation is not supported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn reexec_elevated_captured(args: &[OsString]) -> std::io::Result<(ExitCode, String)> {
    reexec_elevated(args).map(|c| (c, String::new()))
}

/// Quote one argument the way `CommandLineToArgvW` expects (MSVC rules). Pure, so it is unit
/// tested on every host even though only Windows uses it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn quote_windows_arg(s: &str) -> String {
    if !s.is_empty() && !s.chars().any(|c| matches!(c, ' ' | '\t' | '"' | '\n')) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in s.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::quote_windows_arg;

    #[test]
    fn quoting_rules() {
        assert_eq!(quote_windows_arg("plain"), "plain");
        assert_eq!(quote_windows_arg(""), "\"\"");
        assert_eq!(quote_windows_arg("has space"), "\"has space\"");
        assert_eq!(quote_windows_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_windows_arg("C:\\dir\\"), "C:\\dir\\");
        assert_eq!(quote_windows_arg("C:\\my dir\\"), "\"C:\\my dir\\\\\"");
        assert_eq!(quote_windows_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }
}
