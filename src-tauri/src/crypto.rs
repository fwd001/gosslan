//! 端到端加密（E2EE）模块。
//!
//! 密码学原语：
//! - **X25519**：ECDH 密钥交换，为单聊双方派生共享密钥；
//! - **Ed25519**：消息签名，用于身份校验（防伪造 / 防中间人）；
//! - **ChaCha20-Poly1305**：AEAD 对称加密，密文格式 = `nonce(12B) || ciphertext`。
//!
//! 密钥持久化：私钥以 base64 存于本地 SQLite `settings` 表，重启后身份不变。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::{OsRng, RngCore};
use x25519_dalek::{PublicKey, StaticSecret};

const NONCE_LEN: usize = 12;

/// 节点身份：X25519 私钥（ECDH）+ Ed25519 私钥（签名）。
pub struct Identity {
    pub x25519_secret: StaticSecret,
    pub ed25519_signing: SigningKey,
}

impl Identity {
    /// 生成全新身份。
    pub fn generate() -> Self {
        let x25519_secret = StaticSecret::random_from_rng(OsRng);
        let ed25519_signing = SigningKey::generate(&mut OsRng);
        Self {
            x25519_secret,
            ed25519_signing,
        }
    }

    /// 从持久化的 base64 私钥重建身份。
    pub fn from_secrets(x25519_b64: &str, ed25519_b64: &str) -> Option<Self> {
        let xs = STANDARD.decode(x25519_b64).ok()?;
        let es = STANDARD.decode(ed25519_b64).ok()?;
        let xs: [u8; 32] = xs.try_into().ok()?;
        let es: [u8; 32] = es.try_into().ok()?;
        Some(Self {
            x25519_secret: StaticSecret::from(xs),
            ed25519_signing: SigningKey::from_bytes(&es),
        })
    }

    pub fn x25519_secret_b64(&self) -> String {
        STANDARD.encode(self.x25519_secret.to_bytes())
    }

    pub fn ed25519_secret_b64(&self) -> String {
        STANDARD.encode(self.ed25519_signing.to_bytes())
    }

    pub fn x25519_public_b64(&self) -> String {
        STANDARD.encode(self.x25519_public().as_bytes())
    }

    pub fn ed25519_public_b64(&self) -> String {
        STANDARD.encode(self.ed25519_signing.verifying_key().to_bytes())
    }

    pub fn x25519_public(&self) -> PublicKey {
        PublicKey::from(&self.x25519_secret)
    }

    /// 用 Ed25519 私钥签名，返回 base64。
    pub fn sign_b64(&self, data: &[u8]) -> String {
        STANDARD.encode(self.ed25519_signing.sign(data).to_bytes())
    }
}

/// ECDH：用对方 X25519 公钥（base64）+ 自己私钥派生共享密钥。
pub fn shared_secret(my_secret: &StaticSecret, their_public_b64: &str) -> Option<[u8; 32]> {
    let bytes = STANDARD.decode(their_public_b64).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    let their = PublicKey::from(arr);
    Some(*my_secret.diffie_hellman(&their).as_bytes())
}

/// ChaCha20-Poly1305 加密：返回 `nonce || ciphertext`。
pub fn seal(shared: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(shared));
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), plaintext).ok()?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Some(out)
}

/// ChaCha20-Poly1305 解密：输入 `nonce || ciphertext`。
pub fn open(shared: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ct) = data.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(shared));
    cipher.decrypt(Nonce::from_slice(nonce), ct).ok()
}

/// 群密钥加密（对称）。
pub fn seal_symmetric(key: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    seal(key, plaintext)
}

/// 群密钥解密（对称）。
pub fn open_symmetric(key: &[u8; 32], data: &[u8]) -> Option<Vec<u8>> {
    open(key, data)
}

/// 生成随机对称密钥（群密钥）。
pub fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    OsRng.fill_bytes(&mut k);
    k
}

/// 用 Ed25519 公钥校验签名。
pub fn verify_signature(pubkey_b64: &str, data: &[u8], sig_b64: &str) -> bool {
    let Ok(pk) = STANDARD.decode(pubkey_b64) else {
        return false;
    };
    let Ok(sig) = STANDARD.decode(sig_b64) else {
        return false;
    };
    let Ok(pk): Result<[u8; 32], _> = pk.try_into() else {
        return false;
    };
    let Ok(sig): Result<[u8; 64], _> = sig.try_into() else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk) else {
        return false;
    };
    let sig = Signature::from_bytes(&sig);
    vk.verify(data, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecdh_symmetry() {
        let a = Identity::generate();
        let b = Identity::generate();
        let sa = shared_secret(&a.x25519_secret, &b.x25519_public_b64()).unwrap();
        let sb = shared_secret(&b.x25519_secret, &a.x25519_public_b64()).unwrap();
        assert_eq!(sa, sb);
    }

    #[test]
    fn seal_open_roundtrip() {
        let key = random_key();
        let msg = b"hello p2p";
        let sealed = seal_symmetric(&key, msg).unwrap();
        assert_eq!(open_symmetric(&key, &sealed).unwrap(), msg);
    }

    #[test]
    fn signature_verifies() {
        let id = Identity::generate();
        let msg = b"message id";
        let sig = id.sign_b64(msg);
        assert!(verify_signature(&id.ed25519_public_b64(), msg, &sig));
        assert!(!verify_signature(&id.ed25519_public_b64(), b"tampered", &sig));
    }
}
