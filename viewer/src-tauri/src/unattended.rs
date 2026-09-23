use keyring::Entry;

pub const SERVICE: &str = "opendesk";
pub const USER: &str = "unattended";

pub fn set_password(password: &str) -> Result<(), String> {
    Entry::new(SERVICE, USER)
        .map_err(|error| error.to_string())?
        .set_password(password)
        .map_err(|error| error.to_string())
}

pub fn get_password() -> Result<String, String> {
    Entry::new(SERVICE, USER)
        .map_err(|error| error.to_string())?
        .get_password()
        .map_err(|error| error.to_string())
}

pub fn delete_password() -> Result<(), String> {
    Entry::new(SERVICE, USER)
        .map_err(|error| error.to_string())?
        .delete_credential()
        .map_err(|error| error.to_string())
}

pub fn derive_has_password(config_marker: bool, keychain_has_password: Option<bool>) -> bool {
    keychain_has_password.unwrap_or(config_marker)
}

pub fn has_password(config_marker: bool) -> bool {
    derive_has_password(config_marker, get_password().ok().map(|_| true))
}

#[cfg(test)]
mod tests {
    use super::derive_has_password;

    #[test]
    fn status_uses_keychain_when_available_and_marker_as_fallback() {
        assert!(derive_has_password(false, Some(true)));
        assert!(!derive_has_password(true, Some(false)));
        assert!(derive_has_password(true, None));
        assert!(!derive_has_password(false, None));
    }
}
