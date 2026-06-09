// TODO do I want to move this under qemu?

use std::path::PathBuf;

use crate::{
    args::CleanArgs,
    config::{CleanOvmf, Config},
};

use anyhow::{Context, Result, ensure};
use log::debug;
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File, create_dir_all},
    io::AsyncWriteExt,
    process::Command,
};

#[derive(Clone, Copy, Debug)]
#[allow(unused)]
pub enum OvmfFileType {
    Code,
    Vars,
    Shell,
}

impl OvmfFileType {
    fn file_name(&self) -> &'static str {
        match self {
            OvmfFileType::Code => "code.fd",
            OvmfFileType::Vars => "vars.fd",
            OvmfFileType::Shell => "shell.efi",
        }
    }
}

pub async fn download_if_missing(config: &Config) -> Result<()> {
    let ovmf_conf = &config.qemu.ovmf;

    let prebuild_dir = ovmf_prebuild_dir(config);
    if prebuild_dir.exists()
        && prebuild_dir
            .join(format!("{}.sha256", &ovmf_conf.prebuild_tag))
            .exists()
        && prebuild_dir
            .join(format!("{}.tar.xz", &ovmf_conf.prebuild_tag))
            .exists()
    {
        return Ok(());
    }

    download(config).await
}

pub async fn download(config: &Config) -> Result<()> {
    let ovmf_conf = &config.qemu.ovmf;
    let response = reqwest::get(&ovmf_conf.download_url)
        .await
        .context("donwload tarbal")?;
    let data = response.bytes().await.context("get response body bytes")?;

    debug!(
        "downloaded {} bytes from {}",
        data.len(),
        ovmf_conf.download_url
    );

    let prebuild_dir = ovmf_prebuild_dir(config);

    create_dir_all(&prebuild_dir)
        .await
        .context("create ovmf prebuild directory")?;

    let tarball_path = prebuild_dir.join(format!("{}.tar.xz", ovmf_conf.prebuild_tag));
    let mut tarball = File::create(&tarball_path)
        .await
        .context("create ovmf tarball file")?;
    tarball
        .write_all(&data)
        .await
        .context("write ovmf tarball to disk")?;
    drop(tarball);

    let actual_hash = format!("{:x}", Sha256::digest(&data));
    ensure!(
        actual_hash == ovmf_conf.prebuild_hash,
        "prebuild hash {actual_hash} does not match {}",
        ovmf_conf.prebuild_hash
    );

    let sig = prebuild_dir.join(format!("{}.sha256", ovmf_conf.prebuild_tag));
    let mut sig = File::create(sig).await.context("create signature file")?;
    sig.write_all(actual_hash.as_bytes())
        .await
        .context("write signature to file")?;
    drop(sig);

    let mut cmd = Command::new("tar");
    cmd.kill_on_drop(true);
    cmd.arg("--strip-components=1")
        .arg("-xf")
        .arg(&tarball_path)
        .arg("--directory")
        .arg(prebuild_dir);
    let success = cmd.status().await.context("extract tarball")?.success();
    ensure!(success, "'tar xf {}' failed", tarball_path.display());

    Ok(())
}

pub fn ovmf_file_path(config: &Config, typ: OvmfFileType) -> PathBuf {
    let ovmf_conf = &config.qemu.ovmf;
    [
        &config.out_dir,
        &ovmf_conf.storage_path,
        &ovmf_conf.prebuild_tag,
        config.general_build.architecture.short_name(),
        typ.file_name(),
    ]
    .iter()
    .collect()
}

fn ovmf_prebuild_dir(config: &Config) -> PathBuf {
    let ovmf_conf = &config.qemu.ovmf;

    [
        &config.out_dir,
        &ovmf_conf.storage_path,
        &ovmf_conf.prebuild_tag,
    ]
    .iter()
    .collect()
}

pub async fn clean(clean: &CleanArgs, config: &Config) -> Result<()> {
    debug!("clean");
    let clean_subset = if clean.all {
        CleanOvmf::All
    } else {
        config.clean.ovmf
    };

    let ovmf_conf = &config.qemu.ovmf;

    let ovmf_dir: PathBuf = [&config.out_dir, &ovmf_conf.storage_path].iter().collect();
    if !ovmf_dir.exists() {
        return Ok(());
    }

    match clean_subset {
        CleanOvmf::All => {
            fs::remove_dir_all(&ovmf_dir)
                .await
                .context("delete ovmf data")?;
        }
        CleanOvmf::Unused => {
            let mut dir_iter = fs::read_dir(ovmf_dir).await.context("read ovmf data dir")?;
            while let Some(entry) = dir_iter
                .next_entry()
                .await
                .context("get next ovmf data dir entry")?
            {
                if entry.file_name().into_string().as_ref() == Ok(&ovmf_conf.prebuild_tag) {
                    continue;
                }
                if entry.file_type().await?.is_dir() {
                    fs::remove_dir_all(entry.path()).await.with_context(|| {
                        format!("remove ovmf dataset: {}", &ovmf_conf.prebuild_tag)
                    })?;
                }
            }
        }
        CleanOvmf::None => return Ok(()),
    }

    Ok(())
}
