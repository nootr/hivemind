use std::{fs::OpenOptions, io::Write, path::Path};

pub fn write_new_secret(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    ensure_parent_dir(path)?;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
pub fn assert_secret_file_mode(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
