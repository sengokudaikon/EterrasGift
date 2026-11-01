//! Supported ABI check before any remote work. Hashes separately bind these disk bytes
//! to the loaded module; a matching version is not proof of graph/lifetime assumptions.
use std::{io, path::Path};
use win_platform::file_version::{self, FileVersionError};

/// The executable's version resource is Unity's, the engine the supported game ships.
pub fn check_unity(path: &Path) -> io::Result<()> {
    let version = file_version::read(path).map_err(|reason| {
        io::Error::other(match reason {
            FileVersionError::Unreadable(error) => format!("Cannot read UnityPlayer version: {error}"),
            FileVersionError::TooLarge => "UnityPlayer version resource is too large".to_owned(),
            FileVersionError::NoFixedVersion => "UnityPlayer has no fixed file version".to_owned(),
            FileVersionError::InvalidPointer => "Invalid UnityPlayer version pointer".to_owned(),
            FileVersionError::InvalidResource => "Invalid UnityPlayer version resource".to_owned(),
            FileVersionError::InvalidFixedVersion => "Invalid UnityPlayer fixed file version".to_owned(),
        })
    })?;
    check_supported(version)
}

fn check_supported(version: [u32; 4]) -> io::Result<()> {
    if version[..2] != [6000, 4] {
        return Err(io::Error::other(format!(
            "Unsupported UnityPlayer {}.{}.{}.{}; the trainer supports Unity 6000.4. No helper was loaded.",
            version[0], version[1], version[2], version[3]
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_supported_engine_family_passes() {
        for (major, minor, supported) in [(6000, 4, true), (6000, 3, false), (6000, 5, false), (2022, 4, false)] {
            assert_eq!(check_supported([major, minor, 8, 1]).is_ok(), supported);
        }
    }

    #[test]
    fn missing_or_versionless_files_fail_closed() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("UnityPlayer.dll");
        assert!(check_unity(&path).is_err());
        std::fs::write(&path, b"not a PE version resource").unwrap();
        assert!(check_unity(&path).is_err());
    }

    #[test]
    fn windows_version_resource_is_read_but_refused_as_an_unsupported_engine() {
        let path = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/kernel32.dll");
        let error = check_unity(&path).unwrap_err();
        assert!(error.to_string().contains("Unsupported UnityPlayer"), "{error}");
    }
}
