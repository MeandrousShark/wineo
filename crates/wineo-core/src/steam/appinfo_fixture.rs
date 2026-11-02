//! Tests only (and the app's through `test-support`): builds a synthetic
//! `appinfo.vdf` that holds only what the reader keeps. Never copy a real one.

/// What one app's `common` section says. `None` leaves the key out.
pub struct App<'a> {
    pub id: u32,
    pub oslist: Option<&'a str>,
    pub kind: Option<&'a str>,
}

const MAGIC: u32 = 0x0756_4429;
const META_LEN: usize = 60;

/// The bytes of a version 41 file with these apps, in order.
pub fn build(apps: &[App<'_>]) -> Vec<u8> {
    let mut strings: Vec<&'static str> = Vec::new();
    let mut index = |name: &'static str| -> i32 {
        let at = strings.iter().position(|known| *known == name);
        let at = at.unwrap_or_else(|| {
            strings.push(name);
            strings.len() - 1
        });
        i32::try_from(at).expect("few strings")
    };
    let mut entries = Vec::new();
    for app in apps {
        let mut body = vec![0xAA; META_LEN];
        for object in ["appinfo", "common"] {
            body.push(0);
            body.extend(index(object).to_le_bytes());
        }
        for (key, value) in [("type", app.kind), ("oslist", app.oslist)] {
            if let Some(value) = value {
                body.push(1);
                body.extend(index(key).to_le_bytes());
                body.extend(value.as_bytes());
                body.push(0);
            }
        }
        // Ends common, appinfo, and the level the root sits in.
        body.extend([8, 8, 8]);
        entries.extend(app.id.to_le_bytes());
        entries.extend(u32::try_from(body.len()).expect("small").to_le_bytes());
        entries.extend(body);
    }
    // The list's zero id sits before the table.
    let table_start = 16 + entries.len() + 4;
    let mut bytes = Vec::new();
    bytes.extend(MAGIC.to_le_bytes());
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(i64::try_from(table_start).expect("small").to_le_bytes());
    bytes.extend(entries);
    bytes.extend([0; 4]);
    bytes.extend(u32::try_from(strings.len()).expect("few").to_le_bytes());
    for name in strings {
        bytes.extend(name.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// A game whose `common` has this `oslist`, or none.
pub fn game(id: u32, oslist: Option<&str>) -> App<'_> {
    App {
        id,
        oslist,
        kind: Some("game"),
    }
}
