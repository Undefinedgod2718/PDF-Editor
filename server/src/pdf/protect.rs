//! Permission-based PDF protection (P11): owner password + permission
//! flags, empty user password so the document opens without a prompt in
//! any reader, while conforming readers enforce the print/copy/edit
//! restrictions. Real password-to-open encryption is P12, not implemented
//! here.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Permissions};

#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionFlags {
    pub print: bool,
    pub print_high_quality: bool,
    pub modify: bool,
    pub copy: bool,
    pub copy_for_accessibility: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
}

impl PermissionFlags {
    fn to_permissions(self) -> Permissions {
        let mut p = Permissions::empty();
        p.set(Permissions::PRINTABLE, self.print);
        p.set(Permissions::PRINTABLE_IN_HIGH_QUALITY, self.print_high_quality);
        p.set(Permissions::MODIFIABLE, self.modify);
        p.set(Permissions::COPYABLE, self.copy);
        p.set(
            Permissions::COPYABLE_FOR_ACCESSIBILITY,
            self.copy_for_accessibility,
        );
        p.set(Permissions::ANNOTABLE, self.annotate);
        p.set(Permissions::FILLABLE, self.fill_forms);
        p.set(Permissions::ASSEMBLABLE, self.assemble);
        p
    }

    /// Everything allowed — the default when an encrypt request omits flags
    /// (an open password already gates access; per-action limits are opt-in).
    pub fn all_allowed() -> Self {
        Self {
            print: true,
            print_high_quality: true,
            modify: true,
            copy: true,
            copy_for_accessibility: true,
            annotate: true,
            fill_forms: true,
            assemble: true,
        }
    }

