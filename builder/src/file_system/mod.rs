use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    args::{CleanArgs, FsArgs},
    build,
    config::{
        Config, VerifiedConfig,
        file_system::{FileSystem, FsType},
    },
    file_system::{fat::build_fat_fs, wfs::build_wfs_fs},
};

use anyhow::{Context, Result, anyhow};
use tokio::{
    fs::{self, ReadDir},
    spawn,
};

mod fat;
mod wfs;

pub async fn build_from_args(args: FsArgs, config: VerifiedConfig) -> Result<()> {
    let fs = config
        .file_systems
        .get(&args.target)
        .with_context(|| format!("unknown file system: {}", args.target))?;

    build_fs(fs.clone(), Arc::new(config)).await
}

pub async fn build_fs(fs: Arc<FileSystem>, config: Arc<VerifiedConfig>) -> Result<()> {
    let dependencies = {
        let fs = fs.clone();
        let config = config.clone();
        spawn(async move { build_dependencies(fs, config).await })
    };

    let tree_walk = if let Some(input) = fs.static_input.as_ref() {
        Some(TreeWalk::new(input).await?)
    } else {
        None
    };

    match fs.fs_type {
        FsType::WFS => {
            build_wfs_fs(fs, &config.config, tree_walk, async move {
                dependencies
                    .await
                    .context("tokio join build dependencies")?
            })
            .await
        }
        FsType::FAT => {
            build_fat_fs(fs, config, tree_walk, async move {
                dependencies
                    .await
                    .context("tokio join build dependencies")?
            })
            .await
        }
    }
}

pub fn out_path(fs: &FileSystem, config: &Config) -> PathBuf {
    match fs.fs_type {
        FsType::WFS => wfs::out_path(fs, config),
        FsType::FAT => fat::out_path(fs, config),
    }
}

struct FileSource {
    file_name: Arc<str>,
    parent_dir_path: Vec<Arc<str>>,
    source: PathBuf,
}

async fn build_dependencies(
    fs: Arc<FileSystem>,
    config: Arc<VerifiedConfig>,
) -> Result<Vec<FileSource>> {
    let build_targets: Vec<_> = fs
        .generated
        .iter()
        .map(|input| (input.build.clone(), input.artifact))
        .collect();

    let out_paths = build::build_artifacts(build_targets, config)
        .await
        .context("build depencies")?;

    Ok(fs
        .generated
        .iter()
        .zip(out_paths.into_iter())
        .map(|(input_conf, path)| {
            let (parent_dir_path, file_name) =
                split_path_to_parent_and_filename(&input_conf.path_in_fs);
            FileSource {
                file_name,
                parent_dir_path,
                source: path,
            }
        })
        .collect())
}

pub async fn clean(clean: &CleanArgs, config: &Config) -> Result<()> {
    wfs::clean(clean, config).await.context("clean wfs")?;
    fat::clean(clean, config).await.context("clean fat")?;
    Ok(())
}

fn split_path_to_parent_and_filename(path: &str) -> (Vec<Arc<str>>, Arc<str>) {
    let mut path: Vec<Arc<str>> = path
        .trim_matches('/')
        .split('/')
        .map(|seg| seg.to_string().into())
        .collect();

    let file_name = path
        .pop()
        .expect("there should always be at least 1 segment");
    (path, file_name)
}

struct TreeWalk {
    read_dir: ReadDir,
    recurse: Option<Box<TreeWalk>>,
    dir_in_fs: Vec<Arc<str>>,
}

impl TreeWalk {
    async fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let read_dir = fs::read_dir(path).await?;
        Ok(Self {
            read_dir,
            recurse: None,
            dir_in_fs: Vec::new(),
        })
    }
    async fn subdir<P: AsRef<Path>>(path: P, dir_in_fs: Vec<Arc<str>>) -> Result<Self> {
        let read_dir = fs::read_dir(path).await?;
        Ok(Self {
            read_dir,
            recurse: None,
            dir_in_fs,
        })
    }

    async fn next_entry(&mut self) -> Result<Option<FileSource>> {
        if let Some(recurse) = self.recurse.as_mut() {
            match Box::pin(recurse.next_entry()).await? {
                Some(entry) => return Ok(Some(entry)),
                None => {
                    self.recurse = None;
                }
            }
        }

        if let Some(entry) = self.read_dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            let file_name = entry
                .file_name()
                .into_string()
                .map_err(|file| anyhow!("expected utf8 filename but found: {file:?}"))?
                .into();

            if file_type.is_dir() {
                assert!(self.recurse.is_none());
                let mut dir_in_fs = self.dir_in_fs.clone();
                dir_in_fs.push(file_name);
                self.recurse = Some(Box::new(Self::subdir(&entry.path(), dir_in_fs).await?));
                return Box::pin(self.next_entry()).await;
            }
            if file_type.is_symlink() {
                todo!("symlinks not implemented");
            }

            return Ok(Some(FileSource {
                file_name,
                parent_dir_path: self.dir_in_fs.clone(),
                source: entry.path(),
            }));
        }

        return Ok(None);
    }
}
