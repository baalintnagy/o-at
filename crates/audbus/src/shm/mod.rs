//! Named shared memory regions, zero-filled when first created.
//!
//! Windows uses pagefile-backed file mappings in the session namespace, which disappear
//! when the last process closes them. Unix systems use files in `/dev/shm` (or the
//! temporary directory where it does not exist), mapped into memory.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::io;
use std::marker::PhantomData;
use std::ptr::NonNull;

#[cfg(unix)]
use self::unix::Region;
#[cfg(windows)]
use self::windows::Region;

/// Whether opening may create the region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenMode {
    CreateOrOpen,
    /// Fails with [`io::ErrorKind::NotFound`] if no process has created the region.
    Existing,
}

/// A shared region holding one `T`.
///
/// # Safety contract for `T`
///
/// `T` must be `#[repr(C)]`, consist only of atomics (or arrays of them) and be valid
/// when all bytes are zero, because other processes access it concurrently and a new
/// region starts zero-filled. The `layout` module defines the only such types.
pub(crate) struct Shared<T> {
    /// Keeps the mapping alive.
    _region: Region,
    pointer: NonNull<T>,
    _marker: PhantomData<T>,
}

// SAFETY: `T` consists of atomics only (see the contract above), so sharing references
// across threads is sound, and the mapping itself is owned by `Region`.
unsafe impl<T: Sync> Send for Shared<T> {}
// SAFETY: See above.
unsafe impl<T: Sync> Sync for Shared<T> {}

impl<T> Shared<T> {
    /// Opens the region `name` (without platform prefix) with room for one `T`.
    ///
    /// # Safety
    ///
    /// `T` must satisfy the contract documented on [`Shared`].
    pub(crate) unsafe fn open(name: &str, mode: OpenMode) -> io::Result<Self> {
        let size = size_of::<T>();
        let region = Region::open(name, size, mode)?;
        let pointer = region.pointer().cast::<T>();
        if !pointer.as_ptr().is_aligned() {
            return Err(io::Error::other("shared memory is not aligned"));
        }
        Ok(Self {
            _region: region,
            pointer,
            _marker: PhantomData,
        })
    }

    pub(crate) fn get(&self) -> &T {
        // SAFETY: The region is mapped for at least `size_of::<T>()` bytes while `self`
        // lives, is aligned, and any bit pattern written by other processes is valid for
        // atomics. All access goes through shared references to atomics.
        unsafe { self.pointer.as_ref() }
    }
}
