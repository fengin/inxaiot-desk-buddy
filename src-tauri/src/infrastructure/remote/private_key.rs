use aws_lc_rs::{rand::SystemRandom, signature};
use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey};
use rsa::pkcs8::DecodePrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPrivateKey};
use russh::keys::agent::AgentIdentity;
use russh::keys::ssh_encoding::Encode;
use russh::keys::ssh_key::{self, Mpint};
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey, decode_secret_key};
use zeroize::Zeroizing;

use crate::core::error::{AppError, AppResult};

pub(super) enum SigningKey {
    Native(Box<PrivateKey>),
    Rsa(Box<RsaSigner>),
}

impl SigningKey {
    pub(super) fn parse(text: &str, passphrase: Option<&str>) -> AppResult<Self> {
        // rsa仅负责本地格式转换；网络认证的私钥签名始终交给AWS-LC。
        if let Ok(key) =
            RsaPrivateKey::from_pkcs1_pem(text).or_else(|_| RsaPrivateKey::from_pkcs8_pem(text))
        {
            return RsaSigner::new(key).map(|key| Self::Rsa(Box::new(key)));
        }
        let key = decode_secret_key(text, passphrase).map_err(|error| {
            AppError::ssh(
                "解析SSH私钥（支持RSA PKCS#1/PKCS#8/OpenSSH、Ed25519、ECDSA）",
                error,
            )
        })?;
        if let Some(pair) = key.key_data().rsa() {
            let integer = |value: &Mpint| {
                value
                    .as_positive_bytes()
                    .map(BigUint::from_bytes_be)
                    .ok_or_else(|| AppError::InvalidConfig("RSA私钥包含无效整数".into()))
            };
            let rsa = RsaPrivateKey::from_components(
                integer(pair.public().n())?,
                integer(pair.public().e())?,
                integer(pair.private().d())?,
                vec![integer(pair.private().p())?, integer(pair.private().q())?],
            )
            .map_err(|error| AppError::ssh("解析RSA私钥参数", error))?;
            return RsaSigner::new(rsa).map(|key| Self::Rsa(Box::new(key)));
        }
        match key.algorithm() {
            Algorithm::Ed25519 | Algorithm::Ecdsa { .. } => Ok(Self::Native(Box::new(key))),
            _ => Err(AppError::InvalidConfig(
                "SSH私钥算法不受支持；支持RSA、Ed25519和ECDSA，拒绝DSA".into(),
            )),
        }
    }
}

pub(super) struct RsaSigner {
    pair: signature::RsaKeyPair,
    pub(super) public_key: PublicKey,
}

