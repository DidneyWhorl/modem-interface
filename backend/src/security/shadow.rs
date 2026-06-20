//! OpenWRT /etc/shadow password verification.
//!
//! Authenticates users (primarily root) against the system shadow file,
//! the same way SSH does. Supports MD5-crypt ($1$), SHA-256 ($5$),
//! and SHA-512 ($6$) hash formats used by OpenWRT.

/// Verify a password against /etc/shadow for the given username.
///
/// Returns `Ok(true)` if password matches, `Ok(false)` if it doesn't,
/// or `Err` if shadow file is unreadable or user not found.
pub fn verify_shadow_password(username: &str, password: &str) -> Result<bool, String> {
    let shadow_content = std::fs::read_to_string("/etc/shadow")
        .map_err(|e| format!("Cannot read /etc/shadow: {e}"))?;

    verify_against_shadow_content(username, password, &shadow_content)
}

/// Verify password against shadow file content (testable without /etc/shadow).
fn verify_against_shadow_content(
    username: &str,
    password: &str,
    shadow_content: &str,
) -> Result<bool, String> {
    for line in shadow_content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 2 {
            continue;
        }

        if fields[0] != username {
            continue;
        }

        let hash = fields[1];

        // Empty or single-char hash means no password / locked
        if hash.is_empty() || hash == "!" || hash == "*" || hash == "x" {
            return Err(format!("Account '{username}' has no password or is locked"));
        }

        // Use pwhash to verify against the stored hash
        return Ok(pwhash::unix::verify(password, hash));
    }

    Err(format!("User '{username}' not found in shadow file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // MD5-crypt hash for password "internet"
    // Generated with: openssl passwd -1 -salt testSalt internet
    const TEST_SHADOW_MD5: &str =
        "root:$1$testSalt$fdGbqLWApXIfi06hON.Ad1:19000:0:99999:7:::\n\
         nobody:*:0:0:99999:7:::\n";

    #[test]
    fn test_correct_password_md5() {
        let result = verify_against_shadow_content("root", "internet", TEST_SHADOW_MD5);
        assert_eq!(result, Ok(true));
    }

    #[test]
    fn test_wrong_password() {
        let result = verify_against_shadow_content("root", "wrongpass", TEST_SHADOW_MD5);
        assert_eq!(result, Ok(false));
    }

    #[test]
    fn test_user_not_found() {
        let result = verify_against_shadow_content("nonexistent", "pass", TEST_SHADOW_MD5);
        assert!(result.is_err());
    }

    #[test]
    fn test_locked_account() {
        let shadow = "locked:!:19000:0:99999:7:::\n";
        let result = verify_against_shadow_content("locked", "anypass", shadow);
        assert!(result.is_err());
    }

    #[test]
    fn test_no_password_account() {
        let shadow = "nopw::19000:0:99999:7:::\n";
        let result = verify_against_shadow_content("nopw", "anypass", shadow);
        assert!(result.is_err());
    }
}
