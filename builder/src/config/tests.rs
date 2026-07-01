use std::sync::Arc;

use congen::Configuration;
use serde::{Deserialize, Serialize};

use crate::{config::file_system::ImageId, config_id_type};

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
