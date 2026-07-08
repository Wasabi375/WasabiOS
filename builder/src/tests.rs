use std::{
    fmt::Display,
    io::ErrorKind,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    ops::{Add, AddAssign},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use log::{info, trace};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Command,
    select,
    time::{sleep, timeout},
};

use crate::{
    args::TestArgs,
    config::{
        VerifiedConfig,
        file_system::{DiskImage, ImageId},
        tests::{Test, TestGroup, TestId, TestSystem},
    },
    run::{QemuDrive, QemuExitCode, QemuProcess, RunImageWith, run_image_with},
    sync::FutureCache,
};

static TEST_CACHE: FutureCache<TestId, Result<()>> = FutureCache::const_new();

pub async fn test_with_args(args: TestArgs, config: VerifiedConfig) -> Result<()> {
    let config = Arc::new(config);
    let args = Arc::new(args);
    let targets = if args.targets.is_empty() {
        &config.config.default_tests
    } else {
        &args.targets
    };

    test_targets(targets.iter().cloned(), config.clone(), args.clone()).await
}

pub async fn test_targets<T: IntoIterator<Item = TestId>>(
    targets: T,
    config: Arc<VerifiedConfig>,
    args: Arc<TestArgs>,
) -> Result<()> {
    for target in targets {
        match config
            .tests
            .get(&target)
            .ok_or(anyhow!("Could not find test with id: {target}"))?
        {
            Test::System(system) => {
                test_system(system.clone(), config.clone(), args.clone()).await?
            }
            Test::Group(group) => test_group(&*group, config.clone(), args.clone())
                .await
                .with_context(|| format!("test group: {}", group.id))?,
        }
    }
    Ok(())
}

pub async fn test_group(
    group: &TestGroup,
    config: Arc<VerifiedConfig>,
    args: Arc<TestArgs>,
) -> Result<()> {
    for test_id in &group.systems {
        let mut test = config
            .test_systems
            .get(test_id)
            .ok_or(anyhow!("Could not find test with id: {test_id}"))?
            .as_ref()
            .clone();

        if !group.all() {
            if !group.cargo_only && test.cargo.take().is_some() {
                info!(
                    "skip cargo test in {} because of group rule {}",
                    test.id, group.id
                );
            }
            if !group.kernel_only && test.kernel.take().is_some() {
                info!(
                    "skip kernel test in {} because of group rule {}",
                    test.id, group.id
                );
            }
        }
        test_system(Arc::new(test), config.clone(), args.clone()).await?;
    }
    Ok(())
}

pub async fn test_system(
    system: Arc<TestSystem>,
    config: Arc<VerifiedConfig>,
    args: Arc<TestArgs>,
) -> Result<()> {
    let test_id = system.id.clone();
    let test_future = async move {
        if let Some(cargo) = system.cargo.as_ref() {
            test_cargo(cargo, system.id.clone(), &config)
                .await
                .with_context(|| format!("cargo test {}", system.id))?;
        }
        if let Some(kernel) = system.kernel.clone() {
            test_kernel(kernel, &*system, config, args)
                .await
                .with_context(|| format!("kernel test {}", system.id))?;
        }
        anyhow::Ok(())
    };

    if let Err(err) = &*TEST_CACHE
        .spawn_or_get(test_id.clone(), test_future)
        .await
        .join()
        .await
    {
        bail!("test system {test_id}:\n{err:?}");
    }
    Ok(())
}

async fn test_cargo(package: &str, test_id: TestId, config: &VerifiedConfig) -> Result<()> {
    let build_opts = &config.config.general_build;

    let mut cmd = Command::new("cargo");
    cmd.kill_on_drop(true);

    cmd.arg("test");
    cmd.arg("--package").arg(package);

    if let Some(jobs) = build_opts.jobs {
        cmd.arg("--jobs").arg(&format!("{jobs}"));
    }

    info!("cargo test {test_id}");
    let success = cmd
        .status()
        .await
        .context("cargo build execution")?
        .success();
    ensure!(success, "cargo test {} failed", test_id);

    Ok(())
}

