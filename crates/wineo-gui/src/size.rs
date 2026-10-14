pub(crate) use wineo_core::units::size as format_bytes;

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn formats_binary_sizes_without_trailing_zeroes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1 KiB");
        assert_eq!(format_bytes(1_288_490_189), "1.2 GiB");
        assert_eq!(format_bytes(1536 * 1024), "1.5 MiB");
        assert_eq!(format_bytes(320 * 1024 * 1024), "320 MiB");
    }
}
