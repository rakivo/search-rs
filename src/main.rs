use std::env;
#[cfg(feature = "dbg")]
use std::path::PathBuf;
use std::time::Instant;
use std::process::ExitCode;

#[macro_use]
mod core;
use core::*;
mod server;
use server::*;
mod snowball;

const ADDR: &str = "0.0.0.0";
const DEFAULT_PORT: &str = "6969";

fn main() -> ExitCode {
    let args = env::args().collect::<Vec::<_>>();
    if args.len() < 2 {
        eprintln!("usage: {program} <directory to search in> [addr to serve at] [port to serve at]", program = args[0]);
        return ExitCode::FAILURE
    }

    let ref dir_path = args[1];

    let dir_path_buf = Into::<PathBuf>::into(dir_path);
    if !(dir_path_buf.exists() && dir_path_buf.is_dir()) {
        eprintln!("`{dir_path}` is not a valid directory");
        return ExitCode::FAILURE
    }

    let ref port = if args.len() > 3 {
        args[2].as_str()
    } else {
        DEFAULT_PORT
    };

    let contents = dir_get_contents(dir_path);

    #[cfg(feature = "dbg")]
    let start = Instant::now();

    let mut model = Model::new(contents.len());
    model.add_contents(&contents);

    #[cfg(feature = "dbg")] {
        let end = start.elapsed().as_millis();
        println!("indexing took: {end} millis");
    }

    let mut server = Server::new(model);

    let addr = format!("{ADDR}:{port}");
    if let Err(err) = server.serve(addr.as_str()) {
        eprintln!("{err}");
        return ExitCode::FAILURE
    }

    return ExitCode::SUCCESS
}