fn test_qemu_run_extras() -> RunImageWith {
    RunImageWith {
        extra_devices: vec!["isa-debug-exit,iobase=0xf4,iosize=0x04".into()],
        extra_drives: vec![
            QemuDrive::nvme("test_nvme", "test_data/drive_images/test_nvme_data.txt"),
            QemuDrive::nvme(
                "test_byte_pattern",
                "test_data/drive_images/nvme_byte_pattern",
            ),
        ],
        ..Default::default()
    }
}

async fn test_kernel(
    kernel: ImageId,
    system: &TestSystem,
    config: Arc<VerifiedConfig>,
    args: Arc<TestArgs>,
) -> Result<()> {
    let image = config
        .images
        .get(&kernel)
        .ok_or(anyhow!("could not find kernel image {kernel}"))?
        .clone();

    let qemu_test_conf = &config.config.qemu.test;

    if qemu_test_conf.no_tcp {
        return test_no_tcp(image, system, config)
            .await
            .context("no tcp kernel test");
    }

    let host_socket_addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        qemu_test_conf.tcp_port,
    );

    let mut extra_opts = test_qemu_run_extras();
    extra_opts
        .serial
        .push(format!("tcp::{},server=on", qemu_test_conf.tcp_port));

    let mut test_count = TestCount::default();

    // TODO timeout for qemu

    if system.isolated(&args) {
        test_count += run_isolated_tests(
            &host_socket_addr,
            image.clone(),
            config.clone(),
            &extra_opts,
            &args,
            system,
            TestSet::Normal,
        )
        .await
        .context("run isolated tests: normal")?;
    } else {
        test_count += run_combined_tests(
            &host_socket_addr,
            image.clone(),
            config.clone(),
            &extra_opts,
            &args,
            system,
        )
        .await
        .context("run combined tests")?;
    }

    if !system.fast(&args) {
        test_count += run_isolated_tests(
            &host_socket_addr,
            image.clone(),
            config.clone(),
            &extra_opts,
            &args,
            system,
            TestSet::Panic,
        )
        .await
        .context("run isolated tests: normal")?;
    }

    let level = if test_count.success == test_count.total {
        log::Level::Info
    } else {
        log::Level::Error
    };
    log::log!(
        level,
        "{}/{} tests succeded. {} ignored\n",
        test_count.success,
        test_count.total,
        test_count.ignored,
    );

    Ok(())
}

