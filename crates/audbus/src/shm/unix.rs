//! Memory-mapped files in `/dev/shm` or the temporary directory.

use std::env::temp_dir;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use memmap2::MmapRaw;

use super::OpenMode;

pub(super) struct Region {
    map: MmapRaw,
}

impl Region {
    pub(super) fn open(name: &str, size: usize, mode: OpenMode) -> io::Result<Self> {
        let path = directory().join(name);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(mode == OpenMode::CreateOrOpen)
            .truncate(false)
            .open(path)?;
        // Several processes may race to create the file; growing is idempotent and
        // fills with zeros.
        if file.metadata()?.len() < size as u64 {
            if mode == OpenMode::Existing {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "shared memory region is smaller than expected",
                ));
            }
            file.set_len(size as u64)?;
        }
        let map = MmapRaw::map_raw(&file)?;
        Ok(Self { map })
    }

    pub(super) fn pointer(&self) -> NonNull<u8> {
        NonNull::new(self.map.as_mut_ptr()).unwrap_or(NonNull::dangling())
    }
}

fn directory() -> PathBuf {
    let shm = Path::new("/dev/shm");
    if shm.is_dir() {
        shm.to_path_buf()
    } else {
        temp_dir()
    }
}
