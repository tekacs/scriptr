//! scriptr ‑ fast launcher for Rust single‑file packages (`cargo -Zscript`)
#![forbid(unsafe_code)]
#![feature(file_lock)]

use anyhow::{Context, Result};
use blake3::Hasher;
use clap::Parser;
use dirs::cache_dir;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

const NAME: &str = "scriptr";

/// Fast launcher for Rust single-file packages
#[derive(Parser)]
#[command(about, version)]
struct Opts {
    /// Build in debug mode (default is release)
    #[arg(short = 'd', long)]
    debug: bool,

    /// Verbose output
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Force rebuild (ignore cache)
    #[arg(short = 'f', long)]
    force: bool,

    /// Clean cache before building
    #[arg(short = 'c', long)]
    clean: bool,

    /// Clean cache and exit (don't run)
    #[arg(short = 'C', long)]
    clean_only: bool,

    /// Use only hash for comparison (ignore mtime)
    #[arg(short = 'H', long)]
    hash_only: bool,

    /// Path to the Rust script (`.rs`)
    script: PathBuf,
}

/// Persisted fingerprint of a script file.
#[derive(Serialize, Deserialize, Debug)]
struct Fingerprint {
    mtime: u64,
    hash: String, // BLAKE3 hex
}

/// Metadata stored between runs.
#[derive(Serialize, Deserialize, Debug)]
struct Meta {
    fp: Fingerprint,
    bin: PathBuf,
}

fn main() -> Result<()> {
    // Manual argument parsing to prevent script args from being interpreted as scriptr options
    let all_args: Vec<String> = std::env::args().collect();
    let mut script_index = None;
    
    // Find the script path (first non-option argument after scriptr options)
    for (i, arg) in all_args.iter().enumerate().skip(1) {
        if arg == "--" {
            // Everything after "--" is for the script
            if i + 1 < all_args.len() {
                script_index = Some(i + 1);
            }
            break;
        } else if !arg.starts_with('-') {
            // Found the script path
            script_index = Some(i);
            break;
        }
    }
    
    // Split args at the script boundary
    let (scriptr_args, passthrough_args) = if let Some(idx) = script_index {
        // Give clap everything up to and including the script path
        let scriptr_args = all_args[..=idx].to_vec();
        // Everything after the script is passthrough
        let passthrough_args: Vec<OsString> = all_args[(idx + 1)..]
            .iter()
            .map(|s| OsString::from(s))
            .collect();
        (scriptr_args, passthrough_args)
    } else {
        // No script found - let clap handle it (probably --help or error)
        (all_args, vec![])
    };
    
    // Parse only scriptr's portion
    let Opts {
        debug,
        verbose,
        force,
        clean,
        clean_only,
        hash_only,
        script,
    } = Opts::parse_from(scriptr_args);

    let script =
        fs::canonicalize(&script).with_context(|| format!("cannot resolve path {script:?}"))?;

    if verbose {
        eprintln!("[scriptr] Script: {}", script.display());
    }

    // -------------- cache bookkeeping ---------------------------------------
    let cache_root = cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(NAME);
    fs::create_dir_all(&cache_root)?;

    // Key the metadata file by absolute path (not by contents).
    let mut hasher = Hasher::new();
    hasher.update(script.as_os_str().as_encoded_bytes());
    let path_key = hasher.finalize().to_hex();
    let meta_path = cache_root.join(format!("{path_key}.json"));

    if verbose {
        eprintln!("[scriptr] Cache path: {}", meta_path.display());
    }

    // -------------- handle clean flags --------------------------------------
    if clean || clean_only {
        if meta_path.exists() {
            if verbose {
                eprintln!("[scriptr] Removing cache: {}", meta_path.display());
            }
            fs::remove_file(&meta_path)?;
        } else if verbose {
            eprintln!("[scriptr] No cache to clean");
        }
        
        if clean_only {
            if verbose {
                eprintln!("[scriptr] Clean complete, exiting");
            }
            return Ok(());
        }
    }

    // -------------- fast‑path check -----------------------------------------
    match (force, read_meta(&meta_path)) {
        (true, _) => {
            if verbose {
                eprintln!("[scriptr] Force rebuild requested");
            }
        }
        (false, Err(_)) => {
            if verbose {
                eprintln!("[scriptr] No cache found");
            }
        }
        (false, Ok(meta)) => {
            // Check mtime first (unless in hash-only mode)
            let mtime_changed = if hash_only {
                true  // Always check hash in hash-only mode
            } else {
                let cur_mtime = mtime_secs(&script)?;
                if verbose {
                    eprintln!("[scriptr] Cached mtime: {}, current mtime: {}", meta.fp.mtime, cur_mtime);
                }
                meta.fp.mtime != cur_mtime
            };
            
            if !mtime_changed && meta.bin.exists() {
                if verbose {
                    eprintln!("[scriptr] mtime unchanged, using cached binary: {}", meta.bin.display());
                }
                exec(meta.bin, passthrough_args.clone());
            }
            
            // Need to check hash
            if verbose {
                if hash_only {
                    eprintln!("[scriptr] Hash-only mode, checking hash...");
                } else {
                    eprintln!("[scriptr] mtime changed, checking hash...");
                }
            }
            
            let cur_hash = file_hash(&script)?;
            if verbose {
                eprintln!("[scriptr] Cached hash: {}, current hash: {}", &meta.fp.hash[..16], &cur_hash[..16]);
            }
            
            if meta.fp.hash == cur_hash && meta.bin.exists() {
                if verbose {
                    eprintln!("[scriptr] Hash unchanged, using cached binary: {}", meta.bin.display());
                }
                exec(meta.bin, passthrough_args.clone());
            }
        }
    }

    // -------------- rebuild -------------------------------------------------
    if verbose {
        eprintln!("[scriptr] Building script...");
    }
    let bin_path = rebuild(&script, !debug, verbose)?;
    let fp = Fingerprint {
        mtime: mtime_secs(&script)?,
        hash: file_hash(&script)?,
    };
    
    if verbose {
        eprintln!("[scriptr] Writing cache metadata");
    }
    write_meta(
        &meta_path,
        &Meta {
            fp,
            bin: bin_path.clone(),
        },
    )?;

    if verbose {
        eprintln!("[scriptr] Executing: {}", bin_path.display());
    }
    exec(bin_path, passthrough_args)
}