async fn run_combined_tests(
    socket_addr: &SocketAddr,
    image: Arc<DiskImage>,
    config: Arc<VerifiedConfig>,
    extra_opts: &RunImageWith,
    test_args: &TestArgs,
    test_system: &TestSystem,
) -> Result<TestCount> {
    let mut qemu = QemuTestProcess::create(socket_addr, image.clone(), config.clone(), extra_opts)
        .await
        .context("create qemu instance")?;

    let test_count = get_test_count(&mut qemu, TestSet::Normal)
        .await
        .qcontext("query test count")
        .into_anyhow()?;

    let ignore_count = get_ignored_count(&mut qemu, TestSet::Normal)
        .await
        .qcontext("query ingore count")
        .into_anyhow()?;

    let mut success = TestCount::default();
    success.total = test_count;
    success.ignored = ignore_count;

    qemu.write_all(b"test normal\n")
        .await
        .qcontext("send resume command")
        .into_anyhow()?;

    let mut current_test: usize = 0;
    while current_test < test_count {
        let line = match recieve_line(&mut qemu).await.qcontext("receive test index") {
            Ok(line) => line,
            Err(QemuError::Exit(exit_status)) => {
                if !test_system.keep_going(test_args) {
                    bail!("Qemu exited during testing with {exit_status:?}");
                }
                current_test += 1;
                qemu = restart_qemu(
                    exit_status,
                    image.clone(),
                    config.clone(),
                    extra_opts,
                    socket_addr,
                    current_test,
                )
                .await?;
                continue;
            }
            Err(QemuError::WaitError(error)) => {
                bail!("waiting on qemu\n{error:?}")
            }
            Err(QemuError::Error(error)) => return Err(error),
        };

        ensure!(line.starts_with("start test "));
        let next_test_str = &line[11..];
        let next_test: usize = next_test_str
            .parse()
            .context("failed to parse test index")?;
        ensure!(
            current_test == next_test,
            "kernel started test {next_test} but we expected test {current_test} to run"
        );

        match qemu
            .write_all(b"start test\n")
            .await
            .qcontext("sending test start confirmation")
        {
            Ok(_) => {}
            Err(QemuError::Exit(exit_status)) => {
                if !test_system.keep_going(test_args) {
                    bail!("Qemu exited during testing with {exit_status:?}");
                }
                current_test += 1;
                qemu = restart_qemu(
                    exit_status,
                    image.clone(),
                    config.clone(),
                    extra_opts,
                    socket_addr,
                    current_test,
                )
                .await?;
                continue;
            }
            Err(QemuError::WaitError(error)) => bail!("waiting on qemu\n{error:?}"),
            Err(QemuError::Error(error)) => return Err(error),
        }

        match recieve_line(&mut qemu)
            .await
            .qcontext("receive test result")
        {
            Ok(line) => match line.as_str() {
                "test succeeded" => success.success += 1,
                "test failed" => {}
                result => bail!("unknown test result: \"{}\"", result),
            },
            Err(QemuError::Exit(exit_status)) => {
                if !test_system.keep_going(test_args) {
                    bail!("Qemu exited during testing with {exit_status:?}");
                }
                current_test += 1;
                qemu = restart_qemu(
                    exit_status,
                    image.clone(),
                    config.clone(),
                    extra_opts,
                    socket_addr,
                    current_test,
                )
                .await?;
                continue;
            }
            Err(QemuError::WaitError(error)) => {
                bail!("waiting on qemu\n{error:?}")
            }
            Err(QemuError::Error(error)) => return Err(error),
        }

        current_test += 1;
    }

    qemu.wait()
        .await
        .context("waiting for test kernel to finish")?
        .test_ok()
        .context("qemu test-kernel status")?;

    Ok(success)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TestSet {
    Normal,
    Panic,
}

async fn run_isolated_tests(
    socket_addr: &SocketAddr,
    image: Arc<DiskImage>,
    config: Arc<VerifiedConfig>,
    extra_opts: &RunImageWith,
    _test_args: &TestArgs,
    _test_system: &TestSystem,
    test_set: TestSet,
) -> Result<TestCount> {
    let mut qemu = QemuTestProcess::create(socket_addr, image.clone(), config.clone(), extra_opts)
        .await
        .context("create qemu instance")?;

    let test_count = get_test_count(&mut qemu, test_set)
        .await
        .qcontext("query test count")
        .into_anyhow()?;

    let ignore_count = get_ignored_count(&mut qemu, test_set)
        .await
        .qcontext("query ingore count")
        .into_anyhow()?;

    let start_test_cmd: &str = match test_set {
        TestSet::Normal => "isolated\n",
        TestSet::Panic => "test panic\n",
    };

    let mut success = TestCount::default();
    success.total = test_count;
    success.ignored = ignore_count;

    // temp store qemu in option, so we can move it out in the first iteration.
    // This allows reusing the qemu instance used for getting the test counts
    let mut first_iter_qemu = Some(qemu);

    let mut current_test: usize = 0;
    while current_test < test_count {
        let mut qemu = if let Some(qemu) = first_iter_qemu.take() {
            qemu
        } else {
            QemuTestProcess::create(socket_addr, image.clone(), config.clone(), extra_opts)
                .await
                .context("create qemu instance")?
        };

        match qemu
            .write_all(format!("{}{}\n", start_test_cmd, current_test).as_bytes())
            .await
            .qcontext("sending test start confirmation")
        {
            Ok(_) => {}
            Err(QemuError::Exit(exit_status)) => {
                if !matches!(exit_status, QemuExitCode::Fail) {
                    bail!(
                        "Unexpected qemu exit. Expected test failure/panic. Got: {exit_status:?}"
                    );
                }
                current_test += 1;
            }
            Err(QemuError::WaitError(error)) => bail!("waiting on qemu\n{error:?}"),
            Err(QemuError::Error(error)) => return Err(error),
        }

        let line = match recieve_line(&mut qemu).await.qcontext("receive test index") {
            Ok(line) => line,
            Err(QemuError::Exit(exit_status)) => {
                if !matches!(exit_status, QemuExitCode::Fail) {
                    bail!(
                        "Unexpected qemu exit. Expected test failure/panic. Got: {exit_status:?}"
                    );
                }
                current_test += 1;
                continue;
            }
            Err(QemuError::WaitError(error)) => bail!("waiting on qemu\n{error:?}"),
            Err(QemuError::Error(error)) => return Err(error),
        };
        assert!(line.starts_with("start test "));
        let run_test_str = &line[11..];
        let run_test: usize = run_test_str.parse().context("failed to parse test index")?;
        ensure!(
            current_test == run_test,
            "kernel started test {run_test} but we expected test {current_test} to run"
        );

        match qemu.wait().await? {
            QemuExitCode::ExitNormal => {
                bail!("Expected test kernel to exit qemu with either success or failue")
            }
            QemuExitCode::Success => success.success += 1,
            QemuExitCode::Fail => (),
            QemuExitCode::Unknown(code) => {
                bail!("Qemu exited with unexpected exit code 0x{:x}", code)
            }
        }
        current_test += 1;
    }

    Ok(success)
}

async fn restart_qemu(
    exit_status: QemuExitCode,
    kernel: Arc<DiskImage>,
    config: Arc<VerifiedConfig>,
    extra_opts: &RunImageWith,
    host_socket_addr: &SocketAddr,
    next_test: usize,
) -> Result<QemuTestProcess> {
    if !matches!(exit_status, QemuExitCode::Fail) {
        bail!("Unexpected qemu exit. Expected test failure/panic. Got: {exit_status:?}");
    }
    let mut qemu = QemuTestProcess::create(&host_socket_addr, kernel, config, extra_opts)
        .await
        .context("restarting qemu")?;

    qemu.write_all(b"resume\n")
        .await
        .qcontext("send resume command")
        .into_anyhow()?;
    qemu.write_all(format!("{}\n", next_test).as_bytes())
        .await
        .qcontext("send resume command index")
        .into_anyhow()?;

    Ok(qemu)
}

async fn test_no_tcp(
    kernel: Arc<DiskImage>,
    system: &TestSystem,
    config: Arc<VerifiedConfig>,
) -> Result<()> {
    let extra_opts = test_qemu_run_extras();

    let mut qemu = run_image_with(kernel, config, &extra_opts)
        .await
        .context("launch qemu")?;
    with_timeout(Duration::from_secs(system.timeout_secs), qemu.wait())
        .await
        .context("run image")?
        .test_ok()?;

    Ok(())
}

async fn with_timeout<T, F: Future<Output = Result<T>>>(timeout: Duration, f: F) -> Result<T> {
    tokio::time::timeout(timeout, f)
        .await
        .map_err(|_| anyhow!("Qemu timed out after {timeout:?}"))?
}

#[derive(Debug, Clone, Copy, Default)]
struct TestCount {
    success: usize,
    ignored: usize,
    total: usize,
}

impl Add for TestCount {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        TestCount {
            success: self.success + rhs.success,
            ignored: self.ignored + rhs.ignored,
            total: self.total + rhs.total,
        }
    }
}

