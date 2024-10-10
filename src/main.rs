use std::env;
#[cfg(feature = "dbg")]
use std::time::Instant;
use std::process::ExitCode;

#[macro_use]
mod core;
use core::*;
mod server;
use server::*;
mod snowball;

const ADDR: &str = "0.0.0.0:6969";

fn main() -> ExitCode {
    let args = env::args().collect::<Vec::<_>>();
    if args.len() < 2 {
        eprintln!("usage: {program} <directory to search in>", program = args[0]);
        return ExitCode::FAILURE
    }

    let ref dir_path = args[1];

    let contents = dir_get_contents(dir_path);

    #[cfg(feature = "dbg")]
    let start = Instant::now();

    let mut model = Model::new();
    model.add_contents(&contents);

    #[cfg(feature = "dbg")] {
        let end = start.elapsed().as_millis();
        println!("indexing took: {end} millis");
    }

    let mut server = Server::new(am!(model));

    server.serve(ADDR).inspect_err(|err| eprintln!("{err}")).unwrap();

    return ExitCode::SUCCESS
}
