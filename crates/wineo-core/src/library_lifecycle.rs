use crate::paths::Paths;

pub(crate) fn fresh_warning(paths: &Paths, name: &str) -> Option<String> {
    crate::library::fresh(paths, name).err().map(|error| {
        format!(
            "Can't update library.toml: {}. {name} may show an old star or hidden flag once it's repaired.",
            crate::error::chain(&error)
        )
    })
}
