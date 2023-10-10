use crate::{
    args::TestArgs,
    qemu::{launch_qemu, launch_with_timeout, HostArchitecture, Kernel, QemuConfig},
};
use anyhow::{bail, ensure, Context, Result};
use log::debug;
use std::{
    fmt::{Debug, Display},
    io::ErrorKind,
    net::{IpAddr, SocketAddr},
    ops::{Add, AddAssign},
    path::Path,
    process::ExitStatus,
    str::FromStr,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::Child,
    select,
    time::timeout,
};

const QEMU_EXIT_SUCCESS_CODE: i32 = 0x10 << 1 | 1;
const QEMU_EXIT_FAILURE_CODE: i32 = 0x11 << 1 | 1;

const QEMU_EOF_WAIT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Default)]
struct TestCount {
    success: usize,
    total: usize,
}

impl Add for TestCount {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        TestCount {
            success: self.success + rhs.success,
            total: self.total + rhs.total,
        }
    }
}

impl AddAssign for TestCount {
    fn add_assign(&mut self, rhs: Self) {
        self.success += rhs.success;
        self.total += rhs.total;
    }
}

pub async fn test(uefi: &Path, mut args: TestArgs) -> Result<()> {
    if args.tcp_port == "none" {
        args.keep_going = false;
        args.isolated = false;
        args.fast = true;
        return tests_no_tcp(uefi, args).await;
    }

    let tcp_port: u16 = args.tcp_port.parse().context("invalid tcp_port")?;
    let host_socket_addr = SocketAddr::new(
        IpAddr::V4(
            HostArchitecture::get()
                .await
                .host_ip_addr()
                .await
                .context("could not find host addr")?,
        ),
        tcp_port,
    );

    let kernel = Kernel {
        path: &uefi,
        uefi: true,
    };

    let mut qemu_config = QemuConfig {
        devices: "isa-debug-exit,iobase=0xf4,iosize=0x04",
        ..QemuConfig::default()
    };

    let tcp_string = format!("tcp::{},server", args.tcp_port);
    qemu_config.add_serial(&tcp_string)?;

    let mut test_count = TestCount::default();

    if args.isolated {
        test_count += run_isolated_test(&args, &kernel, &qemu_config, &host_socket_addr, false)
            .await
            .context("failed to run isolated tests")?;
    } else {
        test_count += run_combined_tests(&args, &kernel, &qemu_config, &host_socket_addr)
            .await
            .context("failed to run combined tests")?;
    }

    if !args.fast {
        test_count += run_isolated_test(&args, &kernel, &qemu_config, &host_socket_addr, true)
            .await
            .context("failed to run expected-panic tests")?;
    }

    let level = if test_count.success == test_count.total {
        log::Level::Info
    } else {
        log::Level::Error
    };
    log::log!(
        level,
        "{}/{} tests succeded",
        test_count.success,
        test_count.total
    );

    Ok(())
}
async fn run_combined_tests(
    args: &TestArgs,
    kernel: &Kernel<'_>,
    qemu_config: &QemuConfig<'_>,
    host_socket_addr: &SocketAddr,
) -> Result<TestCount> {
    let mut qemu = QemuInstance::create(kernel, qemu_config, host_socket_addr)
        .await
        .context("failed to create qemu instance")?;

    let test_count = get_test_count(&mut qemu, false)
        .await
        .qcontext("query test count")
        .into_anyhow()?;

    qemu.write_all(b"test normal\n")
        .await
        .qcontext("send \"test normal\" command")
        .into_anyhow()?;

    let mut success = TestCount::default();
    success.total = test_count;

    let mut current_test: usize = 0;
    while current_test < test_count {
        let line = match recieve_line(&mut qemu).await.qcontext("receive test index") {
            Ok(line) => line,
            Err(QemuError::Exit(exit_status)) => {
                current_test += 1;
                qemu = restart_qemu_if_required(
                    exit_status,
                    args,
                    kernel,
                    qemu_config,
                    host_socket_addr,
                    current_test,
                )
                .await?;
                continue;
            }
            Err(QemuError::WaitError) => bail!("waiting on qemu failed"),
            Err(QemuError::Error(error)) => return Err(error),
        };
        assert!(line.starts_with("start test "));
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
                current_test += 1;
                qemu = restart_qemu_if_required(
                    exit_status,
                    args,
                    kernel,
                    qemu_config,
                    host_socket_addr,
                    current_test,
                )
                .await?;
                continue;
            }
            Err(QemuError::WaitError) => bail!("waiting on qemu failed"),
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
                if current_test + 1 == test_count {
                    todo!("how do we get the test result, if qemu exited before we could read the result");
                } else {
                    // otherwise restart qemu
                    current_test += 1;
                    qemu = restart_qemu_if_required(
                        exit_status,
                        args,
                        kernel,
                        qemu_config,
                        host_socket_addr,
                        current_test,
                    )
                    .await?;
                    continue;
                }
            }
            Err(QemuError::WaitError) => bail!("waiting on qemu failed"),
            Err(QemuError::Error(error)) => return Err(error),
        };

        current_test += 1;
    }
    let exit_status = qemu.wait().await.context("waiting for tests to finish")?;
    is_successful_exit(exit_status)?;

    Ok(success)
}

