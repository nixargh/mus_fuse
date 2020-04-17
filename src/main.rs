// Fuse staff
extern crate fuse;
extern crate libc;
extern crate time;
use fuse::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
};
use libc::ENOENT;
use reqwest::blocking::Client;
use reqwest::blocking::Response;
use reqwest::header::CONTENT_LENGTH;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::env;
use time::Timespec;
//use http::Method;

// Download lib staff
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use std::path::Path;

#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
pub struct Track {
    pub id: Option<String>,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub format: Option<String>,
    pub filetype: Option<String>,
    pub path: Option<String>,
}

const API_URL: &str = "https://mus.hexor.ru";
const CACHE_HEAD: i64 = 1024 * 1024;
const MAX_CACHE_SIZE: i64 = 10 * 1024 * 1025; // Mb

fn get_basename(path: Option<&String>) -> Option<String> {
    let base = match percent_decode_str(path.unwrap().as_str()).decode_utf8() {
        Ok(path) => {
            let remote_name = path.into_owned();
            let basename = Path::new(&remote_name).file_name();
            match basename {
                Some(name) => Some(name.to_os_string().into_string().unwrap()),
                None => None,
            }
        }
        Err(_) => None,
    };
    base
}

#[tokio::main]
async fn get_tracks() -> Result<Vec<Track>, Box<dyn std::error::Error>> {
    let resp = reqwest::get(format!("{}/songs", API_URL).as_str())
        .await?
        .json::<Vec<Track>>()
        .await?;
    println!("Found {} tracks.", resp.len());
    Ok(resp)
}

#[cfg(target_family = "unix")]
struct JsonFilesystem {
    tree: Vec<Track>,
    attrs: BTreeMap<u64, FileAttr>,
    inodes: BTreeMap<String, u64>,
    buffer_head: BTreeMap<String, Vec<u8>>,
    buffer_length: BTreeMap<String, i64>,
}

#[cfg(target_family = "unix")]
impl JsonFilesystem {
    fn new(tree: &Vec<Track>) -> JsonFilesystem {
        let mut attrs = BTreeMap::new();
        let mut inodes = BTreeMap::new();
        let ts = time::now().to_timespec();
        let attr = FileAttr {
            ino: 1,
            size: 0,
            blocks: 0,
            atime: ts,
            mtime: ts,
            ctime: ts,
            crtime: ts,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 0,
            uid: 0,
            gid: 0,
            rdev: 0,
            flags: 0,
        };
        attrs.insert(1, attr);
        inodes.insert("/".to_string(), 1);
        for (i, track) in tree.iter().enumerate() {
            let basename = get_basename(track.path.as_ref()).unwrap().to_string();
            let attr = FileAttr {
                ino: i as u64 + 2,
                //size: 1024 * 1024 * 1024 as u64,
                size: 591646132,
                blocks: 0,
                atime: ts,
                mtime: ts,
                ctime: ts,
                crtime: ts,
                kind: FileType::RegularFile,
                perm: 0o644,
                nlink: 0,
                uid: 0,
                gid: 0,
                rdev: 0,
                flags: 0,
            };
            attrs.insert(attr.ino, attr);
            inodes.insert(basename.clone(), attr.ino);
        }
        JsonFilesystem {
            tree: tree.clone(),
            attrs: attrs,
            inodes: inodes,
            buffer_head: BTreeMap::new(),
            buffer_length: BTreeMap::new(),
        }
    }
}

