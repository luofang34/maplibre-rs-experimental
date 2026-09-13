//! WGSL validation with relative include expansion.
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationError, ValidationFlags, Validator},
};
use std::{
    collections::HashSet,
    env,
    io::{self, Write},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// A shader build failure with its path and original cause.
#[derive(Debug, thiserror::Error)]
pub enum WgslError {
    /// An included file could not be read or expanded.
    #[error("loading shader {}: {source}", path.display())]
    Io {
        /// Shader file path.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: io::Error,
    },
    /// WGSL syntax is invalid.
    #[error("parsing shader {}: {source}", path.display())]
    Parse {
        /// Shader entry path.
        path: PathBuf,
        /// Parser diagnostics, including source spans.
        #[source]
        source: wgsl::ParseError,
    },
    /// A parsed shader violates WGSL validation rules.
    #[error("validating shader {}: {source}", path.display())]
    Validation {
        /// Shader entry path.
        path: PathBuf,
        /// Validation diagnostics, including source spans.
        #[source]
        source: Box<naga::WithSpan<ValidationError>>,
    },
    /// Cargo did not supply a usable manifest directory.
    #[error("reading CARGO_MANIFEST_DIR: {0}")]
    Manifest(#[from] env::VarError),
    /// A directory entry could not be inspected.
    #[error("walking shader sources: {0}")]
    Walk(#[from] walkdir::Error),
    /// Cargo build instructions could not be written.
    #[error("writing cargo shader instructions: {0}")]
    Output(#[source] io::Error),
}

fn validate_wgsl(validator: &mut Validator, path: &Path) -> Result<(), WgslError> {
    let shader = load_wgsl(path, &mut HashSet::new()).map_err(|source| WgslError::Io {
        path: path.into(),
        source,
    })?;
    let module = wgsl::parse_str(&shader).map_err(|source| WgslError::Parse {
        path: path.into(),
        source,
    })?;
    validator
        .validate(&module)
        .map_err(|source| WgslError::Validation {
            path: path.into(),
            source: Box::new(source),
        })?;
    Ok(())
}

fn load_wgsl(path: &Path, active: &mut HashSet<PathBuf>) -> io::Result<String> {
    let path = path.to_path_buf();
    if !active.insert(path.clone()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cyclic WGSL include at {}", path.display()),
        ));
    }

    let source = std::fs::read_to_string(&path)?;
    let mut expanded = String::new();
    for line in source.lines() {
        if let Some(relative_path) = line.strip_prefix("// @include ") {
            let include_path = path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(relative_path.trim());
            expanded.push_str(&load_wgsl(&include_path, active)?);
        } else {
            expanded.push_str(line);
            expanded.push('\n');
        }
    }
    active.remove(&path);
    Ok(expanded)
}

/// Validates shaders below Cargo's manifest directory and emits rebuild instructions.
///
/// Errors retain shader paths and parser, validation, or filesystem diagnostics.
pub fn validate_project_wgsl_blocking() -> Result<(), WgslError> {
    let root = env::var("CARGO_MANIFEST_DIR")?;
    validate_directory_blocking(Path::new(&root), &mut io::stdout().lock())
}

fn validate_directory_blocking(root: &Path, output: &mut impl Write) -> Result<(), WgslError> {
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    for entry in WalkDir::new(root) {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type().is_file() && path.extension().is_some_and(|ext| ext == "wgsl") {
            writeln!(output, "cargo:rerun-if-changed={}", path.display())
                .map_err(WgslError::Output)?;
            validate_wgsl(&mut validator, path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