impl AddAssign for TestCount {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

struct QemuTestProcess {
    process: QemuProcess,
    tcp: TcpStream,
}

const TCP_CONNECTION_RETRY_COUNT: u32 = 5;
const TCP_CONNECTION_RETRY_DELAY: Duration = Duration::from_millis(200);
const QEMU_EOF_WAIT_TIMEOUT: Duration = Duration::from_millis(500);

enum Never {}

impl QemuTestProcess {
    async fn create(
        socket_addr: &SocketAddr,
        kernel: Arc<DiskImage>,
        config: Arc<VerifiedConfig>,
        extra_opts: &RunImageWith,
    ) -> Result<Self> {
        let mut process = run_image_with(kernel, config, extra_opts)
            .await
            .context("launch qemu")?;

        let mut retries = TCP_CONNECTION_RETRY_COUNT;
        let mut tcp = loop {
            match TcpStream::connect(socket_addr).await.context(format!(
                "failed to establish tcp connection to qemu instance: {socket_addr}"
            )) {
                Ok(tcp) => break tcp,
                Err(e) if retries == 0 => return Err(e),
                Err(e) => {
                    trace!("failed to connect to tcp. Retrying... {e}");
                    retries -= 1;
                    sleep(TCP_CONNECTION_RETRY_DELAY).await;
                }
            }
        };

        select! {
            res = handshake(&mut tcp) => {
                res.context("perform test handshake")?;
            }
            qemu_exit = process.wait() => {
                let status = qemu_exit.context("wait on qemu");
                bail!("Qemu unexpectedly exited with {:?} during handshake", status);
            }
        }
        Ok(Self { process, tcp })
    }

