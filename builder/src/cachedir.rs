//! Mark directories as cachedirs
//!
//! specification at <https://bford.info/cachedir/>

use anyhow::{Context, Result};

use std::{
    fs::File,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

pub const CACHEDIR_FILE: &str = "CACHEDIR.TAG";
pub const CACHEDIR_IDENTITY: &str = "Signature: 8a477f597d28d172789f06886806bc55";
pub const CACHEDIR_INFO: &str =
    "# This file is a cache directory created by the wasabi-os build system.
# For information about cache directory tags see https://bford.info/cachedir/";

pub fn is_cachedir(path: &Path) -> Result<bool> {
    assert!(path.is_dir());

    let cachedir = path.join(CACHEDIR_FILE);
    is_cachedir_file(&cachedir)
}

pub fn is_cachedir_file(cachedir: &Path) -> Result<bool> {
    if !cachedir.exists() {
        return Ok(false);
    }

    let file = File::open(cachedir)
        .context("failed to open CACHEDIR.TAG file even though it should exist")?;
    let mut reader = BufReader::new(file);

    let mut buf = String::new();
    if reader.read_line(&mut buf).is_err() {
        return Ok(false);
    }

    Ok(buf.trim_end() == CACHEDIR_IDENTITY)
}

// NOTE: this is manually desugaring async fn to force implement Send. This is a known compiler
// bug: https://github.com/rust-lang/rust/issues/123072
pub fn mark_dir_cachedir(dir: PathBuf, recurse: bool) -> impl Future<Output = Result<()>> + Send {
    async move {
        if !is_cachedir(&dir)? {
            create_cachedir_file(&dir).await?;
        }

        if recurse {
            let handles: Vec<_> = dir
                .read_dir()
                .context("failed to read directory")?
                .into_iter()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().unwrap().is_dir())
                .map(|dir| {
                    tokio::spawn(async move {
                        let dir = dir.path();
                        mark_dir_cachedir(dir, recurse).await
                    })
                })
                .collect();

            for job in handles {
                job.await.context("failed mark_dir_cachedir task join")??
            }
        }

        Ok(())
    }
}

async fn create_cachedir_file(dir: &Path) -> Result<()> {
    let tag_file = dir.join(CACHEDIR_FILE);
    let mut tag_file = File::create(tag_file).context("failed to create cachedir tag file")?;
    writeln!(tag_file, "{}", CACHEDIR_IDENTITY).context("failed to write cachdir identity")?;
    writeln!(tag_file, "{}", CACHEDIR_INFO).context("failed to write cachdir identity")?;
    Ok(())
}
