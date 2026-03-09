pub mod common;
pub mod driver;
pub mod frontend;

use crate::driver::{cli, pipline};

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

    if let Err(err) = pipline::run(config) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