    async fn wait(&mut self) -> Result<QemuExitCode> {
        self.process
            .wait()
            .await
            .context("waiting on qemu to finish")
    }

    fn handle_qemu_exit(qemu_exit: Result<QemuExitCode>) -> Result<Never, QemuError> {
        match qemu_exit {
            Ok(status) => Err(QemuError::Exit(status)),
            Err(err) => Err(QemuError::WaitError(err)),
        }
    }

    async fn read_u8(&mut self) -> Result<u8, QemuError> {
        select! {
            byte = self.tcp.read_u8() => {
                return match byte                {
                    Ok(byte) => Ok(byte),
                    Err(error) => Err(self.handle_tcp_error(error,
                                    "failed to read u8 from qemu").await),
                }
            }
            qemu_exit = self.process.wait() => {
                Self::handle_qemu_exit(qemu_exit)?;
                unreachable!()
            }
        }
    }

    async fn write_all<'a>(&'a mut self, bytes: &'a [u8]) -> Result<(), QemuError> {
        select! {
            io_result = self.tcp.write_all(bytes) => {
                return match io_result {
                    Ok(_) => Ok(()),
                    Err(error) => Err(self.handle_tcp_error(error,
                                        "failed to write data to tcp stream").await)
                }
            }
            qemu_exit = self.process.wait() => {
                Self::handle_qemu_exit(qemu_exit)?;
                unreachable!()
             }
        }
    }

    /// helper function to handle tcp errors in a way that checks for qemu exit on EOF
    async fn handle_tcp_error<C>(&mut self, error: std::io::Error, context: C) -> QemuError
    where
        C: Display + Send + Sync + 'static,
    {
        match error.kind() {
            ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset => {
                match timeout(QEMU_EOF_WAIT_TIMEOUT, self.process.wait()).await {
                    Ok(Ok(exit_status)) => QemuError::Exit(exit_status),
                    _ => {
                        let anyhow = anyhow::Error::new(error).context(context);
                        QemuError::Error(anyhow)
                    }
                }
            }
            _ => {
                let anyhow = anyhow::Error::new(error).context(context);
                QemuError::Error(anyhow)
            }
        }
    }
}