impl RsaSigner {
    fn new(key: RsaPrivateKey) -> AppResult<Self> {
        let der = key
            .to_pkcs1_der()
            .map_err(|error| AppError::ssh("转换RSA私钥格式", error))?;
        let pair = signature::RsaKeyPair::from_der(der.as_bytes())
            .map_err(|error| AppError::ssh("校验RSA私钥（要求2048至8192位）", error))?;
        let public = ssh_key::public::RsaPublicKey::new(
            Mpint::from_positive_bytes(&key.e().to_bytes_be()),
            Mpint::from_positive_bytes(&key.n().to_bytes_be()),
        )
        .map_err(|error| AppError::ssh("读取RSA公钥", error))?;
        Ok(Self {
            pair,
            public_key: PublicKey::new(public.into(), ""),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SignError {
    #[error("SSH签名通道已关闭")]
    Send(#[from] russh::SendError),
    #[error("RSA签名请求无效或签名失败")]
    Rejected,
}

impl russh::Signer for RsaSigner {
    type Error = SignError;

    async fn auth_sign(
        &mut self,
        identity: &AgentIdentity,
        hash_alg: Option<HashAlg>,
        mut to_sign: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        if identity.public_key().key_data() != self.public_key.key_data() {
            return Err(SignError::Rejected);
        }
        // RSA密钥本身无需更换；仅使用RFC 8332的SHA-2签名，不降级到ssh-rsa/SHA-1。
        let encoding: &'static dyn signature::RsaEncoding = match hash_alg {
            Some(HashAlg::Sha256) => &signature::RSA_PKCS1_SHA256,
            Some(HashAlg::Sha512) => &signature::RSA_PKCS1_SHA512,
            _ => return Err(SignError::Rejected),
        };
        let mut bytes = Zeroizing::new(vec![0; self.pair.public_modulus_len()]);
        self.pair
            .sign(encoding, &SystemRandom::new(), &to_sign, &mut bytes)
            .map_err(|_| SignError::Rejected)?;
        let signature = ssh_key::Signature::new(Algorithm::Rsa { hash: hash_alg }, bytes.to_vec())
            .map_err(|_| SignError::Rejected)?;
        let mut encoded = Vec::new();
        signature
            .encode(&mut encoded)
            .map_err(|_| SignError::Rejected)?;
        encoded
            .encode(&mut to_sign)
            .map_err(|_| SignError::Rejected)?;
        Ok(to_sign)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::traits::PrivateKeyParts;
    use russh::Signer;
    use russh::keys::ssh_encoding::Decode;
    use signature::KeyPair;
    use std::sync::LazyLock;

    static TEST_KEY: LazyLock<RsaPrivateKey> = LazyLock::new(|| {
        RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).expect("生成仅驻内存的单测密钥")
    });

    fn openssh_key(key: &RsaPrivateKey) -> PrivateKey {
        let mpint = |value: &BigUint| Mpint::from_positive_bytes(&value.to_bytes_be());
        let public = ssh_key::public::RsaPublicKey::new(mpint(key.e()), mpint(key.n())).unwrap();
        let private = ssh_key::private::RsaPrivateKey::new(
            mpint(key.d()),
            mpint(&key.crt_coefficient().unwrap()),
            mpint(&key.primes()[0]),
            mpint(&key.primes()[1]),
        )
        .unwrap();
        ssh_key::private::RsaKeypair::new(public, private)
            .unwrap()
            .into()
    }

    #[test]
    fn rsa_pkcs1_pkcs8_and_openssh_formats_preserve_the_same_public_key() {
        let pkcs1 = TEST_KEY.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF).unwrap();
        let pkcs8 = TEST_KEY.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
        let original = openssh_key(&TEST_KEY);
        let openssh = original.to_openssh(ssh_key::LineEnding::CRLF).unwrap();
        for text in [pkcs1.as_str(), pkcs8.as_str(), openssh.as_str()] {
            let SigningKey::Rsa(signer) = SigningKey::parse(text, None).unwrap() else {
                panic!("应使用AWS-LC RSA签名器");
            };
            assert_eq!(
                signer.public_key.key_data(),
                original.public_key().key_data()
            );
        }
    }

    #[tokio::test]
    async fn rsa_sha2_signatures_verify_and_sha1_or_wrong_identity_are_rejected() {
        let mut signer = RsaSigner::new(TEST_KEY.clone()).unwrap();
        let identity = AgentIdentity::from(signer.public_key.clone());
        let message = b"unit-test-only-ssh-session-auth-data";
        for (hash, verification) in [
            (HashAlg::Sha256, &signature::RSA_PKCS1_2048_8192_SHA256),
            (HashAlg::Sha512, &signature::RSA_PKCS1_2048_8192_SHA512),
        ] {
            let encoded = signer
                .auth_sign(&identity, Some(hash), message.to_vec())
                .await
                .unwrap();
            assert_eq!(&encoded[..message.len()], message);
            let mut remaining = &encoded[message.len()..];
            let blob = Vec::<u8>::decode(&mut remaining).unwrap();
            assert!(remaining.is_empty());
            let signature = ssh_key::Signature::decode(&mut blob.as_slice()).unwrap();
            assert_eq!(signature.algorithm(), Algorithm::Rsa { hash: Some(hash) });
            let verifier =
                signature::UnparsedPublicKey::new(verification, signer.pair.public_key().as_ref());
            verifier.verify(message, signature.as_bytes()).unwrap();
            assert!(verifier.verify(b"changed", signature.as_bytes()).is_err());
        }
        assert!(
            signer
                .auth_sign(&identity, None, message.to_vec())
                .await
                .is_err()
        );
        let wrong = PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&[17; 32]));
        assert!(
            signer
                .auth_sign(
                    &wrong.public_key().clone().into(),
                    Some(HashAlg::Sha256),
                    message.to_vec()
                )
                .await
                .is_err()
        );
    }

    #[test]
    fn existing_ed25519_is_preserved_and_invalid_or_weak_rsa_is_rejected() {
        let ed = PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&[18; 32]));
        let text = ed.to_openssh(ssh_key::LineEnding::LF).unwrap();
        assert!(matches!(
            SigningKey::parse(&text, None),
            Ok(SigningKey::Native(_))
        ));
        assert!(SigningKey::parse("not-a-private-key", None).is_err());
        let weak = RsaPrivateKey::new(&mut rand::rngs::OsRng, 1024).unwrap();
        assert!(RsaSigner::new(weak).is_err());
    }
}
