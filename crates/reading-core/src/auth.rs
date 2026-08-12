//! Passwords and sessions.
//!
//! Two credentials with opposite requirements, and they get opposite
//! treatment:
//!
//! A **password** is chosen by a person, so it is low-entropy and probably
//! reused. It is hashed with argon2id, which is slow and memory-hard on
//! purpose: a stolen database must not be runnable through a GPU at billions
//! of guesses a second.
//!
//! A **session token** is 256 bits of CSPRNG output with no structure to
//! guess, so there is nothing for a slow hash to defend against. It is stored
//! as a plain SHA-256 digest, which is fast — and this runs on every single
//! request. Hashing it at all is what stops a database read from yielding
//! working credentials.

// The RNG comes from argon2's own re-export rather than the `rand` crate.
// argon2 0.5 carries rand_core 0.6; `rand` 0.9 carries rand_core 0.9, and the
// two `CryptoRng` traits are different types that do not satisfy each other.
// Taking the one argon2 already depends on removes the question.
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use sha2::{Digest, Sha256};

use crate::db::pg::{Db, User, UserId};
use crate::error::{AppError, Result};

/// How long a session lasts without being used.
///
/// Long enough that a reader is not signed out mid-book, short enough that a
/// token copied off an old machine stops working. Every authenticated request
/// pushes it forward.
pub const SESSION_DAYS: i64 = 30;

/// The shortest password accepted.
///
/// Length is the only requirement. Composition rules — a digit, a symbol, a
/// capital — push people towards `Password1!` and are worse than useless;
/// NIST dropped them for that reason.
pub const MIN_PASSWORD_LEN: usize = 10;

/// A token as issued, which the caller must hand to the user immediately: it
/// is not recoverable from the database afterwards.
#[derive(Debug, Clone)]
pub struct IssuedSession {
    /// The bearer token, in full. Shown once.
    pub token: String,
    pub user: User,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Hash a password for storage.
///
/// Returns a PHC string, which carries the salt and the cost parameters
/// alongside the digest — so the parameters can be raised later without
/// invalidating hashes made under the old ones.
pub fn hash_password(password: &str) -> Result<String> {
    check_password_quality(password)?;

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Other(anyhow::anyhow!("could not hash password: {e}")))
}

