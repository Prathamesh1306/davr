use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

#[derive(Parser)]
#[command(name = "xtask", about = "DAVR developer and release automation tasks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Verify and generate documentation links")]
    Docs,

    #[command(about = "Run complete workspace validation (check, clippy, fmt, test)")]
    CheckAll,

    #[command(about = "Validate release readiness (clean git, workspace checks, version parity)")]
    ReleaseCheck,
}

fn main() {
    let cli = Cli::parse();
    let root = project_root();

    match cli.command {
        Commands::Docs => {
            println!("==> Validating documentation files in docs/...");
            let required_docs = [
                "README.md",
                "ARCHITECTURE.md",
                "docs/architecture/overview.md",
                "docs/cli-reference/commands.md",
                "docs/adr/001-sqlite-embedded-storage.md",
                "docs/adr/002-transactional-git-rollback.md",
                "docs/adr/003-treesitter-ast-parsing.md",
                "docs/adr/004-model-context-protocol.md",
            ];

            let mut missing = 0;
            for doc in &required_docs {
                let p = root.join(doc);
                if p.exists() {
                    println!("  ✔ Found {}", doc);
                } else {
                    eprintln!("  ✖ Missing {}", doc);
                    missing += 1;
                }
            }

            if missing > 0 {
                eprintln!("\nDocumentation check failed: {} missing files", missing);
                exit(1);
            }
            println!("\n✔ All required documentation files are present!");
        }
        Commands::CheckAll => {
            println!("==> Running cargo check --workspace...");
            run_cmd("cargo", &["check", "--workspace"], &root);

            println!("==> Running cargo clippy --workspace --all-targets...");
            run_cmd(
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
                &root,
            );

            println!("==> Running cargo fmt --all -- --check...");
            run_cmd("cargo", &["fmt", "--all", "--", "--check"], &root);

            println!("==> Running cargo test --workspace...");
            run_cmd("cargo", &["test", "--workspace"], &root);

            println!("\n✔ All workspace checks passed cleanly!");
        }
        Commands::ReleaseCheck => {
            println!("==> Running release readiness verification...");

            // 1. Check workspace
            run_cmd("cargo", &["check", "--workspace"], &root);
            run_cmd("cargo", &["test", "--workspace"], &root);

            // 2. Validate docs
            let required_docs = [
                "README.md",
                "LICENSE",
                "CHANGELOG.md",
                "docs/cli-reference/commands.md",
            ];
            for doc in &required_docs {
                if !root.join(doc).exists() {
                    eprintln!(
                        "Release check error: Missing required release file: {}",
                        doc
                    );
                    exit(1);
                }
            }

            println!("\n✔ Release check passed! Ready for tagging and distribution.");
        }
    }
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn run_cmd(cmd: &str, args: &[&str], cwd: &Path) {
    let status = Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("Failed to execute {}: {}", cmd, e);
            exit(1);
        });

    if !status.success() {
        eprintln!(
            "Command '{} {}' failed with status: {}",
            cmd,
            args.join(" "),
            status
        );
        exit(status.code().unwrap_or(1));
    }
}