/* ------------------------------------------------------------------------- */

fn mtime_secs(p: &Path) -> Result<u64> {
    Ok(fs::metadata(p)?
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_secs())
}

fn file_hash(p: &Path) -> Result<String> {
    let mut file = File::open(p)?;
    let mut buf = [0u8; 64 * 1024];
    let mut hasher = Hasher::new();
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn read_meta(p: &Path) -> Result<Meta> {
    let f = File::open(p)?;
    Ok(serde_json::from_reader(f)?)
}

fn write_meta(p: &Path, meta: &Meta) -> Result<()> {
    // lock the file to avoid races
    let tmp = p.with_extension("json.new");
    {
        let f = File::create(&tmp)?;
        f.lock_exclusive()?;
        serde_json::to_writer(&f, meta)?;
        f.unlock()?;
    }
    fs::rename(tmp, p)?;
    Ok(())
}

/// Build the script via Cargo, returning the path to the resulting binary.
fn rebuild(script: &Path, release: bool, verbose: bool) -> Result<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "+nightly",
        "-Zscript",
        "build",
        "--manifest-path",
        script.to_str().unwrap(),
        "--message-format=json",
    ]);
    if !verbose {
        cmd.arg("--quiet");
    }
    if release {
        cmd.arg("--release");
    }

    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn cargo")?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    // Parse the JSON stream to find the executable path and collect errors.
    let reader = BufReader::new(stdout);
    let mut bin_path = None::<PathBuf>;
    let mut error_messages = Vec::new();
    
    for line in reader.lines() {
        let line = line?;
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            match val["reason"].as_str() {
                Some("compiler-artifact") if val["executable"].is_string() => {
                    bin_path = Some(PathBuf::from(val["executable"].as_str().unwrap()));
                }
                Some("compiler-message") => {
                    if let Some(message) = val["message"]["rendered"].as_str() {
                        error_messages.push(message.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    
    // Collect stderr in case of failure
    let mut stderr_output = String::new();
    let mut stderr_reader = BufReader::new(stderr);
    stderr_reader.read_to_string(&mut stderr_output)?;
    
    let status = child.wait()?;
    if !status.success() {
        // Print compilation errors from JSON output
        for error in &error_messages {
            eprint!("{}", error);
        }
        // Also print any stderr output
        if !stderr_output.is_empty() {
            eprintln!("{}", stderr_output);
        }
        anyhow::bail!("cargo build failed with status {}", status);
    }
    
    // Print stderr output in verbose mode even on success
    if verbose && !stderr_output.is_empty() {
        eprintln!("{}", stderr_output);
    }
    bin_path.ok_or_else(|| anyhow::anyhow!("no executable produced"))
}

/// Replace the current process image with `bin`, passing through `args`.
fn exec(bin: PathBuf, args: Vec<OsString>) -> ! {
    // SAFETY: exec only returns on error.
    let err = Command::new(bin).args(args).exec();
    panic!("exec failed: {err:?}");
}
