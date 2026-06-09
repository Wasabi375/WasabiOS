use std::{path::PathBuf, sync::Arc};

use crate::{
    args::CleanArgs,
    config::{Config, file_system::FileSystem},
    file_system::{FileSource, TreeWalk},
};

use anyhow::{Context, Result};
use host_wfs::FileDevice;
use log::error;
use shared::{KiB, alloc_ext::alloc_buffer};
use tokio::{
    fs::{self, File},
    io::AsyncReadExt,
};
use uuid::Uuid;
use wfs::{blocks_required_for, fs_structs::FileId};
use wfs::{fs::OverwriteCheck, mem_structs::Directory};

type WFS =
    wfs::fs::FileSystem<FileDevice, wfs::fs::FsReadWrite, host_shared::sync::StdInterruptState>;

pub async fn build_wfs_fs(
    fs_config: Arc<FileSystem>,
    config: &Config,
    static_files: Option<TreeWalk>,
    dependencies: impl Future<Output = Result<Vec<FileSource>>>,
) -> Result<()> {
    let image_path = out_path(&fs_config, config);
    let block_count = blocks_required_for!(fs_config.size.bytes());
    let device = FileDevice::create(&image_path, block_count)
        .context("failed to create image file for fs")?;

    let name = fs_config.id.as_ref().to_owned();
    let uuid = Uuid::new_v4();

    // TODO wfs is still sync but I want to change it to async in the future
    let mut fs = WFS::create(
        device,
        OverwriteCheck::IgnoreExisting,
        uuid,
        Some(name.into()),
    )
    .context("failed to create wfs filesystem")?;

    if let Some(mut static_files) = static_files {
        while let Some(static_file) = static_files
            .next_entry()
            .await
            .context("walk static files")?
        {
            copy_file(&mut fs, static_file)
                .await
                .context("write staic file to fs")?;
        }
    }

    for dep in dependencies.await.context("build fs dependencies")? {
        copy_file(&mut fs, dep)
            .await
            .context("write dependency to fs")?;
    }

    let device = match fs.close() {
        Ok(device) => device,
        Err((mut fs, err)) => {
            error!("failed to close fs: {err}. Trying to at least fush");
            fs.flush()
                .context("flush fs after failed close. Hoping to preserve some data")?;
            return Err(err).context("close wfs");
        }
    };
    let file = device.close().context("failed to close file-devise")?;
    file.sync_data()
        .context("failed to synd written data to disk")?;

    Ok(())
}

async fn copy_file(fs: &mut WFS, source: FileSource) -> Result<()> {
    let dir_id = get_or_make_dir(fs, source.parent_dir_path)
        .await
        .context("create or get parent")?;

    let file_name: Box<str> = source.file_name.as_ref().into();
    let file_id = fs
        .create_file(file_name, dir_id)
        .context("failed to create file in fs")?;

    let mut source = File::open(source.source)
        .await
        .context("failed to open source file")?;

    const MAX_BUFFER: usize = KiB!(64);
    let mut buffer = alloc_buffer(MAX_BUFFER).context("failed to alloc copy buffer")?;
    let mut offset = 0;
    loop {
        let bytes = source
            .read(&mut buffer)
            .await
            .context("failed to read from source file")?;
        offset += bytes as u64;
        if bytes == 0 {
            break;
        }

        let buf = &buffer[0..bytes];
        fs.write_file(file_id, offset, buf)
            .context("failed to write data to fs")?;
    }

    Ok(())
}

async fn get_or_make_dir<P>(fs: &mut WFS, path: P) -> Result<FileId>
where
    P: IntoIterator<Item = Arc<str>>,
{
    let mut dir_id = FileId::ROOT;

    for segment in path {
        let current_dir = fs.read_directory(dir_id).context("read directory")?;
        dir_id = current_dir
            .entries
            .iter()
            .find(|entry| *entry.name == *segment)
            .map(|entry| Ok(entry.id))
            .unwrap_or_else(|| {
                fs.create_directory(Directory::empty(dir_id), segment.as_ref().into())
                    .context("create direcotry")
            })?;
    }
    Ok(dir_id)
}

pub fn out_path(fs: &FileSystem, config: &Config) -> PathBuf {
    let mut dir = out_dir(config);
    dir.push(format!("{}.wfs", fs.id));
    dir
}

pub fn out_dir(config: &Config) -> PathBuf {
    [&config.out_dir, "file_systems", "wfs"].iter().collect()
}

pub async fn clean(_clean: &CleanArgs, config: &Config) -> Result<()> {
    let out_dir = out_dir(config);
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)
            .await
            .context("delete wfs out dir")?;
    }
    Ok(())
}
