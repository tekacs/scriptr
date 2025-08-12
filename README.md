# scriptr

Fast launcher for Rust single-file packages. Cuts startup time from 200ms to 5ms by caching builds intelligently.

## The Problem

Cargo's single-file package feature (`-Zscript`) makes writing scripts in Rust super enjoyable. The front-matter syntax, the path forward of native scripting support, I really want to use that for all my scripts!

But... every invocation pays a startup tax. Even when nothing changed, cargo walks the entire dependency graph, checks fingerprints, and verifies timestamps. For scripts, the 200ms (minimum) delay is extremely noticeable. Python starts in 20ms. Your shell starts in 5ms.

## The Solution

`scriptr` wraps cargo's build process with caching:

1. **First run**: Builds normally via `cargo +nightly -Zscript`
2. **Subsequent runs**: Checks mtime → if unchanged, exec cached binary (15ms)
3. **After edits**: Checks mtime → changed → checks BLAKE3 hash → if same, exec cached binary; if different, rebuild

This two-level fingerprinting means touching a file doesn't trigger rebuilds and most invocations finish in 5ms.

## Installation

Install from crates.io:

```bash
cargo install scriptr
```

Or build from source:

```bash
git clone https://github.com/tekacs/scriptr
cd scriptr
cargo install --path .
```

## Usage

Write your script with scriptr in the shebang:

```rust
#!/usr/bin/env scriptr
---
[dependencies]
clap = { version = "4.5", features = ["derive"] }
---

use clap::Parser;

#[derive(Parser)]
struct Args {
    name: String,
}

fn main() {
    let args = Args::parse();
    println!("Hello, {}!", args.name);
}
```

Notes:
- The front-matter is standard `Cargo.toml` embedded between lines that contain only `---`. You can use common sections like `[package]`, `[dependencies]`, `[features]`, etc.
- File extension is optional; `hello` or `hello.rs` both work.
- For a quick primer available at the terminal, see:
  - `scriptr --help` (includes a shebang + front-matter example and notes)

Then:

```bash
chmod +x hello.rs
./hello.rs World  # First run: ~1.5s (builds dependencies)
./hello.rs World  # Subsequent runs: ~5ms
```

### Extension-less Scripts

Unlike `cargo -Zscript` which requires `.rs` extensions, scriptr works with any filename:

```bash
chmod +x hello     # No .rs extension needed
./hello World      # Works exactly the same
```

Note: If you plan to also use `cargo -Zscript` directly with your scripts, stick with `.rs` extensions.

## Command-line Options

- `-d, --debug` - Build in debug mode (default is release mode)
- `-v, --verbose` - Show detailed operation logging  
- `-f, --force` - Force rebuild, ignoring cache
- `-c, --clean` - Clean cache before building
- `-C, --clean-only` - Clean cache and exit without running
- `-H, --hash-only` - Use only hash for comparison (skip mtime check)

By default, scriptr builds in release mode for optimal performance. Use `-d` if you need debug symbols:

```rust
#!/usr/bin/env -S scriptr --debug
```

The `-H` flag is useful in environments where mtime is unreliable (like some network filesystems or CI environments).

You can also view usage and examples any time:

```bash
scriptr --help
```

## Performance

Measured on M2 MacBook Pro:

**Simple "Hello World" script:**
| Tool | First Run | Subsequent Runs |
|------|-----------|-----------------|
| `cargo -Zscript` | 157ms | 157ms |
| `scriptr` | 157ms | 4.6ms |

**With clap dependency:**
| Tool | First Run | Subsequent Runs |
|------|-----------|-----------------|
| `cargo -Zscript` | ~2s | 157ms |
| `scriptr` | ~2s | 4.8ms |

**That's a 34x / 416x speedup for cached runs!**

The 4-5ms overhead includes: process spawn, cache lookup, mtime check, and exec.

## The Cache

Note that we deliberately DO NOT cache the binaries ourselves. We let cargo build them into the same location it ordinarily would. That way, if you directly use `cargo --manifest-path <file>` or `cargo -Zscript <file>` to run or poke at them, it'll use the same build. We cache only the mtime + hash, in:

- Linux: `~/.cache/scriptr/`
- macOS: `~/Library/Caches/scriptr/`
- Windows: `%LOCALAPPDATA%\scriptr\`

Cache keys are based on the script's absolute path. Each script gets its own metadata file tracking mtime, BLAKE3 hash, and binary location.

## Compatibility

Works seamlessly with standard cargo workflows:

```bash
# These share the same build artifacts:
./script.rs                                    # via scriptr
cargo +nightly -Zscript run script.rs         # via cargo
```

Both approaches use cargo's default target directory, ensuring consistent behavior and garbage collection.

## Implementation Details

- **Fingerprinting**: BLAKE3 for speed (GiB/s on modern CPUs)
- **Concurrency**: File locks prevent metadata races
- **Binary discovery**: Parses cargo's JSON output for exact executable path
- **Process model**: Uses exec(2) for zero overhead after launch

# Requirements

- Rust nightly toolchain (for `-Zscript`)
- Unix-like OS (Linux, macOS, BSD)

## Why scriptr?

The underlying issue is being tracked in [rust-lang/cargo#12207](https://github.com/rust-lang/cargo/issues/12207) - cargo needs to check many small files on every run, making the 200ms overhead fundamental to the current architecture. Until cargo implements content-based caching or other optimizations, scriptr provides a practical workaround.

## License

MIT OR Apache-2.0