#[cfg(target_family = "unix")]
impl Filesystem for JsonFilesystem {
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        //println!("getattr(ino={})", ino);
        match self.attrs.get(&ino) {
            Some(attr) => {
                let ttl = Timespec::new(1, 0);
                reply.attr(&ttl, attr);
            }
            None => reply.error(ENOENT),
        };
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        //println!("lookup(parent={}, name={})", parent, name.to_str().unwrap());
        let inode = match self.inodes.get(name.to_str().unwrap()) {
            Some(inode) => inode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        match self.attrs.get(inode) {
            Some(attr) => {
                let ttl = Timespec::new(1, 0);
                reply.entry(&ttl, attr, 0);
            }
            None => reply.error(ENOENT),
        };
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        reply: ReplyData,
    ) {
        print!(
            "read(ino={}, fh={}, offset={}, size={}) ",
            ino, fh, offset, size
        );

        let url = &self.tree[(ino - 2) as usize].path.as_ref().unwrap();
        let id = &self.tree[(ino - 2) as usize].id.as_ref().unwrap();
        let full_url = format!("{}/{}", API_URL, url);
        let mut chunk: Vec<u8>;
        let content_length: i64;
        let client = Client::new();
        let mut resp: Response;

        // content_length cache.
        if self.buffer_length.contains_key(id.as_str()) {
            content_length = self.buffer_length[id.as_str()];
            print!("Hit LC ");
        } else {
            resp = client.head(full_url.as_str()).send().unwrap();
            content_length = resp
                .headers()
                .get(CONTENT_LENGTH)
                .unwrap()
                .to_str()
                .unwrap()
                .parse::<i64>()
                .unwrap();
            self.buffer_length.insert(id.to_string(), content_length);
            print!("Miss LC ");
        }
        print!("LC: {} ", self.buffer_length.len());
        print!("HC: {} ", self.buffer_head.len());

        if content_length > offset {
            print!("Content len {:?} ", content_length);
            let mut end_of_chunk = offset - 1 + size as i64;
            let range = format!(
                "bytes={}-{}",
                offset,
                if (end_of_chunk) > content_length {
                    content_length
                } else {
                    end_of_chunk
                }
            );

            // if it's beginning of file...
            if (offset - 1 + size as i64) < CACHE_HEAD {
                // cleaning cache before. it should be less than MAX_CACHE_SIZE bytes
                if self.buffer_head.len() as i64 * CACHE_HEAD > MAX_CACHE_SIZE {
                    let (key, _) = self.buffer_head.iter_mut().next().unwrap();
                    let key_cpy: String = key.to_string();
                    self.buffer_head.remove(&key_cpy);
                    print!(" *Cache Cleaned* ");
                }

                // looking for CACHE_HEAD bytes file beginning in cache
                if self.buffer_head.contains_key(id.as_str()) {
                    print!("Hit head cache! ");
                    chunk = self.buffer_head[id.as_str()]
                        [offset as usize..(size + offset as u32) as usize]
                        .to_vec()
                        .clone();
                    reply.data(&chunk);
                } else {
                    print!("Miss head cache! ");
                    resp = client
                        .get(full_url.as_str())
                        .header(
                            "Range",
                            format!(
                                "bytes=0-{}",
                                if CACHE_HEAD > content_length {
                                    content_length
                                } else {
                                    CACHE_HEAD
                                }
                            ),
                        )
                        .send()
                        .unwrap();
                    let response = resp.bytes().unwrap();
                    self.buffer_head.insert(id.to_string(), response.to_vec());
                    end_of_chunk = if content_length < end_of_chunk {
                        content_length
                    } else {
                        end_of_chunk
                    };
                    chunk = response[offset as usize..end_of_chunk as usize].to_vec();
                    reply.data(&chunk);
                }
                println!("Chunk len: {:?} ", chunk.len());
                return;
            }
            resp = client
                .get(full_url.as_str())
                .header("Range", &range)
                .send()
                .unwrap();
            let test = resp.bytes().unwrap();
            chunk = test.to_vec().clone();
            reply.data(&chunk);
            println!(
                " Len: {}, Chunk {} - {}",
                chunk.len(),
                offset,
                offset - 1 + chunk.len() as i64
            );
        } else {
            println!(
                "Wrong offset. Len is {} but offset {}",
                content_length, offset
            );
            reply.data(&[]);
        }
        return;
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        //println!("readdir(ino={}, fh={}, offset={})", ino, fh, offset);
        if ino == 1 {
            if offset == 0 {
                reply.add(1, 0, FileType::Directory, ".");
                reply.add(1, 1, FileType::Directory, "..");
            }
            for (i, (key, &inode)) in self.inodes.iter().enumerate().skip(offset as usize) {
                if inode == 1 {
                    continue;
                }
                reply.add(inode, (i + 1) as i64, FileType::RegularFile, key);
            }
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}

fn main() {
    let lib = get_tracks().unwrap();
    let fs = JsonFilesystem::new(&lib);
    let mountpoint = match env::args().nth(1) {
        Some(path) => path,
        None => {
            println!("Usage: {} <MOUNTPOINT>", env::args().nth(0).unwrap());
            return;
        }
    };
    let options = ["-o", "ro", "-o", "fsname=musfs", "-o", "async_read"]
        .iter()
        .map(|o| o.as_ref())
        .collect::<Vec<&OsStr>>();
    fuse::mount(fs, &mountpoint, &options).expect("Couldn't mount filesystem");
}
