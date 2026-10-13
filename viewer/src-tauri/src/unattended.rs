//! Unattended access secrets live in the OS keyring (Keychain, Credential
//! Manager, Secret Service); the config only records that one was saved. The
//! keyring can be missing (Linux without a Secret Service), locked, or wiped,
//! so every read and write here turns a failure into a message the user can
//! act on, and unattended access only counts as ready after a successful read.

use crate::config::Config;
use keyring::{Entry, Error};

pub const SERVICE: &str = "opendesk";
pub const USER: &str = "unattended";
pub const TOTP_USER: &str = "unattended-totp";

const PASSWORD: &str = "the unattended access password";
const TOTP: &str = "the 2FA secret";

/// Where secrets are kept. The real one is [`Keyring`]; tests use a fake.
pub trait SecretStore {
    fn get(&self, user: &str) -> keyring::Result<String>;
    fn set(&self, user: &str, secret: &str) -> keyring::Result<()>;
    fn delete(&self, user: &str) -> keyring::Result<()>;
}

/// The platform keyring.
pub struct Keyring;

impl SecretStore for Keyring {
    fn get(&self, user: &str) -> keyring::Result<String> {
        Entry::new(SERVICE, user)?.get_password()
    }
    fn set(&self, user: &str, secret: &str) -> keyring::Result<()> {
        Entry::new(SERVICE, user)?.set_password(secret)
    }
    fn delete(&self, user: &str) -> keyring::Result<()> {
        Entry::new(SERVICE, user)?.delete_credential()
    }
}

/// What to do when the keyring can't be used on this platform.
fn keyring_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Unlock your login keychain in Keychain Access, then try again."
    } else if cfg!(windows) {
        "Make sure Windows Credential Manager is available, then try again."
    } else {
        "Install and unlock a keyring such as GNOME Keyring or KWallet, then try again."
    }
}

#[derive(Clone, Copy)]
enum Action {
    Save,
    Read,
    Remove,
}

fn keyring_failure(action: Action, what: &str, error: &Error) -> String {
    let (verb, preposition) = match action {
        Action::Save => ("save", "to"),
        Action::Read => ("read", "from"),
        Action::Remove => ("remove", "from"),
    };
    log::warn!("could not {verb} {what} {preposition} the keyring: {error}");
    let reason = if matches!(error, Error::NoDefaultStore) {
        format!("OpenDesk couldn't find a system keyring to {verb} {what}.")
    } else {
        format!("OpenDesk couldn't {verb} {what} {preposition} the system keyring.")
    };
    match action {
        Action::Read => format!("{reason} Until it can, connections need your approval. {}", keyring_hint()),
        Action::Save | Action::Remove => format!("{reason} {}", keyring_hint()),
    }
}

/// The result of reading one secret.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    Found(String),
    /// Never saved, or removed from the keyring outside OpenDesk.
    Missing,
    /// The keyring itself failed; holds the message for the user.
    Unavailable(String),
}

fn read(store: &impl SecretStore, user: &str, what: &str) -> Lookup {
    match store.get(user) {
        Ok(secret) => Lookup::Found(secret),
        Err(Error::NoEntry) => Lookup::Missing,
        Err(error) => Lookup::Unavailable(keyring_failure(Action::Read, what, &error)),
    }
}

pub fn read_password(store: &impl SecretStore) -> Lookup {
    read(store, USER, PASSWORD)
}

pub fn read_totp_secret(store: &impl SecretStore) -> Lookup {
    read(store, TOTP_USER, TOTP)
}

pub fn save_password(store: &impl SecretStore, password: &str) -> Result<(), String> {
    store.set(USER, password).map_err(|error| keyring_failure(Action::Save, PASSWORD, &error))
}

pub fn save_totp_secret(store: &impl SecretStore, secret: &str) -> Result<(), String> {
    store.set(TOTP_USER, secret).map_err(|error| keyring_failure(Action::Save, TOTP, &error))
}

/// A secret that is already gone counts as removed, so 2FA can always be
/// turned off after the keyring lost it.
pub fn delete_totp_secret(store: &impl SecretStore) -> Result<(), String> {
    match store.delete(TOTP_USER) {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(keyring_failure(Action::Remove, TOTP, &error)),
    }
}

/// Saves into the platform keyring; used by the legacy config migration.
pub fn set_password(password: &str) -> Result<(), String> {
    save_password(&Keyring, password)
}

const PASSWORD_MISSING: &str = "The unattended access password is missing from the system keyring, so connections need your approval. Enter the password again and save.";
const TOTP_MISSING: &str = "The 2FA secret is missing from the system keyring, so connections need your approval. Turn off \"Require 2FA code\" and set it up again.";

