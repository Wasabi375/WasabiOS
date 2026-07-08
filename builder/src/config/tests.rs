use std::sync::Arc;

use congen::Configuration;
use serde::{Deserialize, Serialize};

use crate::{args::TestArgs, config::file_system::ImageId, config_id_type};

config_id_type!(TestId, TestIdT);

#[derive(Debug, Clone)]
pub enum Test {
    System(Arc<TestSystem>),
    Group(Arc<TestGroup>),
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct TestSystem {
    pub id: TestId,

    /// test the module using "cargo test --package <package-name>"
    pub cargo: Option<String>,

    /// assume the kernel is using "wasabi-test" as the base kernel
    /// and run tests that way
    pub kernel: Option<ImageId>,

    /// timeout for kernelt tests.
    ///
    /// Ignored for cargo
    #[congen(default = 60)]
    pub timeout_secs: u64,

    /// run each test in it's own instance of qemu
    ///
    /// This is can be usefull to isolate global state change introduced by one test
    /// that might cause other tests to fail, but will drastically increase the time
    /// it takes to finish testing, because qemu combined with the bootloader and kernel
    /// startup add up to a not insignificant amount of time.
    ///
    /// Ignored for cargo
    #[congen(default = false)]
    pub isolated: bool,

    /// continue testing even if a test panics, by restarting qemu or passed as a cargo test
    /// flag(no-fail-fast)
    #[congen(default = false)]
    pub keep_going: bool,

    /// Ignored for cargo
    #[congen(default = false)]
    pub fast: bool,
}

impl TestSystem {
    pub fn isolated(&self, global: &TestArgs) -> bool {
        global.isolated.unwrap_or(self.isolated)
    }
    pub fn keep_going(&self, global: &TestArgs) -> bool {
        global.keep_going.unwrap_or(self.keep_going)
    }
    pub fn fast(&self, global: &TestArgs) -> bool {
        global.fast.unwrap_or(self.fast)
    }
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct TestGroup {
    pub id: TestId,

    /// if empty will be autofilled with all TestSystems that match [Self::cargo_only] and
    /// [Self::kernel_only]. If both are false or both are true, every test is run
    #[congen(rust_default)]
    pub systems: Vec<TestId>,

    #[congen(default = false)]
    pub cargo_only: bool,

    #[congen(default = false)]
    pub kernel_only: bool,
}

impl TestGroup {
    pub fn all(&self) -> bool {
        let tests = [self.cargo_only, self.kernel_only];
        tests.iter().all(|t| *t) || tests.iter().all(|t| !t)
    }
}
