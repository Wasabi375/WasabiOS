use libc::{c_int, EBUSY, EINVAL, EIO, ENOMEM, ENOTSUP};
use wfs::fs::FsError;

/// a best effort guess on what stupid errno value each error corresponds to
pub fn fs_error_no(err: FsError) -> c_int {
    match err {
        FsError::BlockDevice(_) => EIO,
        FsError::BlockDeviceToSmall(_, _) => EINVAL,
        FsError::OverrideCheck => EINVAL,
        FsError::Full => ENOMEM,
        FsError::WriteAllocatorFreeList => ENOMEM,
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
    }
}
