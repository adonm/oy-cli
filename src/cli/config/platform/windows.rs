//! Windows ACL helpers: restrict file/directory access to the current
//! process owner via a permissive ACE.
//!
//! Moved from `src/cli/config/paths.rs` (REVIEW #4).

use anyhow::{Result, bail};
use std::path::Path;

pub(crate) fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{
        ACL, ACL_REVISION, AddAccessAllowedAce, DACL_SECURITY_INFORMATION, GetLengthSid,
        InitializeAcl, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetTokenInformation, OpenProcessToken,
    };

    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut token: HANDLE = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            bail!(
                "failed to open process token: Windows error {}",
                std::io::Error::last_os_error()
            );
        }

        let mut size = 0;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
        if size == 0 {
            CloseHandle(token);
            bail!("failed to get token user size");
        }

        let mut buf = vec![0u8; size as usize];
        if GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut _,
            size,
            &mut size,
        ) == 0
        {
            CloseHandle(token);
            bail!(
                "failed to get token user information: Windows error {}",
                std::io::Error::last_os_error()
            );
        }
        CloseHandle(token);

        let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid = token_user.User.Sid;
        let sid_len = GetLengthSid(sid);

        let acl_size = std::mem::size_of::<ACL>()
            + std::mem::size_of::<windows_sys::Win32::Security::ACCESS_ALLOWED_ACE>()
            - 4
            + sid_len as usize;

        let mut acl_buf = vec![0u8; acl_size];
        let acl_ptr = acl_buf.as_mut_ptr() as *mut ACL;

        if InitializeAcl(acl_ptr, acl_size as u32, ACL_REVISION) == 0 {
            bail!(
                "failed to initialize ACL: Windows error {}",
                std::io::Error::last_os_error()
            );
        }

        const GENERIC_ALL: u32 = 0x10000000;
        if AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL, sid) == 0 {
            bail!(
                "failed to add access allowed ACE: Windows error {}",
                std::io::Error::last_os_error()
            );
        }

        let r = SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl_ptr,
            null_mut(),
        );

        if r != 0 {
            bail!("failed to set security info: Windows error code {}", r);
        }
    }

    Ok(())
}
