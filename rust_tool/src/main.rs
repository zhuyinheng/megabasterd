mod crypto;
mod mega;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage(&args[0]);
        std::process::exit(1);
    }

    match args[1].as_str() {
        "download" | "dl" => cmd_download(&args[2..]),
        "ls" | "list" => cmd_ls(&args[2..]),
        "version" => println!("megabasterd-rust v{}", env!("CARGO_PKG_VERSION")),
        "help" | "--help" | "-h" => print_usage(&args[0]),
        other => {
            eprintln!("Unknown command: {other}");
            print_usage(&args[0]);
            std::process::exit(1);
        }
    }
}

fn print_usage(prog: &str) {
    eprintln!("Usage: {prog} <command> [options]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  download <link> [-o <dir>] [-t <threads>]   Download file or folder");
    eprintln!("  ls <folder_link>                             List files in a MEGA folder");
    eprintln!("  version                                      Print version");
    eprintln!("  help                                         Print this help");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -o <dir>       Output directory (default: current directory)");
    eprintln!("  -t <N>         Parallel threads: chunks for files, files for folders (default: 4)");
    eprintln!();
    eprintln!("Supported link formats:");
    eprintln!("  https://mega.nz/file/<id>#<key>       Single file");
    eprintln!("  https://mega.nz/#!<id>!<key>          Single file (legacy)");
    eprintln!("  https://mega.nz/folder/<id>#<key>     Public folder");
}

// ── ls command ────────────────────────────────────────────────────────────────

fn cmd_ls(args: &[String]) {
    if args.is_empty() {
        eprintln!("Error: missing MEGA folder link");
        std::process::exit(1);
    }

    let link = &args[0];

    if !mega::is_folder_link(link) {
        eprintln!("Error: 'ls' only works with folder links (https://mega.nz/folder/...)");
        std::process::exit(1);
    }

    let (folder_id, key_str) = match mega::parse_folder_link(link) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let nodes = match mega::list_folder(&folder_id, &key_str) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    if nodes.is_empty() {
        eprintln!("(no files)");
        return;
    }

    println!("{:>12}  {}", "Size", "Name");
    println!("{:-<12}  {:-<40}", "", "");
    for node in &nodes {
        println!("{:>12}  {}", fmt_size(node.size), node.name);
    }
    println!();
    println!("{} file(s)", nodes.len());
}

// ── download command ──────────────────────────────────────────────────────────

fn cmd_download(args: &[String]) {
    if args.is_empty() {
        eprintln!("Error: missing MEGA link");
        eprintln!("Run with 'help' for usage.");
        std::process::exit(1);
    }

    let link = &args[0];
    let mut output_dir = ".".to_string();
    let mut threads: usize = 4;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output_dir = args[i].clone();
                } else {
                    eprintln!("Error: -o requires a directory argument");
                    std::process::exit(1);
                }
            }
            "-t" | "--threads" => {
                i += 1;
                if i < args.len() {
                    match args[i].parse::<usize>() {
                        Ok(n) if n >= 1 => threads = n,
                        _ => {
                            eprintln!("Error: -t requires a positive integer");
                            std::process::exit(1);
                        }
                    }
                } else {
                    eprintln!("Error: -t requires a number argument");
                    std::process::exit(1);
                }
            }
            other => {
                eprintln!("Unknown option: {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    if mega::is_folder_link(link) {
        cmd_download_folder(link, &output_dir, threads);
    } else {
        cmd_download_file(link, &output_dir, threads);
    }
}

fn cmd_download_file(link: &str, output_dir: &str, threads: usize) {
    let (file_id, key_str) = match mega::parse_link(link) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("Fetching file info from MEGA API...");
    let info = match mega::get_file_info(&file_id, &key_str) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let output_path = format!("{}/{}", output_dir.trim_end_matches('/'), info.name);
    eprintln!("File     : {}", info.name);
    eprintln!("Size     : {} bytes ({:.1} MiB)", info.size, info.size as f64 / (1024.0 * 1024.0));
    eprintln!("Threads  : {threads}");
    eprintln!("Output   : {output_path}");
    eprintln!("Downloading...");

    if let Err(e) = mega::download_file(&info, &output_path, threads) {
        eprintln!("Download failed: {e}");
        std::process::exit(1);
    }

    eprintln!("Done: {output_path}");
}

fn cmd_download_folder(link: &str, output_dir: &str, parallel: usize) {
    let (folder_id, key_str) = match mega::parse_folder_link(link) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    eprintln!("Folder ID: {folder_id}");
    eprintln!("Output   : {output_dir}");
    eprintln!("Parallel : {parallel} file(s) at a time");

    if let Err(e) = mega::download_folder(&folder_id, &key_str, output_dir, parallel) {
        eprintln!("Folder download failed: {e}");
        std::process::exit(1);
    }

    eprintln!("All files downloaded to: {output_dir}");
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn fmt_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}