/// Unattended access as it would work right now.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Check {
    /// A password is saved. While unattended access is off, or the keyring
    /// can't be read, this is the config marker; it only picks the password
    /// field's placeholder.
    pub has_password: bool,
    /// Set only when unattended access is on and every secret it needs was
    /// read. `None` means connections fall back to manual approval.
    pub password: Option<String>,
    pub totp_secret: Option<String>,
    /// Why unattended access is on but not working, in words for the user.
    pub problem: Option<String>,
}

impl Check {
    pub fn ready(&self) -> bool {
        self.password.is_some()
    }
}

/// Read what unattended access needs. `password_marker` is the config's
/// record that a password was saved; `twofa` whether 2FA is turned on.
pub fn check(store: &impl SecretStore, enabled: bool, password_marker: bool, twofa: bool) -> Check {
    // Off means no keyring access at all, so people who never use unattended
    // access never hit a missing or locked keyring. Turning it on re-checks.
    if !enabled {
        return Check { has_password: password_marker, ..Check::default() };
    }
    let lookup = read_password(store);
    let has_password = match &lookup {
        Lookup::Found(_) => true,
        Lookup::Missing => false,
        Lookup::Unavailable(_) => password_marker,
    };
    let not_ready = |problem: String| Check { has_password, problem: Some(problem), ..Check::default() };
    let password = match lookup {
        Lookup::Found(password) => password,
        Lookup::Missing => return not_ready(PASSWORD_MISSING.into()),
        Lookup::Unavailable(message) => return not_ready(message),
    };
    if !twofa {
        return Check { has_password, password: Some(password), ..Check::default() };
    }
    match read_totp_secret(store) {
        Lookup::Found(secret) => Check { has_password, password: Some(password), totp_secret: Some(secret), problem: None },
        Lookup::Missing => not_ready(TOTP_MISSING.into()),
        Lookup::Unavailable(message) => not_ready(message),
    }
}

/// [`check`] against the platform keyring with the saved settings.
pub fn check_config(config: &Config) -> Check {
    check(&Keyring, config.unattended_enabled, config.unattended_password_set, config.unattended_twofa_enabled)
}

/// Save the unattended access settings. A new password is written first; the
/// config marker then follows what the keyring actually holds. Enabling needs
/// a password that can be read back, so a broken keyring fails here instead
/// of later, when someone tries to connect.
pub fn set_access(store: &impl SecretStore, config: &mut Config, enabled: bool, password: Option<&str>) -> Result<(), String> {
    if let Some(password) = password {
        save_password(store, password)?;
    }
    match read_password(store) {
        Lookup::Found(_) => config.unattended_password_set = true,
        Lookup::Missing if enabled => return Err("enter an unattended access password before enabling it".into()),
        Lookup::Missing => config.unattended_password_set = false,
        Lookup::Unavailable(message) if enabled => return Err(message),
        // Turning it off still works; the marker is left as it was.
        Lookup::Unavailable(_) => {}
    }
    config.unattended_enabled = enabled;
    Ok(())
}

/// Whether 2FA is on with its secret in place, so enrolling again is refused.
pub fn twofa_active(store: &impl SecretStore, config: &Config) -> bool {
    config.unattended_twofa_enabled && matches!(read_totp_secret(store), Lookup::Found(_))
}

/// Turn 2FA off. The secret is removed when 2FA was on or one is left over;
/// one the keyring already lost is fine.
pub fn disable_twofa(store: &impl SecretStore, config: &mut Config) -> Result<(), String> {
    if config.unattended_twofa_enabled || matches!(read_totp_secret(store), Lookup::Found(_)) {
        delete_totp_secret(store)?;
    }
    config.unattended_twofa_enabled = false;
    Ok(())
}

/// Store a confirmed 2FA secret and turn 2FA on.
pub fn enable_twofa(store: &impl SecretStore, config: &mut Config, secret: &str) -> Result<(), String> {
    save_totp_secret(store, secret)?;
    config.unattended_twofa_enabled = true;
    Ok(())
}

/// A 2FA secret generated by `set_unattended_2fa` but not yet confirmed with
/// a code. It is never written to the keychain until confirmation, so an
/// abandoned enrollment cannot leave 2FA half-enabled.
#[derive(Default)]
pub struct PendingTotp(pub std::sync::Mutex<Option<String>>);

impl PendingTotp {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// `otpauth://` URI with the account labelled `OpenDesk:<device id>`.
pub fn otpauth_uri(device_id: &str, secret: &str) -> String {
    format!("otpauth://totp/OpenDesk:{device_id}?secret={secret}&issuer=OpenDesk&algorithm=SHA1&digits=6&period=30")
}

/// QR code modules for `data` as (width, row-major dark flags).
pub fn qr_modules(data: &str) -> Result<(usize, Vec<bool>), String> {
    let code = qrcode::QrCode::new(data.as_bytes()).map_err(|error| error.to_string())?;
    let modules = code.to_colors().into_iter().map(|color| color == qrcode::Color::Dark).collect();
    Ok((code.width(), modules))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};

