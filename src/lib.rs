pub mod common;
pub mod driver;

use crate::driver::cli;

pub fn compiler_main() {
    let args = cli::parse();
    let config = args.to_config();
    if let Err(errors) = config.check_config() {
        eprintln!("invalid compiler config:");
        for err in errors {
            eprintln!("  Error: {err}");
        }
        std::process::exit(1);
    }
}
