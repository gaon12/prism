use std::{
    io,
    ptr::{null_mut, NonNull},
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, PSECURITY_DESCRIPTOR,
    },
};

pub(crate) struct Descriptor(NonNull<std::ffi::c_void>);
impl Descriptor {
    pub fn parse(sddl: &str) -> io::Result<Self> {
        let wide: Vec<_> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut raw = null_mut();
        // SAFETY: valid terminated SDDL, output owns a LocalAlloc descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut raw,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(
            NonNull::new(raw).ok_or_else(io::Error::last_os_error)?,
        ))
    }
    pub fn raw(&self) -> PSECURITY_DESCRIPTOR {
        self.0.as_ptr()
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: this descriptor was allocated by the conversion API and is not
        // borrowed by an in-flight call; object creation copies its security data.
        unsafe {
            LocalFree(self.0.as_ptr());
        }
    }
}
