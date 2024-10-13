use std::env;
#[cfg(feature = "dbg")]
use std::time::Instant;
use std::path::PathBuf;
use std::process::ExitCode;

#[macro_use]
mod core;
use core::*;
mod server;
use server::*;
mod dir_rec;
mod snowball;

const ADDR: &str = "localhost";
const DEFAULT_PORT: &str = "6969";

fn main() -> ExitCode {
    let args = env::args().collect::<Vec::<_>>();
    if args.len() < 2 {
        eprintln!("usage: {program} <directory to search in> [port to serve at]", program = args[0]);
        return ExitCode::FAILURE
    }

    let ref dir_path = args[1];
    let dir_path_buf = Into::<PathBuf>::into(dir_path);

    let ref port = if args.len() > 2 {
        let port = args[2].as_str();
        if port.len() != 4 || port.parse::<u16>().is_err() {
            eprintln!("`{port}` is not a valid port to serve at");
            return ExitCode::FAILURE
        }
        port
    } else {
        DEFAULT_PORT
    };

    if !(dir_path_buf.exists() && dir_path_buf.is_dir()) {
        eprintln!("`{dir_path}` is not a valid directory");
        return ExitCode::FAILURE
    }

    println!("reading files..");
    let contents = dir_get_contents(dir_path);

    #[cfg(feature = "dbg")]
    let start = Instant::now();

    println!("indexing files..");
    let mut model = Model::new(contents.len());
    model.add_contents(&contents);

    #[cfg(feature = "dbg")] {
        let end = start.elapsed().as_millis();
        println!("indexing took: {end} millis");
    }

    let Ok(curr_dir) = env::current_dir() else {
        eprintln!("could not get current directory");
        return ExitCode::FAILURE
    };

    let mut server = Server::new(model, &curr_dir);

    let addr = format!("{ADDR}:{port}");
    if let Err(err) = server.serve(addr.as_str()) {
        eprintln!("{err}");
        return ExitCode::FAILURE
    }

    ExitCode::SUCCESS
}

/* TODO:
    Parallelize indexing process and start the server instantly,
    then if user sends a query request the server should respond with
    a data we have indexed right now, even though the indexing process is not finished yet.
*/
