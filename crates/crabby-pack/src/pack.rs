//! Pack-emission orchestrator.
//!
//! [`emit_pack`] writes a ZIP archive holding the 3-entry recipe for every
//! [`RewrittenScript`], plus the VFS canary. The archive is atomically
//! written via a `.tmp` file + rename so a crash mid-write can't leave
//! Godot holding a truncated pack.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};
use tracing::{debug, info};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::canary::{CANARY_ENTRY_NAME, canary_content};
use crate::entry::RewrittenScript;

/// Inputs to [`emit_pack`].
#[derive(Debug)]
pub struct PackInputs<'a> {
    /// Scripts to pack, each producing the full 3-entry recipe.
    pub rewritten_scripts: &'a [RewrittenScript],
    /// Destination file. Parent directory is created if missing.
    pub out_path: &'a Path,
    /// Version string embedded in the canary payload. Usually
    /// `env!("CARGO_PKG_VERSION")` of the crate that owns the bake.
    pub version: &'a str,
}

/// Outputs from [`emit_pack`].
#[derive(Debug, Clone)]
pub struct PackOutputs {
    /// Absolute path of the emitted archive.
    pub zip_path: PathBuf,
    /// Number of scripts packed (each contributes 3 ZIP entries).
    pub script_entry_count: usize,
    /// The canary payload written into the archive; the runtime shim reads
    /// this back via `FileAccess` to verify VFS precedence.
    pub canary_content: String,
}

/// Emit a hook pack containing every rewritten script plus the VFS canary.
pub fn emit_pack(inputs: &PackInputs<'_>) -> Result<PackOutputs> {
    validate_inputs(inputs)?;

    if let Some(parent) = inputs.out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|source| CrabbyError::io_at(parent.to_path_buf(), source))?;
    }

    // Atomic write: build the zip at <out>.tmp, then rename over the target.
    // Mirrors vostok's override.cfg write pattern. On Windows rename won't
    // overwrite an existing target, so the destination is removed first.
    let tmp_path = temp_path_for(inputs.out_path);
    let canary = canary_content(inputs.version);

    write_zip(inputs, &tmp_path, &canary)?;

    if inputs.out_path.exists() {
        fs::remove_file(inputs.out_path)
            .map_err(|source| CrabbyError::io_at(inputs.out_path.to_path_buf(), source))?;
    }
    fs::rename(&tmp_path, inputs.out_path).map_err(|source| CrabbyError::Pack {
        context: format!(
            "renaming {} → {}",
            tmp_path.display(),
            inputs.out_path.display(),
        ),
        source: Box::new(source),
    })?;

    info!(
        zip = %inputs.out_path.display(),
        scripts = inputs.rewritten_scripts.len(),
        "emitted hook pack",
    );

    Ok(PackOutputs {
        zip_path: inputs.out_path.to_path_buf(),
        script_entry_count: inputs.rewritten_scripts.len(),
        canary_content: canary,
    })
}

fn validate_inputs(inputs: &PackInputs<'_>) -> Result<()> {
    if inputs.version.is_empty() {
        return Err(CrabbyError::Pack {
            context: "version must not be empty".into(),
            source: "canary payload requires a version suffix".into(),
        });
    }
    for r in inputs.rewritten_scripts {
        r.validate()?;
    }
    Ok(())
}

fn temp_path_for(target: &Path) -> PathBuf {
    // Append `.tmp` to the file name. `with_extension` would clobber the
    // original `.zip` extension, which isn't what we want.
    let mut name = target
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".tmp");
    target.with_file_name(name)
}

fn write_zip(inputs: &PackInputs<'_>, tmp_path: &Path, canary: &str) -> Result<()> {
    let file = File::create(tmp_path)
        .map_err(|source| CrabbyError::io_at(tmp_path.to_path_buf(), source))?;
    let mut zw = ZipWriter::new(BufWriter::new(file));
    // Deflate gives a modest size win without the decompression surface
    // area of zstd inside the ZIP. Godot's pack loader handles both.
    let opts: SimpleFileOptions =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // Canary first so it always lands in the archive even if a script fails.
    write_entry(&mut zw, CANARY_ENTRY_NAME, canary.as_bytes(), opts)?;

    for script in inputs.rewritten_scripts {
        debug!(entry = %script.zip_path, emit_gdc = script.emit_empty_gdc, "packing script");
        // Always: `.gd` (rewritten source) + `.gd.remap` (self-redirect
        // so Godot's `.gd → .gdc` resolver finds the rewritten source
        // rather than vanilla's bytecode).
        write_entry(
            &mut zw,
            &script.zip_path,
            script.rewritten_source.as_bytes(),
            opts,
        )?;
        write_entry(
            &mut zw,
            &script.remap_entry(),
            script.remap_contents().as_bytes(),
            opts,
        )?;
        // Conditionally: empty `.gdc` companion. Required for Node-typed
        // scripts so vanilla PCK bytecode doesn't pre-empt the rewritten
        // source. Forbidden for Resource-typed (additive) scripts because
        // the empty bytecode confuses Godot's resource-script class binder;
        // see `RewrittenScript::emit_empty_gdc` for the full story.
        if script.emit_empty_gdc {
            write_entry(&mut zw, &script.gdc_entry(), &[], opts)?;
        }
    }

    let mut inner = zw.finish().map_err(|source| CrabbyError::Pack {
        context: "finalizing zip archive".into(),
        source: Box::new(source),
    })?;
    inner.flush().map_err(|source| CrabbyError::Pack {
        context: "flushing zip writer".into(),
        source: Box::new(source),
    })?;
    Ok(())
}

fn write_entry<W: std::io::Write + std::io::Seek>(
    zw: &mut ZipWriter<W>,
    name: &str,
    bytes: &[u8],
    opts: SimpleFileOptions,
) -> Result<()> {
    zw.start_file(name, opts)
        .map_err(|source| CrabbyError::Pack {
            context: format!("start_file({name:?})"),
            source: Box::new(source),
        })?;
    zw.write_all(bytes).map_err(|source| CrabbyError::Pack {
        context: format!("writing bytes for {name:?}"),
        source: Box::new(source),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
