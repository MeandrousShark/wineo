//! Reading the target out of a Windows `.lnk`, which is how wineo learns what
//! an installer meant to advertise.
//!
//! Only what a target path needs is parsed: the header's flags, the length
//! prefix that lets `LinkTargetIDList` be stepped over, and `LinkInfo`'s local
//! base path. The format is MS-SHLLINK.

use std::fmt;

/// Why a `.lnk` yielded no usable target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreadable {
    NotAShortcut,
    Truncated,
    NoLinkInfo,
    NoLocalPath,
    DriveOnly,
    NotAProgram,
}

impl Unreadable {
    pub fn as_str(self) -> &'static str {
        match self {
            Unreadable::NotAShortcut => "not a shortcut",
            Unreadable::Truncated => "the shortcut ends early",
            Unreadable::NoLinkInfo => "the shortcut records no file path",
            Unreadable::NoLocalPath => "the shortcut points at something other than a local file",
            Unreadable::DriveOnly => "the shortcut runs a command rather than a program",
            Unreadable::NotAProgram => "the shortcut doesn't point at a program",
        }
    }
}

impl fmt::Display for Unreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

const HEADER_SIZE: u32 = 0x4C;
const LINK_CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];
const HAS_LINK_TARGET_ID_LIST: u32 = 1;
const HAS_LINK_INFO: u32 = 1 << 1;
const FORCE_NO_LINK_INFO: u32 = 1 << 8;
/// `LinkInfo` carries a local path, as opposed to only a network one.
const VOLUME_ID_AND_LOCAL_BASE_PATH: u32 = 1;
/// The `LinkInfo` header size at which the unicode path offsets are present.
const UNICODE_OFFSETS_AT: u32 = 0x24;

/// The Windows path a shortcut points at, such as `C:\Program Files\A\a.exe`.
pub fn parse(bytes: &[u8]) -> Result<String, Unreadable> {
    if u32_at(bytes, 0) != Some(HEADER_SIZE) || bytes.get(4..20) != Some(&LINK_CLSID[..]) {
        return Err(Unreadable::NotAShortcut);
    }
    let flags = u32_at(bytes, 20).ok_or(Unreadable::Truncated)?;

    let mut at = HEADER_SIZE as usize;
    if flags & HAS_LINK_TARGET_ID_LIST != 0 {
        // The list holds shell items wineo has no use for, so its length is
        // read only to step over it.
        let size = u16_at(bytes, at).ok_or(Unreadable::Truncated)?;
        at += 2 + usize::from(size);
    }
    if flags & HAS_LINK_INFO == 0 || flags & FORCE_NO_LINK_INFO != 0 {
        return Err(Unreadable::NoLinkInfo);
    }

    // Every offset in LinkInfo is relative to its own start.
    let info = bytes.get(at..).ok_or(Unreadable::Truncated)?;
    // Checked against the declared size up front, so a file cut off inside a
    // path is rejected rather than read as a shorter path.
    let info_size = u32_at(info, 0).ok_or(Unreadable::Truncated)?;
    if u64::from(info_size) > info.len() as u64 {
        return Err(Unreadable::Truncated);
    }
    let header_size = u32_at(info, 4).ok_or(Unreadable::Truncated)?;
    let info_flags = u32_at(info, 8).ok_or(Unreadable::Truncated)?;
    if info_flags & VOLUME_ID_AND_LOCAL_BASE_PATH == 0 {
        return Err(Unreadable::NoLocalPath);
    }
    let base_offset = u32_at(info, 16).ok_or(Unreadable::Truncated)?;
    let suffix_offset = u32_at(info, 24).ok_or(Unreadable::Truncated)?;
    // The unicode fields only exist on the longer header, and win when they do,
    // since the others are in whatever code page wrote the file.
    let (base_unicode, suffix_unicode) = if header_size >= UNICODE_OFFSETS_AT {
        (u32_at(info, 28), u32_at(info, 32))
    } else {
        (None, None)
    };

    let base = string_at(info, base_unicode, Encoding::Utf16)
        .or_else(|| string_at(info, Some(base_offset), Encoding::Ansi))
        .ok_or(Unreadable::NoLocalPath)?;
    let suffix = string_at(info, suffix_unicode, Encoding::Utf16)
        .or_else(|| string_at(info, Some(suffix_offset), Encoding::Ansi))
        .unwrap_or_default();

    let path = if suffix.is_empty() {
        base
    } else {
        format!("{}\\{suffix}", base.trim_end_matches('\\'))
    };
    // A bare drive means the real target is a shell item in the ID list, which
    // is what a shortcut to a command line looks like. Seen on GOG installers,
    // which point one at cmd.exe with arguments.
    if path.trim_end_matches('\\').len() <= 2 {
        return Err(Unreadable::DriveOnly);
    }
    if !is_program(&path) {
        return Err(Unreadable::NotAProgram);
    }
    Ok(path)
}

