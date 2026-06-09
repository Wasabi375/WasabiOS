use anyhow::{Context, Result};
use gpt_builder::partition_types;
use std::{
    fs::{self, File},
    io::{self, Seek},
    path::PathBuf,
    sync::Arc,
};
use tokio::task::{JoinSet, spawn_blocking};
use uuid::Uuid;

use crate::{
    args::{BuildImageArgs, CleanArgs},
    config::{
        Config, VerifiedConfig,
        file_system::{DiskImage, FsType},
    },
    file_system::build_fs,
};

pub async fn build_from_args(args: BuildImageArgs, config: VerifiedConfig) -> Result<()> {
    let mut builds = JoinSet::new();
    let config = Arc::new(config);

    for id in args.target {
        let image = config
            .images
            .get(&id)
            .with_context(|| format!("failed to find image with id: {id}"))?
            .clone();
        let config = config.clone();
        builds.spawn(async move { build_image(image, config).await.context("build disk image") });
    }
    while let Some(success) = builds.join_next().await {
        success.context("join tokio")??
    }

    Ok(())
}

pub async fn build_image(image: Arc<DiskImage>, config: Arc<VerifiedConfig>) -> Result<()> {
    let mut build_fs_set = JoinSet::new();
    for partition in image.partitions.iter() {
        let fs = config
            .file_systems
            .get(&partition.fs)
            .with_context(|| format!("could not find fs({}) for partiotion", &partition.fs))?
            .clone();
        let config = config.clone();
        build_fs_set
            .spawn(async move { build_fs(fs, config).await.context("build fs for partition") });
    }
    while let Some(success) = build_fs_set.join_next().await {
        success.context("join tokio")??
    }

    spawn_blocking(move || -> Result<()> {
        let mut partitions = Vec::new();
        let mut disk_size = 1024 * 64; // size for GPT headers
        for partition in image.partitions.iter() {
            let uuid = partition.uuid();
            let name = &partition.name;

            let fs = &config.file_systems[&partition.fs];
            let fs_path = crate::file_system::out_path(&*fs, &config.config);

            let file = File::open(fs_path).context("open file system")?;
            let size = file.metadata().context("load file metadata")?.len();

            let typ = match fs.fs_type {
                FsType::WFS if partition.is_read_only => gpt::partition_types::READ_ONLY.into(),
                FsType::WFS => gpt::partition_types::DATA.into(),
                FsType::FAT if partition.is_boot => partition_types::EFI,
                FsType::FAT => gpt::partition_types::DATA.into(),
            };

            disk_size += size;

            partitions.push(PartitionInfo {
                uuid,
                name,
                file,
                size,
                typ,
            });
        }

        let image_path = image_path(&image, &config.config);

        if let Some(parent) = image_path.parent() {
            fs::create_dir_all(parent).context("create parent dir for disk image")?;
        }

        let mut disk = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(image_path)
            .context("create disk image file")?;

        disk.set_len(disk_size).context("set size for disk image")?;

        let mbr = gpt_builder::mbr::ProtectiveMBR::with_lb_size(
            u32::try_from((disk_size / 512) - 1).unwrap_or(0xffff_ffff),
        );
        mbr.overwrite_lba0(&mut disk)
            .context("failed to write protective MBR")?;

        let block_size = gpt_builder::disk::LogicalBlockSize::Lb512;
        let mut gpt = gpt_builder::GptConfig::new()
            .writable(true)
            .logical_block_size(block_size)
            .create_from_device(&mut disk, image.device_uuid())
            .context("Failed to create GPT disk structure")?;

        for partition in partitions {
            if partition.uuid.is_some() {
                log::error!(
                    "custom partition uuid is not yet supported and set values are ignored"
                );
            }

            let id = gpt
                .add_partition(partition.name, partition.size, partition.typ, 0, None)
                .context("add partition")?;
            let part_info = gpt
                .partitions()
                .get(&id)
                .context("failed to open partition after creation")?;
            let start_offset = part_info
                .bytes_start(block_size)
                .context("failed to get start offset of partition")?;

            gpt.device_mut()
                .seek(io::SeekFrom::Start(start_offset))
                .context("Failed to seek to start offset")?;
            let mut part_image = partition.file;
            io::copy(&mut part_image, gpt.device_mut()).with_context(|| {
                format!(
                    "failed to copy partition data to GPT disk for {}",
                    partition.name
                )
            })?;
        }
        gpt.write().context("write out GPT changes")?;

        Ok(())
    })
    .await?
}

pub fn image_dir(config: &Config) -> PathBuf {
    [&config.out_dir, "images"].iter().collect()
}

pub fn image_path(image: &DiskImage, config: &Config) -> PathBuf {
    image_dir(config).join(image.id.as_str())
}

pub async fn clean(_clean: &CleanArgs, config: &Config) -> Result<()> {
    let image_dir = image_dir(config);
    if image_dir.exists() {
        fs::remove_dir_all(image_dir).context("delete image dir")?;
    }
    Ok(())
}

struct PartitionInfo<'a> {
    file: File,
    uuid: Option<Uuid>,
    name: &'a str,
    size: u64,
    typ: partition_types::Type,
}
