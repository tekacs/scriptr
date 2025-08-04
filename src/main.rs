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

    /// Path to the Rust script (`.rs`)
    script: PathBuf,

    /// Arguments passed to the script binary (after `--`)
    #[arg(trailing_var_arg = true)]
    args: Vec<OsString>,
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
    let Opts {
        debug,
        verbose,
        force,
        clean,
        clean_only,
        script,
        args,
    } = Opts::parse();

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
    if !force {
        if let Ok(meta) = read_meta(&meta_path) {
            let cur_mtime = mtime_secs(&script)?;
            if verbose {
                eprintln!("[scriptr] Cached mtime: {}, current mtime: {}", meta.fp.mtime, cur_mtime);
            }
            
            if meta.fp.mtime == cur_mtime {
                // unchanged → run cached binary
                if verbose {
                    eprintln!("[scriptr] mtime unchanged, using cached binary: {}", meta.bin.display());
                }
                exec(meta.bin, args);
            }
            
            // mtime differs – compute hash only now
            if verbose {
                eprintln!("[scriptr] mtime changed, checking hash...");
            }
            let cur_hash = file_hash(&script)?;
            if verbose {
                eprintln!("[scriptr] Cached hash: {}, current hash: {}", &meta.fp.hash[..16], &cur_hash[..16]);
            }
            
            if meta.fp.hash == cur_hash && meta.bin.exists() {
                if verbose {
                    eprintln!("[scriptr] Hash unchanged, using cached binary: {}", meta.bin.display());
                }
                exec(meta.bin, args);
            }
        } else if verbose {
            eprintln!("[scriptr] No cache found");
        }
    } else if verbose {
        eprintln!("[scriptr] Force rebuild requested");
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
    exec(bin_path, args)
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
        .stderr(if verbose { Stdio::inherit() } else { Stdio::null() })
        .spawn()
        .context("failed to spawn cargo")?;
    let stdout = child.stdout.take().expect("piped");

    // Parse the JSON stream to find the executable path.
    let reader = BufReader::new(stdout);
    let mut bin_path = None::<PathBuf>;
    for line in reader.lines() {
        let line = line?;
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            if val["reason"] == "compiler-artifact" && val["executable"].is_string() {
                bin_path = Some(PathBuf::from(val["executable"].as_str().unwrap()));
            }
        }
    }
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("cargo build failed with status {}", status);
    }
    bin_path.ok_or_else(|| anyhow::anyhow!("no executable produced"))
}

/// Replace the current process image with `bin`, passing through `args`.
fn exec(bin: PathBuf, args: Vec<OsString>) -> ! {
    // SAFETY: exec only returns on error.
    let err = Command::new(bin).args(args).exec();
    panic!("exec failed: {err:?}");
}
