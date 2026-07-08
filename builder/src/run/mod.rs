use std::{
    fmt::Display,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    args::RunArgs,
    config::{VerifiedConfig, file_system::DiskImage},
    disk_image::{build_image, image_path},
    ovmf::{self, OvmfFileType, ovmf_file_path},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use log::info;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, stderr, stdout},
    process::{Child, Command},
    task::{JoinHandle, JoinSet},
};

#[repr(i32)]
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QemuExitCode {
    ExitNormal = 0,
    Success = 0x10 << 1 | 1,
    Fail = 0x11 << 1 | 1,
    Unknown(i32),
}

impl QemuExitCode {
    /// Qemu exit using the qemu-exit device(isa-debug-exit) success
    pub fn test_ok(&self) -> Result<()> {
        match self {
            QemuExitCode::ExitNormal => bail!(
                "qemu exit without shutdown using qemu-exit device(isa-debug-exit). This is unexpected for tests"
            ),
            QemuExitCode::Success => Ok(()),
            QemuExitCode::Fail => bail!("qemu exit with failure"),
            QemuExitCode::Unknown(other) => bail!("qemu exit with unknowne error code: {other}"),
        }
    }

    /// Qemu exit with a success code
    pub fn ok(&self) -> Result<()> {
        match self {
            QemuExitCode::ExitNormal => Ok(()),
            QemuExitCode::Success => {
                info!("qemu was stopped by exit device(isa-debug-exit)");
                Ok(())
            }
            QemuExitCode::Fail => bail!("qemu exit with failure"),
            QemuExitCode::Unknown(other) => bail!("qemu exit with unknowne error code: {other}"),
        }
    }

    pub const fn code(&self) -> i32 {
        match self {
            QemuExitCode::ExitNormal => 0,
            QemuExitCode::Success => 0x10 << 1 | 1,
            QemuExitCode::Fail => 0x11 << 1 | 1,
            QemuExitCode::Unknown(code) => *code,
        }
    }
}

impl From<i32> for QemuExitCode {
    fn from(value: i32) -> Self {
        const EXIT_NORMAL: i32 = QemuExitCode::ExitNormal.code();
        const SUCCESS: i32 = QemuExitCode::Success.code();
        const FAIL: i32 = QemuExitCode::Fail.code();

        match value {
            EXIT_NORMAL => QemuExitCode::ExitNormal,
            SUCCESS => QemuExitCode::Success,
            FAIL => QemuExitCode::Success,
            other => QemuExitCode::Unknown(other),
        }
    }
}

#[must_use = "Drop will cancle qemu process"]
pub struct QemuProcess {
    process: Child,

    finish: Arc<AtomicBool>,
    out_forward: Option<JoinHandle<Result<(), anyhow::Error>>>,
    err_forward: Option<JoinHandle<Result<(), anyhow::Error>>>,
}

impl QemuProcess {
    pub async fn wait(&mut self) -> Result<QemuExitCode> {
        let result = self.process.wait().await.context("get exist status");

        self.finish.store(true, Ordering::Release);
        if let Some(forward) = self.out_forward.take() {
            if let Err(e) = forward.await? {
                log::error!("Failed to forward all of stdout from child:\n{e:#?}");
            }
        }

        if let Some(forward) = self.err_forward.take() {
            if let Err(e) = forward.await? {
                log::error!("Failed to forward all of stdout from child:\n{e:#?}");
            }
        }

        let code = result?.code().ok_or(anyhow!(
            "qemu exited without error code (terminate by signal)"
        ))?;

        Ok(code.into())
    }
}

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

    let mut qemu = run_image(target_image, config.into()).await?;
    let exit = qemu.wait().await.context("wait for qemu to finish")?;

    exit.ok().context("qemu exit status")?;

    Ok(())
}

pub async fn run_image(image: Arc<DiskImage>, config: Arc<VerifiedConfig>) -> Result<QemuProcess> {
    run_image_with(image, config, &RunImageWith::default()).await
}

#[derive(Debug)]
pub struct QemuDrive {
    pub typ: QemuDriveType,
    pub file: PathBuf,
    pub id: String,
}

impl QemuDrive {
    pub fn nvme<S: Into<String>, P: Into<PathBuf>>(id: S, image_file: P) -> Self {
        Self {
            typ: QemuDriveType::Nvme,
            file: image_file.into(),
            id: id.into(),
        }
    }
}

#[derive(Debug)]
pub enum QemuDriveType {
    Nvme,
}

impl Display for QemuDriveType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QemuDriveType::Nvme => f.write_str("nvme"),
        }
    }
}

#[derive(Default)]
pub struct RunImageWith {
    pub extra_devices: Vec<String>,
    pub extra_drives: Vec<QemuDrive>,
    pub serial: Vec<String>,
}

pub async fn run_image_with(
    image: Arc<DiskImage>,
    config: Arc<VerifiedConfig>,
    with: &RunImageWith,
) -> Result<QemuProcess> {
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

    for drive in &with.extra_drives {
        cmd.arg("-drive").arg(&format!(
            "file={},if=none,id={},format=raw",
            drive.file.display(),
            drive.id
        ));
        cmd.arg("-device")
            .arg(&format!("{},serial=deadbeef,drive={}", drive.typ, drive.id));
    }

    for device in &with.extra_devices {
        cmd.arg("-device").arg(device);
    }

    if qemu_config.serial.is_empty() && with.serial.is_empty() {
        cmd.arg("-serial").arg("none");
    } else {
        ensure!(
            qemu_config.serial.len() + with.serial.len() <= 4,
            "Qemu only supports up to 4 serial devices"
        );
        for serial in qemu_config.serial.iter().chain(with.serial.iter()) {
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

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd.kill_on_drop(true);
    info!("launch qemu: {cmd:?}");

    let mut process = cmd.spawn().context("spawn qemu")?;

    let finish = Arc::new(AtomicBool::new(false));
    let child_out = process
        .stdout
        .take()
        .ok_or(anyhow!("Failed to take child stdout stream"))?;
    let out_forward = forward_io(child_out, stdout(), finish.clone());
    let child_err = process
        .stderr
        .take()
        .ok_or(anyhow!("Failed to take child stderr stream"))?;
    let err_forward = forward_io(child_err, stderr(), finish.clone());

    Ok(QemuProcess {
        process,
        finish,
        out_forward: Some(out_forward),
        err_forward: Some(err_forward),
    })
}

fn forward_io<ChildStream, TargetStream>(
    mut child_stream: ChildStream,
    mut target: TargetStream,
    finish: Arc<AtomicBool>,
) -> JoinHandle<Result<(), anyhow::Error>>
where
    ChildStream: AsyncRead + Unpin + Send + 'static,
    TargetStream: AsyncWrite + Unpin + Send + 'static,
{
    const BUFFER_SIZE: usize = 512;
    tokio::spawn(async move {
        let mut buf = [0u8; BUFFER_SIZE];
        loop {
            let len = child_stream
                .read(&mut buf)
                .await
                .context("Failed to read from child process stream")?;

            if len == 0 && finish.load(Ordering::Acquire) {
                return Ok(());
            }

            target
                .write(&buf[..len])
                .await
                .context("Failed to write to output stream")?;
        }
    })
}
