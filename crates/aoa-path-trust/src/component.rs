use std::path::{Component, Path};

use super::UnsafePathComponent;

/// Accept `name` only when joining it to a trusted base cannot escape that base.
///
/// The explicit separator and NUL checks close platform differences: `\` is an
/// ordinary character on Unix but a separator elsewhere, while a trailing `/`
/// can otherwise collapse into one normal component.
pub fn validate_single_component(name: &str) -> Result<(), UnsafePathComponent> {
    let mut components = Path::new(name).components();
    let single_normal =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

    if single_normal && !name.contains('/') && !name.contains('\\') && !name.contains('\0') {
        Ok(())
    } else {
        Err(UnsafePathComponent {
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_single_segments() {
        for name in ["trace.json", "requests", "a-b_c.9", "..hidden"] {
            assert!(validate_single_component(name).is_ok(), "{name:?}");
        }
    }

    #[test]
    fn rejects_escape_shapes_and_cross_platform_separators() {
        for name in [
            "",
            ".",
            "..",
            "/abs",
            "../escape",
            "sub/dir",
            "trailing/",
            "back\\slash",
            "nul\0byte",
        ] {
            assert!(validate_single_component(name).is_err(), "{name:?}");
        }
    }
}