    /// In-memory keyring. Users listed in `broken` fail every call with the
    /// error `fail` builds, the way a locked or missing keyring does.
    #[derive(Default)]
    struct FakeStore {
        secrets: RefCell<HashMap<String, String>>,
        broken: Vec<&'static str>,
        fail: Option<fn() -> Error>,
    }

    impl FakeStore {
        fn with(secrets: &[(&str, &str)]) -> Self {
            let secrets = secrets.iter().map(|(user, secret)| (user.to_string(), secret.to_string())).collect();
            Self { secrets: RefCell::new(secrets), ..Self::default() }
        }
        fn broken(users: &[&'static str], fail: fn() -> Error) -> Self {
            Self { broken: users.to_vec(), fail: Some(fail), ..Self::default() }
        }
        fn failure(&self, user: &str) -> keyring::Result<()> {
            match self.fail {
                Some(fail) if self.broken.contains(&user) => Err(fail()),
                _ => Ok(()),
            }
        }
    }

    impl SecretStore for FakeStore {
        fn get(&self, user: &str) -> keyring::Result<String> {
            self.failure(user)?;
            self.secrets.borrow().get(user).cloned().ok_or(Error::NoEntry)
        }
        fn set(&self, user: &str, secret: &str) -> keyring::Result<()> {
            self.failure(user)?;
            self.secrets.borrow_mut().insert(user.into(), secret.into());
            Ok(())
        }
        fn delete(&self, user: &str) -> keyring::Result<()> {
            self.failure(user)?;
            self.secrets.borrow_mut().remove(user).map(|_| ()).ok_or(Error::NoEntry)
        }
    }

    fn settings(enabled: bool, password_set: bool, twofa: bool) -> Config {
        let mut config = Config::default();
        config.unattended_enabled = enabled;
        config.unattended_password_set = password_set;
        config.unattended_twofa_enabled = twofa;
        config
    }

    fn locked() -> Error {
        Error::NoStorageAccess("keychain is locked".into())
    }
    fn no_service() -> Error {
        Error::NoDefaultStore
    }
    fn platform_failure() -> Error {
        Error::PlatformFailure("org.freedesktop.DBus.Error.ServiceUnknown".into())
    }

    #[test]
    fn otpauth_uri_labels_account_with_device_id() {
        let uri = otpauth_uri("123456789", "ABCDEF");
        assert!(uri.starts_with("otpauth://totp/OpenDesk:123456789?secret=ABCDEF&issuer=OpenDesk"));
    }

    #[test]
    fn qr_modules_are_square() {
        let (width, modules) = qr_modules(&otpauth_uri("123456789", "JBSWY3DPEHPK3PXP")).unwrap();
        assert_eq!(modules.len(), width * width);
        assert!(modules.iter().any(|dark| *dark));
    }

    #[test]
    fn ready_with_every_secret_it_needs() {
        let store = FakeStore::with(&[(USER, "long-password-1"), (TOTP_USER, "JBSWY3DPEHPK3PXP")]);
        let check = check(&store, true, true, true);
        assert!(check.ready());
        assert!(check.has_password);
        assert_eq!(check.password.as_deref(), Some("long-password-1"));
        assert_eq!(check.totp_secret.as_deref(), Some("JBSWY3DPEHPK3PXP"));
        assert_eq!(check.problem, None);
    }

    #[test]
    fn a_wiped_password_is_not_ready_even_when_the_config_says_set() {
        let check = check(&FakeStore::default(), true, true, false);
        assert!(!check.ready());
        assert!(!check.has_password, "the marker must not hide a missing secret");
        assert!(check.problem.unwrap().contains("Enter the password again"));
    }

    #[test]
    fn a_wiped_2fa_secret_is_not_ready() {
        let store = FakeStore::with(&[(USER, "long-password-1")]);
        let check = check(&store, true, true, true);
        assert!(!check.ready());
        assert!(check.has_password);
        assert!(check.problem.unwrap().contains("Require 2FA code"));
    }

    #[test]
    fn a_locked_keyring_is_not_ready_and_says_why() {
        for fail in [locked as fn() -> Error, no_service, platform_failure] {
            let check = check(&FakeStore::broken(&[USER], fail), true, true, false);
            assert!(!check.ready());
            assert!(check.has_password, "falls back to the marker when the keyring can't answer");
            let problem = check.problem.unwrap();
            assert!(problem.contains("system keyring"), "{problem}");
            assert!(problem.contains("approval"), "{problem}");
            assert!(problem.ends_with("then try again."), "{problem}");
        }
    }

