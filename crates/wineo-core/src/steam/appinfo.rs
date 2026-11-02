//! A reader for `<install.root>/appcache/appinfo.vdf`, Steam's binary cache of
//! what each app is. It keeps `common.oslist` and `common.type` for every app
//! and nothing else: the entries also carry access tokens and hashes, so
//! nothing here stores, logs, or prints the rest of one.
//!
//! The file is format version 41 (magic `0x07564429`), all little-endian:
//!
//! - A 16-byte header: magic, universe, and the string table's offset as an
//!   i64.
//! - Entries of a u32 app id and a u32 size, then `size` bytes. The key/values
//!   start 60 bytes in, after Steam's own bookkeeping. App id 0 ends the list.
//! - The string table at the offset: a u32 count and that many null-terminated
//!   strings. Keys are i32 indexes into it.
//!
//! Steam can rewrite the file while it is read, so every read is bounds-checked
//! and every size is capped. An entry that doesn't parse is a miss for its app
//! alone, and its size field still finds the next one. A file whose header,
//! string table, or entry chain doesn't hold together is unreadable, because
//! nothing after the break can be trusted.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use super::Install;

const MAGIC: u32 = 0x0756_4429;
const HEADER_LEN: usize = 16;
/// An entry's bookkeeping between its size field and its key/values: state,
/// update time, token, hash, change number, and binary hash.
const ENTRY_META_LEN: usize = 60;

/// Hitting a cap drops every game to the fallback, so these sit well above any
/// real library and only stop a corrupt file. The file size also limits memory.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// Steam's whole catalog is a few hundred thousand apps.
const MAX_APPS: usize = 1_000_000;
/// Depot ids are keys, so a big catalog has many distinct strings.
const MAX_STRINGS: usize = 1 << 24;
/// Entries nest a handful of levels. The cap keeps a hostile one from
/// exhausting the stack.
const MAX_DEPTH: usize = 64;
/// A kept value is a short word or list. Every app keeps its own copy, so the
/// file cap alone wouldn't bound what stays in memory.
const MAX_VALUE_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy)]
struct Limits {
    file_bytes: u64,
    apps: usize,
    strings: usize,
    depth: usize,
    value_bytes: usize,
}

const LIMITS: Limits = Limits {
    file_bytes: MAX_FILE_BYTES,
    apps: MAX_APPS,
    strings: MAX_STRINGS,
    depth: MAX_DEPTH,
    value_bytes: MAX_VALUE_BYTES,
};

/// Why the whole file can't be used. An app that is merely absent or has a bad
/// entry isn't an error: it's `None` from [`AppInfo::get`].
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("appinfo.vdf can't be read: {0}")]
    Io(#[from] io::Error),
    #[error("appinfo.vdf is larger than {MAX_FILE_BYTES} bytes")]
    TooLarge,
    #[error("appinfo.vdf has an unknown format ({0:#010x})")]
    UnknownMagic(u32),
    #[error("appinfo.vdf is malformed: {0}")]
    Malformed(&'static str),
}

/// What appinfo says about one app. Both values are trimmed and lowercased, so
/// they compare without regard to case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppFacts {
    /// `common.oslist` as a comma list such as `windows,linux`. `None` when the
    /// app has none, and `Some("")` when it is there and empty.
    pub oslist: Option<String>,
    /// `common.type`, such as `game`, `demo`, or `music`.
    pub kind: Option<String>,
}

/// The facts for every app in the file whose entry could be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppInfo {
    apps: HashMap<u32, AppFacts>,
}

impl AppInfo {
    /// `None` when the file has no entry for the app, or its entry was bad.
    pub fn get(&self, app_id: u32) -> Option<&AppFacts> {
        self.apps.get(&app_id)
    }

    pub fn len(&self) -> usize {
        self.apps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }
}

/// Reads and parses the install's `appcache/appinfo.vdf`.
pub fn read(install: &Install) -> Result<AppInfo, ReadError> {
    read_file(&install.root().join("appcache/appinfo.vdf"), LIMITS)
}

fn read_file(path: &Path, limits: Limits) -> Result<AppInfo, ReadError> {
    #[cfg(any(test, feature = "test-support"))]
    crate::read_log::note(path);
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limits.file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limits.file_bytes {
        return Err(ReadError::TooLarge);
    }
    parse_with(&bytes, limits)
}

/// Parses the contents of an `appinfo.vdf`.
pub fn parse(bytes: &[u8]) -> Result<AppInfo, ReadError> {
    parse_with(bytes, LIMITS)
}

fn parse_with(bytes: &[u8], limits: Limits) -> Result<AppInfo, ReadError> {
    if bytes.len() as u64 > limits.file_bytes {
        return Err(ReadError::TooLarge);
    }
    let mut header = Cursor::new(bytes);
    let magic = header
        .u32()
        .ok_or(ReadError::Malformed("header is cut short"))?;
    if magic != MAGIC {
        return Err(ReadError::UnknownMagic(magic));
    }
    let _universe = header.u32();
    let offset = header
        .i64()
        .ok_or(ReadError::Malformed("header is cut short"))?;
    let table_start = usize::try_from(offset)
        .ok()
        .filter(|start| (HEADER_LEN..=bytes.len()).contains(start))
        .ok_or(ReadError::Malformed("string table is out of range"))?;

    let table = bytes
        .get(table_start..)
        .ok_or(ReadError::Malformed("string table is out of range"))?;
    let strings = read_strings(table, limits.strings)?;
    let list = bytes
        .get(HEADER_LEN..table_start)
        .ok_or(ReadError::Malformed("string table is out of range"))?;

    let mut apps = HashMap::new();
    let mut seen = 0usize;
    let mut cursor = Cursor::new(list);
    // The list ends at the terminator, or at the table if Steam omitted it.
    while cursor.remaining() > 0 {
        let app_id = cursor
            .u32()
            .ok_or(ReadError::Malformed("entry list is cut short"))?;
        if app_id == 0 {
            break;
        }
        seen += 1;
        if seen > limits.apps {
            return Err(ReadError::Malformed("too many entries"));
        }
        let size = cursor
            .u32()
            .and_then(|size| usize::try_from(size).ok())
            .ok_or(ReadError::Malformed("entry list is cut short"))?;
        let body = cursor.take(size).ok_or(ReadError::Malformed(
            "an entry runs past the end of the list",
        ))?;
        let facts = body
            .get(ENTRY_META_LEN..)
            .and_then(|kv| entry_facts(kv, &strings, limits));
        if let Some(facts) = facts {
            apps.entry(app_id).or_insert(facts);
        }
    }
    Ok(AppInfo { apps })
}

