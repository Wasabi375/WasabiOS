use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail, ensure};
use log::info;
use tokio::process::Command;

use crate::{
    args::TestArgs,
    config::{
        VerifiedConfig,
        file_system::ImageId,
        tests::{Test, TestGroup, TestId, TestSystem},
    },
    sync::FutureCache,
};

static TEST_CACHE: FutureCache<TestId, Result<()>> = FutureCache::const_new();

pub async fn test_with_args(args: TestArgs, config: VerifiedConfig) -> Result<()> {
    let config = Arc::new(config);
    let targets = if args.targets.is_empty() {
        &config.config.default_tests
    } else {
        &args.targets
    };

    test_targets(targets.iter().cloned(), config.clone()).await
}

pub async fn test_targets<T: IntoIterator<Item = TestId>>(
    targets: T,
    config: Arc<VerifiedConfig>,
) -> Result<()> {
    for target in targets {
        match config
            .tests
            .get(&target)
            .ok_or(anyhow!("Could not find test with id: {target}"))?
        {
            Test::System(system) => test_system(system.clone(), config.clone()).await?,
            Test::Group(group) => test_group(&*group, config.clone())
                .await
                .with_context(|| format!("test group: {}", group.id))?,
        }
    }
    Ok(())
}

pub async fn test_group(group: &TestGroup, config: Arc<VerifiedConfig>) -> Result<()> {
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
        test_system(Arc::new(test), config.clone()).await?;
    }
    Ok(())
}

pub async fn test_system(system: Arc<TestSystem>, config: Arc<VerifiedConfig>) -> Result<()> {
    let test_id = system.id.clone();
    let test_future = async move {
        if let Some(cargo) = system.cargo.as_ref() {
            test_cargo(cargo, system.id.clone(), &config)
                .await
                .with_context(|| format!("cargo test {}", system.id))?;
        }
        if let Some(kernel) = system.kernel.clone() {
            test_kernel(kernel, &config)
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
        bail!("test system {test_id}:\n{err:#}");
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

async fn test_kernel(kernel: ImageId, config: &VerifiedConfig) -> Result<()> {
    log::warn!("not implemented");
    Ok(())
}