async fn run_isolated_test(
    _args: &TestArgs,
    kernel: &Kernel<'_>,
    qemu_config: &QemuConfig<'_>,
    host_socket_addr: &SocketAddr,
    panic_tests: bool,
) -> Result<TestCount> {
    let mut qemu = QemuInstance::create(kernel, qemu_config, host_socket_addr)
        .await
        .context("failed to create qemu instance")?;

    let test_count = get_test_count(&mut qemu, panic_tests)
        .await
        .qcontext("query test count")
        .into_anyhow()?;

    let start_test_cmd: &str = if panic_tests {
        "test panic\n"
    } else {
        "isolated\n"
    };

    let mut success = TestCount::default();
    success.total = test_count;

    // temp store qemu in option, so we can move it out in the first iteration
    let mut first_iter_qemu = Some(qemu);

    let mut current_test: usize = 0;
    while current_test < test_count {
        let mut qemu = if let Some(qemu) = first_iter_qemu.take() {
            qemu
        } else {
            QemuInstance::create(kernel, qemu_config, host_socket_addr)
                .await
                .context("failed to create qemu instance")?
        };

        match qemu
            .write_all(format!("{}{}\n", start_test_cmd, current_test).as_bytes())
            .await
        {
            Ok(_) => {}
            Err(QemuError::Exit(exit_status)) => {
                ensure!(
                    !is_successful_exit(exit_status)?,
                    "Qemu exited unexpectedly with success"
                );
                current_test += 1;
                continue;
            }
            Err(QemuError::WaitError) => bail!("waiting on qemu failed"),
            Err(QemuError::Error(error)) => return Err(error),
        }

        let line = match recieve_line(&mut qemu).await.qcontext("receive test index") {
            Ok(line) => line,
            Err(QemuError::Exit(exit_status)) => {
                ensure!(
                    !is_successful_exit(exit_status)?,
                    "Qemu exited unexpectedly with success"
                );
                current_test += 1;
                continue;
            }
            Err(QemuError::WaitError) => bail!("waiting on qemu failed"),
            Err(QemuError::Error(error)) => return Err(error),
        };
        assert!(line.starts_with("start test "));
        let next_test_str = &line[11..];
        let next_test: usize = next_test_str
            .parse()
            .context("failed to parse test index")?;
        ensure!(
            current_test == next_test,
            "kernel started test {next_test} but we expected test {current_test} to run"
        );

        let exit_status = qemu.wait().await?;
        if is_successful_exit(exit_status)? {
            success.success += 1;
        }

        current_test += 1;
    }

    Ok(success)
}

async fn restart_qemu_if_required(
    exit_status: ExitStatus,
    args: &TestArgs,
    kernel: &Kernel<'_>,
    config: &QemuConfig<'_>,
    host_socket_addr: &SocketAddr,
    next_test: usize,
) -> Result<QemuInstance> {
    if !args.keep_going {
        bail!("Qemu exited during testing with {}", exit_status);
    }
    ensure!(
        !is_successful_exit(exit_status)?,
        "Qemu exited unexpectedly with success"
    );
    let mut qemu = QemuInstance::create(kernel, config, &host_socket_addr)
        .await
        .context("restarting qemu failed")?;

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

enum Never {}

/// The error used for communication to qemu
#[derive(Debug)]
enum QemuError {
    /// Qemu instance terminated
    Exit(ExitStatus),
    /// Waiting for qemu instance failed.
    WaitError,
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
            Err(QemuError::WaitError) => Err(QemuError::WaitError),
            Err(QemuError::Error(error)) => Err(QemuError::Error(error.context(context))),
        }
    }

    fn into_anyhow(self) -> Result<T, anyhow::Error> {
        match self {
            Ok(ok) => Ok(ok),
            Err(QemuError::Error(err)) => Err(err),
            Err(QemuError::WaitError) => bail!("failed to wait on qemu"),
            Err(QemuError::Exit(status)) => bail!("qemu exited unexpectedly with {status}"),
        }
    }
}

