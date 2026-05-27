use crate::NodeError;
use hivemind_core::NodeKey;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

pub fn load_or_create_key(path: &Path) -> Result<NodeKey, NodeError> {
    if path.exists() {
        restrict_key_permissions(path)?;
        return Ok(NodeKey::from_seed_hex(&fs::read_to_string(path)?)?);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let key = NodeKey::generate()?;
    write_private_key(path, &format!("{}\n", key.seed_hex()))?;
    Ok(key)
}

#[cfg(unix)]
fn write_private_key(path: &Path, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    restrict_key_permissions(path)
}

#[cfg(not(unix))]
fn write_private_key(path: &Path, content: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(content.as_bytes())
}

#[cfg(unix)]
fn restrict_key_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn restrict_key_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub fn create_private_state_file_if_missing(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn create_private_state_file_if_missing(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().write(true).create_new(true).open(path)?;
    Ok(())
}

#[cfg(unix)]
pub fn restrict_state_permissions(path: &Path) -> std::io::Result<()> {
    restrict_key_permissions(path)
}

#[cfg(not(unix))]
pub fn restrict_state_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
