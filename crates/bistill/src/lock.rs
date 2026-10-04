//! `poll.lock` holds one process id, in decimal.
//!
//! The file is created so that only one process can create it. A live process
//! id means that process is the poller. Anything else means the next process
//! may take the file. Dropping the lock removes the file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(crate) struct Held {
    path: PathBuf,
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) enum Acquire {
    Holder(Held),
    Busy { pid: u32 },
}

pub(crate) fn acquire(state_dir: &Path) -> io::Result<Acquire> {
    let path = state_dir.join("poll.lock");
    loop {
        match create(&path) {
            Ok(held) => return Ok(Acquire::Holder(held)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => match occupant(&path) {
                Occupant::Alive(pid) => return Ok(Acquire::Busy { pid }),
                Occupant::Free => match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(err) => return Err(err),
                },
            },
            Err(err) => return Err(err),
        }
    }
}

fn create(path: &Path) -> io::Result<Held> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let text = format!("{}\n", std::process::id());
    file.write_all(text.as_bytes())?;
    Ok(Held {
        path: path.to_owned(),
    })
}

enum Occupant {
    Alive(u32),
    Free,
}

fn occupant(path: &Path) -> Occupant {
    let Ok(text) = fs::read_to_string(path) else {
        return Occupant::Free;
    };
    match text.trim().parse::<u32>() {
        Ok(pid) if super::pid_os::pid_alive(pid) => Occupant::Alive(pid),
        _ => Occupant::Free,
    }
}
