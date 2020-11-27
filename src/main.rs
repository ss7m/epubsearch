use std::collections::HashMap;
use std::fs::*;
use std::io::*;
use std::path::PathBuf;

use zip::read::*;

use xml::reader::{EventReader, XmlEvent};

use percent_encoding::percent_decode;

// TODO: parse toc.ncx

fn get_attribute(attributes: &[xml::attribute::OwnedAttribute], name: &str) -> Option<String> {
    for attr in attributes {
        if attr.name.local_name == name {
            return Some(
                percent_decode(attr.value.as_bytes())
                    .decode_utf8_lossy()
                    .into_owned(),
            );
        }
    }
    None
}

fn get_content_file(epub: &mut ZipArchive<File>) -> Option<String> {
    let container = epub.by_name("META-INF/container.xml").ok()?;
    let container = BufReader::new(container);

    for e in EventReader::new(container) {
        if let Ok(XmlEvent::StartElement {
            name, attributes, ..
        }) = e
        {
            if name.local_name == "rootfile" {
                return get_attribute(&attributes, "full-path");
            }
        } else if e.is_err() {
            return None;
        }
    }

    None
}

fn get_spine_documents(epub: &mut ZipArchive<File>) -> Vec<String> {
    let (content_file, oebps) = match get_content_file(epub) {
        Some(file_name) => {
            let mut path = PathBuf::from(file_name.clone());
            path.pop();
            let mut oebps = path.to_str().unwrap().to_owned();
            if oebps != "" {
                oebps += "/";
            }
            match epub.by_name(&file_name) {
                Ok(file) => (file, oebps),
                Err(_) => return Vec::new(),
            }
        }
        None => return Vec::new(),
    };

    let mut content_parser = EventReader::new(content_file);

    // read file until the manifest starts
    loop {
        match content_parser.next() {
            Ok(XmlEvent::StartElement { name, .. }) => {
                if name.local_name == "manifest" {
                    break;
                }
            }
            Err(_) => return Vec::new(),
            _ => {}
        }
    }

    // collect ids of the documents
    let mut content_ids = HashMap::new();
    loop {
        match content_parser.next() {
            Ok(XmlEvent::StartElement { attributes, .. }) => {
                let media_type = get_attribute(&attributes, "media-type");
                let id = get_attribute(&attributes, "id");
                let href = get_attribute(&attributes, "href");

                if let (Some(media_type), Some(id), Some(href)) = (media_type, id, href) {
                    if media_type == "application/xhtml+xml" {
                        content_ids.insert(id, href);
                    }
                }
            }
            Ok(XmlEvent::EndElement { name, .. }) => {
                if name.local_name == "manifest" {
                    break;
                }
            }
            Err(_) => return Vec::new(),
            _ => {}
        }
    }

    // loop until spine (manifest has to appear before spine)
    loop {
        match content_parser.next() {
            Ok(XmlEvent::StartElement { name, .. }) => {
                if name.local_name == "spine" {
                    break;
                }
            }
            Err(_) => return Vec::new(),
            _ => {}
        }
    }

    let mut spine = Vec::new();
    loop {
        match content_parser.next() {
            Ok(XmlEvent::StartElement { attributes, .. }) => {
                let idref = get_attribute(&attributes, "idref");
                if let Some(href) = idref.and_then(|i| content_ids.remove(&i)) {
                    spine.push(format!("{}{}", oebps, href));
                }
            }
            Ok(XmlEvent::EndElement { name, .. }) => {
                if name.local_name == "spine" {
                    break;
                }
            }
            _ => continue,
        }
    }

    spine
}

fn main() -> std::io::Result<()> {
    let file = File::open("Cannibal Magical.epub")?;
    let mut archive = ZipArchive::new(file)?;

    let documents = get_spine_documents(&mut archive);
    for doc in documents {
        let doc = archive.by_name(&doc).unwrap();
        let mut in_paragraph = false;
        for e in EventReader::new(doc) {
            match e {
                Ok(XmlEvent::StartElement { name, .. }) => {
                    if name.local_name == "p" {
                        in_paragraph = true
                    }
                }
                Ok(XmlEvent::EndElement { name, .. }) => {
                    if name.local_name == "p" {
                        println!();
                        in_paragraph = false
                    }
                }
                Ok(XmlEvent::Characters(s)) => {
                    if in_paragraph {
                        print!("{}", s)
                    }
                }
                _ => continue,
            }
        }
    }

    Ok(())
}
