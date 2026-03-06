use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <command>", args[0]);
        eprintln!("Commands:");
        eprintln!("  version    Print version info");
        std::process::exit(1);
    }

    match args[1].as_str() {
        "version" => {
            println!("megabasterd-rust v{}", env!("CARGO_PKG_VERSION"));
        }
        other => {
            eprintln!("Unknown command: {}", other);
            std::process::exit(1);
        }
    }
}
