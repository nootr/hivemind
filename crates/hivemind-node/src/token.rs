use crate::secret_file::write_new_secret;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("token io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("token randomness error: {0}")]
    Random(String),

    #[error("invalid api token file")]
    InvalidToken,
}

pub fn load_or_create_token(path: impl AsRef<Path>) -> Result<String, TokenError> {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(token) => validate_token(token.trim()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let token = generate_token()?;
            write_new_secret(path, format!("{token}\n").as_bytes())?;
            Ok(token)
        }
        Err(err) => Err(TokenError::Io(err)),
    }
}

fn validate_token(token: &str) -> Result<String, TokenError> {
    if token.len() == 64 && token.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        Ok(token.to_owned())
    } else {
        Err(TokenError::InvalidToken)
    }
}

fn generate_token() -> Result<String, TokenError> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|err| TokenError::Random(err.to_string()))?;
    Ok(hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_generate_load_is_idempotent() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("api.token");

        let first = load_or_create_token(&path).unwrap();
        let second = load_or_create_token(&path).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn empty_token_file_is_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("api.token");
        std::fs::write(&path, "\n").unwrap();

        assert!(matches!(
            load_or_create_token(&path),
            Err(TokenError::InvalidToken)
        ));
    }

    #[test]
    fn invalid_token_file_is_rejected() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("api.token");
        std::fs::write(&path, "not-hex\n").unwrap();

        assert!(matches!(
            load_or_create_token(&path),
            Err(TokenError::InvalidToken)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn created_token_file_is_owner_only() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("api.token");

        load_or_create_token(&path).unwrap();

        crate::secret_file::assert_secret_file_mode(&path);
    }
}
