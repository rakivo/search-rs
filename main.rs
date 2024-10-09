use std::path::Path;
use std::fmt::Debug;
use std::io::BufReader;
use std::fs::{File, read_dir};

use rayon::prelude::*;
use hashbrown::HashMap;
use foldhash::fast::RandomState;
use xml::reader::{EventReader, XmlEvent};

type IoResult<T> = std::io::Result::<T>;
type Index<'a> = HashMap::<&'a str, usize>;

fn read_xml<P>(file_path: P) -> IoResult::<String>
where
    P: AsRef::<Path> + Debug
{
    let file = File::open(&file_path).map_err(|err| {
        eprintln!("could not read {file_path:?}: {err}"); err
    })?;

    let file = BufReader::new(file);
    let parser = EventReader::new(file);

    let string = parser.into_iter().filter_map(|event| {
        match event {
            Ok(XmlEvent::Characters(text)) => Some(text),
            _ => None
        }
    }).collect();

    Ok(string)
}

#[inline]
fn index_content<'a>(content: &'a str) -> Index<'a> {
    content.split_whitespace().fold(
        Index::with_hasher(RandomState::default()),
    |mut map, word| {
        *map.entry(word.trim()).or_insert(0) += 1;
        map
    })
}

fn main() -> std::io::Result::<()> {
    let dir_path = "gl4";
    let dir = read_dir(dir_path)?;
    let strings = dir.filter_map(|e| e.map(|e| e.path()).ok())
        .par_bridge()
        .filter_map(|e| read_xml(e).ok())
        .collect::<Vec::<_>>();

    // for string in strings {
    //     let index = index_content(&string);
    //     println!("{index:?}");
    // }

    Ok(())
}