async fn handshake(tcp_stream: &mut TcpStream) -> Result<()> {
    // drain stream until "tests ready" is received
    // there might be random data from the startup in there that we don't care about
    const EXPECTED: &[u8] = b"tests ready\n";
    let mut respones_position = 0;
    loop {
        let c = tcp_stream
            .read_u8()
            .await
            .context("failed to read u8 from tcp")?;
        if c == EXPECTED[respones_position] {
            respones_position += 1;
        } else {
            respones_position = 0;
        }
        if respones_position == EXPECTED.len() {
            break;
        }
    }

    tcp_stream
        .write_all(EXPECTED)
        .await
        .context("failed to send handshake \"start testing\"")?;

    Ok(())
}

async fn recieve_line(qemu: &mut QemuTestProcess) -> Result<String, QemuError> {
    let mut buffer = String::new();
    loop {
        let b = qemu
            .read_u8()
            .await
            .qcontext("failed to read u8 from tcp")?;
        if b == b'\n' {
            return Ok(buffer);
        } else {
            buffer.push(b.into())
        }
    }
}

async fn parse_line<T>(qemu: &mut QemuTestProcess) -> Result<T, QemuError>
where
    T: FromStr,
    T::Err: std::fmt::Debug + std::error::Error + Send + Sync + 'static,
{
    Ok(recieve_line(qemu)
        .await
        .qcontext("failed to receive line")?
        .parse()
        .context("failed to parse line")?)
}

async fn get_test_count(qemu: &mut QemuTestProcess, set: TestSet) -> Result<usize, QemuError> {
    let cmd: &[u8] = match set {
        TestSet::Normal => b"count\n".as_ref(),
        TestSet::Panic => b"count panic\n".as_ref(),
    };

    qemu.write_all(cmd)
        .await
        .qcontext("failed to send \"count\" command")?;

    parse_line(qemu)
        .await
        .qcontext("failed to recieve test count")
}

async fn get_ignored_count(qemu: &mut QemuTestProcess, set: TestSet) -> Result<usize, QemuError> {
    let cmd: &[u8] = match set {
        TestSet::Normal => b"count ignored\n".as_ref(),
        TestSet::Panic => b"count ignored panic\n".as_ref(),
    };

    qemu.write_all(cmd)
        .await
        .qcontext("failed to send \"count\" command")?;

    parse_line(qemu)
        .await
        .qcontext("failed to recieve test count")
}

/// The error used for communication to qemu
#[derive(Debug)]
enum QemuError {
    /// Qemu instance terminated
    Exit(QemuExitCode),
    /// Waiting for qemu instance failed.
    WaitError(anyhow::Error),
    /// The connection failed or recieved invalid data.
    ///
    /// In case of connection EoF functions should wait on Qemu for a short
    /// time and faill with Exit (if possible)
    Error(anyhow::Error),
}

trait QemuErrorContext<T> {
    fn qcontext<C>(self, context: C) -> Result<T, QemuError>
    where
        C: Display + Send + Sync + 'static;

    fn into_anyhow(self) -> Result<T, anyhow::Error>;
}

impl<T> QemuErrorContext<T> for Result<T, QemuError> {
    fn qcontext<C>(self, context: C) -> Result<T, QemuError>
    where
        C: Display + Send + Sync + 'static,
    {
        match self {
            Ok(ok) => Ok(ok),
            Err(QemuError::Exit(exit)) => Err(QemuError::Exit(exit)),
            Err(QemuError::WaitError(error)) => Err(QemuError::WaitError(error.context(context))),
            Err(QemuError::Error(error)) => Err(QemuError::Error(error.context(context))),
        }
    }

    fn into_anyhow(self) -> Result<T, anyhow::Error> {
        match self {
            Ok(ok) => Ok(ok),
            Err(QemuError::Error(err)) => Err(err),
            Err(QemuError::WaitError(err)) => bail!("failed to wait on qemu\n{err:?}"),
            Err(QemuError::Exit(status)) => bail!("qemu exited unexpectedly with {status:?}"),
        }
    }
}

impl From<anyhow::Error> for QemuError {
    fn from(value: anyhow::Error) -> Self {
        Self::Error(value)
    }
}
