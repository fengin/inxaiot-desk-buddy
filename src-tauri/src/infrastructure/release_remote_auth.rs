use crate::application::ports::remote_session::RemoteAuth;
use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;
use crate::formal::credential_crypto::ReleaseCredentials;

pub(crate) fn release_remote_auth(credentials: &ReleaseCredentials) -> AppResult<RemoteAuth> {
    if let Some(private_key) = credentials
        .ssh_private_key
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return Ok(RemoteAuth::PrivateKey {
            username: credentials.ssh_user.clone(),
            private_key: SecretValue::new(private_key),
            passphrase: None,
        });
    }
    let password = credentials
        .ssh_password
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidConfig("SSH凭据为空".into()))?;
    Ok(RemoteAuth::Password {
        username: credentials.ssh_user.clone(),
        password: SecretValue::new(password),
    })
}

#[cfg(test)]
mod tests {
    use crate::application::ports::remote_session::RemoteAuth;
    use crate::formal::credential_crypto::ReleaseCredentials;

    use super::release_remote_auth;

    fn credentials() -> ReleaseCredentials {
        ReleaseCredentials {
            platform_auth_key: "auth-key".into(),
            platform_mqtt_user: "platform-user".into(),
            platform_mqtt_password: "platform-password".into(),
            aio_mqtt_user: "aio-user".into(),
            aio_mqtt_password: "aio-password".into(),
            ssh_user: "root".into(),
            ssh_password: Some("ssh-password".into()),
            ssh_private_key: Some(String::new()),
        }
    }

    #[test]
    fn empty_private_key_falls_back_to_password() {
        let auth = release_remote_auth(&credentials()).expect("password auth");

        assert!(matches!(auth, RemoteAuth::Password { .. }));
    }

    #[test]
    fn non_empty_private_key_takes_precedence() {
        let mut credentials = credentials();
        credentials.ssh_private_key = Some("private-key".into());
        let auth = release_remote_auth(&credentials).expect("private key auth");

        assert!(matches!(auth, RemoteAuth::PrivateKey { .. }));
    }

    #[test]
    fn missing_password_and_private_key_is_rejected() {
        let mut credentials = credentials();
        credentials.ssh_password = Some(String::new());
        credentials.ssh_private_key = None;

        assert!(release_remote_auth(&credentials).is_err());
    }
}