impl From<anyhow::Error> for QemuError {
    fn from(value: anyhow::Error) -> Self {
        Self::Error(value)
    }
}

struct QemuInstance {
    child: Child,
    tcp: TcpStream,
}

impl QemuInstance {
    async fn create(
        kernel: &Kernel<'_>,
        config: &QemuConfig<'_>,
        socket_addr: &SocketAddr,
    ) -> Result<Self> {
        let mut child = launch_qemu(kernel, config)
            .await
            .context("failed to launch qemu")?;
        let mut tcp = TcpStream::connect(socket_addr)
            .await
            .context("failed to establish tcp connection to qemu instance")?;

        select! {
            res = handshake(&mut tcp) => {
                res.context("failed to perform test handshake")?;
            }
            qemu_exit = child.wait() => {
                let exit_status = qemu_exit.context("failed to wait on qemu")?;
                bail!("Qemu unexpectedly exited wiht {:?}", exit_status);
            }
        }

        Ok(QemuInstance { child, tcp })
    }

    async fn wait(&mut self) -> Result<ExitStatus> {
        self.child
            .wait()
            .await
            .context("waiting on qemu to finish failed")
    }

    fn handle_qemu_exit(qemu_exit: Result<ExitStatus, std::io::Error>) -> Result<Never, QemuError> {
        match qemu_exit {
            Ok(status) => Err(QemuError::Exit(status)),
            Err(_) => Err(QemuError::WaitError),
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
            qemu_exit = self.child.wait() => {
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
            qemu_exit = self.child.wait() => {
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
                match timeout(QEMU_EOF_WAIT_TIMEOUT, self.child.wait()).await {
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

/// performs test handshake with the test kernel
///
/// recieves and dropps incoming data until "test ready\n" is recieved
/// and echos back the same message.
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

/// recieves a single line from qemu
async fn recieve_line(qemu: &mut QemuInstance) -> Result<String, QemuError> {
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

async fn parse_line<T>(qemu: &mut QemuInstance) -> Result<T, QemuError>
where
    T: FromStr,
    T::Err: Debug + std::error::Error + Send + Sync + 'static,
{
    Ok(recieve_line(qemu)
        .await
        .qcontext("failed to receive line")?
        .parse()
        .context("failed to parse line")?)
}

async fn get_test_count(qemu: &mut QemuInstance, panicing: bool) -> Result<usize, QemuError> {
    let cmd: &[u8] = if panicing {
        b"count panic\n".as_ref()
    } else {
        b"count\n".as_ref()
    };
    qemu.write_all(cmd)
        .await
        .qcontext("failed to send \"count\" command")?;

    parse_line(qemu)
        .await
        .qcontext("failed to recieve test count")
}

async fn tests_no_tcp(uefi: &Path, args: TestArgs) -> Result<()> {
    let kernel = Kernel {
        path: &uefi,
        uefi: true,
    };

    let qemu = QemuConfig {
        devices: "isa-debug-exit,iobase=0xf4,iosize=0x04",
        ..QemuConfig::default()
    };

    let exit_status = launch_with_timeout(Duration::from_secs(args.timeout), &kernel, &qemu)
        .await
        .context("test kernel")?;

    validate_qemu_run(exit_status)
}

fn validate_qemu_run(exit_status: ExitStatus) -> Result<()> {
    if is_successful_exit(exit_status)? {
        debug!("Qemu instance finished successfully");
    } else {
        bail!("Tests failed");
    }
    Ok(())
}

fn is_successful_exit(exit_status: ExitStatus) -> Result<bool> {
    if exit_status.success() {
        bail!(
            "Qemu exit with code 0, but we expected {}",
            QEMU_EXIT_SUCCESS_CODE
        );
    } else {
        return match exit_status.code() {
            Some(QEMU_EXIT_SUCCESS_CODE) => Ok(true),
            Some(QEMU_EXIT_FAILURE_CODE) => Ok(false),
            Some(code) => bail!("Qemu exited with unexpected exit code 0x{:x}", code),
            None => bail!("Qemu did not succeed, but has no error code????"),
        };
    }
}
