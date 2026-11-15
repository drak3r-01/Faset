//! Placeholder binary: prints `probe <version> <target>` and exits 0.
#![forbid(unsafe_code)]

fn main() {
    println!("{}", banner());
}

fn banner() -> String {
    format!(
        "probe {} {}",
        env!("CARGO_PKG_VERSION"),
        env!("PROBE_TARGET")
    )
}

#[cfg(test)]
mod tests {
    use super::banner;

    #[test]
    fn banner_has_expected_shape() {
        let b = banner();
        let parts: Vec<&str> = b.split(' ').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "probe");
        assert!(parts[2].contains('-'), "target triple expected: {b}");
    }
}
