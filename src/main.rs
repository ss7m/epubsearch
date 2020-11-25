use std::fs::*;
use std::io::*;

use zip::read::*;

use xml::reader::{EventReader, XmlEvent};

fn get_content_file(epub: &mut ZipArchive<File>) -> Option<ZipFile> {
    let container = epub.by_name("META-INF/container.xml").ok()?;
    let container = BufReader::new(container);

    let mut content_file_name = None;
    for e in EventReader::new(container) {
        if let Ok(XmlEvent::StartElement {
            name, attributes, ..
        }) = e
        {
            if name.local_name == "rootfile" {
                for attr in attributes {
                    if attr.name.local_name == "full-path" {
                        content_file_name = Some(attr.value.to_owned());
                    }
                }
            }
        } else if e.is_err() {
            return None;
        }
    }

    match content_file_name {
        Some(fname) => epub.by_name(&fname).ok(),
        None => None,
    }
}

struct EpubItem {
    href: String,
    id: String,
}

fn main() -> std::io::Result<()> {
    let file = File::open("Hanamonogatari.epub")?;
    let mut archive = ZipArchive::new(file)?;
    let mut content_file = get_content_file(&mut archive).unwrap();

    let mut s = String::new();
    content_file.read_to_string(&mut s)?;
    println!("{}", s);

    Ok(())
}
