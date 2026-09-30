//! Known-folder lookup via the shell API (not environment variables, which a
//! process or user can override).

use std::ffi::c_void;
use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
    FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_ProgramFiles,
    FOLDERID_ProgramFilesX86, FOLDERID_RoamingAppData, FOLDERID_Videos, FOLDERID_Windows,
    KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::GUID;

/// Well-known folders Sentinel cares about.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Known {
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Videos,
    Music,
    Windows,
    Profile,
    ProgramFiles,
    ProgramFilesX86,
    RoamingAppData,
    LocalAppData,
}

impl Known {
    fn guid(self) -> &'static GUID {
        match self {
            Self::Desktop => &FOLDERID_Desktop,
            Self::Documents => &FOLDERID_Documents,
            Self::Downloads => &FOLDERID_Downloads,
            Self::Pictures => &FOLDERID_Pictures,
            Self::Videos => &FOLDERID_Videos,
            Self::Music => &FOLDERID_Music,
            Self::Windows => &FOLDERID_Windows,
            Self::Profile => &FOLDERID_Profile,
            Self::ProgramFiles => &FOLDERID_ProgramFiles,
            Self::ProgramFilesX86 => &FOLDERID_ProgramFilesX86,
            Self::RoamingAppData => &FOLDERID_RoamingAppData,
            Self::LocalAppData => &FOLDERID_LocalAppData,
        }
    }
}

/// Resolve a known folder, or `None` if the shell cannot provide it.
pub(crate) fn path_of(folder: Known) -> Option<PathBuf> {
    // SAFETY: the shell returns a NUL-terminated wide string allocated with
    // CoTaskMemAlloc, which we copy and then free exactly once.
    unsafe {
        let pwstr = SHGetKnownFolderPath(folder.guid(), KF_FLAG_DEFAULT, None).ok()?;
        let s = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0 as *const c_void));
        s.map(PathBuf::from)
    }
}
