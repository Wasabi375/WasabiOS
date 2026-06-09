use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
    sync::Arc,
};

use crate::{
    args::CleanArgs,
    config::{Config, VerifiedConfig, file_system::FileSystem},
    file_system::FileSource,
};

use anyhow::{Context, Result, ensure};
use fatfs::{Dir, FileSystem as FatSystem, FormatVolumeOptions, FsOptions, format_volume};
use tokio::{runtime::Handle, task::spawn_blocking};

use super::TreeWalk;

pub async fn build_fat_fs(
    fs_conf: Arc<FileSystem>,
    config: Arc<VerifiedConfig>,
    static_files: Option<TreeWalk>,
    dependencies: impl Future<Output = Result<Vec<FileSource>>> + Send + 'static,
) -> Result<()> {
    spawn_blocking(move || build_fat_fs_sync(&fs_conf, &config.config, static_files, dependencies))
        .await
        .context("join tokio spawn blocking")?
}

fn build_fat_fs_sync(
    fs_conf: &FileSystem,
    config: &Config,
    static_files: Option<TreeWalk>,
    dependencies: impl Future<Output = Result<Vec<FileSource>>>,
) -> Result<()> {
    let path = out_path(fs_conf, config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create parent dir for fat disk image")?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .context("create file for fat disk image")?;

    ensure!(fs_conf.size.bytes() >= 1024, "fs not large enough");

    file.set_len(fs_conf.size.bytes())
        .context("failed to set the file len for disk image")?;

    format_volume(&mut file, FormatVolumeOptions::new()).context("format image with fat fs")?;

    let mut fs = FatSystem::new(file, FsOptions::new()).context("open newly created fat fs")?;

    if let Some(static_files) = static_files {
        for static_file in TreeWalkIter(static_files) {
            let static_file = static_file.context("walk static input file-tree")?;
            copy_file(&mut fs, static_file).context("write staic file to fs")?;
        }
    }

    let tokio = Handle::current();
    let dependencies = tokio
        .block_on(dependencies)
        .context("build fs dependencies")?;
    for dep in dependencies {
        copy_file(&mut fs, dep).context("write dependency to fs")?;
    }
    fs.unmount().context("unmount fs")?;

    Ok(())
}

fn copy_file(fs: &mut FatSystem<File>, source: FileSource) -> Result<()> {
    let dir = get_or_make_dir(fs, source.parent_dir_path).context("create or get parent dir")?;

    let mut file = dir
        .create_file(&source.file_name)
        .context("failed to create file in fs")?;

    let mut source_file = File::open(source.source).context("open file source")?;
    std::io::copy(&mut source_file, &mut file)
        .context("failed to copy data from source file to fs")?;

    Ok(())
}

fn get_or_make_dir<'a, P>(fs: &'a mut FatSystem<File>, path: P) -> Result<Dir<'a, File>>
where
    P: IntoIterator<Item = Arc<str>>,
{
    let mut current_dir = fs.root_dir();
    for path_segment in path {
        current_dir = current_dir
            .create_dir(&path_segment)
            .context("failed to create dir in fs")?;
    }

    Ok(current_dir)
}

pub fn out_path(fs: &FileSystem, config: &Config) -> PathBuf {
    let mut dir = out_dir(config);
    dir.push(format!("{}.fat", fs.id));
    dir
}

pub fn out_dir(config: &Config) -> PathBuf {
    [&config.out_dir, "file_systems", "fat"].iter().collect()
}

pub async fn clean(_clean: &CleanArgs, config: &Config) -> Result<()> {
    let out_dir = out_dir(config);
    if out_dir.exists() {
        tokio::fs::remove_dir_all(out_dir)
            .await
            .context("delete wfs out dir")?;
    }
    Ok(())
}

struct TreeWalkIter(TreeWalk);
impl Iterator for TreeWalkIter {
    type Item = Result<FileSource>;

    fn next(&mut self) -> Option<Self::Item> {
        let handle = Handle::current();
        let next = handle.block_on(async { self.0.next_entry().await });
        next.transpose()
    }
}
