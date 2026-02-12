//! scriptr ‑ fast launcher for Rust single‑file packages (`cargo -Zscript`)
#![forbid(unsafe_code)]

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
#[command(
    version,
    about = "Fast launcher for Rust single-file packages",
    long_about = "Run Rust single-file packages (cargo -Zscript) with millisecond cold starts by caching builds intelligently.",
    after_help = r#"USAGE AS SHEBANG
  Put scriptr in your script's shebang and declare Cargo front-matter at the top:

    #!/usr/bin/env scriptr
    ---
    [dependencies]
    clap = { version = "4.5", features = ["derive"] }
    anyhow = "1.0"
    ---
    // normal Rust code follows

  Notes
  - The front-matter is standard Cargo.toml inlined between lines with just "---".
    You can use typical sections like [package], [dependencies], [features], etc.
  - File extension is optional. "hello" or "hello.rs" both work.
  - When invoking scriptr directly, pass your script args after the path or after "--":
      scriptr ./hello.rs -- arg1 arg2
  - To pass flags in the shebang, use env -S (portable across macOS/Linux):
      #!/usr/bin/env -S scriptr --debug
  - To key cache by a stable global identifier instead of script path:
      #!/usr/bin/env -S scriptr --id=123e4567-e89b-12d3-a456-426614174000

EXAMPLES
  Minimal script:

    #!/usr/bin/env scriptr
    ---
    [dependencies]
    clap = { version = "4.5", features = ["derive"] }
    ---

    use clap::Parser;

    #[derive(Parser)]
    struct Args { name: String }

    fn main() {
        let args = Args::parse();
        println!("Hello, {}!", args.name);
    }

  Run it:
    chmod +x ./hello.rs
    ./hello.rs World
"#
)]
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

    /// Globally unique script identifier (uses this instead of script path for cache keying)
    #[arg(long, value_name = "ID")]
    id: Option<String>,

    /// Path to the Rust script (extension optional)
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
    let (script_index, passthrough_args) = split_invocation_args(&all_args);

    // Split args at the script boundary
    let scriptr_args = if let Some(idx) = script_index {
        // Give clap everything up to and including the script path
        all_args[..=idx].to_vec()
    } else {
        // No script found - let clap handle it (probably --help or error)
        all_args
    };

    // Parse only scriptr's portion
    let Opts {
        debug,
        verbose,
        force,
        clean,
        clean_only,
        hash_only,
        id,
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

    // Key metadata either by explicit ID or by absolute path.
    let mut hasher = Hasher::new();
    if let Some(ref id) = id {
        hasher.update(b"id:");
        hasher.update(id.as_bytes());
    } else {
        hasher.update(b"path:");
        hasher.update(script.as_os_str().as_encoded_bytes());
    }
    let cache_key = hasher.finalize().to_hex();
    let meta_path = cache_root.join(format!("{cache_key}.json"));

    if verbose {
        if let Some(ref id) = id {
            eprintln!("[scriptr] Cache key source: id={id}");
        } else {
            eprintln!("[scriptr] Cache key source: path");
        }
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
                true // Always check hash in hash-only mode
            } else {
                let cur_mtime = mtime_secs(&script)?;
                if verbose {
                    eprintln!(
                        "[scriptr] Cached mtime: {}, current mtime: {}",
                        meta.fp.mtime, cur_mtime
                    );
                }
                meta.fp.mtime != cur_mtime
            };

            if !mtime_changed && meta.bin.exists() {
                if verbose {
                    eprintln!(
                        "[scriptr] mtime unchanged, using cached binary: {}",
                        meta.bin.display()
                    );
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
                eprintln!(
                    "[scriptr] Cached hash: {}, current hash: {}",
                    &meta.fp.hash[..16],
                    &cur_hash[..16]
                );
            }

            if meta.fp.hash == cur_hash && meta.bin.exists() {
                if verbose {
                    eprintln!(
                        "[scriptr] Hash unchanged, using cached binary: {}",
                        meta.bin.display()
                    );
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

/// Return `(script_index, passthrough_args)` where `script_index` is the index in `all_args`
/// for the script path argument, if one is found.
fn split_invocation_args(all_args: &[String]) -> (Option<usize>, Vec<OsString>) {
    let mut script_index = None;
    let mut i = 1;

    while i < all_args.len() {
        let arg = &all_args[i];
        if arg == "--" {
            // `--` separates scriptr args from script invocation.
            if i + 1 < all_args.len() {
                script_index = Some(i + 1);
            }
            break;
        }

        // Options that consume one following value. Keep this list in sync with clap options.
        if arg == "--id" {
            i += 2;
            continue;
        }
        if arg.starts_with("--id=") {
            i += 1;
            continue;
        }

        if !arg.starts_with('-') {
            script_index = Some(i);
            break;
        }

        i += 1;
    }

    let passthrough_args = script_index
        .map(|idx| all_args[(idx + 1)..].iter().map(OsString::from).collect())
        .unwrap_or_default();

    (script_index, passthrough_args)
}

#[cfg(test)]
mod tests {
    use super::split_invocation_args;

    #[test]
    fn split_with_id_value_does_not_consume_script_path() {
        let args = vec![
            "scriptr".to_string(),
            "--id".to_string(),
            "123e4567-e89b-12d3-a456-426614174000".to_string(),
            "/tmp/script.rs".to_string(),
            "--flag".to_string(),
        ];
        let (script_idx, passthrough) = split_invocation_args(&args);
        assert_eq!(script_idx, Some(3));
        assert_eq!(passthrough, vec!["--flag"]);
    }

    #[test]
    fn split_with_id_equals_works() {
        let args = vec![
            "scriptr".to_string(),
            "--id=abc".to_string(),
            "/tmp/script.rs".to_string(),
            "arg1".to_string(),
        ];
        let (script_idx, passthrough) = split_invocation_args(&args);
        assert_eq!(script_idx, Some(2));
        assert_eq!(passthrough, vec!["arg1"]);
    }
}
