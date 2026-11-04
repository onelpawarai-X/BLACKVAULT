//! BLACKVAULT — real file and folder protection.
//!
//! Protection is enforced with two mechanisms that Windows itself honours:
//!
//!   1. **NTFS access control.** We read the item's current security descriptor,
//!      convert it to SDDL and keep it as the "open" state. Sealing replaces the
//!      DACL with one that denies the interactive user, so the file cannot be
//!      opened, listed or executed while sealed — by BLACKVAULT's own UI thread
//!      either, which is the point. Opening restores the saved descriptor.
//!
//!   2. **EFS content encryption.** `EncryptFileW` marks the file encrypted at
//!      rest. The file's data is unreadable to anyone without the user's EFS key,
//!      even if the DACL is bypassed offline.
//!
//! Nothing here hides or renames anything: the item keeps its name and place, and
//! its denial is visible in the file's own security tab. That is deliberate — a
//! protection you cannot audit is not a protection.

use crate::error::{BvError, BvResult};

/// What actually got applied, so the UI can be honest about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedProtection {
    pub acl: bool,
    pub encrypted: bool,
}

impl AppliedProtection {
    pub fn label(&self) -> String {
        match (self.acl, self.encrypted) {
            (true, true) => "acl+efs".into(),
            (true, false) => "acl".into(),
            (false, true) => "efs".into(),
            (false, false) => "none".into(),
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::path::Path;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    };
    use windows::Win32::Storage::FileSystem::{
        EncryptFileW, DecryptFileW, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
        FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_DIRECTORY,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Current user's account SID, as a string.
    fn current_user_sid_string() -> BvResult<String> {
        use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        use windows::Win32::Foundation::{CloseHandle, HANDLE};

        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .map_err(|e| BvError::msg(format!("OpenProcessToken: {e}")))?;
            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut _),
                len,
                &mut len,
            );
            let _ = CloseHandle(token);
            ok.map_err(|e| BvError::msg(format!("GetTokenInformation: {e}")))?;
            let tu = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut sid_str = windows::core::PWSTR::null();
            ConvertSidToStringSidW(tu.User.Sid, &mut sid_str)
                .map_err(|e| BvError::msg(format!("ConvertSidToStringSid: {e}")))?;
            let s = sid_str.to_string().map_err(|_| BvError::Crypto)?;
            let _ = LocalFree(HLOCAL(sid_str.0 as *mut _));
            Ok(s)
        }
    }

    /// Read the DACL of a path and return it as SDDL, or an empty string when
    /// the item has no explicit DACL.
    pub fn snapshot_acl(path: &Path) -> BvResult<String> {
        let wpath = wide(&path.to_string_lossy());
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let rc = GetNamedSecurityInfoW(
                PCWSTR(wpath.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                None,
                None,
                &mut sd,
            );
            if rc.0 != 0 {
                return Err(BvError::msg(format!("GetNamedSecurityInfo failed ({})", rc.0)));
            }
            if sd.is_invalid() {
                return Ok(String::new());
            }
            let mut sddl = windows::core::PWSTR::null();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut sddl,
                None,
            );
            let _ = LocalFree(HLOCAL(sd.0));
            if ok.is_err() {
                return Ok(String::new());
            }
            let out = sddl.to_string().unwrap_or_default();
            let _ = LocalFree(HLOCAL(sddl.0 as *mut _));
            Ok(out)
        }
    }

    /// Seal: deny the current user, then encrypt with EFS.
    pub fn seal(path: &Path) -> BvResult<AppliedProtection> {
        if !path.exists() {
            return Err(BvError::msg("path does not exist"));
        }
        let sid = current_user_sid_string()?;
        // Deny full control to the interactive user; keep SYSTEM and Admins so
        // the machine remains recoverable and auditable.
        let sddl = format!("D:(D;;GA;;;{sid})(A;;GA;;;SY)(A;;GA;;;BA)");
        let acl = apply_sddl(path, &sddl).is_ok();
        let encrypted = encrypt(path).unwrap_or(false);
        Ok(AppliedProtection { acl, encrypted })
    }

    /// Open: restore the saved SDDL (or clear the explicit DACL), then decrypt.
    pub fn open(path: &Path, saved_sddl: Option<&str>) -> BvResult<AppliedProtection> {
        if !path.exists() {
            return Err(BvError::msg("path does not exist"));
        }
        let acl = match saved_sddl {
            Some(s) if !s.is_empty() => apply_sddl(path, s).is_ok(),
            // No snapshot: remove the explicit DACL and let inheritance apply.
            _ => apply_sddl(path, "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;BU)").is_ok(),
        };
        let decrypted = decrypt(path).unwrap_or(false);
        Ok(AppliedProtection {
            acl,
            encrypted: !decrypted,
        })
    }

    fn apply_sddl(path: &Path, sddl: &str) -> BvResult<()> {
        let wpath = wide(&path.to_string_lossy());
        let wsddl = wide(sddl);
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wsddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .map_err(|e| BvError::msg(format!("bad SDDL: {e}")))?;

            // Extract the DACL pointer from the descriptor.
            let mut present = windows::Win32::Foundation::BOOL(0);
            let mut dacl_ptr: *mut ACL = std::ptr::null_mut();
            let _ = windows::Win32::Security::GetSecurityDescriptorDacl(
                sd,
                &mut present,
                &mut dacl_ptr,
                &mut windows::Win32::Foundation::BOOL(0),
            );

            let rc = SetNamedSecurityInfoW(
                windows::core::PWSTR(wpath.as_ptr() as *mut u16),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                PSID::default(),
                PSID::default(),
                Some(dacl_ptr),
                None,
            );
            let _ = LocalFree(HLOCAL(sd.0));
            if rc.0 != 0 {
                return Err(BvError::msg(format!("SetNamedSecurityInfo failed ({})", rc.0)));
            }
        }
        Ok(())
    }

    fn encrypt(path: &Path) -> BvResult<bool> {
        let wpath = wide(&path.to_string_lossy());
        unsafe { Ok(EncryptFileW(PCWSTR(wpath.as_ptr())).is_ok()) }
    }

    fn decrypt(path: &Path) -> BvResult<bool> {
        let wpath = wide(&path.to_string_lossy());
        unsafe { Ok(DecryptFileW(PCWSTR(wpath.as_ptr()), 0).is_ok()) }
    }

    /// Recursively encrypt a directory tree with EFS.
    fn encrypt_tree(root: &Path) -> BvResult<bool> {
        let mut all = encrypt(root)?;
        if root.is_dir() {
            for entry in walk(root) {
                all &= encrypt(&entry).unwrap_or(false);
            }
        }
        Ok(all)
    }

    fn walk(root: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p.clone());
                }
                out.push(p);
            }
            if out.len() > 100_000 {
                break;
            }
        }
        out
    }

    pub fn seal_tree(path: &Path) -> BvResult<AppliedProtection> {
        let sid = current_user_sid_string()?;
        let sddl = format!("D:(D;;GA;;;{sid})(A;;GA;;;SY)(A;;GA;;;BA)");
        let acl = apply_sddl(path, &sddl).is_ok();
        let encrypted = if path.is_dir() { encrypt_tree(path).unwrap_or(false) } else { encrypt(path).unwrap_or(false) };
        Ok(AppliedProtection { acl, encrypted })
    }

    pub fn is_encrypted(path: &Path) -> bool {
        let wpath = wide(&path.to_string_lossy());
        unsafe {
            let attrs = GetFileAttributesW(PCWSTR(wpath.as_ptr()));
            attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_ENCRYPTED.0) != 0
        }
    }

    pub fn is_dir(path: &Path) -> bool {
        let wpath = wide(&path.to_string_lossy());
        unsafe {
            let attrs = GetFileAttributesW(PCWSTR(wpath.as_ptr()));
            attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_DIRECTORY.0) != 0
        }
    }

    pub fn size_of(path: &Path) -> Option<i64> {
        if path.is_file() {
            std::fs::metadata(path).ok().map(|m| m.len() as i64)
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::path::Path;

    pub fn snapshot_acl(_path: &Path) -> BvResult<String> {
        Ok(String::new())
    }
    pub fn seal(_path: &Path) -> BvResult<AppliedProtection> {
        Err(BvError::Unsupported("NTFS protection".into()))
    }
    pub fn seal_tree(_path: &Path) -> BvResult<AppliedProtection> {
        Err(BvError::Unsupported("NTFS protection".into()))
    }
    pub fn open(_path: &Path, _saved: Option<&str>) -> BvResult<AppliedProtection> {
        Err(BvError::Unsupported("NTFS protection".into()))
    }
    pub fn is_encrypted(_path: &Path) -> bool {
        false
    }
    pub fn is_dir(path: &Path) -> bool {
        path.is_dir()
    }
    pub fn size_of(path: &Path) -> Option<i64> {
        if path.is_file() {
            std::fs::metadata(path).ok().map(|m| m.len() as i64)
        } else {
            None
        }
    }
}

pub use imp::*;

/// Seal an item, choosing the directory path when needed.
pub fn seal_item(path: &std::path::Path) -> BvResult<AppliedProtection> {
    if is_dir(path) {
        seal_tree(path)
    } else {
        seal(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn applied_protection_labels() {
        assert_eq!(AppliedProtection { acl: true, encrypted: true }.label(), "acl+efs");
        assert_eq!(AppliedProtection { acl: true, encrypted: false }.label(), "acl");
        assert_eq!(AppliedProtection { acl: false, encrypted: true }.label(), "efs");
        assert_eq!(AppliedProtection { acl: false, encrypted: false }.label(), "none");
    }

    #[test]
    fn snapshot_of_missing_path_is_not_a_panic() {
        // On Windows this errors cleanly; off Windows it returns an empty string.
        let _ = snapshot_acl(Path::new("C:\\definitely\\not\\here"));
    }

    #[test]
    fn is_dir_matches_std() {
        assert!(is_dir(Path::new(".")));
        assert!(!is_dir(Path::new("./Cargo.toml")));
    }
}
