use std::fs;
use std::path::Path;
use std::process::Command;
use std::{env, path::PathBuf};
use walkdir::{DirEntry, WalkDir};

fn main() {
    println!("crago:rerun-if-changed=build.rs");
    build_assembly();
}

fn build_assembly() {
    let base_path = env::var("CARGO_MANIFEST_DIR").unwrap();
    let base_path = &Path::new(base_path.as_str())
        .canonicalize()
        .expect("Failed to cannoicalize");

    let out_path = env::var("OUT_DIR").unwrap();
    let out_path = &Path::new(out_path.as_str())
        .canonicalize()
        .expect("Failed to cannonicalize");

    let asm_files = WalkDir::new(base_path)
        .same_file_system(true)
        .follow_links(true)
        .into_iter()
        .filter_map(|f| f.ok())
        .filter(|f| is_asm_file(f));

    for file in asm_files {
        build_assembly_file(file.path(), base_path, out_path);
    }
}

fn build_assembly_file(file: &Path, base_dir: &PathBuf, out_dir: &PathBuf) {
    println!("Processing source file {}", file.to_str().unwrap());
    let relative = file
        .strip_prefix(base_dir)
        .expect("file must be relative to base");

    println!("cargo:rerun-if-changed={}", relative.to_str().unwrap());

    let output_relative = relative.with_extension("bin");
    let output_file = format!(
        "{}/{}",
        out_dir.to_str().unwrap(),
        output_relative.to_str().unwrap()
    );
    let output_file = PathBuf::from(&output_file);

    if let Some(parent) = output_file.parent() {
        if fs::create_dir_all(parent).is_err() {
            panic!("Failed to crate parent dir for {:?}", output_file);
        }
    }

    let status = Command::new("nasm")
        .arg("-f")
        .arg("bin")
        .arg("-o")
        .arg(output_file)
        .arg(file)
        .status()
        .expect("failed to run nasm");

    if !status.success() {
        panic!("nasm failed to assemble \"{}\"", file.to_str().unwrap());
    }
}

fn is_asm_file(file: &DirEntry) -> bool {
    if !file.file_type().is_file() {
        return false;
    }

    let Some(ext) = file.path().extension().map(|e| e.to_str()).flatten() else {
        return false;
    };

    match ext {
        "asm" | "nasm" => true,
        _ => false,
    }
}