/// Check a password against a stored hash.
///
/// A malformed stored hash reports `false` rather than an error: it means
/// this account cannot be signed into, which from the outside is
/// indistinguishable from a wrong password and should stay that way.
pub fn verify_password(password: &str, stored: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Reject passwords that are too short.
///
/// Deliberately not a composition rule. The only thing that reliably helps is
/// length.
pub fn check_password_quality(password: &str) -> Result<()> {
    // Counted in characters, not bytes, so a passphrase in a non-Latin script
    // is not penalised for encoding to more bytes.
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AppError::Invalid(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

/// A fresh 256-bit token, URL-safe.
fn new_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    // Straight from the operating system's CSPRNG. Not a seeded generator:
    // this is a credential.
    OsRng.fill_bytes(&mut bytes);

    // Hex rather than base64: no +, / or = to be mangled in a header, a URL,
    // or a config file on the way to wherever it ends up.
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// What is stored for a token. Never the token itself.
pub fn token_digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Register an account.
///
/// The first account on a fresh server becomes the administrator, because
/// someone has to be able to set the Ollama address and a server with no
/// administrator cannot be configured at all.
pub async fn register(
    db: &Db,
    email: &str,
    display_name: &str,
    password: &str,
) -> Result<UserId> {
    if !looks_like_email(email) {
        return Err(AppError::Invalid(format!("{email} is not an email address")));
    }
    let hash = hash_password(password)?;
    let first = !db.has_any_user().await?;
    db.create_user(email, display_name, &hash, first).await
}

/// Sign in, and issue a session.
///
/// The same message is returned whether the address is unknown or the
/// password is wrong. Distinguishing them turns the sign-in form into a
/// register of who has an account here.
pub async fn sign_in(db: &Db, email: &str, password: &str, label: &str) -> Result<IssuedSession> {
    let found = db.user_for_sign_in(email).await?;

    let Some((user, stored)) = found else {
        // Hash anyway against a dummy. Returning early for an unknown address
        // would make it measurably faster than a wrong password, and that
        // timing difference is enough to enumerate accounts.
        let _ = verify_password(password, DUMMY_HASH);
        return Err(AppError::Invalid("wrong email or password".into()));
    };

    if !verify_password(password, &stored) {
        return Err(AppError::Invalid("wrong email or password".into()));
    }

    issue_session(db, user, label).await
}

/// Issue a session for a user who has already been authenticated.
pub async fn issue_session(db: &Db, user: User, label: &str) -> Result<IssuedSession> {
    let token = new_token()?;
    let expires_at = chrono::Utc::now() + chrono::Duration::days(SESSION_DAYS);

    db.create_session(&token_digest(&token), user.id, label, expires_at)
        .await?;

    Ok(IssuedSession {
        token,
        user,
        expires_at,
    })
}

/// Resolve a bearer token to the user it belongs to.
///
/// Expired sessions do not resolve, and using one pushes its expiry forward.
pub async fn authenticate(db: &Db, token: &str) -> Result<User> {
    db.user_for_session(&token_digest(token))
        .await?
        .ok_or_else(|| AppError::Invalid("not signed in".into()))
}

pub async fn sign_out(db: &Db, token: &str) -> Result<()> {
    db.delete_session(&token_digest(token)).await
}

/// Good enough to catch a typo, and no more.
///
/// Anything stricter rejects addresses that genuinely work — the real grammar
/// permits quoted strings, plus-addressing, and new top-level domains
/// constantly. The only true test is sending mail to it.
fn looks_like_email(value: &str) -> bool {
    let value = value.trim();
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value.contains(char::is_whitespace)
}

/// A real argon2id hash of a password nobody has, so the unknown-address path
/// does the same work as the wrong-password path.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$\
                          8pI1Zt7Fq1oGf4nJqOKzXcM3sVYQhLzE5uWvNxAqBdY";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_against_its_own_hash() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("Correct horse battery", &hash));
        assert!(!verify_password("", &hash));
    }

    /// The salt is what stops one rainbow table covering every account, so two
    /// people who chose the same password must not share a digest.
    #[test]
    fn the_same_password_hashes_differently_each_time() {
        let a = hash_password("correct horse battery").unwrap();
        let b = hash_password("correct horse battery").unwrap();
        assert_ne!(a, b);
        assert!(verify_password("correct horse battery", &a));
        assert!(verify_password("correct horse battery", &b));
    }

    #[test]
    fn a_short_password_is_refused() {
        assert!(hash_password("short").is_err());
        assert!(hash_password("123456789").is_err());
        assert!(hash_password("1234567890").is_ok());
    }

    /// Counted in characters: a ten-character passphrase is ten characters
    /// whatever it encodes to.
    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        assert!(check_password_quality("aaaaaaaaa").is_err());
        assert!(check_password_quality("паролькороткий").is_ok());
    }

    /// A corrupt or truncated stored hash must read as a failed sign-in, not
    /// as an error that might be handled differently somewhere up the stack.
    #[test]
    fn a_malformed_stored_hash_just_fails() {
        assert!(!verify_password("anything", ""));
        assert!(!verify_password("anything", "not-a-phc-string"));
        assert!(!verify_password("anything", "$argon2id$broken"));
    }

    #[test]
    fn the_dummy_hash_parses() {
        // If this ever stopped being a valid PHC string, verify_password would
        // return early and the timing defence in sign_in would silently stop
        // working.
        assert!(PasswordHash::new(DUMMY_HASH).is_ok());
    }

    #[test]
    fn tokens_are_unique_and_hex() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64, "256 bits as hex");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_digest_is_not_the_token() {
        let token = new_token().unwrap();
        let digest = token_digest(&token);
        assert_eq!(digest.len(), 32);
        assert_ne!(digest, token.as_bytes());
        assert_eq!(digest, token_digest(&token), "and is stable");
    }

    #[test]
    fn obvious_non_addresses_are_refused() {
        assert!(looks_like_email("reader@example.com"));
        assert!(looks_like_email("reader+books@example.co.uk"));
        assert!(!looks_like_email("reader"));
        assert!(!looks_like_email("reader@localhost"));
        assert!(!looks_like_email("reader@.com"));
        assert!(!looks_like_email("two words@example.com"));
        assert!(!looks_like_email("@example.com"));
    }
}
