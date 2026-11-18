//! Pagefile-backed named file mappings.

use std::ffi::c_void;
use std::io;
use std::iter::once;
use std::mem::zeroed;
use std::ptr::{NonNull, null};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
    MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile, VirtualQuery,
};

use super::OpenMode;

pub(super) struct Region {
    handle: HANDLE,
    view: NonNull<u8>,
}

impl Region {
    pub(super) fn open(name: &str, size: usize, mode: OpenMode) -> io::Result<Self> {
        let wide: Vec<u16> = format!("Local\\{name}")
            .encode_utf16()
            .chain(once(0))
            .collect();
        let size_64 = size as u64;
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the calls.
        let handle = unsafe {
            match mode {
                OpenMode::CreateOrOpen => CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    null(),
                    PAGE_READWRITE,
                    (size_64 >> 32) as u32,
                    size_64 as u32,
                    wide.as_ptr(),
                ),
                OpenMode::Existing => OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide.as_ptr()),
            }
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `handle` is a valid file mapping handle.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
        let Some(pointer) = NonNull::new(view.Value.cast::<u8>()) else {
            let error = io::Error::last_os_error();
            // SAFETY: `handle` is valid and not used afterwards.
            unsafe { CloseHandle(handle) };
            return Err(error);
        };
        let region = Self {
            handle,
            view: pointer,
        };
        // A mapping created by an incompatible build may be smaller.
        if mapped_len(pointer) < size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared memory region is smaller than expected",
            ));
        }
        Ok(region)
    }

    pub(super) fn pointer(&self) -> NonNull<u8> {
        self.view
    }
}

fn mapped_len(view: NonNull<u8>) -> usize {
    // SAFETY: An all-zero `MEMORY_BASIC_INFORMATION` is valid plain data.
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
    // SAFETY: `info` is a valid output buffer of the given size.
    let written = unsafe {
        VirtualQuery(
            view.as_ptr().cast::<c_void>(),
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if written == 0 { 0 } else { info.RegionSize }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: The view and handle were created in `open` and are released once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.as_ptr().cast::<c_void>(),
            });
            CloseHandle(self.handle);
        }
    }
}
