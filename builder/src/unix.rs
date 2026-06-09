use std::{io::Write, os::fd::AsFd};

use anyhow::{Context, Result};
use nix::sys::termios::{SetArg, Termios, tcgetattr, tcsetattr};

/// Takes a snapshot of the tty settings and restores them on drop.
///
/// For some reasons QEMU sometimes mangles the tty. This used to be
/// pretty common during tests.
/// This makes sure the tty is useable after qemu is finished
pub struct RestoreTty {
    stdout: Termios,
    stderr: Termios,
}

impl RestoreTty {
    pub fn new() -> Result<Self> {
        let stdout =
            tcgetattr(std::io::stdout().as_fd()).context("Failed to read Termios for stdout")?;
        let stderr =
            tcgetattr(std::io::stderr().as_fd()).context("Failed to read Termios for stderr")?;

        Ok(Self { stdout, stderr })
    }
}

impl Drop for RestoreTty {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        let _ = stdout.flush();
        let _ = stderr.flush();
        let res_1 = tcsetattr(stdout.as_fd(), SetArg::TCSANOW, &self.stdout)
            .context("Failed to restore Termios for stdout");
        let res_2 = tcsetattr(stderr.as_fd(), SetArg::TCSANOW, &self.stderr)
            .context("Failed to restore Termios for stderr");

        match (res_1.err(), res_2.err()) {
            (Some(e1), Some(e2)) => panic!("Failed to restore Termios:\n{e1}\n{e2}"),
            (Some(e), None) | (None, Some(e)) => panic!("Failed to restore Termios:\n{e}"),
            _ => {}
        }
    }
}