    fn from_permissions(p: Permissions) -> Self {
        Self {
            print: p.contains(Permissions::PRINTABLE),
            print_high_quality: p.contains(Permissions::PRINTABLE_IN_HIGH_QUALITY),
            modify: p.contains(Permissions::MODIFIABLE),
            copy: p.contains(Permissions::COPYABLE),
            copy_for_accessibility: p.contains(Permissions::COPYABLE_FOR_ACCESSIBILITY),
            annotate: p.contains(Permissions::ANNOTABLE),
            fill_forms: p.contains(Permissions::FILLABLE),
            assemble: p.contains(Permissions::ASSEMBLABLE),
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectionStatus {
    pub protected: bool,
    pub permissions: Option<PermissionFlags>,
}

/// Caller-fixable (400) vs internal (500) errors, mirroring `sidecar::SidecarError`.
pub enum ProtectError {
    User(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for ProtectError {
    fn from(e: anyhow::Error) -> Self {
        ProtectError::Internal(e)
    }
}

impl From<lopdf::Error> for ProtectError {
    fn from(e: lopdf::Error) -> Self {
        ProtectError::Internal(e.into())
    }
}

/// Argon2id PHC hash of a password, for `storage::DocMeta::protection_hash`.
/// See that field's doc comment for why this app-level verifier exists.
///
/// A password store must resist offline attack even though our sidecar sits
/// next to the PDF: use a memory-hard KDF, not a bare digest. Each call draws
/// a fresh random salt, so identical passwords on two documents produce
/// different hashes (no cross-document reuse leak) and rainbow tables are
/// useless. The returned PHC string (`$argon2id$v=19$m=...$salt$hash`) carries
/// the salt and parameters inline; verify it with [`verify_password`].
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("password hashing failed: {e}"))
}

/// Constant-time verification of `password` against a PHC hash produced by
/// [`hash_password`]. Returns `false` for a wrong password or a malformed
/// hash; never leaks the reason (nor times differently on mismatch length).
pub fn verify_password(password: &str, phc: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    use argon2::Argon2;

    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Apply owner-password + permission-flag protection to an unencrypted PDF.
pub fn protect(
    path: &Path,
    owner_password: &str,
    flags: PermissionFlags,
) -> Result<Vec<u8>, ProtectError> {
    if owner_password.is_empty() {
        return Err(ProtectError::User(
            "owner password must not be empty".into(),
        ));
    }
    let mut doc = Document::load(path)?;
    // `was_encrypted()`: an empty-user-password PDF (our own scheme, or any
    // other tool's) auto-authenticates and gets transparently decrypted by
    // `Document::load` itself, clearing `is_encrypted()` in the process — so
    // both states mean "already has a security handler".
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(ProtectError::User(
            "document is already protected".into(),
        ));
    }

    encrypt_v4(&mut doc, "", owner_password, flags)
}

/// Encrypt with a real user (open) password (P12 密文): the output cannot be
/// opened — nor rendered by our own PDFium viewer — without the password.
/// The owner password gates permission changes; callers default it to the
/// user password when they don't want a separate one.
pub fn encrypt(
    path: &Path,
    user_password: &str,
    owner_password: &str,
    flags: PermissionFlags,
) -> Result<Vec<u8>, ProtectError> {
    if user_password.is_empty() {
        return Err(ProtectError::User("user password must not be empty".into()));
    }
    let mut doc = Document::load(path)?;
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(ProtectError::User("document is already encrypted".into()));
    }
    encrypt_v4(&mut doc, user_password, owner_password, flags)
}

/// Shared V4 (AES-128, `StdCF`) encryption for both the empty-user-password
/// permission scheme (`protect`) and the real open-password scheme
/// (`encrypt`). Returns the encrypted bytes.
fn encrypt_v4(
    doc: &mut Document,
    user_password: &str,
    owner_password: &str,
    flags: PermissionFlags,
) -> Result<Vec<u8>, ProtectError> {
    let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
    let version = EncryptionVersion::V4 {
        document: &*doc,
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password,
        user_password,
        permissions: flags.to_permissions(),
    };
    let state = EncryptionState::try_from(version).map_err(anyhow::Error::from)?;
    doc.encrypt(&state)?;

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(anyhow::Error::from)?;
    Ok(bytes)
}

/// Decrypt an open-password PDF (P12) given its password, returning a plain,
/// viewable PDF. Wrong password → `User("incorrect password")`. A document
/// with no open password (empty user password, or unencrypted) is rejected
/// as `User` rather than silently returning it, so the endpoint's contract
/// stays "this removes an open password".
pub fn decrypt(path: &Path, password: &str) -> Result<Vec<u8>, ProtectError> {
    // Must load *with* the password: a plain `Document::load` on an
    // open-password PDF cannot authenticate (the empty password fails), so it
    // returns a document with an empty object set — saving that yields a
    // near-empty file. `load_with_password` authenticates first, then
    // decrypts and populates every object.
    let mut doc = match Document::load_with_password(path, password) {
        Ok(d) => d,
        Err(lopdf::Error::InvalidPassword) => {
            return Err(ProtectError::User("incorrect password".into()));
        }
        Err(e) => return Err(e.into()),
    };
    // `was_encrypted()` is set once an /Encrypt handler has been authenticated
    // and stripped in memory. If it's clear, the input had no encryption at
    // all — nothing to remove.
    if !doc.was_encrypted() {
        return Err(ProtectError::User("document is not encrypted".into()));
    }

    // Objects are now plaintext in memory and the trailer's /Encrypt has been
    // dropped; a full save writes an unencrypted PDF.
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(anyhow::Error::from)?;
    Ok(bytes)
}

/// Remove protection.
///
/// - `is_encrypted()` still true after load: a real open-password PDF —
///   `password` is checked by `lopdf::decrypt`.
/// - `was_encrypted()` only (empty user password, auto-decrypted on load):
///   `lopdf` cannot re-check the owner password. The caller must have already
///   verified it against `DocMeta::protection_hash` and pass
///   `owner_verified = true`. Without that, refuse — otherwise a re-uploaded
///   protected PDF (hash not carried) would unprotect with any password.
pub fn unprotect(
    path: &Path,
    password: &str,
    owner_verified: bool,
) -> Result<Vec<u8>, ProtectError> {
    let mut doc = Document::load(path)?;
    if doc.is_encrypted() {
        match doc.decrypt(password) {
            Ok(()) => {}
            Err(lopdf::Error::InvalidPassword) => {
                return Err(ProtectError::User("incorrect password".into()));
            }
            Err(e) => return Err(e.into()),
        }
    } else if doc.was_encrypted() {
        if !owner_verified {
            return Err(ProtectError::User(
                "cannot verify owner password for this document".into(),
            ));
        }
        // Empty user password already auto-authenticated during load;
        // objects are plaintext. Just save a copy without /Encrypt.
    } else {
        return Err(ProtectError::User("document is not protected".into()));
    }

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(anyhow::Error::from)?;
    Ok(bytes)
}

/// Bail if `path` has a security handler. PDFium/`lopdf` loads of
/// empty-user-password PDFs auto-decrypt and drop `/Encrypt` on save —
/// every in-place rewrite must call this first or protection silently dies.
pub fn assert_editable(path: &Path) -> anyhow::Result<()> {
    let status = inspect(path)?;
    anyhow::ensure!(
        !status.protected,
        "document is protected; unprotect it before editing"
    );
    Ok(())
}

/// Read current protection state without a password: permission bits are
/// always readable without one — either via `EncryptionState` (empty user
/// password, auto-decrypted by `Document::load`) or straight from the
/// still-encrypted `/Encrypt` dictionary's cleartext `/P` entry (a real
/// password-required PDF that failed auto-auth).
pub fn inspect(path: &Path) -> anyhow::Result<ProtectionStatus> {
    let doc = Document::load(path)?;
    if let Some(state) = &doc.encryption_state {
        return Ok(ProtectionStatus {
            protected: true,
            permissions: Some(PermissionFlags::from_permissions(state.permissions())),
        });
    }
    if doc.is_encrypted() {
        let dict = doc.get_encrypted()?;
        let p = dict.get(b"P")?.as_i64()?;
        let permissions = Permissions::from_bits_truncate(p as u64);
        return Ok(ProtectionStatus {
            protected: true,
            permissions: Some(PermissionFlags::from_permissions(permissions)),
        });
    }
    Ok(ProtectionStatus {
        protected: false,
        permissions: None,
    })
}
