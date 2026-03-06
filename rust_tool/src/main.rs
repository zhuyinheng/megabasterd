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
    eprintln!("  download <mega_link> [-o <output_dir>]   Download and decrypt a MEGA file");
    eprintln!("  version                                   Print version");
    eprintln!("  help                                      Print this help");
    eprintln!();
    eprintln!("Supported link formats:");
    eprintln!("  https://mega.nz/file/<id>#<key>");
    eprintln!("  https://mega.nz/#!<id>!<key>");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  {prog} download 'https://mega.nz/file/ABC123#KEY456'");
    eprintln!("  {prog} download 'https://mega.nz/#!ABC123!KEY456' -o /tmp");
}

fn cmd_download(args: &[String]) {
    if args.is_empty() {
        eprintln!("Error: missing MEGA link");
        eprintln!("Run with 'help' for usage.");
        std::process::exit(1);
    }

    let link = &args[0];
    let mut output_dir = ".".to_string();

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
            other => {
                eprintln!("Unknown option: {other}");
                std::process::exit(1);
            }
        }
        i += 1;
    }

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
    eprintln!("Output   : {output_path}");
    eprintln!("Downloading...");

    if let Err(e) = mega::download_file(&info, &output_path) {
        eprintln!("Download failed: {e}");
        std::process::exit(1);
    }

    eprintln!("Done: {output_path}");
}
