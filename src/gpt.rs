use anyhow::{Context, Result, bail};
use fs_err::File;
use gpt_builder::partition_types::{self, Type as PartitionType};
use std::{
    borrow::Cow,
    cmp::max,
    fmt::Debug,
    fs,
    io::{self, Seek},
    path::Path,
};
use tempfile::NamedTempFile;
use uuid::Uuid;

/// Builder to construct gpt based disk image
#[derive(Clone)]
pub struct GptBuilder {
    image_path: Cow<'static, Path>,
    kernel_elf: Cow<'static, Path>,

    boot_image_files: Vec<FileSource>,
    partitions: Vec<Partition>,

    gpt_device_guid: Option<Uuid>,
}

impl GptBuilder {
    const KERNEL_FILE_NAME: &str = "kernel-x86_64";
    const UEFI_BOOT_FILENAME: &str = "efi/boot/bootx64.efi";
    const BOOT_IMAGE_RESERVED_FILES: &[&str] = &[Self::UEFI_BOOT_FILENAME, Self::KERNEL_FILE_NAME];

    pub fn new(
        image_path: impl Into<Cow<'static, Path>>,
        kernel_elf: impl Into<Cow<'static, Path>>,
    ) -> Self {
        Self {
            image_path: image_path.into(),
            kernel_elf: kernel_elf.into(),
            boot_image_files: Vec::new(),
            partitions: Vec::new(),
            gpt_device_guid: None,
        }
    }

    #[allow(dead_code)]
    pub fn set_gpt_device_uuid(&mut self, uuid: Option<Uuid>) -> Option<Uuid> {
        let old = self.gpt_device_guid.take();
        self.gpt_device_guid = uuid;
        old
    }

    #[allow(dead_code)]
    pub fn add_file_to_boot(&mut self, file: FileSource) -> Result<()> {
        if Self::BOOT_IMAGE_RESERVED_FILES
            .iter()
            .any(|reserved| *reserved == file.destination)
        {
            bail!("file {file:?} uses a reserved filepath");
        }

        if let Some(other) = self.boot_image_files.iter().find(
            |FileSource {
                 destination: path,
                 source: _,
             }| *path == file.destination,
        ) {
            bail!(
                "file {file:?} has the same path as another file that was already added: {other:?}"
            );
        }

        self.boot_image_files.push(file);

        Ok(())
    }

    #[allow(dead_code)]
    pub fn add_partition(&mut self, partition: Partition) {
        self.partitions.push(partition);
    }

    pub fn build(mut self) -> Result<Cow<'static, Path>> {
        let mut fat_builder = bootloader::DiskImageBuilder::new(self.kernel_elf.to_path_buf());
        for f in self.boot_image_files.drain(..) {
            match f.source {
                FileDataSource::File(path) => {
                    fat_builder.set_file(f.destination.into_owned(), path.into_owned())
                }
                FileDataSource::Data(data) => {
                    fat_builder.set_file_contents(f.destination.into_owned(), data.into_owned())
                }
            };
        }

        let fat_image =
            NamedTempFile::new().context("Failed to create temp file for FAT partition")?;

        fat_builder
            .create_uefi_fat_partition(fat_image.path())
            .context("build boot partition")?;

        let fat_partition = Partition {
            image_path: fat_image.path().to_owned().into(),
            name: "boot".into(),
            min_size: 0,
            typ: partition_types::EFI,
            uuid: None,
        };

        self.partitions.insert(0, fat_partition);
        let disk_size = self
            .partitions
            .iter_mut()
            .map(|partition| {
                fs::metadata(partition.image_path.as_ref())
                    .with_context(|| {
                        format!(
                            "Failed to read metadata for partition image at: {}",
                            partition.image_path.display()
                        )
                    })
                    .map(|meta| meta.len())
                    .map(|size| max(size, partition.min_size))
                    .map(|size| {
                        partition.min_size = size;
                        dbg!(size)
                    })
            })
            .sum::<Result<u64>>()
            .context("failed to calculate disk size")?;
        let disk_size = disk_size + 1024 * 64; // for GPT headers

        let mut disk = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(self.image_path.as_ref())
            .with_context(|| {
                format!(
                    "failed to create GPT file at `{}`",
                    self.image_path.display()
                )
            })?;
        disk.set_len(disk_size)
            .context("failed to set image file length")?;

        let mbr = gpt_builder::mbr::ProtectiveMBR::with_lb_size(
            u32::try_from((disk_size / 512) - 1).unwrap_or(0xffff_ffff),
        );
        mbr.overwrite_lba0(&mut disk)
            .context("failed to write protective MBR")?;

        let block_size = gpt_builder::disk::LogicalBlockSize::Lb512;
        let mut gpt = gpt_builder::GptConfig::new()
            .writable(true)
            .logical_block_size(block_size)
            .create_from_device(&mut disk, self.gpt_device_guid)
            .context("Failed to create GPT disk structure")?;

        for partition in self.partitions {
            if partition.uuid.is_some() {
                log::error!(
                    "custom partition uuid is not yet supported and set values are ignored"
                );
            }

            let id = gpt.add_partition(
                partition.name.as_ref(),
                partition.min_size,
                partition.typ,
                0,
                None,
            )?;
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
            let mut part_image = File::open(partition.image_path.as_ref()).with_context(|| {
                format!(
                    "failed to open partition image at: {}",
                    partition.image_path.display()
                )
            })?;
            io::copy(&mut part_image, gpt.device_mut()).with_context(|| {
                format!(
                    "failed to copy partition data to GPT disk for {}",
                    partition.name
                )
            })?;
        }
        gpt.write().context("failed to write out GPT changes")?;

        Ok(self.image_path)
    }
}

#[derive(Clone, Debug)]
pub struct Partition {
    pub image_path: Cow<'static, Path>,
    pub name: Cow<'static, str>,
    pub min_size: u64,
    pub typ: PartitionType,
    pub uuid: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub struct FileSource {
    pub destination: Cow<'static, str>,
    pub source: FileDataSource,
}

#[derive(Clone)]
/// Defines a data source, either a source `std::path::PathBuf`, or a vector of bytes.
pub enum FileDataSource {
    File(Cow<'static, Path>),
    #[allow(dead_code)]
    Data(Cow<'static, [u8]>),
}

impl From<Cow<'static, Path>> for FileDataSource {
    fn from(value: Cow<'static, Path>) -> Self {
        Self::File(value)
    }
}

impl Debug for FileDataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("FileDataSource");
        match self {
            Self::File(path) => ds.field("File", path).finish(),
            Self::Data(_data) => ds.finish_non_exhaustive(),
        }
    }
}
