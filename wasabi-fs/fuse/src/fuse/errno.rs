use libc::{EBUSY, EINVAL, EIO, ENOENT, ENOMEM, ENOSPC, ENOTSUP, c_int};
use wfs::{fs::FsError, mem_tree::MemTreeError};

/// a best effort uess on what stupid errno value each error corresponds to
// NOTE ENOSPC: no space on device
//  ENOMEM: no memory left on RAM
pub fn fs_error_no(err: FsError) -> c_int {
    match err {
        FsError::BlockDevice(_) => EIO,
        FsError::BlockDeviceToSmall(_, _) => EINVAL,
        FsError::OverrideCheck => EINVAL,
        FsError::WriteAllocatorFreeList => ENOSPC,
        FsError::HeaderMismatch => EIO,
        FsError::HeaderVersionMismatch(_) => ENOTSUP,
        FsError::HeaderMagicInvalid => EINVAL,
        FsError::HeaderWithoutTransient => EINVAL,
        FsError::NotInitialized => EINVAL,
        FsError::CompareExchangeFailedToOften => EBUSY,
        FsError::AlreadyMounted => EINVAL,
        FsError::MalformedStringLength => EIO,
        FsError::MalformedStringUtf8(_) => EIO,
        FsError::StringToLong => EINVAL,
        FsError::BlockDeviceFull(_) => ENOSPC,
        FsError::Oom => ENOMEM,
        FsError::NoConsecutiveFreeBlocks(_) => ENOSPC,
        FsError::MemTreeError(mem_err) => match mem_err {
            MemTreeError::Oom(_) => ENOMEM,
            MemTreeError::BlockDevice(_) => EIO,
            MemTreeError::FileNodeExists(_) => EINVAL,
            MemTreeError::FileDoesNotExist(_) => EINVAL,
        },
        FsError::FileDoesNotExist(_) => ENOENT,
        FsError::NotADirectory(_, _) => EINVAL,
    }
}
