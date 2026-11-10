//! Fonts built from the bundled faces, so tests never depend on what the
//! machine has installed.

fn be16(data: &[u8], at: usize) -> usize {
    usize::from(u16::from_be_bytes([data[at], data[at + 1]]))
}

fn table_record(data: &[u8], tag: &[u8; 4]) -> usize {
    (0..be16(data, 4))
        .map(|n| 12 + n * 16)
        .find(|start| &data[*start..*start + 4] == tag)
        .expect("table")
}

/// A copy of a bundled font standing in for a system face.
pub(crate) fn fake(font: &[u8], family: &str, weight: u16) -> Vec<u8> {
    let mut data = font.to_vec();

    let os2 = table_record(&data, b"OS/2");
    let offset = u32::from_be_bytes(data[os2 + 8..os2 + 12].try_into().unwrap()) as usize;
    data[offset + 4..offset + 6].copy_from_slice(&weight.to_be_bytes());

    // A new name table at the end, with the family as ids 1 and 16 and
    // the PostScript name fontdb insists on as id 6.
    let utf16 =
        |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_be_bytes).collect() };
    let names = [
        (1u16, utf16(family)),
        (6, utf16("Fake")),
        (16, utf16(family)),
    ];
    let mut name = Vec::new();
    for value in [0u16, names.len() as u16, 6 + 12 * names.len() as u16] {
        name.extend_from_slice(&value.to_be_bytes());
    }
    let mut strings = Vec::new();
    for (id, text) in &names {
        let at = strings.len() as u16;
        for value in [3u16, 1, 0x409, *id, text.len() as u16, at] {
            name.extend_from_slice(&value.to_be_bytes());
        }
        strings.extend_from_slice(text);
    }
    name.extend_from_slice(&strings);
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let record = table_record(&data, b"name");
    let start = u32::try_from(data.len()).unwrap();
    data[record + 8..record + 12].copy_from_slice(&start.to_be_bytes());
    data[record + 12..record + 16].copy_from_slice(&(name.len() as u32).to_be_bytes());
    data.extend_from_slice(&name);
    data
}

/// Makes a face look like a variable font by tagging an unused table `fvar`.
pub(crate) fn variable(mut data: Vec<u8>) -> Vec<u8> {
    let record = table_record(&data, b"gasp");
    data[record..record + 4].copy_from_slice(b"fvar");
    data
}

/// `fake` with every advance doubled, so its text lays out twice as wide as
/// Noto Sans and a switch to it shows in any measurement.
pub(crate) fn wide(font: &[u8], family: &str, weight: u16) -> Vec<u8> {
    let mut data = fake(font, family, weight);
    let hhea = table_offset(&data, b"hhea");
    let metrics = be16(&data, hhea + 34);
    let hmtx = table_offset(&data, b"hmtx");
    for glyph in 0..metrics {
        let at = hmtx + glyph * 4;
        let advance = u16::from_be_bytes([data[at], data[at + 1]]);
        data[at..at + 2].copy_from_slice(&advance.saturating_mul(2).to_be_bytes());
    }
    data
}

fn table_offset(data: &[u8], tag: &[u8; 4]) -> usize {
    let record = table_record(data, tag);
    u32::from_be_bytes(data[record + 8..record + 12].try_into().unwrap()) as usize
}

/// Noto Sans with every advance doubled, and only a regular face.
pub(crate) const WIDE: &str = "Wineo Test Wide";

/// Adds `WIDE` to the font system once per process. Other tests never ask
/// for it, and sans-serif still points at the bundle, so they measure as
/// before.
pub(crate) fn install_wide() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        super::install_once();
        super::font_system()
            .write()
            .expect("font system lock")
            .load_font(std::borrow::Cow::Owned(wide(super::REGULAR, WIDE, 400)));
    });
}

/// `fake` with a character map that maps nothing, like a CJK or Arabic font
/// with no Latin letters to draw its name.
pub(crate) fn no_latin(font: &[u8], family: &str, weight: u16) -> Vec<u8> {
    let mut data = fake(font, family, weight);
    // One format 4 subtable holding only the closing 0xFFFF segment.
    let mut cmap = Vec::new();
    for value in [0u16, 1, 3, 1] {
        cmap.extend_from_slice(&value.to_be_bytes());
    }
    cmap.extend_from_slice(&12u32.to_be_bytes());
    for value in [4u16, 24, 0, 2, 2, 0, 0, 0xFFFF, 0, 0xFFFF, 1, 0] {
        cmap.extend_from_slice(&value.to_be_bytes());
    }
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let record = table_record(&data, b"cmap");
    let start = u32::try_from(data.len()).unwrap();
    data[record + 8..record + 12].copy_from_slice(&start.to_be_bytes());
    data[record + 12..record + 16].copy_from_slice(&(cmap.len() as u32).to_be_bytes());
    data.extend_from_slice(&cmap);
    data
}

/// Noto Sans with no characters mapped, and only a regular face.
pub(crate) const NO_LATIN: &str = "Wineo Test No Latin";

/// Adds `NO_LATIN` to the font system once per process. Nothing maps to it,
/// so no other text ever falls back to it.
pub(crate) fn install_no_latin() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        super::install_once();
        super::font_system()
            .write()
            .expect("font system lock")
            .load_font(std::borrow::Cow::Owned(no_latin(
                super::REGULAR,
                NO_LATIN,
                400,
            )));
    });
}
