use std::{
    fs::File,
    io::{Read, Seek},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use host_shared::sync::StdInterruptState;
use log::info;
use shared::sync::InterruptState;
use wfs::{
    fs::{FsReadWrite, OverrideCheck},
    fs_structs::{FileId, FileType},
    mem_structs::{Directory, FileNode},
};

use crate::block_device::FileDevice;

type FileSystem = wfs::fs::FileSystem<FileDevice, FsReadWrite, StdInterruptState>;

pub fn test_main() -> Result<()> {
    let path: PathBuf = "__test_image.wfs".into();

    info!("create test image");
    create_test_image(&path)?;

    info!("mount test image");
    let mut fs = mount_image(&path)?;

    info!("create directory: /dir1/");
    let dir = fs.create_directory(Directory::empty(FileId::ROOT), "dir1".into())?;
    info!("dir file id: {dir}");

    let src_path: PathBuf = "../src".into();

    let mut copy_count = 0;

    copy_files(&mut fs, &src_path, dir, &mut copy_count).context("copy src_path to image")?;

    info!("copied {copy_count} files to the test image");

    let read_count = count_files(&fs)?;

    assert_eq!(
        read_count, copy_count,
        "read count {read_count} does not match copy count {copy_count}"
    );

    info!("all tests successfull");

    match fs.close() {
        Ok(_) => Ok(()),
        Err(_) => bail!("Failed to close file system"),
    }
}

fn count_files(fs: &FileSystem) -> Result<usize> {
    let root_node = fs.get_file_meta(FileId::ROOT)?;

    fn count_files<I: InterruptState>(
        fs: &FileSystem,
        dir_node: Arc<FileNode<I>>,
    ) -> Result<usize> {
        assert_eq!(dir_node.typ, FileType::Directory);
        let mut count = 0;

        let dir = fs.read_directory(dir_node.id)?;

        for entry in dir.entries {
            let file = fs.get_file_meta(entry.id)?;
            match file.typ {
                FileType::File => count += 1,
                FileType::Directory => count += count_files(fs, file)?,
            }
        }

        Ok(count)
    }

    count_files(fs, root_node)
}

fn copy_files(
    fs: &mut FileSystem,
    src_path: &Path,
    target_dir: FileId,
    total_file_count: &mut usize,
) -> Result<()> {
    // const COPY_BLOCK_SIZE: usize = 512; FIXME this triggers an insert in fuse::BlockDevice
    const COPY_BLOCK_SIZE: usize = wfs::BLOCK_SIZE;

    for file in std::fs::read_dir(src_path)? {
        let file = file?;
        let file_type = file.file_type()?;
        let file_name = file
            .file_name()
            .into_string()
            .map_err(|_| anyhow::format_err!("Failed to convert OsStr to string"))?;

        if file_type.is_dir() {
            let dir = fs
                .create_directory(Directory::empty(target_dir), file_name.into())
                .context("Failed to create dir")?;
            copy_files(fs, &file.path(), dir, total_file_count)?;
        } else if file_type.is_file() {
            let file_id = fs
                .create_file(file_name.into(), target_dir)
                .context("create file")?;
            *total_file_count += 1;

            let mut os_file = File::open(&file.path())?;
            let len = os_file.seek(std::io::SeekFrom::End(0))?;
            os_file.rewind()?;

            let mut coppied = 0;
            let mut buf = [0; COPY_BLOCK_SIZE];
            while coppied < len {
                let read = os_file.read(&mut buf)?;

                fs.write_file(file_id, coppied, &buf[..read])
                    .context("write file")?;
                coppied += read as u64;
            }
        }
    }

    Ok(())
}

fn create_test_image(image_path: &Path) -> Result<()> {
    let device = FileDevice::create(image_path, 512).expect("Failed to create block device");

    let uuid = uuid::Uuid::new_v4();
    let name = "internal test image".into();

    let fs = FileSystem::create(device, OverrideCheck::IgnoreExisting, uuid, Some(name))
        .expect("Failed to create image");

    return match fs.close() {
        Ok(_) => Ok(()),
        Err(_) => panic!("Failed to close created file system"),
    };
}

fn mount_image(image_path: &Path) -> Result<FileSystem> {
    let device = FileDevice::open(&image_path).context("Failed to open image file")?;
    FileSystem::open(device).context("failed to open file system")
}
