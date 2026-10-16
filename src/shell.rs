//! Windows shell integration:
//! - single-instance guard with a path "inbox" (files launched from the
//!   Explorer context menu are forwarded to the already running app),
//! - the "Add to EasyToPrint" context-menu verb for .pdf files (per-user
//!   registry, no admin rights needed).

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // ------------------------------------------------------------------
    // single instance
    // ------------------------------------------------------------------

    /// True when this is the first running instance (owns the named mutex).
    pub fn is_primary() -> bool {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        static mut MUTEX: HANDLE = std::ptr::null_mut(); // leaked handle
        unsafe {
            if !MUTEX.is_null() {
                return true;
            }
            let name = wide("Local\\EasyToPrintSingleInstance");
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if !handle.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
                // secondary instance: the running primary owns the mutex
                return false;
            }
            MUTEX = handle;
            true
        }
    }

    // ------------------------------------------------------------------
    // file inbox (secondary → primary instance)
    // ------------------------------------------------------------------

    fn appdata_root() -> PathBuf {
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        base.join("EasyToPrint")
    }

    pub fn inbox_dir() -> PathBuf {
        appdata_root().join("inbox")
    }

    /// Writes the paths as one inbox file (atomic per file, no locking).
    pub fn submit_paths_in(dir: &Path, paths: &[PathBuf]) {
        let _ = std::fs::create_dir_all(dir);
        let name = format!(
            "{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let body: String = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(dir.join(name), body);
    }

    /// Reads and deletes every inbox file, returning the collected paths.
    pub fn take_inbox_in(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "txt").unwrap_or(false) {
                if let Ok(body) = std::fs::read_to_string(&path) {
                    for line in body.lines() {
                        let p = PathBuf::from(line);
                        if p.is_file() {
                            out.push(p);
                        }
                    }
                }
                let _ = std::fs::remove_file(&path);
            }
        }
        out
    }

    pub fn submit_paths(paths: &[PathBuf]) {
        submit_paths_in(&inbox_dir(), paths);
    }

    pub fn take_inbox() -> Vec<PathBuf> {
        take_inbox_in(&inbox_dir())
    }

    // ------------------------------------------------------------------
    // Explorer context menu (HKCU, no admin needed)
    // ------------------------------------------------------------------

    const VERB_KEY: &str = "Software\\Classes\\SystemFileAssociations\\.pdf\\shell\\EasyToPrintAdd";

    /// Registers (or refreshes) the context-menu verb with the given label.
    pub fn register_menu(label: &str, exe: &Path) -> bool {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
            REG_OPTION_NON_VOLATILE, REG_SZ,
        };
        unsafe {
            let mut hk: HKEY = std::ptr::null_mut();
            let subkey = wide(VERB_KEY);
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut hk,
                std::ptr::null_mut(),
            ) != 0
            {
                return false;
            }
            let set = |key: HKEY, name: &str, value: &str| {
                let data = wide(value);
                RegSetValueExW(
                    key,
                    wide(name).as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    (data.len() * 2) as u32,
                )
            };
            set(hk, "", label);
            set(hk, "Icon", &format!("\"{}\"", exe.display()));

            let mut hkc: HKEY = std::ptr::null_mut();
            let cmd_key = wide("command");
            let ok = RegCreateKeyExW(
                hk,
                cmd_key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut hkc,
                std::ptr::null_mut(),
            ) == 0;
            if ok {
                set(hkc, "", &format!("\"{}\" \"%1\"", exe.display()));
                RegCloseKey(hkc);
            }
            RegCloseKey(hk);
            ok
        }
    }

    pub fn unregister_menu() {
        use windows_sys::Win32::System::Registry::{
            RegDeleteTreeW, HKEY_CURRENT_USER,
        };
        unsafe {
            let subkey = wide(VERB_KEY);
            RegDeleteTreeW(HKEY_CURRENT_USER, subkey.as_ptr());
        }
    }

    pub fn menu_registered() -> bool {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        };
        unsafe {
            let mut hk: HKEY = std::ptr::null_mut();
            let subkey = wide(VERB_KEY);
            let ok = RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hk) == 0;
            if ok {
                RegCloseKey(hk);
            }
            ok
        }
    }
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;
    pub fn is_primary() -> bool {
        true
    }
    pub fn submit_paths(_paths: &[PathBuf]) {}
    pub fn take_inbox() -> Vec<PathBuf> {
        Vec::new()
    }
    pub fn register_menu(_label: &str, _exe: &std::path::Path) -> bool {
        false
    }
    pub fn unregister_menu() {}
    pub fn menu_registered() -> bool {
        false
    }
}

#[cfg(not(windows))]
pub use imp::*;

#[cfg(all(test, windows))]
mod tests {
    use super::imp::{register_menu, menu_registered, take_inbox_in, submit_paths_in, unregister_menu};

    #[test]
    fn inbox_roundtrip() {
        let dir = std::env::temp_dir().join("etp_inbox_test");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("a.pdf");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&src, b"x").unwrap();
        submit_paths_in(&dir, &[src.clone()]);
        let got = take_inbox_in(&dir);
        assert_eq!(got, vec![src]);
        // consumed: nothing left
        assert!(take_inbox_in(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_menu_register_roundtrip() {
        let exe = std::env::current_exe().unwrap();
        assert!(register_menu("ETP test verb", &exe));
        assert!(menu_registered());
        unregister_menu();
        assert!(!menu_registered());
    }
}
