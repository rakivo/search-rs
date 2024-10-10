mod core;
use core::*;

use std::env;
#[cfg(feature = "dbg")]
use std::time::Instant;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = env::args().collect::<Vec::<_>>();
    if args.len() < 3 {
        eprintln!("usage: {program} <directory to search in> <term to search with>", program = args[0]);
        return ExitCode::FAILURE
    }

    let ref dir_path = args[1];
    let term = args[2..].join(" ").to_lowercase();

    let contents = dir_get_contents(dir_path);

    #[cfg(feature = "dbg")]
    let start = Instant::now();

    let mut model = Model::new();
    model.add_contents(&contents);
    let results = model.search(&term);

    #[cfg(feature = "dbg")] {
        let end = start.elapsed().as_millis();
        println!("indexing took: {end} millis");
    }

    return ExitCode::SUCCESS
}