/// What wineo can make a target out of. A shortcut to a document or a folder
/// is a real shortcut, just not one worth offering.
fn is_program(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".exe") || lower.ends_with(".msi")
}

enum Encoding {
    Ansi,
    Utf16,
}

/// A null-terminated string at `offset`, where zero means the field is absent.
/// A string with no terminator in `info` is treated as absent, since what it
/// holds is only as much of the path as survived.
fn string_at(info: &[u8], offset: Option<u32>, encoding: Encoding) -> Option<String> {
    let at = usize::try_from(offset?).ok().filter(|&at| at != 0)?;
    let rest = info.get(at..)?;
    let text = match encoding {
        // Not a real code page conversion. Installer paths are ASCII, and
        // latin1 at least keeps a stray high byte from losing the rest.
        Encoding::Ansi => {
            let end = rest.iter().position(|&byte| byte == 0)?;
            rest[..end].iter().copied().map(char::from).collect()
        }
        Encoding::Utf16 => {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            let end = units.iter().position(|&unit| unit == 0)?;
            String::from_utf16(&units[..end]).ok()?
        }
    };
    Some(text).filter(|text: &String| !text.is_empty())
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSI_HEADER: u32 = 0x1C;

    /// Assembles the parts of a `.lnk` this module reads, so each test shows
    /// what it varies rather than a wall of bytes.
    struct Fixture {
        flags: u32,
        id_list: usize,
        info_flags: u32,
        base: String,
        suffix: String,
        unicode: bool,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                flags: HAS_LINK_TARGET_ID_LIST | HAS_LINK_INFO,
                id_list: 8,
                info_flags: VOLUME_ID_AND_LOCAL_BASE_PATH,
                base: r"C:\Program Files\A\a.exe".to_owned(),
                suffix: String::new(),
                unicode: false,
            }
        }
    }

    impl Fixture {
        fn base(path: &str) -> Self {
            Self {
                base: path.to_owned(),
                ..Self::default()
            }
        }

        fn build(&self) -> Vec<u8> {
            let header_size = if self.unicode {
                UNICODE_OFFSETS_AT
            } else {
                ANSI_HEADER
            };
            let mut info = vec![0u8; header_size as usize];
            let base_at = append(&mut info, &self.base, self.unicode);
            let suffix_at = append(&mut info, &self.suffix, self.unicode);
            // A unicode fixture zeroes the ansi offsets, so a test that reads
            // the path proves the unicode fields are what was used.
            let (ansi, wide) = if self.unicode {
                ((0, 0), (base_at, suffix_at))
            } else {
                ((base_at, suffix_at), (0, 0))
            };
            let size = u32::try_from(info.len()).unwrap();
            put(&mut info, 0, size);
            put(&mut info, 4, header_size);
            put(&mut info, 8, self.info_flags);
            put(&mut info, 16, ansi.0);
            put(&mut info, 24, ansi.1);
            if self.unicode {
                put(&mut info, 28, wide.0);
                put(&mut info, 32, wide.1);
            }

            let mut out = Vec::new();
            out.extend(HEADER_SIZE.to_le_bytes());
            out.extend(LINK_CLSID);
            out.extend(self.flags.to_le_bytes());
            // Nothing past the flags is read, so the rest of the header is zeros.
            out.resize(HEADER_SIZE as usize, 0);
            if self.flags & HAS_LINK_TARGET_ID_LIST != 0 {
                out.extend(u16::try_from(self.id_list).unwrap().to_le_bytes());
                // Filled with bytes that would parse as something if the list
                // were not stepped over.
                out.resize(out.len() + self.id_list, 0xAA);
            }
            out.extend(info);
            out
        }
    }

    fn append(info: &mut Vec<u8>, text: &str, unicode: bool) -> u32 {
        let at = u32::try_from(info.len()).unwrap();
        if unicode {
            info.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            info.extend([0, 0]);
        } else {
            info.extend(text.bytes());
            info.push(0);
        }
        at
    }

    fn put(info: &mut [u8], at: usize, value: u32) {
        info[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn reads_a_local_path() {
        let lnk = Fixture::default().build();
        assert_eq!(parse(&lnk).unwrap(), r"C:\Program Files\A\a.exe");
    }

    #[test]
    fn steps_over_any_id_list() {
        for id_list in [0, 1, 8, 300] {
            let lnk = Fixture {
                id_list,
                ..Fixture::default()
            }
            .build();
            assert_eq!(parse(&lnk).unwrap(), r"C:\Program Files\A\a.exe");
        }
        // A shortcut with no list at all still has to find LinkInfo.
        let lnk = Fixture {
            flags: HAS_LINK_INFO,
            ..Fixture::default()
        }
        .build();
        assert_eq!(parse(&lnk).unwrap(), r"C:\Program Files\A\a.exe");
    }

    #[test]
    fn prefers_the_unicode_path() {
        let lnk = Fixture {
            base: r"C:\Jeux\Café\game.exe".to_owned(),
            unicode: true,
            ..Fixture::default()
        }
        .build();
        assert_eq!(parse(&lnk).unwrap(), r"C:\Jeux\Café\game.exe");
    }

    #[test]
    fn joins_a_common_path_suffix() {
        let lnk = Fixture {
            base: r"C:\Program Files\".to_owned(),
            suffix: r"A\a.exe".to_owned(),
            ..Fixture::default()
        }
        .build();
        assert_eq!(parse(&lnk).unwrap(), r"C:\Program Files\A\a.exe");
    }

    #[test]
    fn accepts_an_installer() {
        let lnk = Fixture::base(r"C:\Program Files\A\setup.MSI").build();
        assert_eq!(parse(&lnk).unwrap(), r"C:\Program Files\A\setup.MSI");
    }

    // Real GOG shortcuts do this: the path is a bare drive and the command
    // they actually run lives in the ID list.
    #[test]
    fn skips_a_bare_drive() {
        for base in ["C:", r"C:\"] {
            let lnk = Fixture::base(base).build();
            assert_eq!(parse(&lnk), Err(Unreadable::DriveOnly));
        }
    }

    #[test]
    fn skips_a_target_that_is_not_a_program() {
        let lnk = Fixture::base(r"C:\Program Files\A\readme.txt").build();
        assert_eq!(parse(&lnk), Err(Unreadable::NotAProgram));
    }

    #[test]
    fn skips_a_shortcut_with_no_usable_path() {
        let no_info = Fixture {
            flags: HAS_LINK_TARGET_ID_LIST,
            ..Fixture::default()
        }
        .build();
        assert_eq!(parse(&no_info), Err(Unreadable::NoLinkInfo));

        let forced = Fixture {
            flags: HAS_LINK_TARGET_ID_LIST | HAS_LINK_INFO | FORCE_NO_LINK_INFO,
            ..Fixture::default()
        }
        .build();
        assert_eq!(parse(&forced), Err(Unreadable::NoLinkInfo));

        let network = Fixture {
            info_flags: 0,
            ..Fixture::default()
        }
        .build();
        assert_eq!(parse(&network), Err(Unreadable::NoLocalPath));

        let empty = Fixture::base("").build();
        assert_eq!(parse(&empty), Err(Unreadable::NoLocalPath));
    }

    #[test]
    fn rejects_files_that_are_not_shortcuts() {
        assert_eq!(parse(&[]), Err(Unreadable::NotAShortcut));
        assert_eq!(parse(b"MZ\x90\x00garbage"), Err(Unreadable::NotAShortcut));
        // Right size, wrong CLSID.
        let mut lnk = Fixture::default().build();
        lnk[4] = 0x02;
        assert_eq!(parse(&lnk), Err(Unreadable::NotAShortcut));
    }

    // A shortcut cut off anywhere has to report it, never panic, since these
    // files come from whatever wrote them.
    #[test]
    fn never_panics_on_a_truncated_shortcut() {
        let lnk = Fixture::default().build();
        for end in 0..lnk.len() {
            assert!(
                parse(&lnk[..end]).is_err(),
                "accepted {end} of {} bytes",
                lnk.len()
            );
        }
        assert!(parse(&lnk).is_ok());
    }
}
