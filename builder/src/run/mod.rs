use std::sync::Arc;

use crate::{
    args::RunArgs,
    config::{VerifiedConfig, file_system::DiskImage},
    disk_image::{build_image, image_path},
    ovmf::{self, OvmfFileType, ovmf_file_path},
};

use anyhow::{Context, Result, anyhow, ensure};
use log::info;
use tokio::{process::Command, task::JoinSet};

pub async fn run_with_args(args: RunArgs, config: VerifiedConfig) -> Result<()> {
    let target_image = args
        .target
        .or(config.config.default_run_image.clone())
        .ok_or(anyhow!("no disk image selected! Either set `default_run_image` in config or use the target argument"))?;

    let target_image = config
        .images
        .get(&target_image)
        .with_context(|| format!("disk image {target_image} does not exist"))?
        .clone();

    run_image(target_image, config.into()).await
}

pub async fn run_image(image: Arc<DiskImage>, config: Arc<VerifiedConfig>) -> Result<()> {
    let mut pre_run = JoinSet::new();
    {
        let image = image.clone();
        let config = config.clone();
        pre_run.spawn(async move {
            build_image(image, config.clone())
                .await
                .context("build disk image")
        });
    }
    {
        let config = config.clone();
        pre_run.spawn(async move {
            ovmf::download_if_missing(&config.config)
                .await
                .context("download ovmf")
        });
    }
    while let Some(status) = pre_run.join_next().await {
        status.context("join tokio")?.context("pre-run image")?;
    }

    let image_path = image_path(&image, &config.config);
    let qemu_config = &config.config.qemu;

    let mut cmd = Command::new(format!(
        "qemu{}",
        config.config.general_build.architecture.qemu_extension()
    ));

    cmd.arg("-drive")
        .arg(&format!(
            "if=pflash,format=raw,readonly=on,file={}",
            ovmf_file_path(&config.config, OvmfFileType::Code)
                .into_string()
                .map_err(|_| anyhow!("ovmf path should be utf8"))?
        ))
        .arg("-drive")
        .arg(&format!(
            "if=pflash,format=raw,readonly=off,file={}",
            ovmf_file_path(&config.config, OvmfFileType::Vars)
                .into_string()
                .map_err(|_| anyhow!("ovmf path should be utf8"))?
        ));

    cmd.arg("-drive").arg(&format!(
        "format=raw,id=boot,if=none,file={}",
        image_path
            .into_string()
            .map_err(|_| anyhow!("disk image path should be utf8"))?,
    ));
    cmd.arg("-device").arg("nvme,serial=deadbeef,drive=boot");

    if qemu_config.serial.is_empty() {
        cmd.arg("-serial").arg("none");
    } else {
        for serial in qemu_config.serial.iter() {
            cmd.arg("-serial").arg(serial);
        }
    }

    if qemu_config.debug_log.is_none() {
        cmd.arg("-enable-kvm");
        cmd.arg("-cpu").arg("host");
    }

    cmd.arg("-m").arg(&qemu_config.memory);
    cmd.arg("-smp").arg(qemu_config.processor_count.to_string());

    if let Some(log_path) = qemu_config.debug_log.as_ref() {
        cmd.arg("-d").arg(&qemu_config.debug_info);
        cmd.arg("-D").arg(log_path);
    }
    cmd.arg("-no-reboot");

    cmd.kill_on_drop(true);
    info!("launch qemu: {cmd:?}");

    let output = cmd
        .spawn()
        .context("spawn qemu")?
        .wait_with_output()
        .await
        .context("qemu")?;
    ensure!(output.status.success(), "Qemu failed to run {}", image.id);

    Ok(())
}