    #[test]
    fn an_unreadable_2fa_secret_is_not_ready() {
        let mut store = FakeStore::broken(&[TOTP_USER], locked);
        store.secrets.get_mut().insert(USER.into(), "long-password-1".into());
        let check = check(&store, true, true, true);
        assert!(!check.ready());
        assert!(check.password.is_none(), "must not run unattended without the 2FA secret");
        assert!(check.problem.unwrap().contains("2FA secret"));
    }

    #[test]
    fn disabled_reports_no_problem() {
        assert_eq!(check(&FakeStore::broken(&[USER], no_service), false, false, false), Check::default());
        let check = check(&FakeStore::broken(&[USER], no_service), false, true, true);
        assert_eq!(check, Check { has_password: true, ..Check::default() });
    }

    #[test]
    fn save_failures_explain_what_to_do() {
        let store = FakeStore::broken(&[USER, TOTP_USER], no_service);
        let error = save_password(&store, "long-password-1").unwrap_err();
        assert!(error.starts_with("OpenDesk couldn't find a system keyring to save the unattended access password."), "{error}");
        let error = save_totp_secret(&FakeStore::broken(&[TOTP_USER], locked), "SECRET").unwrap_err();
        assert!(error.starts_with("OpenDesk couldn't save the 2FA secret to the system keyring."), "{error}");
    }

    #[test]
    fn enabling_fails_with_the_keyring_message_and_leaves_config_alone() {
        let mut config = Config::default();
        let error = set_access(&FakeStore::broken(&[USER], no_service), &mut config, true, Some("long-password-1")).unwrap_err();
        assert!(error.contains("system keyring"), "{error}");
        assert!(!config.unattended_enabled);
        assert!(!config.unattended_password_set);

        let error = set_access(&FakeStore::broken(&[USER], locked), &mut config, true, None).unwrap_err();
        assert!(error.contains("system keyring"), "{error}");
        assert!(!config.unattended_enabled);
    }

    #[test]
    fn enabling_saves_and_marks_the_password() {
        let store = FakeStore::default();
        let mut config = Config::default();
        assert_eq!(
            set_access(&store, &mut config, true, None),
            Err("enter an unattended access password before enabling it".into())
        );
        set_access(&store, &mut config, true, Some("long-password-1")).unwrap();
        assert!(config.unattended_enabled && config.unattended_password_set);
        assert!(check(&store, true, true, false).ready());
    }

    #[test]
    fn turning_off_works_with_a_broken_keyring() {
        let mut config = settings(true, true, false);
        set_access(&FakeStore::broken(&[USER], locked), &mut config, false, None).unwrap();
        assert!(!config.unattended_enabled);
        assert!(config.unattended_password_set, "unknown, so the marker stays");

        let mut config = settings(true, true, false);
        set_access(&FakeStore::default(), &mut config, false, None).unwrap();
        assert!(!config.unattended_password_set, "a wiped password clears the marker");
    }

    #[test]
    fn twofa_can_be_turned_off_after_the_keyring_lost_its_secret() {
        let store = FakeStore::with(&[(USER, "long-password-1")]);
        let mut config = settings(true, false, true);
        assert!(!twofa_active(&store, &config), "so enrolling again is allowed");
        disable_twofa(&store, &mut config).unwrap();
        assert!(!config.unattended_twofa_enabled);
    }

    #[test]
    fn twofa_stays_on_when_the_keyring_cannot_remove_it() {
        let mut config = settings(false, false, true);
        assert!(disable_twofa(&FakeStore::broken(&[TOTP_USER], locked), &mut config).is_err());
        assert!(config.unattended_twofa_enabled);
        // Cancelling an enrollment when 2FA was never on doesn't touch it.
        let mut config = Config::default();
        assert_eq!(disable_twofa(&FakeStore::broken(&[TOTP_USER], locked), &mut config), Ok(()));
    }

    #[test]
    fn twofa_is_only_turned_on_once_its_secret_is_saved() {
        let mut config = Config::default();
        assert!(enable_twofa(&FakeStore::broken(&[TOTP_USER], no_service), &mut config, "SECRET").is_err());
        assert!(!config.unattended_twofa_enabled);
        let store = FakeStore::default();
        enable_twofa(&store, &mut config, "SECRET").unwrap();
        assert!(config.unattended_twofa_enabled && twofa_active(&store, &config));
    }

    #[test]
    fn deleting_a_missing_2fa_secret_succeeds() {
        assert_eq!(delete_totp_secret(&FakeStore::default()), Ok(()));
        let store = FakeStore::with(&[(TOTP_USER, "SECRET")]);
        assert_eq!(delete_totp_secret(&store), Ok(()));
        assert_eq!(read_totp_secret(&store), Lookup::Missing);
        let error = delete_totp_secret(&FakeStore::broken(&[TOTP_USER], locked)).unwrap_err();
        assert!(error.starts_with("OpenDesk couldn't remove the 2FA secret from the system keyring."), "{error}");
    }
}