/// Each string needs its terminator, so a count larger than the bytes left is
/// corrupt, and that is checked before anything is allocated.
fn read_strings(table: &[u8], max: usize) -> Result<Vec<&[u8]>, ReadError> {
    let mut cursor = Cursor::new(table);
    let count = cursor
        .u32()
        .and_then(|count| usize::try_from(count).ok())
        .ok_or(ReadError::Malformed("string table is cut short"))?;
    if count > max || count > cursor.remaining() {
        return Err(ReadError::Malformed("string table count is out of range"));
    }
    let mut strings = Vec::with_capacity(count);
    for _ in 0..count {
        strings.push(
            cursor
                .cstr()
                .ok_or(ReadError::Malformed("string table is cut short"))?,
        );
    }
    Ok(strings)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The children of the entry's root object.
    Root,
    Common,
    /// Anywhere else, where an `oslist` or `type` means something different.
    Other,
}

/// The facts in one entry's key/values, or `None` when it is bad.
fn entry_facts(kv: &[u8], strings: &[&[u8]], limits: Limits) -> Option<AppFacts> {
    let mut walker = Walker {
        cursor: Cursor::new(kv),
        strings,
        limits,
        facts: AppFacts::default(),
    };
    // One root object, then the end marker of the level it sits in. Trailing
    // bytes are ignored, since the entry's size is what bounds it.
    if walker.cursor.u8()? != 0 {
        return None;
    }
    walker.key()?;
    walker.object(1, Scope::Root)?;
    is_end(walker.cursor.u8()?).then_some(walker.facts)
}

struct Walker<'a, 's> {
    cursor: Cursor<'a>,
    strings: &'s [&'a [u8]],
    limits: Limits,
    facts: AppFacts,
}

impl<'a> Walker<'a, '_> {
    fn key(&mut self) -> Option<&'a [u8]> {
        let index = usize::try_from(self.cursor.i32()?).ok()?;
        self.strings.get(index).copied()
    }

    /// Reads values up to the end marker closing the object at `depth`.
    fn object(&mut self, depth: usize, scope: Scope) -> Option<()> {
        loop {
            let kind = self.cursor.u8()?;
            if is_end(kind) {
                return Some(());
            }
            let key = self.key()?;
            match kind {
                0 => {
                    if depth >= self.limits.depth {
                        return None;
                    }
                    let child = if scope == Scope::Root && key.eq_ignore_ascii_case(b"common") {
                        Scope::Common
                    } else {
                        Scope::Other
                    };
                    self.object(depth + 1, child)?;
                }
                1 => {
                    let value = self.cursor.cstr()?;
                    if scope == Scope::Common {
                        self.keep(key, value)?;
                    }
                }
                2 | 3 | 6 => {
                    self.cursor.take(4)?;
                }
                7 | 10 => {
                    self.cursor.take(8)?;
                }
                9 => {
                    let len = usize::try_from(self.cursor.u32()?).ok()?;
                    self.cursor.take(len)?;
                }
                // Types 4 and 5 are left out because public readers disagree on
                // their size, and a wrong guess would misread the rest.
                _ => return None,
            }
        }
    }

    /// `None` when a value we keep isn't UTF-8 or is over the cap. Other strings
    /// are skipped unchecked, since a bad one there can't change what we report.
    fn keep(&mut self, key: &[u8], value: &[u8]) -> Option<()> {
        let is_oslist = key.eq_ignore_ascii_case(b"oslist");
        if !is_oslist && !key.eq_ignore_ascii_case(b"type") {
            return Some(());
        }
        if value.len() > self.limits.value_bytes {
            return None;
        }
        let value = std::str::from_utf8(value).ok()?;
        if is_oslist {
            self.facts
                .oslist
                .get_or_insert_with(|| normalize_list(value));
        } else {
            self.facts
                .kind
                .get_or_insert_with(|| value.trim().to_ascii_lowercase());
        }
        Some(())
    }
}

fn is_end(kind: u8) -> bool {
    kind == 8 || kind == 11
}

fn normalize_list(value: &str) -> String {
    value
        .split(',')
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Every read returns `None` instead of going past the end.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Option<i32> {
        self.take(4)?.try_into().ok().map(i32::from_le_bytes)
    }

    fn i64(&mut self) -> Option<i64> {
        self.take(8)?.try_into().ok().map(i64::from_le_bytes)
    }

    /// The bytes before the next null, which is consumed too.
    fn cstr(&mut self) -> Option<&'a [u8]> {
        let rest = self.data.get(self.pos..)?;
        let len = rest.iter().position(|&byte| byte == 0)?;
        self.pos += len + 1;
        rest.get(..len)
    }
}

#[cfg(test)]
mod tests;
