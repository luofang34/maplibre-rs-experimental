//! Build-time WGSL comment & whitespace minifier writing shaders into `$OUT_DIR/wgsl/`.

use std::{
    env, fs, io,
    io::Write as _,
    path::{Path, PathBuf},
};

type BuildResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn minify_wgsl(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for raw_line in source.lines() {
        let trimmed = raw_line
            .split_once("//")
            .map_or(raw_line, |(head, _)| head)
            .trim();
        if !trimmed.is_empty() {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

fn collect_wgsl_files(dir: &Path, out: &mut Vec<PathBuf>) -> BuildResult {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_wgsl_files(&path, out)?;
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.ends_with(".wgsl") || name.ends_with(".wgsl.part") {
            out.push(path);
        }
    }
    Ok(())
}

pub fn generate_minified_wgsl_blocking() -> BuildResult {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let src_dir = manifest_dir.join("src");
    let out_root = PathBuf::from(env::var("OUT_DIR")?).join("wgsl");
    let mut files = Vec::new();
    collect_wgsl_files(&src_dir, &mut files)?;
    files.sort();
    let mut stdout = io::stdout().lock();
    for path in files {
        writeln!(stdout, "cargo:rerun-if-changed={}", path.display())?;
        let rel = path.strip_prefix(&src_dir)?;
        let dst = out_root.join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        let source = fs::read_to_string(&path)?;
        fs::write(&dst, minify_wgsl(&source))?;
    }
    Ok(())
}
