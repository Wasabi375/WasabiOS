use std::{fs::canonicalize, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail, ensure};
use log::info;
use wfs::fs::FileSystem;

use super::*;

pub(super) fn copy_dir(args: InternalTestOptions) -> Result<()> {
    let (mut fs, dir) = setup_fs_image()?;

    let src_path: PathBuf = "../../wasabi-kernel/src".into();

    let mut copy_count = 0;

    info!(
        "copy files from {} into test image",
        canonicalize(&src_path)?.display()
    );
    copy_files(&mut fs, &src_path, dir, &mut copy_count).context("copy src_path to image")?;
    fs.flush()?;

    info!("copied {copy_count} files to the test image");

    if args.create_only {
        fs.close().map_err(|_| anyhow!("failed to close fs"))?;
        return Ok(());
    }

    let fs = reopen_fs(fs)?;

    let read_count = count_files(&fs)?;

    ensure!(
        read_count == copy_count,
        "copied {copy_count} files but only found {read_count} in the file tree"
    );

    info!("all tests successfull");

    match fs.close() {
        Ok(_) => Ok(()),
        Err(_) => bail!("Failed to close file system"),
    }
}

pub(super) fn simplified(args: InternalTestOptions) -> Result<()> {
    let (mut fs, dir) = setup_fs_image().context("setup test fs image")?;

    let file_count = 20;

    info!("creating {file_count} files");
    for i in 0..file_count {
        fs.create_file(format!("File{}", i).into(), dir)
            .context("create file")?;
    }
    fs.flush().context("flush after file writes")?;

    if args.create_only {
        fs.close().map_err(|_| anyhow!("failed to close fs"))?;
        return Ok(());
    }

    let fs = reopen_fs(fs).context("reopen fs")?;

    let read_count = count_files(&fs).context("counting files")?;
    ensure!(
        read_count == file_count,
        "created {file_count} files but only found {read_count} in the file tree"
    );

    info!("all test successfull");

    Ok(())
}

fn setup_fs_image() -> Result<(
    FileSystem<FileDevice, FsReadWrite, StdInterruptState>,
    FileId,
)> {
    let path: PathBuf = "__test_image.wfs".into();

    info!("create test image");
    create_test_image(&path).context("create test image")?;

    info!("mount test image");
    let mut fs = mount_image(&path).context("mount test image")?;

    info!("create directory: /dir1/");
    let dir = fs
        .create_directory(Directory::empty(FileId::ROOT), "dir1".into())
        .context("create dir")?;

    info!("dir file id: {dir}");
    Ok((fs, dir))
}
